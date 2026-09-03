//! Summary service: maintains the tile pyramid and writes tiles to object storage.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_nats::jetstream::consumer::{pull, AckPolicy, DeliverPolicy};
use clap::Parser;
use futures::StreamExt;
use object_store::{path::Path as StorePath, ObjectStore, PutPayload};
use protocol::{paths, subjects, ChunkEvents, TileMeta};
use sim_core::{Event, PixelId, Rgba, TILE_SIZE, TILE_WRITE_INTERVAL_MS};
use tokio::sync::Mutex;
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "summary-service")]
struct Args {
    #[arg(long, env = "WORLD_ID")]
    world: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats: String,
    /// `local:<dir>` or `r2:<bucket>`.
    #[arg(long, env = "STORE", default_value = "local:./data")]
    store: String,
    /// When the store is local, serve it over HTTP here so the client can fetch tiles.
    #[arg(long, env = "BIND", default_value = "0.0.0.0:8082")]
    bind: String,
    #[arg(long, env = "CONSUMER_NAME", default_value = "summary")]
    consumer_name: String,
}

type TexelKey = (u32, u32);
type TileKey = (u32, u32, u32);

struct Pyramid {
    levels: u32,
    /// Index 0 is level 1. Each map is texel -> color histogram.
    hist: Vec<HashMap<TexelKey, HashMap<Rgba, u32>>>,
    colors: HashMap<PixelId, Rgba>,
    terrain: HashMap<(u32, u32), Rgba>,
    dirty: HashSet<TileKey>,
}

impl Pyramid {
    fn new(levels: u32) -> Self {
        Pyramid {
            levels,
            hist: (0..levels).map(|_| HashMap::new()).collect(),
            colors: HashMap::new(),
            terrain: HashMap::new(),
            dirty: HashSet::new(),
        }
    }

    fn add(&mut self, x: u32, y: u32, c: Rgba) {
        for l in 1..=self.levels {
            let (tx, ty) = (x >> l, y >> l);
            *self.hist[(l - 1) as usize]
                .entry((tx, ty))
                .or_default()
                .entry(c)
                .or_insert(0) += 1;
            self.dirty.insert((l, tx / TILE_SIZE, ty / TILE_SIZE));
        }
    }

    fn remove(&mut self, x: u32, y: u32, c: Rgba) {
        for l in 1..=self.levels {
            let (tx, ty) = (x >> l, y >> l);
            let level = &mut self.hist[(l - 1) as usize];
            if let Some(h) = level.get_mut(&(tx, ty)) {
                if let Some(n) = h.get_mut(&c) {
                    *n -= 1;
                    if *n == 0 {
                        h.remove(&c);
                    }
                }
                if h.is_empty() {
                    level.remove(&(tx, ty));
                }
            }
            self.dirty.insert((l, tx / TILE_SIZE, ty / TILE_SIZE));
        }
    }

    fn apply(&mut self, ev: &Event) {
        match ev {
            Event::Config { .. } => {}
            Event::Placed { id, x, y, color } | Event::Arrived { id, x, y, color, .. } => {
                self.colors.insert(*id, *color);
                self.add(*x, *y, *color);
            }
            Event::CellSet { x, y, color, .. } => {
                if let Some(old) = self.terrain.remove(&(*x, *y)) {
                    self.remove(*x, *y, old);
                }
                if let Some(c) = color {
                    self.terrain.insert((*x, *y), *c);
                    self.add(*x, *y, *c);
                }
            }
            Event::AttributeSet { .. } => {}
            Event::Moved { id, from, to } => {
                if let Some(c) = self.colors.get(id).copied() {
                    self.remove(from[0], from[1], c);
                    self.add(to[0], to[1], c);
                }
            }
            Event::Departed { id, from, .. } => {
                if let Some(c) = self.colors.get(id).copied() {
                    self.remove(from[0], from[1], c);
                }
            }
        }
    }

    /// Render one tile as RGBA. RGB is the dominant color, A is density.
    fn render(&self, level: u32, tile_x: u32, tile_y: u32) -> Vec<u8> {
        let mut buf = vec![0u8; (TILE_SIZE * TILE_SIZE * 4) as usize];
        let map = &self.hist[(level - 1) as usize];
        let capacity = 4f64.powi(level as i32);
        let denom = (1.0 + capacity).ln();
        for j in 0..TILE_SIZE {
            for i in 0..TILE_SIZE {
                let key = (tile_x * TILE_SIZE + i, tile_y * TILE_SIZE + j);
                let Some(h) = map.get(&key) else { continue };
                let mut best: Option<(Rgba, u32)> = None;
                let mut total = 0u32;
                for (c, n) in h {
                    total += n;
                    match best {
                        None => best = Some((*c, *n)),
                        Some((bc, bn)) => {
                            if *n > bn || (*n == bn && c.0 < bc.0) {
                                best = Some((*c, *n));
                            }
                        }
                    }
                }
                let Some((color, _)) = best else { continue };
                let density = ((255.0 * (1.0 + total as f64).ln() / denom).round() as u32).clamp(1, 255) as u8;
                let o = ((j * TILE_SIZE + i) * 4) as usize;
                buf[o] = color.0[0];
                buf[o + 1] = color.0[1];
                buf[o + 2] = color.0[2];
                buf[o + 3] = density;
            }
        }
        buf
    }
}

fn encode_png(rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, TILE_SIZE, TILE_SIZE);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header()?;
        w.write_image_data(rgba)?;
        w.finish()?;
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<()> {
    common::init_logging();
    let args = Args::parse();
    let (_client, js) = common::connect_nats(&args.nats).await?;
    let meta = Arc::new(common::load_world(&js, &args.world).await?);
    let store = common::build_store(&args.store)?;
    let levels = meta.config.summary_levels();
    let world = meta.world_id.clone();

    let tile_meta = TileMeta {
        world_id: world.clone(),
        levels,
        map_width: meta.config.map_width,
        map_height: meta.config.map_height,
    };
    store
        .put(
            &StorePath::from(paths::tile_meta(&world)),
            PutPayload::from(serde_json::to_vec(&tile_meta)?),
        )
        .await?;

    let pyramid = Arc::new(Mutex::new(Pyramid::new(levels)));
    info!(world = %world, levels, "summary service starting");

    if let Some(dir) = common::local_store_dir(&args.store) {
        let dir = dir.to_string();
        let bind = args.bind.clone();
        tokio::spawn(async move {
            let app = axum::Router::new()
                .fallback_service(ServeDir::new(&dir))
                .layer(CorsLayer::permissive());
            match tokio::net::TcpListener::bind(&bind).await {
                Ok(listener) => {
                    info!(bind = %bind, dir = %dir, "serving tiles");
                    let _ = axum::serve(listener, app).await;
                }
                Err(e) => warn!(error = %e, "tile server bind failed"),
            }
        });
    }

    tokio::spawn(writer(pyramid.clone(), store.clone(), world.clone()));

    let stream = js.get_stream(subjects::stream_name(&world)).await?;
    let consumer = stream
        .get_or_create_consumer(
            &args.consumer_name,
            pull::Config {
                durable_name: Some(args.consumer_name.clone()),
                filter_subject: subjects::events_all(&world),
                deliver_policy: DeliverPolicy::All,
                ack_policy: AckPolicy::Explicit,
                ..Default::default()
            },
        )
        .await
        .context("creating summary consumer")?;

    let mut messages = consumer.messages().await?;
    let mut processed = 0u64;
    while let Some(msg) = messages.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "consumer error");
                continue;
            }
        };
        if subjects::parse_events(&msg.subject).is_some() {
            if let Ok(ce) = serde_json::from_slice::<ChunkEvents>(&msg.payload) {
                let mut p = pyramid.lock().await;
                for ev in &ce.events {
                    p.apply(ev);
                }
            }
        }
        if let Err(e) = msg.ack().await {
            warn!(error = %e, "ack failed");
        }
        processed += 1;
        if processed % 10_000 == 0 {
            info!(processed, "events consumed");
        }
    }
    Ok(())
}

async fn writer(pyramid: Arc<Mutex<Pyramid>>, store: Arc<dyn ObjectStore>, world: String) {
    let mut interval = tokio::time::interval(Duration::from_millis(TILE_WRITE_INTERVAL_MS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let dirty: Vec<TileKey> = {
            let mut p = pyramid.lock().await;
            p.dirty.drain().collect()
        };
        if dirty.is_empty() {
            continue;
        }
        let mut writes = Vec::with_capacity(dirty.len());
        for (level, tx, ty) in dirty {
            let rgba = {
                let p = pyramid.lock().await;
                p.render(level, tx, ty)
            };
            let png = match encode_png(&rgba) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "png encode failed");
                    continue;
                }
            };
            let store = store.clone();
            let path = StorePath::from(paths::tile(&world, level, tx, ty));
            writes.push(tokio::spawn(async move {
                if let Err(e) = store.put(&path, PutPayload::from(png)).await {
                    warn!(error = %e, %path, "tile write failed");
                }
            }));
        }
        for w in writes {
            let _ = w.await;
        }
    }
}
