# arcade-chess-server

Bring-up bridge server for the sensor-driven illuminated chessboard. The ESP32
board connects as a WebSocket client to `/board`; browser clients connect to
`/ws` and receive the device's events fanned out verbatim. All state is in
memory — no database.

Contracts implemented exactly:

- [`docs/websocket-api.md`](../docs/websocket-api.md) — device side (`/board`).
- [`docs/client-api.md`](../docs/client-api.md) — client side (`/ws`, `/api/state`).

## Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/healthz` | Liveness, returns `200 ok`. |
| GET | `/api/state` | `{ "devices": [DeviceView…] }`, same shape as `init`. |
| WS | `/board` | ESP32 device connection. |
| WS | `/ws` | Browser client connection. |

## Environment

| Variable | Required | Default | Meaning |
| --- | --- | --- | --- |
| `ADMIN_PASSWORD` | yes | — | Admin password for client `auth`. Server refuses to start if unset/empty. |
| `PORT` | no | `8080` | HTTP/WebSocket listen port (binds `0.0.0.0`). |
| `DEVICE_TOKEN` | no | — | If set, `/board` upgrades must send `Authorization: Bearer <token>`. |
| `RUST_LOG` | no | `info` | Tracing filter. |
| `POSITIONS_PATH` | no | `/srv/positions.json` in the image, unset locally | Puzzle deck to load; falls back to the embedded one. |
| `STOCKFISH_PATH` | no | `/usr/games/stockfish`, else `stockfish` on `PATH` | Eval engine; falls back to material count. |
| `GAME_SNAPSHOT_PATH` | no | `/tmp/arcade-game.json` | Phase snapshot, so a restart mid-round is recoverable. |

Game tunables, all optional: `SETTLE_MS` 700, `AUTOSTART_STABLE_MS` 1500,
`UNKNOWN_TOLERANCE` 0, `TIER3_MAX_DISTANCE` 1.0, `TIER3_MARGIN` 1.0,
`DRAW_BAND_CP` 40, `COUNTDOWN_MS` 3000, `MAX_PLY` 10, `GAME_OPEN_CONTROLS` 0.

## The puzzle deck

Two decks are committed, and the distinction matters:

| File | Role |
| --- | --- |
| `positions.json` | The mined deck. Copied to `/srv/positions.json` in the image; this is what gets dealt. |
| `positions.fallback.json` | Eight hand-checked endgames, `include_str!`d into the binary. Dealt only if the mined deck is missing, empty or malformed — which shows as `positions_embedded` in `degraded`. |

CapRover builds from git, so the deck reaches production by being committed.
Regenerate it from the curated `.arcpos` (also committed, at
`position-miner/data/curated.arcpos`) and commit the result:

```bash
arcpos dump position-miner/data/curated.arcpos --json -n 100000 > server/positions.json
cargo test -p arcade-chess-server positions   # every shipped position must validate
```

`-n` defaults to 20, so it always needs raising past the record count. Mounting
a file over `/srv/positions.json` swaps the deck without a rebuild.

The two files have to be regenerated together: the `.arcpos` container is
versioned, and `arcpos` refuses to read a file written by an older build rather
than misinterpreting its records. A curated deck committed under a previous
format version cannot be re-dumped by the current binary, so it has to be
re-mined — which is why the JSON is committed and not generated at build time.

`validate` in `game/positions.rs` caps a dealt position at 16 pieces, matching
the miner's format ceiling rather than any particular mining run. Positions past
the cap are skipped individually, so a deck that trips it does not fail loudly —
it thins out, or empties and silently falls back. The `the_shipped_deck_is_playable`
test is what turns that into a build failure, and it is the reason `cargo test`
belongs in the regeneration steps above rather than being optional.

## Run locally

```bash
cd server
ADMIN_PASSWORD=changeme cargo run
# in another shell:
curl localhost:8080/healthz          # -> ok
curl localhost:8080/api/state        # -> {"devices":[]}
```

## Build the container

The Docker build context is the **git repo root** (Caprover inherits it), so run
the build from the repo root, not from `server/`:

```bash
docker build -f server/Dockerfile -t arcade-chess-server .
docker run --rm -p 8080:8080 -e ADMIN_PASSWORD=changeme arcade-chess-server
```

## Caprover deployment

- Domain: `chess-be.qinnovate.nz`.
- Set the app's `captain-definition` relative path to `./captain-definition` (the file at the repo root).
- Container/app port: `8080`.
- Enable **WebSocket support** for the app.
- Set `ADMIN_PASSWORD` (and optionally `DEVICE_TOKEN`) in the app's env vars.
