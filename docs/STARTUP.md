# Startup

Day-to-day commands for running Pixel Ecosystem locally. Installation and production details are in `SETUP.md`.

## The short version

Start everything with the small map:

```bash
make start
```

Or pick a map size, `small` or `large`, from `configs/`:

```bash
make start large
```

Or pick which actions pixels can take. Any word that is not a size is treated as a comma-separated action list. No list means every action.

```bash
make start small move,build
```

See what actions exist, with their parameters and settings:

```bash
make actions
```

`make` reserves `--help` for itself, so use `make help` for usage.

Stop everything but keep the world:

```bash
make stop
```

Stop everything and delete the world, its data, and its event log:

```bash
make stop wipe
```

`make start` builds the service image if it is missing, creates the world if it does not exist yet, writes `.env`, and brings every container up. Running it again with the same arguments on an existing world just restarts the containers. A different map or action list is a different world, so run `make stop wipe` first if you want to replace the current one. The rest of this file is what these commands do by hand.

## View it

Open http://localhost:3000

- Scroll wheel or pinch to zoom
- Drag to pan, or hold W, A, S, D or the arrow keys. Shift moves faster
- Gear icon, top right, for graphics settings
- Status bar, bottom left, shows connection, mode (live or tiles), zoom, tick, and fps

## Check what is running

```bash
docker compose ps
```

## Stop

```bash
docker compose down
```

The world is kept. It lives in `./data` (snapshots and tiles), `./worlds` (driver credentials and genesis), and the `nats-data` Docker volume (the event log).

## Start again

```bash
docker compose up -d
```

## Start from scratch with a new world

Wipe everything:

```bash
docker compose down -v && rm -rf data worlds
```

Start NATS:

```bash
docker compose up -d nats
```

Create the world. The last line printed is the world id. Add `--actions move,build` to limit which actions are enabled.

```bash
docker compose run --rm worldgen --config /configs/small.json --out /worlds
```

Put that id in `.env` at the repo root:

```bash
echo "WORLD_ID=<paste the id here>" > .env
```

Start everything:

```bash
docker compose up -d
```

## Change the world

Edit `configs/small.json` before running worldgen, or point `--config` at a different file. Fields: `map_width`, `map_height`, `pixel_count`, `color_count`, `tick_rate`, `seed`, and an optional `actions` map of module name to settings. Same config always produces the same world id. Running worldgen again on an existing world only rewrites the driver files.

## Rebuild after code changes

Rust services:

```bash
WORLD_ID=pending docker compose build worldgen && docker compose up -d
```

Client:

```bash
docker compose up -d --build client
```

## Logs

```bash
docker compose logs -f region-server
```

Replace `region-server` with `gateway`, `action-interface`, `summary-service`, `driver-random`, or `nats`.

## Ports

| Port | What                                   |
| ---- | -------------------------------------- |
| 3000 | client                                 |
| 8080 | gateway                                |
| 8081 | action interface: HTTP, WebSocket, MCP |
| 8082 | tiles                                  |
| 4222 | NATS                                   |
| 8222 | NATS monitoring                        |
