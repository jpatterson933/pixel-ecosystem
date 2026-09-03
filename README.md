# Pixel Ecosystem

A live, shared world rendered as pixels on a white map. Pixels move, mark the ground, and over time build structure. Anyone can open it in a browser, zoom into any part of the map, and watch. Anything that can call the action interface can drive pixels: a script, a model, a person.

The world is built to scale from a laptop to very large maps without being rewritten. Every layer runs as a cluster of one today and scales by adding instances.

## Quick start

Requires Docker.

```bash
make start
```

Open http://localhost:3000. `make start large` uses the large map, `make start small move,build` limits which actions pixels can take, `make actions` lists the actions, `make stop` stops everything, `make stop wipe` also deletes the world. Details in [docs/STARTUP.md](docs/STARTUP.md).

## Documentation

| Document                                     | What it covers                                                                                                                       |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| [docs/STARTUP.md](docs/STARTUP.md)           | Starting, stopping, wiping, rebuilding, logs, ports                                                                                  |
| [docs/SETUP.md](docs/SETUP.md)               | Local prerequisites, running without containers, world configuration, environment variables, production on Cloudflare and Kubernetes |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | The nine layers, the pixel, data flow, deployment, constants                                                                         |
| [docs/ACTIONS.md](docs/ACTIONS.md)           | Action modules, primitive events, how to add an action                                                                               |

## Repository layout

```
crates/sim-core          Rules of the world and the action modules. Pure Rust, also compiles to WASM.
crates/protocol          Wire formats, NATS subject names, credentials.
crates/common            Logging, NATS connection, world metadata, object store setup.
crates/sim-wasm          wasm-bindgen wrapper around sim-core for the browser.
crates/worldgen          Creates a world from a config file.
crates/region-server     Owns chunks, ticks them, appends events to the log.
crates/action-interface  The one way to act on the world. HTTP, WebSocket, MCP adapters.
crates/driver-random     Reference driver. Pixels wander, and build when build is enabled.
crates/gateway           Fans live chunk events out to browsers over WebSocket.
crates/summary-service   Maintains the tile pyramid and writes tiles to object storage.
client/                  Browser viewer. Vite, React, PixiJS.
configs/                 World configs.
deploy/                  Dockerfiles and Kubernetes manifests.
docs/                    Documentation.
docker-compose.yml       Runs everything locally.
Makefile                 make start, make stop.
```
