//! WASM bindings for the simulation core so the browser runs the same rules.

use wasm_bindgen::prelude::*;

use sim_core::{Chunk, ChunkCoord, Pixel, QueuedAction, TickInput, WorldConfig};

#[wasm_bindgen]
pub struct WasmChunk {
    cfg: WorldConfig,
    chunk: Chunk,
}

#[wasm_bindgen]
impl WasmChunk {
    /// `config` is the world config as a JS object; `cx`, `cy` the chunk coords.
    #[wasm_bindgen(constructor)]
    pub fn new(config: JsValue, cx: u32, cy: u32) -> Result<WasmChunk, JsValue> {
        let cfg: WorldConfig = serde_wasm_bindgen::from_value(config)?;
        Ok(WasmChunk {
            cfg,
            chunk: Chunk::new(ChunkCoord { cx, cy }),
        })
    }

    /// Replace state from a list of pixels.
    pub fn load(&mut self, pixels: JsValue) -> Result<(), JsValue> {
        let pixels: Vec<Pixel> = serde_wasm_bindgen::from_value(pixels)?;
        self.chunk = Chunk::new(self.chunk.coord);
        for p in pixels {
            self.chunk.insert(p);
        }
        Ok(())
    }

    /// Apply a list of events from the log.
    pub fn apply(&mut self, events: JsValue) -> Result<(), JsValue> {
        let events: Vec<sim_core::Event> = serde_wasm_bindgen::from_value(events)?;
        for e in &events {
            self.chunk.apply(e);
        }
        Ok(())
    }

    /// Run one tick with the given queued actions. Returns { events, outcomes, departures }.
    pub fn tick(&mut self, tick: u64, actions: JsValue) -> Result<JsValue, JsValue> {
        let actions: Vec<QueuedAction> = serde_wasm_bindgen::from_value(actions)?;
        let out = self.chunk.tick(
            &self.cfg,
            TickInput {
                tick,
                arrivals: Vec::new(),
                actions,
            },
        );
        let value = serde_json::json!({
            "events": out.events,
            "outcomes": out.outcomes,
            "departures": out.departures,
        });
        Ok(serde_wasm_bindgen::to_value(&value)?)
    }

    pub fn pixels(&self) -> Result<JsValue, JsValue> {
        let v: Vec<&Pixel> = self.chunk.pixels.values().collect();
        Ok(serde_wasm_bindgen::to_value(&v)?)
    }
}

#[wasm_bindgen]
pub fn chunk_size() -> u32 {
    sim_core::CHUNK_SIZE
}
