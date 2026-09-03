# Setup

What you need to install, provision, and configure to run Pixel Ecosystem locally and in production. The architecture itself is in `PROPOSAL.md`.

## 1. Repository layout

```
crates/sim-core          Rules of the world. Pure Rust library, also compiles to WASM.
crates/protocol          Wire formats, NATS subject names, credentials.
crates/common            Logging, NATS connection, world metadata, object store setup.
crates/sim-wasm          wasm-bindgen wrapper around sim-core for the browser.
crates/worldgen          Creates a world from a config file.
crates/region-server     Owns chunks, ticks them, appends events to the log.
crates/action-interface  The one way to act on the world. HTTP, WebSocket, MCP adapters.
crates/driver-random     Reference driver. Moves every pixel randomly every tick.
crates/gateway           Fans live chunk events out to browsers over WebSocket.
crates/summary-service   Maintains the tile pyramid and writes tiles to object storage.
client/                  Browser viewer. Vite, React, PixiJS.
configs/                 Example world configs.
deploy/                  Dockerfiles and Kubernetes manifests.
docker-compose.yml       Runs everything locally.
```

## 2. Local prerequisites

| Tool                      | Version               | Purpose                                              |
| ------------------------- | --------------------- | ---------------------------------------------------- |
| Rust toolchain via rustup | stable, 1.80 or newer | Building the services natively                       |
| Node.js                   | 20 or newer           | Building and running the client                      |
| Docker with Compose       | any current           | NATS locally, and the all-in-one local run           |
| wasm-pack                 | latest                | Only if you want to build `sim-wasm` for the browser |

Install Rust:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Install wasm-pack (optional):

```bash
cargo install wasm-pack
```

## 3. Fastest local run: Docker Compose

Everything runs in containers. The client is served on port 3000.

The Makefile wraps the whole sequence below: `make start small`, `make start large`, `make stop`, `make stop wipe`. See `STARTUP.md`. The manual steps:

Start NATS:

```bash
docker compose up -d nats
```

Build the service image:

```bash
WORLD_ID=pending docker compose build worldgen
```

Create a world. The last line printed is the world id.

```bash
docker compose run --rm worldgen --config /configs/small.json --out /worlds
```

Put the world id in a `.env` file at the repo root:

```bash
echo "WORLD_ID=<paste the id here>" > .env
```

Start everything:

```bash
docker compose up -d --build
```

Open http://localhost:3000. You should see the map fitted to the window with pixels moving.

Ports:

| Port | Service                                                        |
| ---- | -------------------------------------------------------------- |
| 3000 | client                                                         |
| 8080 | gateway, WebSocket at `/ws`, world info at `/world`            |
| 8081 | action interface, HTTP `/actions`, WebSocket `/ws`, MCP `/mcp` |
| 8082 | tiles, served from the local object store directory            |
| 4222 | NATS                                                           |
| 8222 | NATS monitoring                                                |

Local state lives in `./data` (snapshots and tiles), `./worlds` (driver credentials and genesis), and the `nats-data` Docker volume (the event log). Deleting all three and re-running worldgen gives you a fresh world.

## 4. Local run without containers for the services

Use this when iterating on Rust code. NATS still runs in Docker.

```bash
docker compose up -d nats
```

```bash
cargo build --release
```

Create a world:

```bash
./target/release/worldgen --config configs/small.json
```

Export the id it prints, then in separate terminals:

```bash
WORLD_ID=<id> ./target/release/region-server
```

```bash
WORLD_ID=<id> ./target/release/action-interface
```

```bash
WORLD_ID=<id> ./target/release/gateway
```

```bash
WORLD_ID=<id> ./target/release/summary-service
```

```bash
./target/release/driver-random --world-dir worlds/<id>
```

Client:

```bash
cd client && npm install && npm run dev
```

The client defaults to `http://localhost:8080` for the gateway and `http://localhost:8082` for tiles. Override with `VITE_GATEWAY_URL` and `VITE_TILE_BASE_URL` in `client/.env`.

## 5. World configuration

A world is one JSON file. See `configs/small.json` and `configs/large.json`.

| Field                     | Meaning                                         |
| ------------------------- | ----------------------------------------------- |
| `map_width`, `map_height` | Map size in cells                               |
| `pixel_count`             | Pixels placed at genesis                        |
| `color_count`             | Unique colors generated, 1 to 16,777,216        |
| `tick_rate`               | Ticks per second, 1 to 1000. Default 10         |
| `seed`                    | 64-bit integer. All randomness descends from it |

The world id is a hash of these fields. Running worldgen twice with the same config produces the same id. The second run leaves the existing world untouched and only rewrites the driver files.

`worldgen` writes `worlds/<id>/config.json`, `worlds/<id>/driver.json` (the secret that derives every pixel's control token), and `worlds/<id>/genesis.json` (every pixel's starting position). The driver needs all three. Treat `driver.json` as a secret.

## 6. Service environment variables

Every service accepts these as flags or environment variables.

| Variable                                                    | Used by                                    | Meaning                                                      |
| ----------------------------------------------------------- | ------------------------------------------ | ------------------------------------------------------------ |
| `WORLD_ID`                                                  | all                                        | The world to serve                                           |
| `NATS_URL`                                                  | all                                        | Default `nats://localhost:4222`                              |
| `SERVER_ID`                                                 | region-server                              | Which ownership ranges this server takes. Default `region-0` |
| `STORE`                                                     | region-server, summary-service             | `local:<dir>` or `r2:<bucket>`                               |
| `BIND`                                                      | action-interface, gateway, summary-service | Listen address                                               |
| `INTERFACE_URL`                                             | driver-random                              | WebSocket adapter of the action interface                    |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `R2_ENDPOINT` | anything with `STORE=r2:`                  | R2 credentials                                               |
| `RUST_LOG`                                                  | all                                        | Log filter, default `info`                                   |

## 7. Production: what to provision

### Cloudflare

1. **R2 bucket** for snapshots and tiles. Create an API token with object read and write on that bucket. Note the S3 endpoint, `https://<account id>.r2.cloudflarestorage.com`.
2. **Public access on the bucket**, either a custom domain or the `r2.dev` URL. The client fetches tiles from `<public bucket url>/tiles/<world id>/<level>/<tx>/<ty>.png`, so `VITE_TILE_BASE_URL` is the public bucket URL. Put Cloudflare's CDN in front by using a custom domain on the bucket.
3. **Pages project** for the client. Build command `npm run build` in `client/`, output directory `dist`. Set `VITE_GATEWAY_URL` to the public gateway host and `VITE_TILE_BASE_URL` to the public bucket URL as build-time environment variables.
4. **DNS** for the gateway and action interface hosts pointing at the Kubernetes ingress.

### Kubernetes

Any cluster with an ingress controller and a storage class that supports `ReadWriteMany` for the `worlds` volume. Manifests are in `deploy/k8s/` and apply in filename order.

1. Build and push the service image to a registry your cluster can pull from, and replace `pixel-ecosystem:latest` in the manifests with that reference.

```bash
docker build -f deploy/Dockerfile -t <registry>/pixel-ecosystem:<tag> .
```

2. Fill in `deploy/k8s/10-config.yaml`: the R2 credentials in the secret, and `STORE` if your bucket has a different name. Leave `WORLD_ID` for now.
3. Put your world config in `deploy/k8s/40-worldgen-job.yaml`.
4. Apply namespace, config, NATS, and the worlds volume:

```bash
kubectl apply -f deploy/k8s/00-namespace.yaml -f deploy/k8s/10-config.yaml -f deploy/k8s/20-nats.yaml -f deploy/k8s/30-worlds-pvc.yaml
```

5. Run the worldgen job and read the world id from its log:

```bash
kubectl apply -f deploy/k8s/40-worldgen-job.yaml && kubectl -n pixel-ecosystem logs job/worldgen
```

6. Set `WORLD_ID` in `10-config.yaml`, re-apply it, then apply the services and ingress:

```bash
kubectl apply -f deploy/k8s/10-config.yaml -f deploy/k8s/50-region-server.yaml -f deploy/k8s/60-services.yaml -f deploy/k8s/70-ingress.yaml
```

7. Set the ingress hosts in `70-ingress.yaml` to your DNS names.

### Scaling region servers

Ownership lives in the NATS key-value bucket `pe_<world id>` under the key `directory`. It is a JSON document of Hilbert index ranges to server ids. To add a region server, scale the StatefulSet so a pod with a new `SERVER_ID` exists, then rewrite the directory to split a range and assign part of it to the new id. Both servers reload chunks from snapshots and the log on restart. A single server owning the full range is the default that worldgen writes.

## 8. Using the action interface directly

List the actions the world supports, with parameter schemas:

```bash
curl -s localhost:8081/actions
```

Submit actions over HTTP. An action is `{ "kind": "<module name>", "params": { ... } }`.

```bash
curl -s localhost:8081/actions -H 'content-type: application/json' -d '{"envelopes":[{"pixel_id":1,"action":{"kind":"move","params":{"dx":1,"dy":0}},"at":[10,20],"credential":"<token>"}]}'
```

A pixel's credential is the first 32 hex characters of HMAC-SHA256 keyed by the world secret over the pixel id as 8 little-endian bytes. The reference driver computes them from `driver.json`.

MCP: point an MCP client at `http://<action interface>/mcp` using the Streamable HTTP transport. There is one tool per action module, named after the module, plus `world_info` and `list_actions`.

## 9. Adding an action

Actions are modules in `crates/sim-core/src/actions/`. See `PROPOSAL-ACTIONS.md` for the design.

1. Create `crates/sim-core/src/actions/<name>.rs` with a params struct deriving `Deserialize` and `JsonSchema`, and a type implementing `Module` with `NAME`, `DESCRIPTION`, `Params`, and `resolve`.
2. Add `pub mod <name>;` and one `Box::new(<name>::<Type>)` line in `crates/sim-core/src/actions/mod.rs`.
3. Rebuild.

The module acts only through `ActionContext`: `move_to`, `set_cell`, `set_attribute`, and the read methods. Validation, the HTTP listing, and the MCP tool come from the params struct automatically. Nothing outside sim-core changes.

## 10. Building the WASM simulation core

The client does not require it in this version, since a viewer submits no actions and therefore has nothing to predict. To build it anyway:

```bash
cd crates/sim-wasm && wasm-pack build --target web
```

## 11. Things to know

- The client's prediction toggle is wired but has no effect until the client is also a driver.
- Cross-server pixel handoffs travel over core NATS, not the durable log. If a region server dies in the same tick a pixel crosses to it from another server, that pixel is lost. Single-server deployments have no such window.
- Snapshots are written every 1,000 ticks. Restart cost is the replay of events since the oldest snapshot among a server's chunks.
- The summary service holds a per-texel color histogram for every non-empty texel at every level in memory. Memory grows with pixel count, not map size.
