//! Create a world from a config: generate, register in NATS, publish genesis, write driver files.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use async_nats::jetstream::stream;
use clap::Parser;
use protocol::{kv_keys, subjects, ChunkEvents, Directory, DriverFile};
use rand::RngCore;
use sim_core::{generate, ChunkCoord, Event, WorldConfig};
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "worldgen", about = "Create a Pixel Ecosystem world")]
struct Args {
    /// Path to the world config JSON.
    #[arg(long, default_value = "configs/small.json")]
    config: PathBuf,
    /// NATS server URL.
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats: String,
    /// Directory to write world files into.
    #[arg(long, default_value = "./worlds")]
    out: PathBuf,
    /// Region server id that owns every chunk in the initial directory.
    #[arg(long, default_value = "region-0")]
    server_id: String,
    /// Comma-separated action modules to enable. Overrides the config's `actions`.
    /// Default: every registered module.
    #[arg(long, value_delimiter = ',')]
    actions: Option<Vec<String>>,
    /// Print every registered action module and exit.
    #[arg(long)]
    list_actions: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    common::init_logging();
    let args = Args::parse();

    if args.list_actions {
        for info in sim_core::registry().infos() {
            println!("{}", info.name);
            println!("    {}", info.description);
            println!("    params:   {}", summarize_schema(&info.schema));
            println!("    settings: {}", serde_json::to_string(&info.default_settings)?);
        }
        return Ok(());
    }

    let raw = std::fs::read_to_string(&args.config)
        .with_context(|| format!("reading {}", args.config.display()))?;
    let mut cfg: WorldConfig = serde_json::from_str(&raw).context("parsing config")?;
    cfg.finalize_actions(args.actions.as_deref())
        .map_err(|e| anyhow::anyhow!(e))?;
    cfg.validate().map_err(|e| anyhow::anyhow!(e))?;
    info!(actions = ?cfg.actions.keys().collect::<Vec<_>>(), "enabled actions");
    let world_id = cfg.world_id();
    info!(world_id, "generating");

    let generated = generate::generate(&cfg);
    info!(
        colors = generated.palette.len(),
        pixels = generated.pixels.len(),
        "generated"
    );

    let (_client, js) = common::connect_nats(&args.nats).await?;

    js.get_or_create_stream(stream::Config {
        name: subjects::stream_name(&world_id),
        subjects: vec![subjects::events_all(&world_id)],
        storage: stream::StorageType::File,
        ..Default::default()
    })
    .await
    .context("creating event stream")?;

    let kv = common::kv_bucket(&js, &world_id).await?;
    let existing_secret = match kv.get(kv_keys::SECRET).await? {
        Some(v) if kv.get(kv_keys::CONFIG).await?.is_some() => {
            Some(String::from_utf8(v.to_vec())?.trim().to_string())
        }
        _ => None,
    };

    let secret_hex = if let Some(secret_hex) = existing_secret {
        info!(world_id, "world already exists, rewriting driver files only");
        secret_hex
    } else {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        let secret_hex = hex::encode(secret);
        let directory = Directory::single(&cfg, &args.server_id);
        let genesis_ms = protocol::now_ms();

        kv.put(kv_keys::CONFIG, serde_json::to_vec(&cfg)?.into()).await?;
        kv.put(kv_keys::DIRECTORY, serde_json::to_vec(&directory)?.into()).await?;
        kv.put(kv_keys::SECRET, secret_hex.clone().into()).await?;
        kv.put(kv_keys::GENESIS_MS, genesis_ms.to_string().into()).await?;
        publish_genesis(&js, &world_id, &cfg, &generated).await?;
        secret_hex
    };

    let dir = args.out.join(&world_id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("config.json"), serde_json::to_vec_pretty(&cfg)?)?;
    std::fs::write(
        dir.join("driver.json"),
        serde_json::to_vec_pretty(&DriverFile {
            world_id: world_id.clone(),
            secret: secret_hex,
        })?,
    )?;
    std::fs::write(dir.join("genesis.json"), serde_json::to_vec(&generated.pixels)?)?;

    println!("{world_id}");
    info!(dir = %dir.display(), "world files written");
    Ok(())
}

async fn publish_genesis(
    js: &async_nats::jetstream::Context,
    world_id: &str,
    cfg: &WorldConfig,
    generated: &generate::Generated,
) -> Result<()> {
    // Event zero.
    let zero = ChunkEvents {
        tick: 0,
        cx: 0,
        cy: 0,
        events: vec![Event::Config { config: cfg.clone() }],
    };
    js.publish(subjects::events_world(world_id), serde_json::to_vec(&zero)?.into())
        .await?
        .await?;

    // Genesis placements, one message per chunk.
    let mut by_chunk: BTreeMap<ChunkCoord, Vec<Event>> = BTreeMap::new();
    for p in &generated.pixels {
        by_chunk
            .entry(ChunkCoord::of_cell(p.x, p.y))
            .or_default()
            .push(Event::Placed {
                id: p.id,
                x: p.x,
                y: p.y,
                color: p.color,
            });
    }
    let chunk_count = by_chunk.len();
    let mut pending = Vec::new();
    for (coord, events) in by_chunk {
        let msg = ChunkEvents {
            tick: 0,
            cx: coord.cx,
            cy: coord.cy,
            events,
        };
        let ack = js
            .publish(
                subjects::events(world_id, coord.cx, coord.cy),
                serde_json::to_vec(&msg)?.into(),
            )
            .await?;
        pending.push(ack);
        if pending.len() >= 512 {
            for a in pending.drain(..) {
                a.await?;
            }
        }
    }
    for a in pending {
        a.await?;
    }
    info!(chunks = chunk_count, "genesis published");

    Ok(())
}

fn summarize_schema(schema: &serde_json::Value) -> String {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return "none".into();
    };
    if props.is_empty() {
        return "none".into();
    }
    props
        .iter()
        .map(|(k, v)| {
            let ty = v
                .get("type")
                .map(|t| t.to_string().replace('"', ""))
                .unwrap_or_else(|| "any".into());
            format!("{k}: {ty}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}
