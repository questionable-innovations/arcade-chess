# Arcade Chess client WebSocket API v1 — bring-up contract

Status: **implementation contract for bring-up**. Browser clients connect to
`wss://chess-be.qinnovate.nz/ws`. This is the fan-out side of the device API in
[`websocket-api.md`](./websocket-api.md): the server relays device events to
clients verbatim and accepts a small set of client requests. The frontend builds
its board state from the same semantic messages the device sends, so what the UI
shows is what the server actually received.

## Transport

- UTF-8 JSON text frames. One connection per browser tab.
- Every message has a stable `type`. Unknown fields and unknown `type` values
  must be ignored.
- No client authentication is required to observe. Admin commands require a
  password auth step (below).

## Server → client messages

### `init`

Sent once, immediately after the connection opens. Carries the full known state
for every connected device, plus devices that disconnected within the last
10 minutes (disconnected entries are dropped after that retention window; the
server tracks at most 64 devices, evicting the oldest disconnected entry when
full).

```json
{
  "type": "init",
  "devices": [
    {
      "device_id": "arcade-chess-001",
      "connected": true,
      "hello": { "...": "last hello envelope from the device, or null" },
      "snapshot": { "...": "latest board.snapshot event envelope, or null" },
      "node_status": [null, null, null, null],
      "device_status": null,
      "recent": []
    }
  ]
}
```

- `node_status` is a 4-entry array indexed by node id; each entry is the latest
  `node.status` event envelope for that node, or `null` if never seen.
- `snapshot`, `device_status`, and `hello` are the latest full envelopes of the
  corresponding device messages, or `null`.
- `recent` is a bounded ring (newest last, at most 200 entries) of recent device
  event envelopes, for the debug ticker.
- `node_events` is a bounded ring (newest last, at most 64) of quadrant
  transitions, each `{ unix_ms, node, online, reset_cause, timeouts,
  event_overflow }`. It exists to answer "why did node 2 drop off at 14:32"
  after the fact, and is deliberately kept out of `recent`: an active bus trace
  fills that ring in a few seconds and would evict the very history you want.
  Only genuine transitions are recorded, not every poll.

`init` also carries a `server` object — `{ oversized_dropped, device_count,
uptime_ms }` — so counters the server keeps are visible somewhere other than its
own stderr.

### `device.connected` / `device.disconnected`

```json
{ "type": "device.connected", "device_id": "arcade-chess-001" }
{ "type": "device.disconnected", "device_id": "arcade-chess-001" }
```

`device.connected` is followed by fresh events (the server requests a snapshot
on every device connect via `snapshot_required`).

### `event`

Every device event is relayed verbatim inside this wrapper, in arrival order:

```json
{
  "type": "event",
  "device_id": "arcade-chess-001",
  "recv_unix_ms": 1737000000000,
  "event": { "v": 1, "type": "sensor.changed", "seq": 18, "data": {} }
}
```

`recv_unix_ms` is the server's wall-clock receive time. It sits on the wrapper,
never inside the envelope, so the device payload stays byte-for-byte verbatim.
Device `at_ms` values are monotonic milliseconds since that board booted and
cannot be compared against anything else; this is the anchor that lets a client
render a real timestamp and correlate an event with a server log line.

The `event` value is the unmodified device envelope from `websocket-api.md`
(`board.snapshot`, `sensor.changed`, `node.status`, `device.status`,
`diagnostic.log`, `diagnostic.bus`, `calibration.progress`, `calibration.result`,
`command.result`, …). Clients must apply `sensor.changed` only when
`(boot_id, seq)` advances without a gap and otherwise wait for the next
`board.snapshot`; the server independently requests a snapshot from the device
when it detects a gap.

### `auth.result`

```json
{ "type": "auth.result", "ok": true }
```

### `command.queued` / `error`

Reply to a client `command` request. `command.queued` means the command was
forwarded to the device with the given correlation `id`; the terminal
`command.result` arrives later as a relayed `event`.

```json
{ "type": "command.queued", "id": "cmd-42", "device_id": "arcade-chess-001" }
{ "type": "error", "reason": "unauthorized" }
```

Stable `reason` values for a rejected request: `unauthorized`, `unknown_device`,
`device_offline`, `invalid_args`.

The server also pushes unsolicited `error` messages to explain things it would
otherwise do silently:

| reason | meaning |
| --- | --- |
| `shed_slow_client` | this client's outbound queue stayed full; the connection is about to close |
| `shed_lagged` | this client missed `dropped` broadcast events and cannot catch up; reconnect for a fresh `init` |
| `unknown_event_type` | a device sent an event `type` the server does not recognise (`etype` names it) — normally a firmware/server version skew |

A client that is shed should reconnect and rebuild from `init`; it must not
assume its current state is still correct.

## Client → server messages

### `auth`

```json
{ "type": "auth", "password": "..." }
```

Compared against the server's `ADMIN_PASSWORD` environment variable, in constant
time. On success the connection is marked admin until it closes — a client that
reconnects must re-authenticate. The server replies with `auth.result` either
way and never echoes the password. Failures are logged, delayed, and capped: the
fifth failed attempt on a connection closes it.

### `command` (admin only)

```json
{
  "type": "command",
  "device_id": "arcade-chess-001",
  "name": "lighting.set",
  "args": { "squares": [12], "effect": "solid", "colour": "00a0ff" }
}
```

`name` and `args` follow the device command table in `websocket-api.md`. The
server assigns `id` and `server_seq` and forwards it to the device. Non-admin
connections receive `error: unauthorized`.

## HTTP endpoints

- `GET /healthz` — `200 ok`, plain text.
- `GET /api/state` — JSON `{ "devices": [DeviceView…] }`, the same shape as
  `init`. Useful for curl-based debugging.

## Server environment

| Variable | Required | Meaning |
| --- | --- | --- |
| `ADMIN_PASSWORD` | yes | Static admin password for client `auth`. |
| `PORT` | no | HTTP/WebSocket listen port, default `8080`. |
| `DEVICE_TOKEN` | no | If set, device upgrades to `/board` must carry `Authorization: Bearer <token>`. |

## Quadrant mapping (bring-up assumption)

Square index `i` (0–63) has `row = i / 8`, `col = i % 8` with square 0 = a1
viewed from white's side, row-major. Nodes cover 4×4 quadrants:

| Node | Squares |
| --- | --- |
| 0 | rows 0–3, cols 0–3 (a1–d4) |
| 1 | rows 0–3, cols 4–7 (e1–h4) |
| 2 | rows 4–7, cols 0–3 (a5–d8) |
| 3 | rows 4–7, cols 4–7 (e5–h8) |

The physical local-to-global map is device configuration (UART config key 9);
this table is only the frontend's display assumption during bring-up.
