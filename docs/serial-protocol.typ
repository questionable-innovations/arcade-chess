#set heading(numbering: "1.1")

= Serial protocol v1 direction

#block(
  fill: rgb("fff7e6"),
  stroke: 0.8pt + rgb("c28a28"),
  inset: 9pt,
  radius: 4pt,
)[
  *Status: design contract, not frozen wire assignments.* This replaces the early
  broadcast C-struct sketch. Numeric message IDs, exact payload layouts, timeout
  values, and discovery timing must be frozen with golden byte vectors before
  firmware modules depend on them.
]

== Bus rules

- ESP32 address `0x80` is the only normal transaction initiator.
- Quadrants use ISP-provisioned addresses `0x00` through `0x03`.
- `0xFF` is broadcast and must never cause a response.
- `0xFE` identifies an unassigned node during scheduled discovery.
- Nodes report queued events only in addressed responses.
- Each request receives zero or one bounded response with the same sequence number.
- A retry cannot repeat a non-idempotent action such as a relay pulse.
- Bad CRC, impossible length, unknown version, wrong destination, or a timed-out
  partial frame is discarded and counted.

== Framing

The decoded frame is COBS-encoded and terminated on the wire with `0x00`.

#table(
  columns: (1.4fr, 0.6fr, 2.8fr),
  table.header([*Field*], [*Bytes*], [*Meaning*]),
  [Protocol version], [1], [`1` for the first compatible contract.],
  [Flags], [1], [Response, event pending, acknowledgement required, or error.],
  [Source], [1], [ESP, assigned node, or unassigned discovery source.],
  [Destination], [1], [One assigned address or broadcast.],
  [Message type], [1], [Stable numeric operation/event enum.],
  [Sequence], [1], [Correlates request, response, retry, and backend command.],
  [Payload length], [2], [Little-endian and checked before copying.],
  [Payload], [N], [Explicit serialization; never a compiler C struct.],
  [CRC-16/CCITT], [2], [Covers the decoded header and payload.],
)

The v1 decoded-frame limit should begin at 128 bytes so an AVR can use fixed
buffers. COBS, CRC, and serializers are shared between PlatformIO targets and host
tests. Rust compatibility is checked using the same golden vectors.

== Transaction lifecycle

1. The ESP selects one node and sends a request.
2. The addressed node validates the complete frame before acting.
3. The node waits the specified turnaround interval and sends one response.
4. The ESP accepts only the expected source, type, sequence, and response window.
5. Timeout or corruption updates health counters. Idempotent work may be retried;
   output actions use a deduplication/correlation key.
6. Repeated failures degrade then remove the node until a successful probe or
   rediscovery restores it.

On quadrant hardware, PD1 transmits through D8 onto the wired-low return. Firmware
must preserve UART idle-high and must never schedule two responders. If WS2812
output disables interrupts, bus traffic occurs only in the agreed bus-active window.

== Message families

#table(
  columns: (1fr, 3fr),
  table.header([*Family*], [*Initial messages*]),
  [Core], [`PING`, `INFO`, `CAPABILITIES`, `STATUS`, `RESET_CAUSE`, `TIME_SYNC`,
    `CONFIG_GET`, `CONFIG_SET`, `ERROR`],
  [Discovery], [`DISCOVER`, `IDENTIFY`, `ASSIGN_ADDRESS`, `RENEW_LEASE`],
  [Events], [`POLL_EVENTS`, `EVENT_BATCH`, `GET_SNAPSHOT`],
  [Quadrant], [`SENSOR_SNAPSHOT`, `SENSOR_EVENT`, `CALIBRATE`,
    `CALIBRATION_RESULT`],
  [Lighting], [`SET_SCENE`, `RUN_EFFECT`, `STOP_EFFECT`, `SET_BRIGHTNESS`,
    `CLEAR_LAYER`],
  [Clock], [`CLOCK_SET`, `CLOCK_START`, `CLOCK_PAUSE`, `CLOCK_TEXT`,
    `CLOCK_BUTTON_EVENT`],
  [Relay], [`RELAY_ARM`, `RELAY_PULSE`, `RELAY_OFF`, `RELAY_STATUS`,
    `RELAY_ALL_OFF`],
  [Firmware], [`FW_PREFLIGHT`, `MAINTENANCE_BEGIN`, `FW_PREPARE`,
    `FW_ENTER_BOOTLOADER`, `MAINTENANCE_END`, `FW_HEALTH`, `FW_CONFIRM`],
)

== Module discovery

Quadrants do not need runtime collision discovery because their IDs are programmed
and verified during manufacturing. Each hot-plug module has a persistent 64-bit
identity. If the connector cannot provide a deterministic port address, the ESP
broadcasts a discovery epoch and slot count. Unassigned modules choose a response
slot from a hash of identity and epoch. A collision yields an invalid response and
is retried with another epoch or a larger window. The ESP then assigns a leased
runtime address in the module range `0x10` through `0x7F`.

== High-level lighting contract

The ESP sends effects rather than LED frames. A lighting payload describes effect,
target mask, layer, palette, synchronized start time, duration, speed, easing, and
repetition. Every quadrant renders from its local monotonic clock. A time-sync
update adjusts future start epochs without visibly jumping an active animation.

== Backend relationship

The WebSocket API does not tunnel arbitrary user bytes to the UART. It exposes
typed semantic commands and may mirror decoded bus frames for diagnostics. Each
remote command receives a correlation ID and progresses through accepted, applied,
rejected, or timed-out state after ESP-side validation and the actual bus response.

== Bootloader mode

Firmware update begins in the framed protocol and is always addressed to one node.
After preflight, the ESP broadcasts a no-response maintenance lease so non-target
nodes ignore the temporary raw programmer stream. The target persists an update
marker, ACKs `FW_ENTER_BOOTLOADER`, drains its UART, makes outputs safe, and uses a
watchdog software reset into the protected bootloader. Only then does the ESP switch
the bus to the tested STK500-compatible programming subset.

Normal framed traffic resumes after readback verification and application reboot.
Success additionally requires matching `FW_HEALTH`, then `FW_CONFIRM` and its ACK
to mark the candidate application valid. A recoverable failure ends through the
maintenance lease without falsely confirming the application.
The complete state machine, artifact contract, recovery rules, and observability
schema are defined in `avr-ota.typ`.
