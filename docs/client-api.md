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

## Puzzle mode

Two message types, additive on the same `/ws`. Unknown types are already ignored
by contract, so a client that predates them keeps working.

### `game.state` (server → all clients)

Full snapshot on change, coalesced to at most 10 Hz, and embedded in `init` as a
top-level `game` field so a browser refresh mid-game is free. Full snapshots
match the rest of this API: they supersede, so there are no incremental sync
bugs to have.

```json
{
  "type": "game.state",
  "game_seq": 17,
  "phase": "idle | setup | countdown | playing | awaiting_choice | scoring | finished",
  "device_id": "arcade-chess-001",
  "position": { "id": "a1b2c3", "start_fen": "8/5k2/… w - - 0 1",
                "verified_cp": 12, "drop_cp": 340 },
  "start_fen": "…", "fen": "current FEN", "turn": "white",
  "ply": 3, "max_ply": 10,
  "moves": [{ "uci": "e4d5", "san": "exd5", "by": "sensor | manual | chosen | autopilot",
              "confidence": "certain | likely" }],
  "legal_moves": ["d6d7", "d6e7"],
  "setup": { "placed": 6, "needed": 8, "missing": [12, 44], "extra": [13],
             "unknown": [55], "auto_start_in_ms": null },
  "detect": { "mode": "auto | suggest | off", "sensors_live": true,
              "board_synced": true, "mismatch": [], "masked": [], "rotation": 0,
              "nudge": null, "observed": "..+.-?x…" },
  "choice": { "kind": "capture | promotion | no_match | suggest",
              "prompt": "Which capture was that?",
              "options": [{ "uci": "e4d5", "san": "exd5", "confidence": "likely" }] },
  "eval": { "cp": 34, "mate": null, "win_prob": 0.53, "status": "ok | pending",
            "source": "stockfish | material | admin | unknown", "depth": 14, "start_cp": 12 },
  "result": { "winner": "white | black | draw", "final_cp": 123, "start_cp": 12,
              "swing": 111, "reason": "eval | mate | stalemate | admin" },
  "tunables": { "settle_ms": 700, "…": 0 },
  "settings": [{ "key": "settle_ms", "group": "detection", "label": "settle window",
                 "kind": "int", "min": 100, "max": 3000, "step": 50, "unit": "ms",
                 "live": true, "help": "…" }],
  "palette": { "alert": "d02020", "needed": "ff8c00", "focus": "2a6ad0",
               "sweep": "f0f0f0", "bar_white": "e8e8e8", "bar_black": "303452" },
  "lighting": { "squares": "override", "bars_supported": true, "bar_map": [],
                "eval_sides": [0, 2], "turn_sides": [1, 3],
                "colours_neutralised": true },
  "autopilot": null,
  "deck": { "source": "/srv/positions.json", "count": 4821, "skipped": 3 },
  "degraded": ["node2_offline", "engine_unavailable"]
}
```

`detect.observed` is a 64-character string — `.` empty, `+` positive polarity,
`-` negative, `?` mid-transition, `x` unknown — rather than a 64-entry array,
which matters at 10 Hz to every connected client. `sensors_live` is false when no
device is bound or its events have gone stale, so the UI says "manual mode"
instead of pretending. `setup` appears only during `setup`/`countdown`, `choice`
only when one is pending, `result` only in `finished`.

`degraded` is a product feature, not debug output: it renders as amber chips and
gives whoever is presenting something honest to say when a subsystem drops.
Stable values are `no_device`, `sensors_stale`, `node<N>_offline`,
`engine_unavailable`, `bars_unsupported`, `positions_fallback`, `stream_gap`,
`verdict_incomparable`,
`restored_after_restart`, `detect_suggest`, `detect_off`,
`sensor_<square>_masked` and `sensor_<square>_suspect`. Clients must render an
unrecognised code rather than dropping it.

`eval.source` is never guessed at. A material count is labelled `material`, an
operator's decree is labelled `admin`, and only a real search is `stockfish`.

### `game` (client → server)

Gated exactly like `command`. Everything is admin-only by default; setting
`GAME_OPEN_CONTROLS=1` relaxes the player-facing subset — `new_game`, `start`,
`move`, `choose`, `undo`, `resync` — for an unauthenticated tablet at the board.

```json
{ "type": "game", "action": "new_game", "position_id": "optional", "fen": "optional" }
{ "type": "game", "action": "start" }                    // force-start from setup
{ "type": "game", "action": "move", "uci": "e2e4" }
{ "type": "game", "action": "choose", "uci": "e4d5" }    // "" dismisses the prompt
{ "type": "game", "action": "undo" }
{ "type": "game", "action": "resync" }                   // board == game state now
{ "type": "game", "action": "set_detect", "mode": "auto | suggest | off" }
{ "type": "game", "action": "mask_square", "square": 27, "masked": true }
{ "type": "game", "action": "set_config", "key": "settle_ms", "value": 700 }
{ "type": "game", "action": "set_tunables", "settle_ms": 700 }   // older flat form, still accepted
{ "type": "game", "action": "set_eval", "cp": 150 }
{ "type": "game", "action": "set_fen", "fen": "…" }      // overwrite the position
{ "type": "game", "action": "rescore" }
{ "type": "game", "action": "end", "winner": "white" }
{ "type": "game", "action": "abort" }
{ "type": "game", "action": "bind_device", "device_id": "…" }
{ "type": "game", "action": "set_rotation", "degrees": 180 }
{ "type": "game", "action": "bars_map", "side": 0, "half": 1, "node": 2, "strip": "a", "reversed": true }
{ "type": "game", "action": "bars_test", "node": 2, "strip": "a", "pixel": 3 }
{ "type": "game", "action": "autopilot", "on": true, "interval_ms": 4000 }
{ "type": "game", "action": "bars_sides", "eval_sides": [0, 2], "turn_sides": [1, 3] }
{ "type": "game", "action": "set_palette", "needed": "ff8c00" }
{ "type": "game", "action": "node_config", "node": 2, "key": 6, "value": 96 }
```

`settings` is the schema the admin rail renders itself from: one descriptor per
tunable, carrying the range the server also enforces. Pair it with `tunables`,
which holds the current value under the same key. Adding a knob is one entry in
`config::SETTINGS` and no client change at all — and because both ends read the
same table, the UI cannot offer a value the server would refuse. Values outside a
declared range are **clamped**, not rejected; an unknown key is `invalid_args`.

`node_config` is a pass-through to the AVR's own EEPROM keys (see the config-key
table in `docs/uart-api.md`) — sensor thresholds, debounce, LED brightness and
per-quadrant orientation. Each write commits EEPROM (~240 ms), so it is one key
per call and human-driven, never per frame.

Rejections reuse the `error` envelope (`unauthorized`, `invalid_args`); a success
is acknowledged by the next `game.state`.

Square indices are the same ones the device uses — `a1 = 0`, row-major — with no
conversion anywhere. `set_rotation` is the one place that changes, and it applies
to occupancy interpretation and to lighting together.

## HTTP endpoints

- `GET /healthz` — `200 ok`, plain text.
- `GET /api/state` — JSON `{ "devices": [DeviceView…] }`, the same shape as
  `init`. Useful for curl-based debugging.
- `GET /api/game` — the latest `game.state`, for when the UI is the thing that
  is broken.

## Server environment

| Variable | Required | Meaning |
| --- | --- | --- |
| `ADMIN_PASSWORD` | yes | Static admin password for client `auth`. |
| `PORT` | no | HTTP/WebSocket listen port, default `8080`. |
| `DEVICE_TOKEN` | no | If set, device upgrades to `/board` must carry `Authorization: Bearer <token>`. |
| `POSITIONS_PATH` | no | Puzzle deck, JSON array or JSON lines, each record needing only `fen`. Falls back to the deck compiled into the binary. |
| `STOCKFISH_PATH` | no | Engine binary, default `/usr/games/stockfish` with a `PATH` lookup behind it. Eval falls back to a material count, labelled. |
| `GAME_OPEN_CONTROLS` | no | `1` lets unauthenticated clients drive the player-facing game actions. Also live-settable as `open_controls`. |
| `GAME_SNAPSHOT_PATH` | no | Where the game is persisted, default `/tmp/arcade-game.json`, so a restart mid-demo is survivable. Written on every ply. |
| `CONFIG_PATH` | no | Where the **venue profile** is persisted, default `/srv/venue.json`. Deliberately not `/tmp`: on a container platform `/tmp` does not survive the container being replaced, which is exactly the event this file exists to survive. |
| `MAX_PLY` | no | Plies per game, default `10`. Also live-settable; takes effect on the next new game. |
| `SETTLE_MS`, `AUTOSTART_STABLE_MS`, `UNKNOWN_TOLERANCE`, `TIER3_*`, `AUTO_MASK_STREAK`, `DRAW_BAND_CP`, `COUNTDOWN_MS`, `AUTOPILOT_INTERVAL_MS` | no | Boot defaults for the tunables. All are live-editable via `set_config`, which is the point — none of their right values are knowable before the hardware is on the table. On a container platform an environment variable means a restart, so treat these as build defaults and the admin rail as the actual control. |

### The venue profile

Ten minutes of setup at the venue — binding the board, squaring up its mounting,
masking whichever sensors lie, assigning the bar map, choosing a detect mode — is
the expensive artifact of the evening; the five-minute puzzle can simply be
replayed. So all of it is written to `CONFIG_PATH` on every change and read back
at boot *before* the game snapshot, and the tunables and palette ride along with
it. Auto-masks are deliberately excluded: they are per-game evidence about a
position that no longer exists, not calibration.

The file is versioned. A profile written by a different build is discarded with a
log line rather than half-applied, and any value outside the range the server
currently advertises is clamped on the way in — a hand-edited file cannot smuggle
in something the wire would refuse.

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
