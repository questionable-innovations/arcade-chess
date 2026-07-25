#set page(
  paper: "a4",
  margin: (x: 2cm, y: 2cm),
  numbering: "1",
)
#set heading(numbering: "1.1")
#set table(inset: 6pt, stroke: 0.5pt + rgb("c8c8c8"))

#align(center)[
  #text(size: 22pt, weight: "bold")[Arcade Chess]
  #linebreak()
  #text(size: 13pt)[Project context and system architecture]
  #v(0.5em)
  #text(size: 9pt, fill: rgb("555555"))[
    Electronics repository · schematic revision 1R0 · firmware planning context
  ]
]

#v(1em)

#block(
  fill: rgb("eef5ff"),
  stroke: 0.8pt + rgb("6a8bb8"),
  inset: 10pt,
  radius: 4pt,
)[
  *Purpose of this document.* Give engineers and software agents enough context to
  make changes without accidentally moving responsibility to the wrong processor,
  flooding the serial bus, breaking smooth animation, or treating a dangerous
  output as an ordinary UI control. Detailed implementation work belongs in
  `firmware-plan.md`; confirmed board-level details belong in `board.typ`.
]

= Product vision

Arcade Chess is a physical, illuminated chessboard designed as a platform rather
than a single-purpose appliance. The base board senses pieces, guides play with
local lighting, validates moves while offline, and connects to a remote chess
service when online. Modules attached through a USB-C-shaped serial connector can
extend the experience without replacing the base board.

The initial module concepts are:

- a two-sided chess clock that displays time or text and reports two buttons; and
- a two-channel relay module for external effects such as lights, horns, or other
  isolated loads.

The desired experience is responsive and physical. Sensor feedback, clock updates,
and lighting animation must remain smooth even when Wi-Fi is slow or unavailable.
The browser interface begins as a live viewer and should evolve into an accessible
controller and configuration surface for the board.

= Responsibility boundaries

#table(
  columns: (1fr, 2.4fr, 1.6fr),
  table.header([*Component*], [*Owns*], [*Must not own*]),
  [ESP32 controller],
  [Canonical physical board state, chess rules, move transactions, bus scheduling,
   effect orchestration, Wi-Fi, backend session, configuration, and diagnostics.],
  [Per-frame rendering of all 320 LEDs or direct sensor sampling.],
  [Quadrant ATmega],
  [Scanning and classifying 16 Hall sensors, queueing stable transitions, and
   rendering four local LED chains from high-level effect instructions.],
  [Chess rules, backend state, or unsolicited serial transmission.],
  [Hot-plug module],
  [Its local input/output timing, display/rendering, capability report, and
   fail-safe behavior.],
  [Becoming a second bus master.],
  [Rust backend],
  [Authenticated device sessions, event fan-out, persistence as required, typed
   remote commands, and full Stockfish analysis.],
  [Real-time sensor or LED deadlines.],
  [SvelteKit frontend],
  [Viewing state and offering friendly, asynchronous board controls.],
  [Writing raw UART bytes or assuming a command was applied before acknowledgement.],
)

#pagebreak()

= End-to-end architecture

```text
SvelteKit viewers/controllers
           |
           | typed WebSocket events and commands
           v
Rust backend + Stockfish
           |
           | authenticated TLS WebSocket
           v
ESP32 controller (the only UART initiator)
           |
           | shared request/response UART bus
           +----------+----------+----------+----------+
           v          v          v          v          v
       Quadrant 0  Quadrant 1  Quadrant 2  Quadrant 3  Modules
       sensors +   sensors +   sensors +   sensors +   clock,
       80 LEDs     80 LEDs     80 LEDs     80 LEDs     relay, ...
```

The frontend is an asynchronous extension of the board, not an electrical or
logical UART master. The backend may receive a diagnostic copy of bus traffic, but
normal application behavior uses semantic events such as square changes, moves,
clock state, node health, and effect status. Remote commands are validated by the
backend, validated again by the ESP32, scheduled on the bus, and acknowledged with
accepted, applied, rejected, or timed-out status.

= Electronics summary

The full base board contains one ESP32-WROOM-32E and four identical 5 V,
16 MHz ATmega328PB quadrants. Each quadrant owns 16 squares and therefore:

- 16 SS49E analog Hall sensors;
- two 74HC4051 eight-channel analog multiplexers;
- one TLC082 dual op-amp package for conditioned Hall readings;
- 64 square LEDs split across two 32-pixel WS2812 chains; and
- 16 edge LEDs split across two independent eight-pixel chains.

The complete board has 64 Hall sensors and 320 RGB LEDs. The four LED outputs per
quadrant are PE3 primary squares, PE2 secondary squares, PE0 edge half-bar 1, and
PE1 edge half-bar 2. The PE0/PE1 separation depends on the planned hardware bodge.

Hall readings reach PC0/ADC0 (`SenseA`) and PC1/ADC1 (`SenseB`). The mux address
pins are PB0-PB2 for local squares 1-8 and PD4-PD6 for local squares 9-16. Empty
Hall output is near the middle of the ADC range; magnet polarity moves the
conditioned voltage above or below the per-square baseline. Firmware therefore
classifies empty, polarity A, polarity B, and uncertain states using calibration,
filtering, hysteresis, and debounce.

The schematic does not determine final chess coordinates. Quadrant rotation,
mirroring, square order, two-pixel orientation, and edge-bar direction must be
captured from walking-pixel and magnet tests on assembled hardware.

#pagebreak()

= Shared serial network

The ESP32 transmits on GPIO16, which the schematic calls `BusRX`. All nodes receive
this line on their UART RX inputs. Quadrants return data from PD1 through Schottky
diode D8 onto `BusTX`, which is pulled up to 3.3 V by 10 kohm and received by the
ESP32 on GPIO17. The diode blocks each AVR's 5 V idle-high output and allows an
active transmitter to pull the shared line low.

Important invariants:

- The ESP32 is the only initiator outside explicitly scheduled discovery slots.
- Only one node may respond to a request.
- A broadcast never receives a response.
- Nodes queue events locally and return them when polled; they never speak
  spontaneously.
- Corruption, timeouts, duplicate commands, resets, and queue overflow must be
  detectable and recoverable with a fresh snapshot.
- Output commands, especially relay pulses, must be idempotent under retries.

Bring-up starts at 38,400 baud, 8-N-1 because the diode-OR return and 10 kohm pull-up
may have slow rising edges. Faster rates require scope and error-rate testing with
all nodes and the longest supported cable.

Frames use a versioned, bounded binary envelope with source, destination, type,
sequence, payload length, and CRC-16/CCITT. COBS encoding with a zero delimiter
allows a parser to recover cleanly after dropped or malformed bytes. Quadrants have
IDs 0-3 provisioned during ISP flashing. Modules use persistent unique identities
and either deterministic port addresses or collision-tolerant discovery followed
by a leased runtime address.

The USB-C-shaped expansion connector is *not USB data*. Its D+/D- contacts carry
the two UART nets through series resistors, and VBUS carries 5 V. Documentation,
enclosures, and firmware UX must avoid implying that arbitrary USB devices are
compatible.

= Real-time firmware model

Quadrant firmware is a cooperative deadline-driven event loop with no blocking
delays. UART reception, one sensor scan step, filtering/debounce, command dispatch,
animation advancement, LED transport, and diagnostics are serviced incrementally.

The ESP sends compact effect descriptions rather than RGB frames. An instruction
contains an effect ID, target squares or edge, layer, palette, synchronized start
time, duration, speed, easing, and repetition. The ATmega interpolates locally.
Suggested layers are ambient, evaluation, legal moves, selection/move progress,
illegal-move warning, and fault/identify.

WS2812 output is a central timing constraint. Sending all 80 pixels takes about
2.4 ms per quadrant, and common AVR drivers disable interrupts during this period.
The implementation must prove either an interrupt-safe transport or synchronized
bus-active and LED-render windows. All quadrants may render concurrently, but every
quadrant hears all ESP bus traffic.

= Chess state and offline behavior

The ESP32 should initially contain a complete chess rules engine and move validator,
not a strong search engine. Hall polarity distinguishes sides but not individual
piece types. The controller begins from a verified position, tracks inferred piece
identity, and interprets lift/place sequences as move transactions.

The move coordinator must cover captures performed in either physical order,
cancelled lifts, castling, en passant, promotion choice, takebacks, multiple lifted
pieces, lost events, and ambiguous positions. It must request or guide a resync when
there is not exactly one defensible state; it must not guess.

Sensing, legal/illegal feedback, clocks, and local effects continue offline.
Stockfish analysis and remote control are optional network services and cannot
block the local event loop.

= Module behavior and safety

Modules report identity, hardware revision, firmware version, capabilities, reset
reason, and health. They receive time synchronization and a controller/session
lease, and enter a defined local state when that lease expires.

The clock keeps time and handles button/display feedback locally. The controller
sends an absolute state plus synchronized start epoch rather than continuous display
frames.

The relay module is fail-off. Both outputs are off at boot, reset, disconnect,
protocol failure, and lease expiry. Activation requires a short-lived arm operation,
bounded pulse duration, deduplication, and an explicit maximum enforced by the
module itself. Active outputs are never restored after reboot. Loads capable of
injury or damage require suitable isolation and a physical inhibit/interlock;
authentication and software are not safety barriers.

= ATmega firmware updates

Every ATmega must be updateable by the ESP32 over only the shared UART lines. The
application accepts an addressed or coordinated all-node CRC-protected maintenance
command, persists a one-time update request, drains any ACK, and enters the protected
resident bootloader through an explicit one-shot handoff. The ESP can program one
target or make a selected set consume one shared stream while only one leader
responds. It restores the framed protocol and confirms each new application's CRC
and self-test before declaring success.

The website starts the same operation through a durable backend job. Firmware is
validated and fully staged on the ESP before the bus enters maintenance, so loss of
the browser or network does not interrupt programming. Interrupted writes leave the
target recoverable in its protected bootloader; ISP remains the root recovery path.

Update observability is part of correctness. Backend and UI consume structured,
sequenced phase/progress/error events correlated by update and attempt IDs. Success
requires readback verification, `FW_HEALTH`, and an acknowledged `FW_CONFIRM`, not
merely a completed file upload or page-write ACK. See `avr-ota.typ` for the memory,
artifact, recovery, state-machine, and telemetry contracts.

= Network contract

Each board opens an authenticated TLS WebSocket to the Rust service. Prototype
authentication may use a static header, but credentials must be distinct per board,
revocable, and replaceable. A reconnect begins with a protocol/schema handshake,
device and node inventory, current snapshot, and event sequence information.

The ESP publishes semantic state plus an optional bounded diagnostic bus trace.
Trace entries include session, direction, ESP timestamp, node, frame sequence, type,
decoded payload, and result. Raw frames are useful diagnostics but are not the only
backend API. Stockfish output is a separate semantic stream and does not mutate the
physical board unless an authorized command explicitly requests an effect or mode.

= Engineering priorities

Work proceeds in this order:

1. Prove bodges, pin continuity, voltage margins, diode-bus timing, and LED/UART
   coexistence on one then four quadrants.
2. Freeze protocol v1 framing, golden vectors, parser tests, IDs, timeouts, retries,
   and diagnostics.
3. Deliver reliable sensor calibration/classification and smooth local effects on
   one quadrant.
4. Integrate all quadrants, coordinate mapping, health, reconnect, and resync.
5. Implement local chess transactions and complete rule validation.
6. Connect the authenticated backend, Stockfish stream, and viewer/controller.
7. Add the clock, then the relay after its safety contract and physical interlock
   are defined.
8. Deliver addressed ATmega UART updates with a protected bootloader, verified
   page programming, interruption recovery, and end-to-end observable web jobs.
9. Harden with watchdogs, brownout testing, ESP OTA rollback, manufacturing tests,
   soak tests, and fault injection.

= Sources of truth for agents

#table(
  columns: (1.2fr, 2.8fr),
  table.header([*File*], [*Role*]),
  [`docs/project-overview.typ`], [Goals, ownership, invariants, and system context.],
  [`docs/firmware-plan.md`], [Detailed implementation phases, protocol proposal,
    acceptance gates, risks, and open decisions.],
  [`docs/board.typ`], [Firmware-facing electrical and pin-level board summary.],
  [`docs/serial-protocol.typ`], [Short protocol-v1 contract; detailed numeric wire
    assignments still need to be frozen.],
  [`docs/avr-ota.typ`], [ATmega bootloader entry, artifact, programming, recovery,
    remote website workflow, and update observability contract.],
  [`docs/TeamTwo_ArcadeChess.pdf`], [Source schematic revision 1R0.],
  [`firmware-atmega/`], [Quadrant and module firmware targets.],
  [`firmware-esp/`], [ESP32 controller firmware.],
  [`frontend/`], [SvelteKit viewer and future controller.],
)

When documents conflict, prefer a continuity-checked schematic/hardware result over
a prose assumption, then update every affected document. Do not silently encode
board orientation, electrical timing, relay safety policy, or protocol numbers in
firmware before the corresponding open decision has been recorded.
