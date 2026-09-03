# Actions

Status: as built. Builds on `ARCHITECTURE.md`.

## 1. Goal

Adding an action to the world means writing one file and registering it. Nothing else changes: not the transport, not the log, not the client, not the tile service, not the MCP tool list. The rest of this document is how that is true.

## 2. The principle

Actions are open. Events are closed.

An **action** is what a driver asks for. There will be many, they will change often, and each is owned by a module.

An **event** is what the world records as having happened. There are few, they change rarely, and every consumer of the world understands all of them. Modules do not invent events. They compose primitives.

This split is what makes the system modular. A module can only produce primitive events, so the client, the summary service, the replay logic, and every future consumer stay correct without knowing which actions exist. When a genuinely new kind of change is needed, a new primitive is added once, every consumer learns it once, and every module from then on can use it.

## 3. Primitive events

The whole vocabulary.

| Event | Meaning |
|---|---|
| `placed` | Pixel created at a cell, genesis only |
| `moved` | Pixel changed cell within a chunk |
| `departed` | Pixel left this chunk for a cell in another |
| `arrived` | Pixel entered this chunk |
| `cell_set` | A cell's terrain color changed, or was cleared |
| `attribute_set` | A per-pixel attribute owned by a module changed |

`cell_set` is what makes roads, houses, trails, and territory possible. It colors the map itself, beneath pixels. `attribute_set` is how modules give pixels state such as energy, inventory, or allegiance, without touching the core pixel struct.

Any future primitive, for example a `said` event for speech, is a deliberate, rare addition that touches consumers once.

## 4. Chunk state

Beyond pixels, a chunk holds two sparse maps, both carried through snapshots and rebuilt by replay:

- `cells`: map from cell to terrain color. Absent means white.
- `attributes`: map from pixel id to a bag of values, namespaced by module name. Absent means the module has never touched that pixel.

The typed pixel stays exactly position and color. Modules never edit core structs and never see each other's attributes.

## 5. The module contract

A module is a Rust type in `crates/sim-core/src/actions/<name>.rs` implementing one trait.

```rust
pub trait Module: Send + Sync + 'static {
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    type Params: DeserializeOwned + JsonSchema;
    type Settings: DeserializeOwned + Serialize + Default + JsonSchema;

    fn resolve(&self, ctx: &mut ActionContext, pixel: &Pixel, params: Self::Params)
        -> Result<(), RejectReason>;
}
```

`Params` is what a driver sends with each action. `Settings` is world-level configuration for the module, stored in the world config's `actions` map and read through `ctx.settings()`. Both are plain structs deriving `Deserialize` and `JsonSchema`, so validation, the `GET /actions` listing, and the MCP tool definition are generated and cannot drift from the code. A blanket implementation turns every `Module` into the object-safe `ActionModule` the registry and tick loop use.

### Enabling actions per world

The world config's `actions` map names the enabled modules and their settings. A module not in the map is rejected with `disabled_action` and hidden from `GET /actions` and MCP. Changing the set changes the world id. `worldgen --actions a,b` sets it from the command line; `make start small a,b` does the same.

### ActionContext

The only surface a module has. It is scoped to the chunk the pixel is in, so a module can never reach across the map.

| Method | Effect |
|---|---|
| `config()` | The world config |
| `tick()` | Current tick |
| `rng()` | Deterministic random source seeded by world seed, tick, chunk, and action sequence |
| `occupant(x, y)` | Pixel id at a cell in this chunk, if any |
| `terrain(x, y)` | Terrain color at a cell in this chunk, if any |
| `attribute(pixel, key)` | This module's attribute on a pixel |
| `settings()` | This module's world-level settings, typed |
| `cell_in_chunk(x, y)` | Whether a cell is inside this chunk and the map |
| `reply(value)` | Payload returned to the driver in the action result's `data` field |
| `move_to(x, y)` | Move the acting pixel one cell. Handles same-chunk moves and cross-chunk departures. Fails with `occupied` or `off_map` |
| `set_cell(x, y, color)` | Set or clear terrain at a cell in this chunk |
| `set_attribute(pixel, key, value)` | Set this module's attribute on a pixel |

Every mutating method emits the matching primitive event. Modules cannot emit events directly, so the log is always a faithful record of primitives.

Constraints that keep the core sound:

- Reads and writes are limited to the acting pixel's chunk. Crossing a boundary is only possible through `move_to`, which uses the existing departure and arrival machinery.
- A module resolves one pixel's action. It cannot act on another pixel except by setting its own namespaced attribute on it.
- Determinism holds because the only randomness is `rng()` and the only clock is `tick()`.

### Registry

`crates/sim-core/src/actions/mod.rs` holds the list:

```rust
fn registry_modules() -> Vec<Box<dyn ActionModule>> {
    vec![Box::new(move_::Move), Box::new(mark::Mark)]
}
```

Adding an action is one new file, one `pub mod` line, and one entry here.

### Adding an action

1. Create `crates/sim-core/src/actions/<name>.rs` with a params struct deriving `Deserialize` and `JsonSchema`, and a type implementing `Module`.
2. Add `pub mod <name>;` and a `Box::new(...)` entry in `crates/sim-core/src/actions/mod.rs`.
3. Rebuild.

The module acts only through `ActionContext`. If it returns an error, none of its buffered events are applied.

## 6. What each layer knows

| Layer | Knows about | Cost per new action |
|---|---|---|
| Simulation core | The registry, the six primitives | One file, one registry entry |
| Protocol | The `Action` shape and the primitives | None |
| Region server | Nothing action-specific | None |
| Action interface | The registry, to validate params and generate `GET /actions` and MCP tools | None |
| Gateway, event log, snapshots, replay | Nothing. Events are opaque | None |
| Summary service | `cell_set`. Terrain contributes to a texel's dominant color and density like a pixel does | None |
| Client | `cell_set`, painted beneath pixels. Ignores `attribute_set` | None |
| Drivers | Whichever actions they choose to send | Opt in |
| WASM build | Same registry, compiled in | None |

## 7. Wire format

An action inside an envelope:

```json
{ "kind": "move", "params": { "dx": 1, "dy": 0 } }
```

`params` may be omitted or null for modules with no parameters. `GET /actions` on the action interface lists every module with its schema. MCP exposes one tool per module, named after the module, taking `pixel_id`, `x`, `y`, `credential`, and `params`.

## 8. Reference modules

**move**. Params `dx`, `dy` in -1..=1, not both zero. Calls `move_to`. This is the pattern every module follows.

**mark**. No params. Sets the terrain of the acting pixel's current cell to the pixel's own color via `set_cell`. A pixel that alternates move and mark leaves a trail. Many pixels leave roads. The reference driver sends it with probability `MARK_CHANCE` per pixel per tick, default zero.

**build**. Params `blueprint`, default `boundary_fence`. Settings `build_chance` (a suggestion drivers read, default 0.0005) and `blueprints` (allowed list, empty means all). The builder stands at the blueprint's center and stays there. Each call places the next cell of the blueprint in the builder's own color. Rules:

- **Site.** Starting a structure is refused if the footprint would leave the builder's chunk, overlap any other structure's footprint in that chunk, finished or not, or put a wall on a cell that already has terrain. Walls never cross.
- **Intrusion.** While unfinished, if any other pixel is anywhere in the footprint when `build` is called, all placed cells are cleared, progress resets to zero, and the reply says `destroyed`. The builder starts over on the same site.
- **Completion.** A finished structure is permanent. Pixels may walk through it without effect, and further `build` calls just reply `done`.

Replies carry `status`, `progress`, and `total`. A refused start sends the reference driver back to wandering.

Blueprints are cells relative to the builder at (0, 0), in build order, plus the footprint they claim. `boundary_fence` is a 5 by 5 ring: 16 wall cells around a 3 by 3 interior. Adding a structure is adding a blueprint in `build.rs`, not a module. Walls are terrain and do not block movement yet; blocking is a later change to the move module that this design leaves room for.

## 9. The reference driver

The driver decides; the world enforces. Each pixel wanders with `move`, optionally marking. If `build` is enabled, each wandering pixel starts a build with probability `build_chance` per tick, then sends `build` every tick until the structure is finished. Once finished the pixel sits inside and sends nothing. A refused build, for example a footprint that would cross a chunk edge or another structure, sends the pixel back to wandering.

## 10. What this does not do

- It does not let modules read or write outside their chunk. Actions that need a wider view, such as "look around," will need a read-only query surface on the action interface, which is a separate proposal.
- It does not load modules at runtime. Modules compile into the core so the WASM build, determinism, and replay stay exact. Adding a module is a rebuild.
- It does not decide what drives pixels. Modules define what a pixel can do, not what it chooses to do. The reference driver is one policy among many possible ones.
