# Action Modules: Proposal

Status: draft for review
Builds on: `PROPOSAL.md`

## 1. Goal

Adding an action to the world should mean writing one file and registering it. Nothing else changes: not the transport, not the log, not the client, not the tile service, not the MCP tool list. The rest of this document is how to make that true.

## 2. The principle

Actions are open. Events are closed.

An **action** is what a driver asks for. There will be many, they will change often, and each is owned by a module.

An **event** is what the world records as having happened. There are few, they change rarely, and every consumer of the world understands all of them. Modules do not invent events. They compose primitives.

This split is what makes the system modular. A module can only produce primitive events, so the client, the summary service, the replay logic, and every future consumer stay correct without knowing which actions exist. When a genuinely new kind of change is needed, a new primitive is added once, every consumer learns it once, and every module from then on can use it.

## 3. Primitive events

The vocabulary after this proposal. The first four exist today.

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

Two additions, both sparse, both carried through snapshots and rebuilt by replay:

- `cells`: map from cell to terrain color. Absent means white.
- `attributes`: map from pixel id to a bag of values, namespaced by module name. Absent means the module has never touched that pixel.

The typed pixel stays exactly position and color. Modules never edit core structs and never see each other's attributes.

## 5. The module contract

A module is a Rust type in `crates/sim-core/src/actions/<name>.rs` implementing one trait.

```rust
pub trait ActionModule: Send + Sync {
    /// Wire name of the action, e.g. "move".
    fn name(&self) -> &'static str;
    /// Shown to drivers and in MCP tool listings.
    fn description(&self) -> &'static str;
    /// JSON Schema for `params`, derived from the module's params struct.
    fn schema(&self) -> serde_json::Value;
    /// Resolve the action for one pixel at one tick. Only `ctx` touches the world.
    fn resolve(
        &self,
        ctx: &mut ActionContext,
        pixel: &Pixel,
        params: &serde_json::Value,
    ) -> Result<(), RejectReason>;
}
```

Parameters are a plain struct deriving `Deserialize` and `JsonSchema`. The schema is generated from it, so validation and the MCP tool definition come for free and cannot drift from the code.

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
pub fn registry() -> Vec<Box<dyn ActionModule>> {
    vec![
        Box::new(r#move::Move),
        Box::new(mark::Mark),
    ]
}
```

Adding an action is one new file and one line here.

## 6. What changes per layer

| Layer | Change | Ongoing cost per new action |
|---|---|---|
| Simulation core | Action becomes `{ kind, params }`. Tick dispatches by name to the registry. Chunk gains cells and attributes. `cell_set` and `attribute_set` primitives added | One file, one registry line |
| Protocol | `Action` type changes shape. Two new event variants | None |
| Region server | None beyond recompiling | None |
| Action interface | MCP tools generated from the registry. A `list_actions` tool and `GET /actions` endpoint expose schemas | None |
| Gateway, event log, snapshots, replay | None. Events are opaque to them | None |
| Summary service | Learns `cell_set`. Terrain contributes to a texel's dominant color and density the same way a pixel does | None |
| Client | Learns `cell_set` and paints terrain beneath pixels. Ignores `attribute_set` | None |
| Drivers | Choose actions by name and pass params. The random driver keeps sending `move` | Opt in |
| WASM build | Unchanged in shape. Registry compiles into it | None |

## 7. Wire format

The envelope keeps its fields. Only the action changes shape.

Before:

```json
{ "kind": "move", "dx": 1, "dy": 0 }
```

After:

```json
{ "kind": "move", "params": { "dx": 1, "dy": 0 } }
```

MCP exposes one tool per module, named after the module, with the module's schema plus `pixel_id`, `x`, `y`, and `credential`.

## 8. Reference modules

**move**, ported from today's hand-written rules. Params `dx`, `dy` in -1..=1, not both zero. Calls `move_to`. This is the pattern every module follows.

**mark**, the first real second action and the one that proves the design. No params. Sets the terrain of the acting pixel's current cell to the pixel's own color via `set_cell`. A pixel that alternates move and mark leaves a trail. Many pixels leave roads. It touches nothing outside sim-core, which is the point.

## 9. What this does not do

- It does not let modules read or write outside their chunk. Actions that need a wider view, such as "look around," will need a read-only query surface on the action interface, which is a separate proposal.
- It does not load modules at runtime. Modules compile into the core so the WASM build, determinism, and replay stay exact. Adding a module is a rebuild.
- It does not decide what drives pixels. Modules define what a pixel can do, not what it chooses to do.
