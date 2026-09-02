# Pixel Ecosystem: Infrastructure Proposal

Status: draft for review
Date: 2026-09-01

## 1. Vision

Pixel Ecosystem is a live, shared world rendered as pixels on a white map. Anyone can open it in a browser and watch pixels move, cluster, and interact in real time. Over time pixels gain richer actions and the world gains structure: paths, dwellings, territories. Version one is deliberately minimal so that the infrastructure underneath it is complete and correct before the world on top of it grows.

What version one shows on screen: a white map, fitted to the browser window, covered in colored pixels the size of a single screen pixel. The pixels move. The viewer can zoom into any part of the map and pan around it. When zoomed out, dense areas of pixels appear as saturated color and sparse areas as faint tints, so the whole map reads as a living thing at any scale.

## 2. Principles

**Build what scales.** Every component is the one we keep. Where a single instance is sufficient today, it is a cluster of one running the same binary and protocol. Nothing is a stand-in to be replaced when scale arrives, because that replacement window may never come.

**Config defines the world.** A world is fully determined by a small, immutable configuration document. The same config always produces the same world. Changing the config creates a new world.

**The world is indifferent to drivers.** The world exposes one contract for acting on it. Anything that can produce a valid action through that contract is a legitimate driver: a script, a model, a human, a test harness. The world does not know or care which. This is what keeps the question of pixel intelligence open without blocking the infrastructure.

**Cells are not stored, pixels are.** The map is white by default. The world's state is the set of pixels and their positions, not a grid of cells. Empty space costs nothing, which is what makes very large maps practical.

## 3. Configuration

A world is created from one configuration document. It is stored as the first event in the world's log, so every replay is self-describing, and the world's identity is the hash of the config.

| Field         | Type           | Meaning                                        |
| ------------- | -------------- | ---------------------------------------------- |
| `map_width`   | integer        | Map width in cells                             |
| `map_height`  | integer        | Map height in cells                            |
| `pixel_count` | integer        | Number of pixels placed at genesis             |
| `color_count` | integer        | Number of unique colors generated              |
| `tick_rate`   | integer        | Simulation ticks per second                    |
| `seed`        | 64-bit integer | Source of all random choices during generation |

The config is immutable. There is no mechanism to change a live world's config. A different config is a different world.

### Generation

Generation runs once, at world creation, and is fully determined by the config.

1. Draw `color_count` unique RGB values from the seed. Alpha is fixed at fully opaque. Pixels are RGBA in the data model so alpha is available to future mechanics, but a translucent pixel on white only looks like a paler pixel, so it is not part of the random draw.
2. Place `pixel_count` pixels at uniformly random unoccupied cells, each assigned one of the generated colors at random.
3. Emit the config as event zero and the placements as the genesis events.

Because every choice descends from the seed, any world can be reconstructed exactly from its config alone.

## 4. The Pixel

A pixel is the only entity in version one.

| Field    | Size         | Meaning                                  |
| -------- | ------------ | ---------------------------------------- |
| `id`     | 8 bytes      | Stable identity for the pixel's lifetime |
| `x`, `y` | 4 bytes each | Position in cells                        |
| `color`  | 4 bytes      | RGBA                                     |

About 20 bytes per pixel. A billion pixels is roughly 20 gigabytes distributed across region servers.

### The action: move

A pixel has one action. `move(dx, dy)` where `dx` and `dy` are each in {-1, 0, 1} and not both zero. One move per pixel per tick.

Rules:

- A cell holds at most one pixel.
- A move into an occupied cell is rejected. The pixel stays where it is.
- A move off the map edge is rejected.
- Actions submitted between ticks are queued and resolved at the tick in arrival order, with ties broken by ascending pixel id. Resolution is therefore deterministic.
- A pixel with no queued action stays put.

Occupancy is deliberate. Collision is the first interaction the world has, and every later mechanic is built on the fact that space is contested.

### Crossing chunk boundaries

When a move takes a pixel from one chunk into another, the source chunk emits a departure event and the destination chunk emits an arrival event on the following tick. Ownership transfers exactly once. The log records both halves, so a pixel is never in two chunks or in none.

## 5. Layers

The system is nine layers. Each has one job, one language, and a defined interface to its neighbors.

```
  Drivers ──▶ Action Interface ──▶ NATS ──▶ Region Servers
                                              │
                                        Simulation Core
                                              │
                                        Event Log (JetStream)
                                         │            │
                                    Gateway      Summary Service
                                         │            │
                                       Client ◀── CDN (tiles)
```

### 5.1 Simulation Core

**Language:** Rust, compiled to native and to WASM.

A library crate containing the rules of the world and nothing else. No networking, no storage, no clock. Its interface is a pure function: given a chunk's state and the batch of actions queued for one tick, return the new state and the list of events that occurred.

Determinism is the core requirement. The same inputs produce the same outputs on every machine, every time. This is what allows the same code to run authoritatively on region servers, predictively in the browser, and exhaustively in tests with no infrastructure at all.

The core is the only place world rules live. Adding an action to the world means adding it here and nowhere else.

### 5.2 Region Servers

**Language:** Rust.

Region servers own chunks. Each server holds a set of chunks in memory, runs the simulation core on each of them every tick, and appends the resulting events to that chunk's stream in the event log.

A chunk is a 256 by 256 cell square of the map. See section 9 for why this is a constant. Chunks are the unit of ownership, storage, streaming, and rendering. They are invisible to pixels and viewers.

Ownership is assigned along a Hilbert curve over the chunk grid, so spatially adjacent chunks tend to live on the same server and boundary crossings are usually local. A chunk containing no pixels is not loaded. Scaling means adding servers and reassigning ownership ranges through the directory. A single region server owning every chunk is the same system with one range.

### 5.3 Coordination and Messaging

**Technology:** NATS with JetStream.

One dependency serving three needs:

- **Directory.** The NATS key-value store maps chunk ranges to region servers. The action interface and gateways read it to find owners.
- **Messaging.** NATS pub/sub carries action envelopes from the action interface to the owning region server, and pixel handoffs between chunks on different servers.
- **Event log.** JetStream provides the durable, append-only streams that make up the world's history. See 5.4.

NATS is chosen over Kafka because it also provides request routing and the directory, and it has a first-class Rust client. It clusters horizontally.

### 5.4 Event Log and Snapshots

**Technology:** JetStream streams, snapshots in Cloudflare R2.

Every accepted action becomes an event in its chunk's stream. A chunk's state is rebuilt by loading its most recent snapshot and folding the events that follow it. Snapshots are written to object storage on a fixed cadence.

The log provides:

- **Durability.** A region server that dies loses nothing. Its chunks are reassigned and rebuilt from snapshot plus log.
- **Fan-out.** Everything downstream reads the stream, never region server memory. The gateway and the summary service are consumers.
- **Replay and history.** Any moment in the world's past can be reconstructed. Time scrubbing in the viewer and full per-pixel history are consequences, not features that need building.

### 5.5 Summary Service

**Language:** Rust.

The summary service exists because a single pixel on a white map disappears once the viewer is zoomed out past one screen pixel per cell. It maintains a pyramid of downsampled images of the map, in the same way every web map does.

Level zero is the map at one texel per cell. Each level above halves the resolution: level one is one texel per 2 by 2 cells, level two per 4 by 4, and so on until a single 256 by 256 tile covers the whole map. Level eight is one texel per chunk. A trillion-cell map has about twenty levels.

Each texel above level zero stores two things: the dominant color among the pixels in its footprint, and the count of pixels in that footprint. The client renders count as saturation against white, so sparse areas read as faint tints and dense areas as solid color.

The service consumes chunk event streams. A single pixel move changes exactly one texel at each level above zero, so the cost per event is the number of levels, about twenty operations for the largest maps. Tiles are written to object storage once per second and served through the CDN.

Instances are sharded by the same chunk ranges as region servers. The top few levels, which are tiny, are merged by a single instance.

### 5.6 Gateway

**Language:** Rust.

The gateway is how a browser's screen stays current. Region servers produce events onto internal NATS streams. Browsers must not connect to the internal bus. The gateway sits between them.

Each gateway is a stateless WebSocket server. A client connects, reports which chunks are under its window, and the gateway subscribes to those chunk streams and forwards event deltas as they arrive. As the client pans, the gateway updates its subscriptions. It holds no world state, only who is looking where.

Gateways are separate from region servers because region servers should spend their CPU ticking chunks, not fanning out to viewers, and because viewer load and world size scale independently. A small map with a million viewers needs many gateways and one region server. A huge map with ten viewers needs the reverse. Gateways run behind a load balancer and are added as viewers grow.

Gateways carry live cell events only. Summary tiles come from the CDN and never pass through a gateway.

### 5.7 Action Interface

**Language:** Rust.

The action interface is the only way to act on the world. It is defined by one contract, the action envelope:

| Field        | Meaning                                   |
| ------------ | ----------------------------------------- |
| `pixel_id`   | The pixel being acted on                  |
| `action`     | The action name, `move` in version one    |
| `params`     | Action parameters, `dx` and `dy` for move |
| `credential` | Proof that the caller controls this pixel |

The interface validates the envelope, resolves the owning region server through the directory, routes the envelope over NATS, and returns whether the action was accepted and at which tick it will resolve.

The contract is exposed through adapters. All adapters produce the same envelope, so adding one is cheap and none of them is the interface's identity:

- **MCP** for agents and tooling that speak the protocol.
- **HTTP** for anything that can make a request.
- **WebSocket** for drivers submitting many actions per tick.
- **In-process** for drivers running inside the cluster with no network hop.

### 5.8 Drivers

**Language:** any. The reference driver is Rust.

A driver is a process that holds credentials for a set of pixels and submits actions through the action interface. It may control one pixel or millions. It may be a script, a model, a human, or a test. The world does not distinguish.

Version one ships one driver: the **random walk driver**. Every tick, for each pixel it controls, it submits a move in a random direction. It exists to exercise every layer end to end and to give the map life on day one. It commits to nothing about how pixels will eventually think. Anything smarter is another driver plugging into the same interface.

Credentials are issued at world creation. Each pixel has a control token, and a driver is handed the tokens for the pixels it drives. In version one, the random walk driver holds all of them.

### 5.9 Client

**Language:** TypeScript. PixiJS for rendering, React for the controls around the canvas.

The client opens with the map fitted to the window. The viewer zooms toward any point with the wheel or a pinch and pans by dragging. Zoom is capped at both ends: fully out, the map fits the window; fully in, one cell is 64 screen pixels. Small maps that fit the window at one screen pixel per cell open already magnified. Rendering is nearest-neighbor throughout so cells are always crisp squares.

The scale that matters internally is screen pixels per cell. It selects the data source automatically, with no mode for the viewer to choose:

- **At one screen pixel per cell or more**, the client shows live cells. It subscribes through the gateway to the chunks under the window and renders events as they arrive. Each visible chunk is one GPU texture with one texel per cell. The WASM build of the simulation core applies predicted moves before the server confirms them, so motion looks immediate.
- **Below one screen pixel per cell**, the client shows summary tiles. It picks the pyramid level where one texel is about one screen pixel, fetches the tiles under the window from the CDN, and refreshes them on the tile cadence. Each visible tile is one GPU texture.

A viewport is a handful of draw calls at any map size and any zoom.

## 6. Data Flow: One Move

What happens between a driver deciding to move a pixel and a viewer seeing it.

1. The driver submits an envelope through an adapter of the action interface.
2. The interface checks the credential, looks up the owning region server in the directory, and publishes the envelope on that server's NATS subject.
3. The region server queues the envelope for the pixel's chunk.
4. At the next tick, the region server hands the chunk's state and queued actions to the simulation core. The core resolves all moves deterministically and returns the new state and events.
5. The region server appends the events to the chunk's JetStream stream.
6. Every gateway with a client watching that chunk receives the events and forwards them over WebSocket. The client updates the chunk texture.
7. The summary service receives the same events and updates one texel at each pyramid level. Within a second the affected tiles are rewritten to R2.
8. Any client zoomed out over that region refetches the tile on its next cadence and sees the change in density or dominant color.

At 10 ticks per second, a viewer at live scale sees the move within one tick plus network latency. A viewer at tile scale sees it within about a second.

## 7. Deployment

| Component                                                                     | Where                              |
| ----------------------------------------------------------------------------- | ---------------------------------- |
| Region servers, gateways, summary service, action interface, reference driver | Containers on a Kubernetes cluster |
| NATS with JetStream                                                           | Same cluster, as a stateful set    |
| Snapshots and tiles                                                           | Cloudflare R2 with CDN in front    |
| Client                                                                        | Static site on Cloudflare Pages    |

All backend services are Rust and build from one workspace into one set of container images. Scaling any layer is adjusting its replica count. Region servers additionally require a directory update to reassign chunk ranges, which is an operation on the NATS key-value store.

## 8. Graphics Settings

Viewer-adjustable in the client, saved locally, with defaults that work on ordinary hardware. Their purpose is that no combination of map size and hardware produces a broken experience, only a slower or coarser one that the viewer can tune.

| Setting              | Default  | Effect                                                                                                                                         |
| -------------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| Live chunk limit     | 64       | Most chunks the client subscribes to at once. If the window at live scale would cover more, the client stays on tiles until zoomed further in. |
| Tile refresh cadence | 1 second | How often visible tiles refetch.                                                                                                               |
| Density gain         | 1.0      | How strongly pixel count maps to saturation on tiles.                                                                                          |
| Frame rate cap       | 60       | Upper bound on render frames per second.                                                                                                       |
| Prediction           | on       | Whether predicted moves render before server confirmation.                                                                                     |

## 9. System Constants

Values fixed in the system rather than in world config, because no one creating a world has a reason to vary them and they interact with each other in ways that work best fixed.

| Constant           | Value                     | Rationale                                                                                                                                                                                                            |
| ------------------ | ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Chunk size         | 256 by 256 cells          | Small enough that a viewport covers tens of chunks, large enough that a trillion-cell map is only millions of chunks. Natural GPU texture size. Aligns with tile size so level eight is exactly one texel per chunk. |
| Tile size          | 256 by 256 texels         | Standard web map tile size.                                                                                                                                                                                          |
| Maximum zoom       | 64 screen pixels per cell | Enough to watch individual pixels move.                                                                                                                                                                              |
| Snapshot cadence   | every 1,000 ticks         | Bounds replay time on recovery.                                                                                                                                                                                      |
| Tile write cadence | 1 second                  | Balances freshness against object storage write volume.                                                                                                                                                              |
| Default tick rate  | 10 per second             | Reads as motion. Gives a driver 100 milliseconds to decide. Config may override.                                                                                                                                     |

## 10. Version One

**Included**

- World generation from config
- The pixel with the move action and occupancy rules
- All nine layers as specified, each deployable and horizontally scalable
- The random walk driver controlling every pixel
- Free zoom and pan client with live and tile rendering
- Graphics settings
- Snapshots, recovery, and replay from the log

**Excluded**

- Any action other than move
- Any pixel attribute other than color
- Any driver other than random walk
- Time scrubbing in the client, though the log supports it
- Viewer accounts or any viewer-side state beyond graphics settings
- Public access to the action interface beyond the reference driver

Everything excluded is additive. None of it requires changing anything included.
