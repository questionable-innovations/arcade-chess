# Arcade Chess UART protocol v1 — bring-up contract

This freezes the numeric subset implemented by the initial ESP32 and quadrant
firmware. The shared implementation is in `protocol/`. Multi-byte integers are
little-endian. A decoded frame is:

`version, flags, source, destination, type, sequence, payload_length:u16, payload, crc:u16`

It is COBS-encoded and terminated by `00`. CRC-16/CCITT-FALSE covers the decoded
header and payload. Maximum payload is 112 bytes. ESP is `0x80`, quadrants are
`0x00`-`0x03`, and broadcast is `0xff`. Flags are response `0x01`, event-pending
`0x02`, ack-required `0x04`, and error `0x08`.

Only the ESP initiates. A quadrant responds only to its address, never to a
broadcast, and copies the request sequence. Initial bus settings are 38,400 baud,
8-N-1. ESP allows 700 microseconds turnaround and a 20 ms normal response deadline;
calibration has its own longer lifecycle. Bytes arriving during LED refresh are a
known hardware-validation gate, so the bring-up firmware prioritizes UART and caps
LED refresh rate.

## Message payloads

| ID | Name | Request payload | Response payload |
| ---: | --- | --- | --- |
| `0x01` | `PING` | empty | `uptime_ms:u32` |
| `0x02` | `INFO` | empty | `fw_major, fw_minor, fw_patch, hw_rev, node_id, capabilities:u16` |
| `0x03` | `STATUS` | empty | `uptime_ms:u32, reset_cause, calibrated, event_depth, last_scan_ms:u16, rx_good:u16, rx_bad:u16, event_overflow:u16, supply_mv:u16` |
| `0x05` | `CONFIG_GET` | optional `key` (`0` means all) | repeated `key, value:u16`; see keys below |
| `0x06` | `CONFIG_SET` | repeated `key, value:u16` | effective repeated values; persisted with CRC |
| `0x20` | `POLL_EVENTS` | `max_events` | zero to eight event records: `count`, then repeated `local_square, state, raw_adc:u16, at_ms:u32` |
| `0x22` | `GET_SNAPSHOT` | empty | `state[16], raw_adc[16]:u16` |
| `0x24` | `GET_RAW_SCAN` | `samples_per_square` (1-32) | `sample_count, measured_avcc_mv:u16`, then for each of 16 squares `raw_adc:u16, baseline_adc:u16, noise_adc, state` |
| `0x30` | `CALIBRATE` | `action` (`1` start-empty, `2` cancel) | `phase, percent, samples:u16` |
| `0x40` | `SET_SQUARES` | `mask:u16, red, green, blue, duration_ms:u16` | `applied_mask:u16` |
| `0x41` | `SET_BRIGHTNESS` | `brightness` 0-255 | effective value |
| `0x42` | `IDENTIFY` | `duration_ms:u16` | effective duration |
| `0x43` | `CLEAR_LIGHTING` | optional `mask:u16`; empty means all | cleared mask |
| `0x44` | `RENDER_WINDOW` | broadcast, empty | no response; all quadrants render concurrently and ESP keeps the bus quiet for 4 ms |
| `0x50` | `SET_DEBUG` | `flags, raw_interval_ms:u16` | effective values |
| `0x60` | `FW_PREFLIGHT` | empty | `node, high_fuse, bootloader_enabled, handoff_version, page_size:u16, flash_size:u32, app_limit:u32, marker_state, reset_cause, supply_mv:u16` |
| `0x61` | `MAINTENANCE_BEGIN` | broadcast: `target, token:u32, lease_ms:u16` | no response; non-target nodes suppress responses |
| `0x62` | `FW_PREPARE` | `token:u32, update_id:u32, image_size:u32, image_crc32:u32` | echoed `token, update_id`; metadata is persisted before ACK |
| `0x63` | `FW_ENTER_BOOTLOADER` | `token:u32, update_id:u32` | echoed values, followed by TX drain, LED shutdown, and watchdog reset |
| `0x64` | `MAINTENANCE_END` | broadcast: `token:u32` | no response; normal framed traffic resumes |
| `0x65` | `FW_HEALTH` | empty | `marker_state, reset_cause, uptime_ms:u32, update_id:u32, image_crc32:u32` |
| `0x66` | `FW_CONFIRM` | `update_id:u32` | `update_id:u32, marker_state`; requires a candidate/valid marker with matching ID |

Responses echo the request type, with two exceptions: `GET_RAW_SCAN` is answered
with type `0x25` `RAW_SCAN` and `CALIBRATE` with type `0x31` `CALIBRATION_RESULT`.

`GET_RAW_SCAN` is intentionally a relatively long response: 99 payload bytes.
Noise and state use compact 8-bit fields. The ESP requests quadrants one at a time. A
quadrant takes the requested extra samples cooperatively, then responds; it never
streams unsolicited data onto the shared return line.

## Sensor state and LED defaults

Sensor states are empty `0`, positive `1`, negative `2`, uncertain `3`. Stable
positive pieces render green and stable negative pieces render blue by default.
Those colours, thresholds, hysteresis, debounce count, brightness, scan interval,
and local-to-global coordinate map are configuration rather than protocol
constants. Explicit lighting commands temporarily override the settled-piece base
layer and expire using `duration_ms`; zero means persist until cleared.

Configuration keys are enter threshold `1` (ADC counts), exit threshold `2`,
debounce scans `3`, mux settling microseconds `4`, full scan target milliseconds
`5`, brightness `6`, positive RGB565 `7`, negative RGB565 `8`, orientation `9`
(`0`-`7`: four rotations with optional mirror), and runtime mode `10` (`0`
normal, `1` bring-up). Invalid combinations are rejected;
notably exit must be lower than enter. Defaults are centralized in
`firmware-atmega/src/bringup_config.h` and can be restored by programming a
fresh EEPROM image.

## Calibration

`CALIBRATE(start-empty)` requires an empty quadrant. The node collects 128 complete
scans, computes a per-square baseline and peak-to-peak noise, rejects rail values
or excessive noise, and atomically stores a versioned CRC-protected EEPROM record.
Until calibration succeeds, the classifier uses conservative defaults and reports
`calibrated=0`. Progress is read via `STATUS`/`CALIBRATE`; no node sends unsolicited
progress.

For guided bring-up, use a one-shot or bounded raw stream over WebSocket, verify all
empty values and noise, run empty-board calibration, then place one positive and
one negative magnet on every square. Threshold and coordinate configuration can be
tuned from the ESP serial console without recompiling.

## Errors

An error response sets response+error flags, uses type `0x0f`, and contains
`request_type, code`. Codes are `1` malformed payload, `2` unsupported message,
`3` busy, `4` not calibrated, `5` invalid configuration, `6` resident bootloader
missing, `7` maintenance lease missing, and `8` token/update mismatch. Parsers count all
bad COBS, length, version, CRC, and destination failures; counters saturate rather
than wrapping.

## Bootloader handoff and UART flashing

The application implements the safe entry portion of the OTA design. `FW_PREPARE`
is accepted only when the high fuse shows `BOOTRST=0`, the maintenance target is
this node, the token matches, the image fits below the reserved boot section, and
calibration/raw capture is idle. It writes a generation-numbered, CRC-protected
marker to alternating EEPROM slots at addresses 128 and 160.

`FW_ENTER_BOOTLOADER` must repeat the token and update ID. The node advances the
marker to `programming`, ACKs and physically drains UART, turns every LED chain
off, then uses a 15 ms watchdog reset. It never jumps to a literal flash address;
the boot-reset fuse and resident bootloader own reset dispatch.

After the entry ACK the ESP stops framed traffic, switches its bus UART to the
bootloader baud, and speaks STK500v1 to Urboot: sync, 128-byte page writes with
bounded retries, then complete page readback verification. It then restores the
bus baud and broadcasts `MAINTENANCE_END`. Non-target applications ignore
programmer traffic during the 60-second maintenance lease and recover
automatically when it expires.

Marker states are `0` none, `1` requested, `2` programming, `3` candidate, and
`4` valid. An application that boots with a `programming` marker promotes it to
`candidate`: the handoff completed and this image is awaiting confirmation. The
ESP polls `FW_HEALTH`, requires a candidate marker whose `update_id` and
`image_crc32` match the staged image, and only then sends `FW_CONFIRM`, which
persists the `valid` state. Flashing is reported successful only after readback,
health, and confirmation all pass; on any failure the target is left with Urboot
resident and re-entry is always possible. The USB-console front end for this flow
is documented in [bringup.md](bringup.md).
