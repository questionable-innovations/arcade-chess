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
- The maximum accepted device-to-server JSON message is 4096 bytes. The server should keep
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
| `board.snapshot` | `squares`: 64 values (`-1` negative polarity, `0` empty/uncertain, `1` positive polarity), `valid`: 64 booleans, `online_node_mask`, `online_node_count`, and four node-slot summaries |
| `sensor.changed` | `square` 0-63, `state`: `empty`, `positive`, `negative`, or `uncertain`, `raw`, `baseline`, `node`, `local_square` |
| `sensor.raw_scan` | `scan_id`, `complete`, `captured_ms`, `target_node_mask`, `response_node_mask`, `online_node_mask`, and 64-entry arrays: `raw_adc`, `baseline_adc`, `noise_adc`, `state`; missing/offline squares are `null` |
| `node.status` | `node`, `online`, `firmware`, `calibrated`, `reset_cause`, `reboots`, and the counters below |
| `diagnostic.log` | `level`, `component`, `message`, optional `node` and `suppressed`; rate limited and never contains credentials |
| `diagnostic.bus` | `direction`, `node`, `uart_seq`, `message_type`, `result`; `code` when `result` is `error`, and optional `raw_hex` when trace mode is enabled |
| `calibration.progress` | `node`, `phase`, `samples`, `percent` |
| `calibration.result` | `node`, `ok`, `baseline`: 16 values, `noise`: 16 values, optional `reason` |
| `device.status` | Wi-Fi RSSI, heap, uptime, WebSocket reconnect count, UART health, and the counters below |

### Health counters

These exist so an intermittent fault leaves evidence. Anything that silently
drops data on this stack increments something here.

`node.status` reports the ESP's own view of a quadrant plus, when that quadrant
runs firmware whose `STATUS` payload reaches 19 bytes, the quadrant's own
counters. The node-sourced fields are **omitted entirely** on older firmware
rather than sent as zero, so "the counter is zero" and "the node cannot tell
you" never read the same downstream.

| field | source | meaning |
| --- | --- | --- |
| `timeouts`, `consecutive_timeouts` | ESP | unanswered polls for this node |
| `last_seen_ms` | ESP | device-monotonic time of the last good response |
| `reboots` | ESP | times the node's uptime was observed to go backwards |
| `node_uptime_ms` | node | the quadrant's own `millis()` |
| `event_depth` | node | sensor events queued on the node right now |
| `last_scan_ms` | node | duration of the node's last full scan |
| `node_rx_good`, `node_rx_bad` | node | frames the node decoded / rejected |
| `node_event_overflow` | node | **sensor transitions the node dropped** because its queue was full |
| `supply_mv` | node | measured supply rail |

`node_event_overflow` and `node_rx_bad` are the two that mean data loss; a
non-zero value is the signal that the board is lying about its own state.

`device.status` adds `uptime_ms` (milliseconds — the older `uptime` field is the
same value under a name that read as seconds, and is kept for one release),
`ws_send_failed`, `events_dropped_offline` (events discarded while the socket
was not welcomed), `snapshot_repairs` (squares corrected by a `GET_SNAPSHOT`
reconciliation — non-zero means events are being lost between node and ESP),
`raw_stream` and `trace` (the device's actual streaming/trace state, so a client
cannot keep claiming a capture that already expired), and `reset_reason`.

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
The ESP rejects malformed, stale, unsupported, or unsafe commands with one of
these stable reasons — the set is closed, so a client may switch on it:

| reason | meaning |
| --- | --- |
| `invalid_args` | the args failed validation |
| `unsupported` | no such command name on this firmware |
| `busy` | the bus command queue could not accept the request |
| `node_offline` | the named node is not responding |
| `target_node_offline` | no addressed square belongs to an online node |
| `no_nodes_online` | the command needs at least one quadrant and there are none |
| `bus_queue_full` | the request would not fit in the outbound bus queue |
| `confirmation_required` | `device.restart` without the exact `confirm` string |
| `timeout` | the node never answered |
| `partial_scan` | a raw scan completed for only some target nodes |
| `node_error` | the node refused; `data` carries `{ node, code }` with the UART error code from the table in [uart-api.md](uart-api.md) |

Quadrants are optional runtime participants. The ESP continuously discovers node
addresses 0-3, polls online nodes normally, and probes empty sockets with bounded
backoff. One, two, three, or four installed quadrants are all healthy board
configurations. Commands naming a particular offline node are rejected; commands
using `"all"` fan out only to nodes online when the command is accepted. A node
that reconnects is rediscovered and receives the persisted orientation and runtime
mode before normal polling resumes.

Raw scans are diagnostic observations, not stable transition events. The ESP
targets the quadrants online when the scan starts, assigns one `scan_id`, and emits
the aggregate after those nodes respond or time out. `complete` means every node
in `target_node_mask` responded; it does not require all four sockets to be
populated. `response_node_mask` identifies the contributors, while all squares in
unpopulated quadrants remain `null`. Continuous raw streaming is deliberately
bounded and lower priority than event polling; the ESP may lengthen the requested interval and reports
the effective settings in the command result. The frontend should plot ADC counts
(0-1023) and may derive volts as `adc * measured_avcc_mv / 1023`; it must retain the
raw counts because AVCC and analog gain vary during bring-up.

## Heartbeat and reconnect

Both peers ping. The server sends a WebSocket ping every `heartbeat_ms` and
drops a device that sends nothing at all for 45 seconds; the device pings on the
same interval and also publishes `device.status`. Either side alone is enough to
collapse a half-open TCP flow — a silent NAT eviction or a power cut produces no
FIN, so without a deadline the peer stays "connected" forever.

If transport is lost, the ESP uses jittered exponential backoff from 1 second to
60 seconds while local sensing and lighting continue. Events produced while the
socket is down are dropped, not buffered, and counted in
`device.status.events_dropped_offline`; on reconnect the ESP sends `hello` and
then a fresh `board.snapshot`, which supersedes everything missed. There is no
exactly-once event delivery.

## Frontend/server bring-up checklist

1. Accept `hello`, reply `welcome`, and log `device_id` plus `boot_id`.
2. Store the latest `board.snapshot` keyed by device.
3. Apply `sensor.changed` only when `(boot_id, seq)` advances without a gap;
   request `board.snapshot.get` after a gap.
4. Display all four node slots and their health, but treat any population from one
   through four online quadrants as operational.
5. Track command state asynchronously by `id`; do not optimistically claim
   hardware application.
6. Provide an explicit raw diagnostics view, but build normal UI state from
   semantic messages.
