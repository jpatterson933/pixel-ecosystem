//! The one contract for acting on the world. Validates, authenticates, routes.
//! This module is the in-process adapter; the binary wraps it in HTTP, WebSocket, and MCP.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::WorldMeta;
use futures::future::join_all;
use protocol::{
    subjects, ActionBatch, ActionResult, ActionResults, ServerAction, ServerActionBatch,
    ServerActionResults,
};
use sim_core::{ChunkCoord, PixelId};

#[derive(Clone)]
pub struct Router {
    client: async_nats::Client,
    pub meta: Arc<WorldMeta>,
    timeout: Duration,
}

impl Router {
    pub fn new(client: async_nats::Client, meta: Arc<WorldMeta>) -> Self {
        let tick_ms = meta.config.tick_interval_ms();
        Router {
            client,
            meta,
            timeout: Duration::from_millis(tick_ms * 4 + 500),
        }
    }

    pub async fn submit(&self, batch: ActionBatch) -> ActionResults {
        let n = batch.envelopes.len();
        let mut results: Vec<Option<ActionResult>> = vec![None; n];
        let mut by_server: HashMap<String, (Vec<usize>, Vec<ServerAction>)> = HashMap::new();

        for (i, e) in batch.envelopes.into_iter().enumerate() {
            if !e.action.is_valid() {
                results[i] = Some(error_result(e.pixel_id, "invalid_action"));
                continue;
            }
            if !protocol::verify_token(&self.meta.secret, e.pixel_id, &e.credential) {
                results[i] = Some(error_result(e.pixel_id, "unauthorized"));
                continue;
            }
            if !self.meta.config.contains_cell(e.at[0] as i64, e.at[1] as i64) {
                results[i] = Some(error_result(e.pixel_id, "off_map"));
                continue;
            }
            let chunk = ChunkCoord::of_cell(e.at[0], e.at[1]);
            let Some(owner) = self.meta.directory.owner_of(chunk) else {
                results[i] = Some(error_result(e.pixel_id, "no_owner"));
                continue;
            };
            let entry = by_server.entry(owner.to_string()).or_default();
            entry.0.push(i);
            entry.1.push(ServerAction {
                pixel_id: e.pixel_id,
                action: e.action,
                at: e.at,
            });
        }

        let world = self.meta.world_id.clone();
        let requests = by_server.into_iter().map(|(server, (idxs, actions))| {
            let client = self.client.clone();
            let subject = subjects::actions(&world, &server);
            let timeout = self.timeout;
            async move {
                let payload = match serde_json::to_vec(&ServerActionBatch {
                    actions: actions.clone(),
                }) {
                    Ok(p) => p,
                    Err(e) => return (idxs, actions, Err(e.to_string())),
                };
                let res = tokio::time::timeout(timeout, client.request(subject, payload.into())).await;
                let outcome = match res {
                    Err(_) => Err("timeout".to_string()),
                    Ok(Err(e)) => Err(format!("request failed: {e}")),
                    Ok(Ok(msg)) => serde_json::from_slice::<ServerActionResults>(&msg.payload)
                        .map_err(|e| format!("bad reply: {e}")),
                };
                (idxs, actions, outcome)
            }
        });

        for (idxs, actions, outcome) in join_all(requests).await {
            match outcome {
                Ok(reply) => {
                    for (k, idx) in idxs.into_iter().enumerate() {
                        results[idx] = reply.results.get(k).cloned().or_else(|| {
                            Some(error_result(actions[k].pixel_id, "missing_result"))
                        });
                    }
                }
                Err(msg) => {
                    for (k, idx) in idxs.into_iter().enumerate() {
                        results[idx] = Some(error_result(actions[k].pixel_id, &msg));
                    }
                }
            }
        }

        ActionResults {
            results: results
                .into_iter()
                .map(|r| r.unwrap_or_else(|| error_result(0, "unresolved")))
                .collect(),
        }
    }
}

fn error_result(pixel_id: PixelId, error: &str) -> ActionResult {
    ActionResult {
        pixel_id,
        accepted: false,
        tick: 0,
        x: 0,
        y: 0,
        reason: None,
        error: Some(error.to_string()),
    }
}
