//! Runtime plumbing shared by every service: logging, NATS, world metadata, object storage.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_nats::jetstream::{self, kv};
use object_store::{aws::AmazonS3Builder, local::LocalFileSystem, ObjectStore};
use protocol::{kv_keys, subjects, Directory};
use sim_core::{Tick, WorldConfig};

pub fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

pub async fn connect_nats(url: &str) -> Result<(async_nats::Client, jetstream::Context)> {
    let client = async_nats::connect(url)
        .await
        .with_context(|| format!("connecting to NATS at {url}"))?;
    let js = jetstream::new(client.clone());
    Ok((client, js))
}

pub async fn kv_bucket(js: &jetstream::Context, world_id: &str) -> Result<kv::Store> {
    let name = subjects::kv_bucket(world_id);
    match js.get_key_value(&name).await {
        Ok(store) => Ok(store),
        Err(_) => js
            .create_key_value(kv::Config {
                bucket: name.clone(),
                history: 1,
                ..Default::default()
            })
            .await
            .with_context(|| format!("creating KV bucket {name}")),
    }
}

#[derive(Debug, Clone)]
pub struct WorldMeta {
    pub world_id: String,
    pub config: WorldConfig,
    pub directory: Directory,
    pub secret: Vec<u8>,
    pub genesis_ms: u64,
}

impl WorldMeta {
    pub fn current_tick(&self) -> Tick {
        protocol::tick_at(&self.config, self.genesis_ms, protocol::now_ms())
    }
}

pub async fn load_world(js: &jetstream::Context, world_id: &str) -> Result<WorldMeta> {
    let kv = js
        .get_key_value(subjects::kv_bucket(world_id))
        .await
        .map_err(|e| anyhow!("world {world_id} not found: {e}"))?;

    let get = |key: &'static str| {
        let kv = kv.clone();
        async move {
            let v = kv
                .get(key)
                .await
                .map_err(|e| anyhow!("reading {key}: {e}"))?
                .ok_or_else(|| anyhow!("world {world_id} is missing {key}"))?;
            Ok::<_, anyhow::Error>(v)
        }
    };

    let config: WorldConfig = serde_json::from_slice(&get(kv_keys::CONFIG).await?)?;
    let directory: Directory = serde_json::from_slice(&get(kv_keys::DIRECTORY).await?)?;
    let secret = hex::decode(String::from_utf8(get(kv_keys::SECRET).await?.to_vec())?.trim())?;
    let genesis_ms: u64 = String::from_utf8(get(kv_keys::GENESIS_MS).await?.to_vec())?
        .trim()
        .parse()?;

    Ok(WorldMeta {
        world_id: world_id.to_string(),
        config,
        directory,
        secret,
        genesis_ms,
    })
}

/// Object store from a spec string.
/// `local:<dir>` uses the filesystem. `r2:<bucket>` (or `s3:<bucket>`) uses the S3 API
/// with `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `R2_ENDPOINT` from the environment.
pub fn build_store(spec: &str) -> Result<Arc<dyn ObjectStore>> {
    if let Some(dir) = spec.strip_prefix("local:") {
        std::fs::create_dir_all(dir)?;
        let fs = LocalFileSystem::new_with_prefix(dir)?;
        return Ok(Arc::new(fs));
    }
    let bucket = spec
        .strip_prefix("r2:")
        .or_else(|| spec.strip_prefix("s3:"))
        .ok_or_else(|| anyhow!("store spec must be local:<dir>, r2:<bucket>, or s3:<bucket>"))?;
    let mut b = AmazonS3Builder::from_env()
        .with_bucket_name(bucket)
        .with_region(std::env::var("AWS_REGION").unwrap_or_else(|_| "auto".into()));
    if let Ok(endpoint) = std::env::var("R2_ENDPOINT") {
        b = b.with_endpoint(endpoint);
    }
    Ok(Arc::new(b.build()?))
}

pub fn local_store_dir(spec: &str) -> Option<&str> {
    spec.strip_prefix("local:")
}
