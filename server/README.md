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
- Set the app's `captain-definition` relative path to `server/captain-definition`.
- Container/app port: `8080`.
- Enable **WebSocket support** for the app.
- Set `ADMIN_PASSWORD` (and optionally `DEVICE_TOKEN`) in the app's env vars.
