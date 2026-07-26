# Firmware build and board bring-up

The initial implementation is intentionally instrumented rather than quiet. The
ESP USB serial port is the primary operator console at 115200 baud; the shared AVR
bus remains binary at 38400 baud and is decoded/logged by the ESP.

## Build

```sh
uvx --from platformio platformio run -d firmware-atmega -e ATmega328PB
uvx --with intelhex --from platformio platformio run -d firmware-esp -e esp32dev
sh protocol/test/run-host-tests.sh
```

The extra `intelhex` package is only needed by a transient `uvx` PlatformIO
environment on some machines. A normal PlatformIO installation brings its own
ESP image dependencies. Upload and monitor the ESP with:

The active quadrant clock is the `QUADRANT_CLOCK` constant at the top of
`tools/pio-targets.py`. It currently selects the crystal-free 8 MHz internal RC
profile. The same value drives application builds, Custom tasks, and fuse burning.

```sh
pio run -d firmware-esp -e esp32dev -t upload
pio device monitor -b 115200
```

At the console, type `help`. Initial persistent setup is:

```text
wifi YOUR_SSID YOUR_PASSWORD
device-id arcade-chess-001
token OPTIONAL_BEARER_TOKEN
reboot
```

The shipped firmware defaults to `mode normal`. Use `mode bringup` for verbose
transition/WebSocket tracing, and return with `mode normal`; raw scans, calibration,
configuration and recovery commands are available in both modes.

The ESP connects to `wss://chess-be.qinnovate.nz/board`. Certificate validation is
disabled in the prototype. Secrets live in ESP Preferences/NVS, not the repository.

## Provision a quadrant over ISP

Connect VCC, GND, RESET, MOSI, MISO, and SCK from the programmer to the quadrant
ISP header. Confirm 5 V and header orientation. The default provisioning profile
uses the internal 8 MHz RC oscillator and does not require a fitted crystal.

```sh
# Dry run: builds and describes the target without writing it
tools/provision-quadrant.sh --id 0

# Auto-select USBasp when attached, otherwise Arduino as ISP
tools/provision-quadrant.sh --id 0 --yes

# Explicit alternatives
tools/provision-quadrant.sh --id 0 --programmer usbasp --yes
tools/provision-quadrant.sh --id 0 --programmer arduino-as-isp \
    --port /dev/cu.usbserial-1130 --yes
```

Repeat with IDs 1, 2, and 3. The tool generates a complete EEPROM image containing
the CRC-protected identity and configuration defaults, sets clock/BOD/
EEPROM-preserve fuses, writes the matching application, and verifies flash plus EEPROM. It
accepts `--port` and `--bitclock` for programmers that need them. Auto-selection
prefers an enumerated USBasp; otherwise it requires exactly one USB modem/serial
port and uses the standard ArduinoISP sketch protocol at 19,200 baud. Pass
`--port` when multiple serial devices are present. The Arduino running ArduinoISP
should have its usual reset-disable capacitor fitted when required by that board.

The pinned Arcade Chess Urboot build in `firmware-atmega/bootloader/` is installed
by default before the application so addressed
`fw-flash` updates remain available. `--no-bootloader` is a recovery/manufacturing
escape hatch and disables that update path. Never attempt a plain avrdude serial
upload with multiple quadrants on the shared UART: they receive the same bytes and
their responses would collide. `fw-flash` is safe with all quadrants attached
because the maintenance broadcast suppresses non-target responses first.

Fuse programming is recoverability-sensitive. Before changing clock source, set
`QUADRANT_CLOCK` to `"internal"` or `"external"` in `tools/pio-targets.py`, then
rebuild and re-provision every quadrant using the same Custom tasks. Do not select
external until the 16 MHz crystals are fitted.

### Update the application over ISP

`tools/flash-isp.sh` is the fast path for iterating with a programmer attached.
It skips the fuse pass, the EEPROM image, and the second PlatformIO project,
leaving a single avrdude session that runs the ISP clock eight times faster than
provisioning does:

```sh
# Build, then write whatever quadrant is on the programmer
tools/flash-isp.sh --build

# Same programmer defaults as provisioning; --bitclock/--port when needed
tools/flash-isp.sh --programmer arduino-as-isp --port /dev/cu.usbserial-1130
```

Classic AVRs have no ISP page erase — only ATxmega and UPDI parts do — so
avrdude's `-D` would AND the new image into the resident one. The chip erase is
therefore unavoidable, and Urboot is rewritten from the same pinned image and
lock bits that provisioning installs, so the `fw-flash` path keeps working. The
quadrant identity in EEPROM survives because provisioning programs EESAVE; the
tool reads the high fuse before erasing anything and falls back to saving and
rewriting EEPROM when EESAVE is clear. That pre-flight read also means a bad ISP
link fails before the flash is touched. `--bitclock` sets the ISP period in
microseconds (default 1, i.e. 750 kHz on a USBasp); a part still on its factory
1 MHz clock needs `--bitclock 8`.

## Update a quadrant over the ESP USB port

Once a quadrant is provisioned with Urboot, application updates no longer need
the ISP header. The ESP stages an Intel HEX image received over its own USB
console, walks the target into the resident bootloader, programs and verifies
every 128-byte page over the shared bus, and confirms the new application:

```sh
# Build firmware-atmega and flash quadrant 0 through the ESP's USB port
tools/flash-quadrant.py --port /dev/cu.usbserial-0001 --node 0 --build

# Build once, then flash every quadrant in sequence over one connection
tools/flash-quadrant.py --all --build

# Build once, then program every attached quadrant from the same byte stream
tools/flash-quadrant.py --simultaneous --build

# Flash an existing image
tools/flash-quadrant.py --port /dev/cu.usbserial-0001 --node 2 \
    --hex firmware-atmega/.pio/build/ATmega328PB/firmware.hex

```

`--port` is auto-detected when exactly one USB serial device is attached. These
tools are also exposed as PlatformIO project tasks (sidebar: Project Tasks →
ATmega328PB → Custom): simultaneous/sequential flash-all, flash/provision per quadrant,
the fast `flash_isp` update, and the
protocol host tests, e.g. `pio run -d firmware-atmega -e ATmega328PB -t
flash_all_quadrants`. The ISP fuse/bootloader environments are kept out of this
list in the separate `firmware-atmega/provisioning/` project.

Close any open serial monitor first; the script owns the port. The full sequence
per target is: `MAINTENANCE_BEGIN` broadcast (non-targets suppress responses),
`FW_PREPARE` (CRC-protected EEPROM marker), `FW_ENTER_BOOTLOADER` (direct validated
handoff into Urboot), Urprotocol sync and page programming with complete readback verification
at the 76800 bootloader baud, `MAINTENANCE_END`, then an `FW_HEALTH` check of the
rebooted application and an acknowledged `FW_CONFIRM` that marks the image valid.

76800 is used rather than 115200 because the quadrants run the 8 MHz internal RC:
115200 is unreachable by any UBRR divisor there (closest is 111111, −3.6%), while
76800 quantises to +0.16% with U2X. Urboot autobauds onto either.

A raw scan in flight blocks the handoff, so `fw-flash` holds off new raw sweeps for
the duration of the upload. If a flash still fails with `handoff rejected (bus
busy?)`, check `status` for `busy=0`. `prepare rejected code=N` names the node-side
blocker (see `docs/uart-api.md`); for `fw-flash-all`, whose broadcasts get no reply,
read `last_broadcast_refusal` from `fw-preflight <node>` instead.
Success is only reported after readback, health, and confirmation all pass.

The bus runs at 38,400 baud but the installed Urboot build autobauds, so the
programming rate is set solely by `kBootloaderBaud` (115200) in
`firmware-esp/src/avr_flasher.cpp`; the ESP switches its bus UART for the
programming window only. If sync proves unreliable on the diode-OR return at
that rate, lower `kBootloaderBaud`. Quadrant LEDs freeze during the
programming window because render broadcasts pause.

If flashing fails, the target is left recoverable: Urboot times out back to the
resident application, and re-running `fw-flash` retries from the top. ISP remains
the root recovery method. Manual console equivalents (`fw-preflight`, `fw-flash
<node>`, `fw-abort`, and the low-level `fw-enter`/`fw-end`) are listed under
`help`. `fw-flash-all`/`--simultaneous` broadcasts prepare and entry, selects the
lowest online node as the only Urboot responder, and makes every other attached
quadrant silently execute the same program/read commands. Each application is
then health-checked and confirmed separately, so a follower that missed any page
prevents overall success.

## Analog diagnostics

For one isolated quadrant, `bringup_ascii` emits CSV every 250 ms:

```sh
pio run -d firmware-atmega -e bringup_ascii
```

Format: `RAW,node_id,millis,adc0,...,adc15`. This mode does not run the binary bus
protocol and must not be used with other transmitting nodes.

With normal firmware and the ESP connected, useful commands are:

```text
nodes
raw 1
raw 16
identify 0 5000
config 0 1 70
config 0 2 42
brightness 0 48
```

`raw 16` averages 16 complete scans on each currently online quadrant. The output
still has 64 board positions; squares belonging to empty sockets are reported as
missing/null rather than making the scan fail.
The equivalent WebSocket commands are `sensor.raw_scan.get` and bounded
`sensor.raw_stream.set`; the frontend receives raw, baseline, noise, state, and
missing-node information for every square.

## Calibration workflow

Baselines are stored per square, so they are only meaningful against the square
map that recorded them. Settings records carry their own version (2 = baselines
indexed by board geometry); a node holding an older record loads defaults and
reports uncalibrated rather than pinning stale baselines to the wrong squares.
Expect to recalibrate any node flashed across that change.

1. Remove every piece and magnet. Run `raw 16` repeatedly. Empty readings should
   be away from ADC rails and stable around their own baselines.
2. Run `calibrate all`. Each AVR averages 128 scans, captures per-square baseline
   and peak-to-peak noise, validates them, and writes a CRC-protected EEPROM record.
3. Run `nodes`, then `raw 16`. Calibration rejects a baseline below 120 or above
   900, or noise above 40 ADC counts.
4. Walk a known positive magnet across all squares. Settled squares should report
   positive and illuminate green. Reverse it; settled negative squares are blue.
   The square that lights must be the one under the magnet: a lit neighbour means
   the board map is wrong, not the calibration.
5. Tune enter threshold (key 1), exit threshold (2), debounce scans (3), mux
   settling time (4), and full-scan time (5). Change one factor at a time while
   retaining frontend raw-scan captures.
6. Use `identify`, the debug UI's **wave**, and walking-magnet tests to determine
   physical orientation. The wave sweeps a lit file left to right and then a lit
   rank bottom to top; because it drives global squares it exercises the whole
   chain — global square, node, local square, LED — so a quadrant sitting in the
   wrong corner or turned the wrong way lights a whole 4x4 block out of step.
   Persist orientation key 9 and then freeze the ESP global coordinate map.

The ESP discovers each quadrant independently. Empty addresses are probed with
backoff, so a board with one, two, three, or four quadrants remains responsive.
Hot-connected nodes are automatically returned to normal polling and receive the
current orientation and runtime mode. `calibrate all` applies only to online nodes.

The green/blue settled-piece layer is local and continues without network or ESP.
Explicit lighting commands temporarily override it.

## First-hardware checks

- Scope ESP GPIO16 at every AVR RX and verify a reliable high at 5 V AVR supply.
- Scope the GPIO17 diode-OR return rise with all nodes and final cable attached.
- Verify only the addressed AVR drives a response.
- Capture the 25 Hz `RENDER_WINDOW` marker and verify UART remains idle while all
  quadrants perform roughly 2.4 ms of interrupt-masking WS2812 output.
- Check PE3/PE2 square chains and the PE0/PE1 bodge with `identify`.
- Watch reset cause, parser/CRC errors, event overflow, timeouts, heap, and RSSI.

See [uart-api.md](uart-api.md) for binary payloads and
[websocket-api.md](websocket-api.md) for the server/frontend contract.
