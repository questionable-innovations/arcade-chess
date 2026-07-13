# Arcade Chess firmware plan

Hardware basis: schematic revision 1R0 (1 July 2026) and the corrected firmware engineering handoff supplied after the initial plan. Where `board.typ` differs, the corrected handoff is treated as authoritative until continuity testing says otherwise.

## 1. Goals and boundaries

Arcade Chess has three firmware roles:

1. The ESP32 is the only bus master. It owns the canonical board state, chess rules, Wi-Fi, backend connection, configuration, and orchestration.
2. Four ATmega328PB quadrant controllers continuously scan their 16 Hall sensors, classify occupancy/polarity, and render LED effects locally.
3. Hot-plug modules are addressed bus nodes with their own event loops and capabilities. The first module types are a chess clock and a two-channel relay controller.

The first release should prioritize deterministic sensing, smooth local animation, recoverable communications, safe module behavior, useful diagnostics, and recoverable ATmega firmware updates over the shared bus. It should not initially attempt distributed chess state, direct per-frame LED streaming, or arbitrary node scripting.

## 2. Hardware gates to close before firmware implementation

The schematic handoff confirms the basic topology, but these checks still gate firmware completion:

- **Shared return line:** each 5 V AVR TX reaches `BusTX` through a Schottky diode, and a 10 kΩ resistor pulls the ESP32 side up to 3.3 V. This avoids tying four push-pull high outputs together, but the wired-low return is safe only when at most one node transmits. Scope the slow rising edge with all quadrants and the final cable connected.
- **ESP-to-AVR logic margin:** ESP32 GPIO16 drives the 5 V AVRs directly at 3.3 V. Verify that every AVR reliably recognizes high across supply, temperature, and production variation; there is no level shifter on this direction.
- **Clock/voltage pairing:** the confirmed quadrant operating point is 5 V and 16 MHz. Capture it in the board target and fuse programming specification.
- **Hot-plug behavior:** verify ground-first/power sequencing, inrush limiting, ESD protection, connector pin assignment, and that an unpowered or booting module cannot clamp either UART line.
- **Module identity:** quadrant IDs are confirmed as uniquely provisioned during initial ISP flashing. Hot-plug modules still need a factory/programmed unique ID in EEPROM unless module ports provide address straps.
- **LED timing:** each quadrant has four confirmed chains totaling 80 pixels and about 2.4 ms of WS2812 wire time. Confirm whether the chosen driver masks interrupts and define a bus-quiet/render schedule before enabling continuous animation.
- **Board bodges:** continuity-test the CH340 UART crossing fix and the independent PE0/PE1 edge-bar outputs before firmware bring-up.
- **Coordinate map:** the schematic still does not establish quadrant rotation/mirroring, local-square-to-file/rank mapping, square LED orientation, or assembled edge-bar direction. Determine these with walking-pixel and magnet tests.

Recommended bus bring-up target: 38,400 baud, 8-N-1. Raise it to 57,600 or 115,200 only after scope and error-rate tests with all nodes, the 10 kΩ return pull-up, and the longest supported cable. The exact AVR baud divisor/error must be documented for the selected rate.

### Confirmed runtime pin and chain map

Use firmware-oriented names because the schematic names `BusRX` and `BusTX` from the quadrants' perspective:

| Function | Confirmed pin/net |
| --- | --- |
| ESP bus TX to all nodes | GPIO16, schematic `BusRX` |
| ESP bus RX from nodes | GPIO17, schematic `BusTX` |
| AVR UART RX/TX | PD0 / PD1 through D8 |
| Low mux S0/S1/S2 | PB0 / PB1 / PB2 |
| High mux S0/S1/S2 | PD4 / PD5 / PD6 |
| Hall ADC low/high banks | PC0/ADC0 `SenseA`; PC1/ADC1 `SenseB` |
| Square primary chain | PE3, 32 pixels |
| Square secondary chain | PE2, 32 pixels |
| Edge half-bar 1 | PE0, 8 pixels after bodge |
| Edge half-bar 2 | PE1, 8 pixels after bodge |

The two square chains map two pixels per local square in SSM1 through SSM16 order. Each edge half-bar's schematic designator order is `LED5, LED6, LED7, LED9, LED8, LED10, LED11, LED12`; verify its physical direction after assembly.

The module USB-C connector is not USB data: D+ carries the node-to-ESP `BusTX` return through 120 ohms, D- carries ESP-to-node `BusRX` through 120 ohms, and VBUS carries the board's 5 V rail. Firmware and user-facing documentation must never present this connector as a standards-compliant USB host/device interface.

## 3. System ownership

### ESP32 controller

- Polls exactly one node at a time and is the only initiator after discovery.
- Maintains the authoritative 64-square physical state and inferred chess position.
- Converts square events into move transactions and validates them against embedded chess rules.
- Sends high-level visual instructions; it does not stream LED frames.
- Maintains node registry, capability data, health, configuration, and time synchronization.
- Owns Wi-Fi provisioning, WebSocket reconnect/backoff, authentication, ESP OTA, ATmega update coordination, and persistent settings.
- Mirrors bus and semantic events to the backend and accepts authorized asynchronous commands from it.

### Quadrant ATmega

- Scans and filters 16 Hall channels continuously, independent of polling cadence.
- Classifies each square as empty, polarity A, polarity B, or uncertain.
- Queues edge events with local timestamps and exposes snapshots on request.
- Runs local, layered LED effects from compact scene/effect commands.
- Reports reset reason, calibration status, queue overflow, sensor faults, and animation load.
- Never tries to infer a chess move or communicate without a poll/discovery slot.

### Clock module

- Keeps both clocks locally so display and button response remain smooth during bus delays.
- Accepts absolute clock state plus a synchronized start epoch, active side, increment/delay mode, display text, brightness, and buzzer/display effects if present.
- Queues timestamped left/right button events and reports them on poll.
- Stops or enters a defined safe display state when its lease expires.

### Relay module

- Exposes two independently controlled outputs, hardware/firmware identity, and output feedback if available.
- Defaults both outputs off at reset, disconnect, protocol error, and lease expiry.
- Supports bounded pulses and explicit off; indefinite-on must be disabled by default.
- Requires an explicit arm command with a short lease before activation, clamps maximum pulse duration locally, and never restores an active output after reboot.
- Dangerous loads require a physical inhibit/interlock and suitably rated isolation. Network authentication is not a safety mechanism.

## 4. Shared serial protocol v1

Replace the draft broadcast C struct with a versioned framed protocol. C structs are compiler-layout-dependent, hard to extend, and cannot safely recover after a dropped byte.

### Framing

Use COBS encoding with `0x00` as the frame delimiter. The decoded frame is:

| Field | Size | Notes |
| --- | ---: | --- |
| protocol version | 1 | Start at `1` |
| flags | 1 | response, event-pending, ack-required, error |
| source address | 1 | `0x80` ESP32, assigned node, or `0xFE` unassigned |
| destination address | 1 | Assigned node or broadcast |
| message type | 1 | Stable numeric enum |
| sequence | 1 | Matches each response to its request |
| payload length | 2 | Little-endian, bounded per node |
| payload | N | Explicitly serialized fields |
| CRC-16/CCITT | 2 | Covers header and payload |

Set a small v1 maximum decoded frame size, such as 128 bytes, so every AVR can use fixed buffers. Reject unknown versions, impossible lengths, bad CRCs, wrong destinations, and duplicate non-idempotent commands. COBS and CRC implementations should be shared source used by both firmware projects and host tests.

### Addressing and discovery

- Reserve fixed addresses `0x00`-`0x03` for the four quadrants. Provision and verify each unique quadrant ID during initial ISP flashing; runtime discovery must not be required to distinguish them.
- Reserve `0x80` for the ESP, `0xFF` for broadcast, and `0xFE` for unassigned discovery responses. Allocate hot-plug modules from a non-overlapping range such as `0x10`-`0x7F`.
- Dynamically assign hot-plug modules from a separate range. Each has a persistent 64-bit unique ID and reports module type, hardware revision, firmware version, and capabilities.
- On boot and periodically, the ESP sends `DISCOVER(epoch, slot_count)`. Unassigned nodes choose a response slot from a hash of unique ID and epoch. Collisions produce no valid frame; the ESP retries with a new epoch or larger window.
- The ESP sends `ASSIGN_ADDRESS(unique_id, address, lease)` and renews leases while polling. A node returns to unassigned state after lease expiry or controller epoch change.
- If address straps are available on every Type-C port, prefer deterministic port addresses and keep unique IDs for replacement/configuration tracking.

### Transaction rules

- Outside discovery, only the ESP begins a transaction.
- A node transmits only after receiving a valid request addressed to it, waits a defined turnaround interval, and sends exactly one bounded response. On the quadrant hardware, UART idle-high is blocked by D8 while low bits pull the shared return low; firmware must never configure TX idle-low or allow two responders.
- Every request has a bounded response window. The ESP retries idempotent requests, marks a node degraded after repeated misses, and eventually removes it from the registry.
- Node events are never unsolicited. A poll response carries queued events or an `event_pending` flag, and the ESP drains the queue with further polls.
- Commands that change outputs carry a sequence/correlation ID and are acknowledged with applied/rejected status. Retries must not repeat a pulse or button action.
- Broadcasts are limited to commands requiring no response, such as controller epoch, emergency all-off, and coarse time sync.

### Initial message families

- Core: `PING`, `INFO`, `CAPABILITIES`, `STATUS`, `RESET_CAUSE`, `TIME_SYNC`, `CONFIG_GET/SET`, `ERROR`.
- Discovery: `DISCOVER`, `IDENTIFY`, `ASSIGN_ADDRESS`, `RENEW_LEASE`.
- Events: `POLL_EVENTS`, `EVENT_BATCH`, `GET_SNAPSHOT`.
- Quadrant: `SENSOR_SNAPSHOT`, `SENSOR_EVENT`, `CALIBRATE`, `CALIBRATION_RESULT`.
- Lighting: `SET_SCENE`, `RUN_EFFECT`, `STOP_EFFECT`, `SET_BRIGHTNESS`, `CLEAR_LAYER`.
- Clock: `CLOCK_SET`, `CLOCK_START`, `CLOCK_PAUSE`, `CLOCK_TEXT`, `CLOCK_BUTTON_EVENT`.
- Relay: `RELAY_ARM`, `RELAY_PULSE`, `RELAY_OFF`, `RELAY_STATUS`, `RELAY_ALL_OFF`.
- Firmware: `FW_PREFLIGHT`, `MAINTENANCE_BEGIN`, `FW_PREPARE`, `FW_ENTER_BOOTLOADER`, `MAINTENANCE_END`, `FW_HEALTH`, `FW_CONFIRM`.

Publish the wire contract as one protocol document plus golden byte vectors. Generate or hand-maintain matching enums carefully in C++ and Rust; never expose MCU memory layouts as the wire format.

## 5. Quadrant firmware architecture

Use a cooperative, deadline-driven event loop with no `delay()` calls:

1. UART RX parser runs from a ring buffer populated by the UART ISR.
2. Sensor scan advances one mux/sample step at a time.
3. Sensor classifier updates filters and debouncers.
4. LED animation engine advances against monotonic time.
5. LED transport flushes only when safe for UART reception.
6. Command dispatcher applies parsed commands.
7. Event queue and health counters are serviced.

### Sensor pipeline

- Advance both 8-way mux banks together through addresses 0-7. Start with a 50-100 Hz complete quadrant scan and increase only if settling/noise measurements and CPU budget allow it.
- After changing mux address, allow analog settling and, if necessary, discard the first ADC sample.
- Start with a 10-20 microsecond mux/op-amp settling delay and AVCC as the ADC reference, then tune from scope traces and repeated-read variance. The nominal conditioned range of 1-4 V is approximately ADC counts 205-818 at a 5 V, 10-bit reference.
- Maintain filtered raw ADC, baseline, noise estimate, classification, and confidence per square.
- Use independently calibrated positive/negative thresholds, hysteresis, and time debounce. Report `uncertain` instead of oscillating between states.
- Store calibration with a version and CRC in EEPROM. Support empty-board calibration first; add guided piece/polarity calibration if production spread requires it.
- Emit only stable transitions in normal operation, but provide rate-limited raw ADC diagnostics and a complete snapshot for setup/service.
- Map local square indices to global board coordinates in one shared configuration table, tested for all four physical rotations.

### Animation engine

Use built-in effect IDs plus parameters, not arbitrary remote bytecode in v1. A command describes target squares/edges, layer, palette, start time, duration, speed, easing, and repetition. Nodes interpolate locally at a target of 60 frames/second.

Suggested layers, from lowest to highest: idle/ambient, board evaluation, legal moves, selection/move progress, warning/illegal move, and fault/identify. Higher layers temporarily mask lower layers without destroying their state.

Initial effects: solid, fade, pulse, blink, chase, square highlight, origin-to-destination sweep, invalid-move flash, evaluation bar, and module identify. Define perceptual brightness/gamma and a global current/brightness ceiling locally.

Prototype the LED transport before committing to the frame rate. The two 32-pixel and two 8-pixel chains require about 2.4 ms total wire time per quadrant. If output masks interrupts, define synchronized bus-active and LED-render windows (all quadrants may render concurrently), or use an explicit ready/quiet handshake; merely ACKing one node is insufficient because every node hears ESP traffic. Record worst-case UART and animation timing with a logic analyzer.

One RGB framebuffer is 240 bytes per quadrant; two are 480 bytes. This fits in AVR SRAM, but animation layers should remain compact descriptors composited into a single output buffer rather than one framebuffer per layer. Use no dynamic allocation.

## 6. ESP32 firmware architecture

Use FreeRTOS tasks or isolated state machines with queues; avoid sharing mutable state directly:

- **Bus manager:** framing, discovery, polling, retries, node registry, command queues, time sync, and trace capture. It alone owns the UART peripheral.
- **Board model:** combines quadrant snapshots/events, detects gaps/overflow, and requests resynchronization.
- **Move coordinator:** turns lift/place sensor transitions into a physical move transaction.
- **Chess rules:** legal move generation, turn, check, castling, en passant, promotion, and position serialization.
- **Effects coordinator:** translates semantic states into high-level per-node effect commands.
- **Network manager:** provisioning, reconnect/backoff, TLS, authentication, WebSocket heartbeat, and outbound buffering.
- **Command gateway:** validates backend commands against capabilities, game state, safety policy, and authorization before queuing bus work.
- **Persistence/diagnostics:** settings schema, crash/reset data, metrics, structured log ring, and OTA status.

The local chess component should initially be a complete rules engine/validator, not a search engine. Hall polarity identifies side but not piece type, so the ESP must begin from a known position and infer identity from legal moves. It must explicitly handle lift/cancel, capture in either physical order, castling, en passant, promotion choice, multiple lifted pieces, and ambiguous or lost events. When it cannot infer one legal position, it enters a resync/setup state instead of guessing. Optional shallow search can come later; Stockfish remains a backend concern.

Keep the board playable offline: sensing, legal/illegal move feedback, clocks, and local effects must continue without Wi-Fi. Backend engine suggestions and remote controls become unavailable but must not stall the bus loop.

## 7. Backend and frontend bridge

Treat the frontend as an asynchronous observer/controller, not another UART bus master.

- ESP opens one authenticated TLS WebSocket to the Rust backend. A static device credential header is acceptable for a prototype, but provision a distinct revocable credential per board and avoid embedding a fleet-wide secret.
- ESP publishes a boot/session ID, monotonic event sequence, firmware/hardware versions, node registry, board snapshots, chess position, sensor/move events, health, and connection status.
- For diagnostics, it can also publish a bounded UART trace envelope containing direction, ESP timestamp, node address, sequence, type, decoded payload, result, and optionally the raw frame. Do not make raw frames the only API.
- Backend persists the latest canonical snapshot, runs Stockfish, and fans events out to subscribed SvelteKit clients.
- Frontend commands use typed semantic requests such as `start_game`, `set_clock`, `run_effect`, or `pulse_relay`. Backend checks user/device authorization, assigns a correlation ID, and sends them to the ESP.
- ESP validates again, schedules the corresponding bus transaction, and returns accepted/applied/rejected/timeout status. This preserves single-master ordering and gives the UI honest asynchronous state.
- On reconnect, exchange last sequence numbers and a fresh snapshot. Never assume WebSocket delivery alone is durable or exactly once.

Define the WebSocket schema alongside the UART schema, including compatibility/version rules. Rust, TypeScript, and ESP test fixtures should share golden JSON/CBOR examples. JSON is easiest for v1 diagnostics; compact binary encoding can be introduced only if measurements justify it.

## 8. ATmega UART firmware updates

ATmega OTA is an addressed maintenance operation coordinated by the ESP, not a normal application payload stream. The proven reference in `../ec209-2025-project-2025_team_20` demonstrates software entry into a resident ATmega328PB bootloader and ESP-driven STK500 page programming. Arcade Chess keeps that mechanism but requires production-safe framing, artifact validation, verification, interruption recovery, and structured observability.

### Bootloader entry and recovery

- Install a protected bootloader, matching BOOTRST/BOOTSZ fuses, lock bits, quadrant ID, and initial application during manufacturing ISP.
- Do not jump from a UART ISR or hard-code a boot address in the application. An addressed two-phase command stores a CRC-protected EEPROM update marker, ACKs and drains UART, makes outputs safe, then uses a watchdog software reset. The bootloader build/linker owns its start address.
- The bootloader runs first after every reset. A requested/programming marker or invalid application keeps it in recovery mode; a valid normal boot takes only a short boot window.
- There is no dual-bank rollback. Recovery means the protected bootloader stays available so the ESP can safely rewrite the complete application. ISP remains the root recovery method.
- Maintenance is exclusive. The ESP broadcasts a bounded no-response quiet lease, updates one target at a time, and temporarily changes the shared bus from framed traffic to the bootloader protocol. Non-target nodes continue local sensing/animation but suppress responses and ignore raw programmer bytes.

### Artifact and programming

- Accept Intel HEX at the website boundary if desired, but validate record checksums, addresses, record types, EOF, overlaps, gaps, MCU/application bounds, and boot-section exclusion before producing a canonical binary.
- Attach a manifest with target/hardware compatibility, application/bootloader protocol ranges, version/build/source identity, size/page count, SHA-256, application CRC, signing metadata, and downgrade policy.
- Stage and verify the entire artifact on the ESP before entering maintenance. The backend or browser may disconnect after this point without interrupting programming.
- Synchronize with the target bootloader, verify device identity and flash geometry, write 128-byte pages with bounded retries, read every page back, verify the complete image, reboot, restore framed UART, and require a matching `FW_HEALTH` application self-test followed by an acknowledged `FW_CONFIRM` that atomically marks the candidate valid.
- Never clear the recovery marker or report success before readback, application health, and confirmation all succeed.

### Website and backend workflow

The browser uploads to the Rust backend, which validates and stores the artifact and creates a durable update job. An authorized operator selects a board and compatible target(s). The ESP downloads the artifact over authenticated HTTPS and updates multiple quadrants sequentially. Browser loss does not stop a staged job, and reconnect reloads the current job snapshot plus subsequent events.

An upload HTTP response means only that the artifact was accepted. The UI separately displays artifact upload, device download, preflight, bootloader entry, page programming, verification, reboot, and health-check progress. Cancellation is permitted before destructive writing; afterward the available actions are finish, retry, or explicitly leave the target recoverable in its bootloader.

### Update observability

Every job has a backend-generated `update_id` and each target attempt has an `attempt_id`. ESP state is a persisted state machine: validating, downloading, staged, preflight, quieting bus, bootloader sync, programming, verifying, rebooting, health check, succeeded, failed retryable, failed recoverable in bootloader, or failed needing ISP.

Structured update events contain identity, ESP session and sequence, phase transition, target and artifact build/hash, bytes/pages complete and total, current address, elapsed time, retries, UART counters, bootloader response/error, expected/observed verification values, recoverability, and final application health. ESP checkpoints active-job metadata in NVS; backend stores a durable job snapshot and append-only event history. The UI must never infer state by parsing log text.

Page progress may be sampled, but no phase transition, retry, verification failure, or terminal result may be dropped. Keep a bounded UART trace around failures and expose metrics for phase duration, sync failures, page retries, readback mismatches, interrupted recoveries, and ISP-required outcomes. Human logs include update/attempt IDs and dropped-log counts but are supporting evidence only.

The complete contract and failure matrix are in `docs/avr-ota.typ`.

## 9. Repository shape

Keep `firmware-atmega` and `firmware-esp`, and add a small shared protocol package that both PlatformIO projects compile:

```text
protocol/
  include/arcade_protocol/   # frame, enums, serialization, CRC, limits
  src/
  test/vectors/
firmware-atmega/
  bootloader/                # resident updater, linker/fuse/lock manifest
  src/board/                 # pin map and board revision
  src/drivers/               # mux ADC, LED transport, EEPROM, UART
  src/protocol/
  src/sensors/
  src/animation/
firmware-esp/
  src/avr_update/            # artifact staging, programmer, state machine
  src/bus/
  src/board_model/
  src/chess/
  src/effects/
  src/network/
  src/config/
```

Use compile-time board/module targets for quadrant, clock, and relay firmware while sharing the framing and scheduler primitives. Pin maps and capability descriptors must identify hardware revision explicitly.

## 10. Delivery phases and acceptance gates

### Phase 0 — electrical and timing proof

- Confirm the diode-OR bus behavior, ESP-to-AVR high margin, Type-C hot-plug safety, corrected pin-map continuity, LED bodges/layout, and quadrant orientation.
- Build a two-node then six-node bus test with a logic analyzer.
- Prove frame reception during worst-case LED output and sensor scanning.

**Exit:** no electrical contention; 24-hour traffic test has no unexplained corrupt frames or resets; hot-plug does not disturb existing nodes.

### Phase 1 — protocol and host tests

- Implement COBS, CRC, parser, fixed-buffer serialization, sequence handling, core messages, discovery, node leases, and golden vectors.
- Add parser fuzz/property tests on the host and cross-check C++ against Rust fixtures.

**Exit:** arbitrary noise/truncation cannot wedge a parser, all golden vectors agree, and duplicate commands are handled safely.

### Phase 2 — one quadrant

- Implement pin map, sensor scanner, calibration, classification, event queue, snapshot/status, and basic LED transport/effects.
- Build a host bus simulator for repeatable commands and fault injection.

**Exit:** all 16 squares classify both polarities reliably; transitions meet latency targets; 60 fps effects remain smooth while protocol traffic is saturated.

### Phase 3 — four-quadrant ESP bus

- Implement bus manager, discovery/fixed quadrant addressing, polling, time sync, global coordinate mapping, health monitoring, retries, and resync.
- Add semantic effect orchestration across quadrant boundaries.

**Exit:** all 64 squares produce correctly mapped events; simultaneous moves and animation stay responsive; reset/unplug/replug of one node recovers without rebooting the ESP.

### Phase 4 — local chess behavior

- Implement board setup verification, move transaction state machine, full legal move validation, FEN/position export, illegal-move feedback, and recovery UI states.

**Exit:** automated suites cover normal moves, captures in both orders, castling, en passant, promotion, takebacks/cancelled lifts, ambiguous states, and event loss.

### Phase 5 — network/backend integration

- Implement provisioning, TLS WebSocket, per-device auth, schema/version handshake, snapshots, event stream, command acknowledgements, reconnect/backoff, and offline buffering limits.
- Connect the Rust service and SvelteKit viewer; add Stockfish results as a separate semantic stream.

**Exit:** network loss never affects local bus deadlines; reconnection converges from a snapshot; two viewers see ordered state; unauthorized or stale commands are rejected.

### Phase 6 — ATmega UART update path

- Build and manufacturing-flash the protected bootloader and fuse/lock contract.
- Implement addressed entry, maintenance quieting, validated artifacts, ESP staging, page programming/readback, persistent recovery markers, and application health confirmation.
- Add durable backend jobs and structured progress/error events to the SvelteKit interface.
- Test power loss, ESP reset, network/browser loss, wrong/corrupt artifacts, UART faults, duplicate entry commands, readback mismatch, and sequential four-quadrant update.

**Exit:** at least 100 repeated updates per target revision succeed with measured retry/failure rates; interruption at every page returns to a remotely programmable bootloader; the UI never reports success before verification and health check; every induced failure has an actionable code, trace, and recovery classification.

### Phase 7 — modules

- Implement hot-plug discovery/capabilities and clock module first.
- Implement relay module only after local safety constraints and physical interlock behavior are defined and tested.

**Exit:** clock remains visually smooth and accurate through bus/network interruptions; relay always fails off and repeated/lost frames cannot extend or duplicate a pulse.

### Phase 8 — production hardening

- ESP OTA with signed images and rollback; release signing and downgrade policy for AVR artifacts.
- Brownout/watchdog tests, EEPROM wear policy, manufacturing self-test, calibration workflow, version compatibility matrix, metrics, and service diagnostics.
- Long-duration soak, malformed-frame fuzzing on hardware, Wi-Fi loss, backend restart, hot-plug storms, and power interruption tests.

**Exit:** documented factory/service process, recoverable ESP update, stable soak results, and a release checklist tied to hardware and protocol versions.

## 11. Initial measurable targets

- Stable square transition visible to ESP: under 60 ms typical and under 100 ms worst case at the 38,400-baud bring-up rate; tighten after the final baud and debounce measurements.
- Local button-to-display feedback: under 20 ms.
- LED animation: 60 fps target, no visible stalls during saturated valid bus traffic.
- Full four-quadrant polling cycle: 40 ms or better at 38,400 baud; target 20 ms only if 115,200 baud passes the electrical tests. Use slower adaptive polling for idle modules.
- Clock drift: measure first, then target under 100 ms/hour between synchronizations.
- Event queue: enough for at least one second of worst-case legitimate sensor/button activity, with an explicit overflow flag and snapshot recovery.
- Node recovery after unplug/replug: under two seconds without controller reboot.
- All relay outputs off within the locally enforced lease/fault timeout, independent of ESP or backend state.
- ATmega update progress event visible end-to-end within one second, with no missing phase/error transitions.
- Interrupted ATmega application write recoverable over UART without ISP whenever the protected bootloader and bus remain healthy.

## 12. Decisions needed before Phase 0 closes

1. Scope-verified maximum baud rate and timing margin for the diode-OR return bus.
2. Measured ESP32 3.3 V high reliability at every 5 V AVR RX input.
3. Quadrant physical rotations, square/LED coordinate map, and edge-bar direction.
4. Address straps versus EEPROM identities and discovery behavior for hot-plug modules.
5. Required maximum cable length, module count, and connector topology.
6. Exact UART/LED quiet-window scheduling strategy if the WS2812 driver masks interrupts.
7. Magnet polarity conventions and acceptable sensor calibration process.
8. Chess clock rules required in v1.
9. Relay maximum pulse/on-time, physical interlocks, and whether indefinite activation is forbidden.
10. Offline game requirements and who is authoritative after backend reconnect.
11. Backend command authorization model and device credential provisioning/rotation.
12. Bootloader implementation/size, BOOTRST/BOOTSZ and lock-bit values, programming baud, application validity marker, and release signing policy.
