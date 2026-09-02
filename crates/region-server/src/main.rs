//! Region server: owns chunks, ticks them, appends events to the log.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_nats::jetstream::consumer::{pull, AckPolicy, DeliverPolicy};
use async_nats::jetstream::{self};
use clap::Parser;
use common::WorldMeta;
use futures::StreamExt;
use object_store::{path::Path as StorePath, ObjectStore, PutPayload};
use protocol::{
    paths, subjects, ActionResult, ChunkEvents, ChunkState, HandoffMessage, ServerActionBatch,
    ServerActionResults, Snapshot, StateRequest,
};
use sim_core::{
    Arrival, Chunk, ChunkCoord, Departure, Event, Pixel, PixelId, QueuedAction, Tick, TickInput,
    SNAPSHOT_INTERVAL_TICKS,
};
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "region-server")]
struct Args {
    #[arg(long, env = "WORLD_ID")]
    world: String,
    #[arg(long, env = "SERVER_ID", default_value = "region-0")]
    server_id: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats: String,
    /// `local:<dir>` or `r2:<bucket>`.
    #[arg(long, env = "STORE", default_value = "local:./data")]
    store: String,
}

struct Batch {
    reply: Option<async_nats::Subject>,
    results: Vec<Option<ActionResult>>,
    seq_to_idx: HashMap<u64, usize>,
}

struct Server {
    meta: Arc<WorldMeta>,
    me: String,
    client: async_nats::Client,
    js: jetstream::Context,
    store: Arc<dyn ObjectStore>,
    chunks: HashMap<ChunkCoord, Chunk>,
    /// Where each pixel this server owns currently is, or is about to arrive.
    index: HashMap<PixelId, ChunkCoord>,
    last_seq: HashMap<ChunkCoord, u64>,
    dirty: HashSet<ChunkCoord>,
    actions: HashMap<ChunkCoord, Vec<QueuedAction>>,
    arrivals: HashMap<ChunkCoord, Vec<Arrival>>,
    batches: Vec<Batch>,
    seq_counter: u64,
    tick: Tick,
}

#[tokio::main]
async fn main() -> Result<()> {
    common::init_logging();
    let args = Args::parse();

    let (client, js) = common::connect_nats(&args.nats).await?;
    let meta = Arc::new(common::load_world(&js, &args.world).await?);
    let store = common::build_store(&args.store)?;

    let mut server = Server {
        me: args.server_id.clone(),
        client,
        js,
        store,
        chunks: HashMap::new(),
        index: HashMap::new(),
        last_seq: HashMap::new(),
        dirty: HashSet::new(),
        actions: HashMap::new(),
        arrivals: HashMap::new(),
        batches: Vec::new(),
        seq_counter: 0,
        tick: meta.current_tick(),
        meta,
    };

    info!(
        world = %server.meta.world_id,
        server = %server.me,
        ranges = server.meta.directory.ranges_for(&server.me).len(),
        "starting"
    );

    server.restore().await?;
    server.run().await
}

impl Server {
    fn owned(&self, c: ChunkCoord) -> bool {
        self.meta.config.contains_chunk(c) && self.meta.directory.owner_of(c) == Some(self.me.as_str())
    }

    /// Load snapshots for owned chunks, then replay the log from the oldest snapshot.
    async fn restore(&mut self) -> Result<()> {
        let world = self.meta.world_id.clone();
        let prefix = StorePath::from(format!("snapshots/{world}"));
        let mut listing = self.store.list(Some(&prefix));
        let mut min_seq: Option<u64> = None;
        let mut loaded = 0usize;
        while let Some(obj) = listing.next().await {
            let obj = match obj {
                Ok(o) => o,
                Err(e) => {
                    warn!(error = %e, "listing snapshots");
                    continue;
                }
            };
            let Some((cx, cy)) = parse_snapshot_path(obj.location.as_ref()) else {
                continue;
            };
            let coord = ChunkCoord { cx, cy };
            if !self.owned(coord) {
                continue;
            }
            let bytes = self.store.get(&obj.location).await?.bytes().await?;
            let snap: Snapshot = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing snapshot {}", obj.location))?;
            min_seq = Some(min_seq.map_or(snap.seq, |m| m.min(snap.seq)));
            self.last_seq.insert(coord, snap.seq);
            if !snap.chunk.is_empty() {
                self.chunks.insert(coord, snap.chunk);
            }
            if !snap.pending_arrivals.is_empty() {
                self.arrivals.entry(coord).or_default().extend(snap.pending_arrivals);
            }
            loaded += 1;
        }
        info!(snapshots = loaded, "snapshots loaded");

        let stream = self.js.get_stream(subjects::stream_name(&world)).await?;
        let deliver_policy = match min_seq {
            Some(s) => DeliverPolicy::ByStartSequence {
                start_sequence: s + 1,
            },
            None => DeliverPolicy::All,
        };
        let consumer = stream
            .create_consumer(pull::Config {
                deliver_policy,
                ack_policy: AckPolicy::None,
                filter_subject: subjects::events_all(&world),
                ..Default::default()
            })
            .await
            .context("creating replay consumer")?;

        // Pixels that departed an owned chunk and whose arrival has not been logged yet.
        let mut in_flight: HashMap<PixelId, Pixel> = HashMap::new();
        let mut replayed = 0u64;
        loop {
            let mut batch = consumer
                .fetch()
                .max_messages(2000)
                .expires(Duration::from_millis(1500))
                .messages()
                .await?;
            let mut n = 0;
            while let Some(msg) = batch.next().await {
                let msg = msg.map_err(|e| anyhow::anyhow!("replay fetch: {e}"))?;
                n += 1;
                let Some((cx, cy)) = subjects::parse_events(&msg.subject) else {
                    continue;
                };
                let coord = ChunkCoord { cx, cy };
                let ce: ChunkEvents = match serde_json::from_slice(&msg.payload) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "bad event payload during replay");
                        continue;
                    }
                };
                for ev in &ce.events {
                    if let Event::Arrived { id, .. } = ev {
                        in_flight.remove(id);
                    }
                }
                if !self.owned(coord) {
                    continue;
                }
                let seq = msg.info().map(|i| i.stream_sequence).unwrap_or(0);
                if self.last_seq.get(&coord).is_some_and(|s| *s >= seq) {
                    continue;
                }
                let chunk = self.chunks.entry(coord).or_insert_with(|| Chunk::new(coord));
                for ev in &ce.events {
                    if let Event::Departed { id, to, .. } = ev {
                        if let Some(p) = chunk.pixels.get(id) {
                            in_flight.insert(
                                *id,
                                Pixel {
                                    id: *id,
                                    x: to[0],
                                    y: to[1],
                                    color: p.color,
                                },
                            );
                        }
                    }
                    chunk.apply(ev);
                }
                if chunk.is_empty() {
                    self.chunks.remove(&coord);
                }
                self.last_seq.insert(coord, seq);
                replayed += 1;
            }
            if n == 0 {
                break;
            }
        }
        // Requeue anything still in flight, then drop pending arrivals for pixels
        // that already exist somewhere we own.
        for (_, p) in in_flight {
            let dest = ChunkCoord::of_cell(p.x, p.y);
            if self.owned(dest) {
                self.arrivals.entry(dest).or_default().push(Arrival { pixel: p });
            } else if let Some(owner) = self.meta.directory.owner_of(dest) {
                let msg = HandoffMessage {
                    tick: self.tick,
                    departures: vec![Departure { pixel: p, to: dest }],
                };
                if let Ok(payload) = serde_json::to_vec(&msg) {
                    let _ = self.client.publish(subjects::handoff(&world, owner), payload.into()).await;
                }
            }
        }
        let present: HashSet<PixelId> = self.chunks.values().flat_map(|c| c.pixels.keys().copied()).collect();
        let mut seen: HashSet<PixelId> = HashSet::new();
        let mut requeued = 0usize;
        for list in self.arrivals.values_mut() {
            list.retain(|a| !present.contains(&a.pixel.id) && seen.insert(a.pixel.id));
            requeued += list.len();
        }
        self.arrivals.retain(|_, l| !l.is_empty());
        self.index.clear();
        for (c, chunk) in &self.chunks {
            for id in chunk.pixels.keys() {
                self.index.insert(*id, *c);
            }
        }
        for (c, list) in &self.arrivals {
            for a in list {
                self.index.insert(a.pixel.id, *c);
            }
        }
        let pixels: usize = self.chunks.values().map(|c| c.pixels.len()).sum();
        info!(replayed, chunks = self.chunks.len(), pixels, in_flight = requeued, "restore complete");
        Ok(())
    }

    async fn run(&mut self) -> Result<()> {
        let world = self.meta.world_id.clone();
        let mut act_sub = self.client.subscribe(subjects::actions(&world, &self.me)).await?;
        let mut ho_sub = self.client.subscribe(subjects::handoff(&world, &self.me)).await?;
        let mut st_sub = self.client.subscribe(subjects::state(&world, &self.me)).await?;

        let mut interval = tokio::time::interval(Duration::from_millis(self.meta.config.tick_interval_ms()));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        info!(tick = self.tick, "running");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.run_tick().await {
                        error!(error = %e, "tick failed");
                    }
                }
                Some(msg) = act_sub.next() => self.queue_batch(msg),
                Some(msg) = ho_sub.next() => self.handoff(msg),
                Some(msg) = st_sub.next() => self.state(msg).await,
            }
        }
    }

    fn queue_batch(&mut self, msg: async_nats::Message) {
        let batch: ServerActionBatch = match serde_json::from_slice(&msg.payload) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "bad action batch");
                return;
            }
        };
        let n = batch.actions.len();
        let mut b = Batch {
            reply: msg.reply,
            results: vec![None; n],
            seq_to_idx: HashMap::with_capacity(n),
        };
        for (i, a) in batch.actions.into_iter().enumerate() {
            let hinted = ChunkCoord::of_cell(a.at[0], a.at[1]);
            if !self.owned(hinted) {
                b.results[i] = Some(error_result(a.pixel_id, "not_owner"));
                continue;
            }
            let target = self.locate(a.pixel_id, hinted);
            self.seq_counter += 1;
            let seq = self.seq_counter;
            self.actions.entry(target).or_default().push(QueuedAction {
                seq,
                pixel_id: a.pixel_id,
                action: a.action,
            });
            b.seq_to_idx.insert(seq, i);
        }
        self.batches.push(b);
    }

    /// The chunk that actually holds the pixel: the hinted one, or an owned neighbor
    /// if the hint is one move stale. Falls back to the hint (an arrival may be pending).
    fn locate(&self, id: PixelId, hinted: ChunkCoord) -> ChunkCoord {
        if self.chunks.get(&hinted).is_some_and(|c| c.pixels.contains_key(&id)) {
            return hinted;
        }
        if let Some(c) = self.index.get(&id) {
            return *c;
        }
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let cx = hinted.cx as i64 + dx;
                let cy = hinted.cy as i64 + dy;
                if cx < 0 || cy < 0 {
                    continue;
                }
                let c = ChunkCoord {
                    cx: cx as u32,
                    cy: cy as u32,
                };
                if self.chunks.get(&c).is_some_and(|ch| ch.pixels.contains_key(&id)) {
                    return c;
                }
            }
        }
        hinted
    }

    fn handoff(&mut self, msg: async_nats::Message) {
        let h: HandoffMessage = match serde_json::from_slice(&msg.payload) {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, "bad handoff");
                return;
            }
        };
        for d in h.departures {
            if self.owned(d.to) {
                self.index.insert(d.pixel.id, d.to);
                self.arrivals.entry(d.to).or_default().push(Arrival { pixel: d.pixel });
            } else {
                warn!(?d.to, "handoff for chunk this server does not own");
            }
        }
    }

    async fn state(&self, msg: async_nats::Message) {
        let Some(reply) = msg.reply else { return };
        let req: StateRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(_) => return,
        };
        let coord = ChunkCoord {
            cx: req.cx,
            cy: req.cy,
        };
        let pixels = self
            .chunks
            .get(&coord)
            .map(|c| c.pixels.values().copied().collect())
            .unwrap_or_default();
        let state = ChunkState {
            tick: self.tick,
            cx: req.cx,
            cy: req.cy,
            pixels,
        };
        if let Ok(payload) = serde_json::to_vec(&state) {
            let _ = self.client.publish(reply, payload.into()).await;
        }
    }

    async fn run_tick(&mut self) -> Result<()> {
        let target = self.meta.current_tick();
        if target <= self.tick {
            return Ok(());
        }
        self.tick = target;
        let tick = self.tick;
        let cfg = self.meta.config.clone();
        let world = self.meta.world_id.clone();

        let mut coords: HashSet<ChunkCoord> = self.actions.keys().copied().collect();
        coords.extend(self.arrivals.keys().copied());

        let mut publishes: Vec<(ChunkCoord, Vec<u8>)> = Vec::new();
        let mut remote: HashMap<String, Vec<Departure>> = HashMap::new();
        let mut local_arrivals: Vec<(ChunkCoord, Arrival)> = Vec::new();
        let mut outcomes = Vec::new();

        for c in coords {
            let input = TickInput {
                tick,
                arrivals: self.arrivals.remove(&c).unwrap_or_default(),
                actions: self.actions.remove(&c).unwrap_or_default(),
            };
            let chunk = self.chunks.entry(c).or_insert_with(|| Chunk::new(c));
            let out = chunk.tick(&cfg, input);
            outcomes.extend(out.outcomes);
            if !out.events.is_empty() {
                let msg = ChunkEvents {
                    tick,
                    cx: c.cx,
                    cy: c.cy,
                    events: out.events,
                };
                publishes.push((c, serde_json::to_vec(&msg)?));
                self.dirty.insert(c);
            }
            for d in out.departures {
                if self.owned(d.to) {
                    self.index.insert(d.pixel.id, d.to);
                    local_arrivals.push((d.to, Arrival { pixel: d.pixel }));
                } else if let Some(owner) = self.meta.directory.owner_of(d.to) {
                    self.index.remove(&d.pixel.id);
                    remote.entry(owner.to_string()).or_default().push(d);
                } else {
                    warn!(?d.to, "departure to unowned chunk");
                }
            }
            if self.chunks.get(&c).is_some_and(|ch| ch.is_empty()) {
                self.chunks.remove(&c);
            }
        }
        for (c, a) in local_arrivals {
            self.arrivals.entry(c).or_default().push(a);
        }

        // Fill batch results from outcomes.
        for o in outcomes {
            for b in self.batches.iter_mut() {
                if let Some(&idx) = b.seq_to_idx.get(&o.seq) {
                    b.results[idx] = Some(ActionResult {
                        pixel_id: o.pixel_id,
                        accepted: o.accepted,
                        tick,
                        x: o.x,
                        y: o.y,
                        reason: o.reason,
                        error: None,
                    });
                    break;
                }
            }
        }

        // Append events to the log.
        let mut acks = Vec::with_capacity(publishes.len());
        for (c, payload) in publishes {
            let ack = self
                .js
                .publish(subjects::events(&world, c.cx, c.cy), payload.into())
                .await?;
            acks.push((c, ack));
        }
        for (c, ack) in acks {
            let a = ack.await?;
            self.last_seq.insert(c, a.sequence);
        }

        // Cross-server handoffs.
        for (owner, departures) in remote {
            let msg = HandoffMessage { tick, departures };
            self.client
                .publish(subjects::handoff(&world, &owner), serde_json::to_vec(&msg)?.into())
                .await?;
        }

        // Reply to every batch queued for this tick.
        for b in self.batches.drain(..) {
            let Some(reply) = b.reply else { continue };
            let results = b
                .results
                .into_iter()
                .map(|r| r.unwrap_or_else(|| error_result(0, "unresolved")))
                .collect();
            let msg = ServerActionResults { tick, results };
            self.client.publish(reply, serde_json::to_vec(&msg)?.into()).await?;
        }

        if tick % SNAPSHOT_INTERVAL_TICKS == 0 && (!self.dirty.is_empty() || !self.arrivals.is_empty()) {
            self.snapshot();
        }
        Ok(())
    }

    fn snapshot(&mut self) {
        let world = self.meta.world_id.clone();
        let tick = self.tick;
        let mut targets: HashSet<ChunkCoord> = self.dirty.drain().collect();
        targets.extend(self.arrivals.keys().copied());
        let count = targets.len();
        for c in targets {
            let chunk = self.chunks.get(&c).cloned().unwrap_or_else(|| Chunk::new(c));
            let seq = self.last_seq.get(&c).copied().unwrap_or(0);
            let pending_arrivals = self.arrivals.get(&c).cloned().unwrap_or_default();
            let snap = Snapshot {
                tick,
                seq,
                chunk,
                pending_arrivals,
            };
            let store = self.store.clone();
            let path = StorePath::from(paths::snapshot(&world, c.cx, c.cy));
            tokio::spawn(async move {
                match serde_json::to_vec(&snap) {
                    Ok(bytes) => {
                        if let Err(e) = store.put(&path, PutPayload::from(bytes)).await {
                            warn!(error = %e, %path, "snapshot write failed");
                        }
                    }
                    Err(e) => warn!(error = %e, "snapshot serialize failed"),
                }
            });
        }
        info!(tick, chunks = count, "snapshots written");
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

fn parse_snapshot_path(p: &str) -> Option<(u32, u32)> {
    let mut parts = p.rsplit('/');
    let file = parts.next()?;
    let cy = file.strip_suffix(".json")?.parse().ok()?;
    let cx = parts.next()?.parse().ok()?;
    Some((cx, cy))
}
