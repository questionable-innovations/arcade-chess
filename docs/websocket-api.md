# Arcade Chess device WebSocket API v1

Status: **implementation contract for bring-up**. The ESP32 device connects to
`wss://chess-be.qinnovate.nz/board`. The prototype firmware disables server
certificate validation. A production configuration can add CA or public-key
validation later.

## Transport and compatibility

- One WebSocket connection per physical board.
- UTF-8 JSON text frames in v1. Binary frames are reserved.
- Every object has `v: 1`, a stable `type`, and `device_id` where applicable.
- The ESP sends `hello` immediately after connection. The server replies with
  `welcome`. No commands are applied before that reply.
- Device event sequence numbers are unsigned 32-bit integers scoped to `boot_id`.
  A new boot creates a new random `boot_id` and resets `seq` to zero.
- Times ending in `_ms` are device monotonic milliseconds unless explicitly named
  `unix_ms`. The ESP does not claim wall-clock validity until time is synchronized.
- Unknown object fields must be ignored. Unknown `type` values must be logged and
  ignored. A different major `v` is incompatible.
- The maximum accepted inbound JSON message is 2048 bytes. The server should keep
  commands below 1024 bytes for the initial ESP implementation.

## Session handshake

Device to server:

```json
{
  "v": 1,
  "type": "hello",
  "device_id": "arcade-chess-001",
  "boot_id": "7e4c18b2",
  "firmware": "0.1.0",
  "hardware": "esp32-main-1R0",
  "protocols": { "uart": 1, "websocket": 1 },
  "last_server_seq": 0,
  "capabilities": ["board.snapshot", "sensor.events", "lighting.basic", "diagnostics"]
}
```

Server to device:

```json
{
  "v": 1,
  "type": "welcome",
  "server_seq": 41,
  "session_id": "01J2SESSION",
  "heartbeat_ms": 15000,
  "snapshot_required": true
}
```

The current prototype optionally supplies a bearer token as
`Authorization: Bearer <token>` during the HTTP upgrade. The token and `device_id`
are configured locally and are never printed in full in logs.

## Device events

All device events share this envelope:

```json
{
  "v": 1,
  "type": "sensor.changed",
  "device_id": "arcade-chess-001",
  "boot_id": "7e4c18b2",
  "seq": 18,
  "at_ms": 83422,
  "data": {}
}
```

Initial event types and their `data` payloads:

| `type` | `data` |
| --- | --- |
| `board.snapshot` | `squares`: 64 values (`-1` negative polarity, `0` empty/uncertain, `1` positive polarity), `valid`: 64 booleans, `nodes`: four node summaries |
| `sensor.changed` | `square` 0-63, `state`: `empty`, `positive`, `negative`, or `uncertain`, `raw`, `baseline`, `node`, `local_square` |
| `sensor.raw_scan` | `scan_id`, `complete`, `captured_ms`, and 64-entry arrays: `raw_adc`, `baseline_adc`, `noise_adc`, `state`; missing/offline squares are `null` |
| `node.status` | `node`, `online`, `firmware`, `calibrated`, `reset_cause`, error counters |
| `diagnostic.log` | `level`, `component`, `message`; rate limited and never contains credentials |
| `diagnostic.bus` | `direction`, `node`, `uart_seq`, `message_type`, `result`; optional `raw_hex` when trace mode is enabled |
| `calibration.progress` | `node`, `phase`, `samples`, `percent` |
| `calibration.result` | `node`, `ok`, `baseline`: 16 values, `noise`: 16 values, optional `reason` |
| `device.status` | Wi-Fi RSSI, heap, uptime, WebSocket reconnect count, UART health |

`board.snapshot` is sent after `welcome`, on server request, after an event gap or
node reset, and periodically while bring-up tracing is enabled. The latest snapshot
supersedes all earlier sensor events in the same boot session.

## Server commands and acknowledgements

Commands use a server-generated correlation ID and monotonically increasing
`server_seq`:

```json
{
  "v": 1,
  "type": "command",
  "server_seq": 42,
  "id": "cmd-01J2ABC",
  "name": "lighting.set",
  "args": {
    "squares": [12, 20],
    "effect": "solid",
    "colour": "00a0ff",
    "duration_ms": 0
  }
}
```

The ESP responds promptly with `accepted` or `rejected`, then with `applied` or
`timeout` after the addressed nodes respond. Duplicate command IDs return the
previous terminal result and must not repeat side effects.

```json
{
  "v": 1,
  "type": "command.result",
  "device_id": "arcade-chess-001",
  "id": "cmd-01J2ABC",
  "status": "applied",
  "reason": null
}
```

Initial command names:

| `name` | `args` |
| --- | --- |
| `board.snapshot.get` | empty object |
| `node.identify` | `node`, optional `duration_ms` |
| `lighting.set` | `squares`, `effect`, RGB hex `colour`, optional `duration_ms` |
| `lighting.clear` | optional `squares` |
| `calibration.start` | `node` 0-3 or `"all"`; board must be empty |
| `sensor.raw_scan.get` | Optional `samples_per_square` from 1-32; returns one averaged `sensor.raw_scan` event |
| `sensor.raw_stream.set` | `enabled`, `interval_ms` (clamped to 250-10000), `samples_per_square` (1-8), and optional `duration_ms` (maximum 10 minutes) |
| `diagnostics.trace` | `enabled`, optional `raw_frames`, `duration_ms` |
| `device.restart` | `confirm`: exact `"restart"` string |
| `device.mode.set` | `mode`: `"normal"` or `"bringup"`; persisted and propagated to quadrants |

Calibration and restart commands require a physical/operator workflow in the UI.
The ESP rejects malformed, stale, unsupported, or unsafe commands with a stable
reason such as `invalid_args`, `unsupported`, `busy`, `node_offline`, or
`confirmation_required`.

Raw scans are diagnostic observations, not stable transition events. The ESP polls
the four quadrants sequentially, assigns one `scan_id`, and emits the aggregate only
after all responses arrive or their deadlines expire. `complete` is false when any
node is missing. Continuous raw streaming is deliberately bounded and lower
priority than event polling; the ESP may lengthen the requested interval and reports
the effective settings in the command result. The frontend should plot ADC counts
(0-1023) and may derive volts as `adc * measured_avcc_mv / 1023`; it must retain the
raw counts because AVCC and analog gain vary during bring-up.

## Heartbeat and reconnect

Either peer may send WebSocket ping frames. The device also publishes
`device.status` at the negotiated heartbeat interval. If transport is lost, the ESP
uses jittered exponential backoff from 1 second to 60 seconds while local sensing
and lighting continue. It buffers only the newest board snapshot plus a bounded
ring of recent events. On reconnect it sends `hello`, then a fresh snapshot; it
does not imply exactly-once event delivery.

## Frontend/server bring-up checklist

1. Accept `hello`, reply `welcome`, and log `device_id` plus `boot_id`.
2. Store the latest `board.snapshot` keyed by device.
3. Apply `sensor.changed` only when `(boot_id, seq)` advances without a gap;
   request `board.snapshot.get` after a gap.
4. Display all four node health summaries and calibration state.
5. Track command state asynchronously by `id`; do not optimistically claim
   hardware application.
6. Provide an explicit raw diagnostics view, but build normal UI state from
   semantic messages.
