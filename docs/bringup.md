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
ISP header. Confirm 5 V, the external 16 MHz crystal, and header orientation.

```sh
# Dry run: builds and describes the target without writing it
tools/provision-quadrant.sh --id 0

# Program and verify using USBasp
tools/provision-quadrant.sh --id 0 --programmer usbasp --yes
```

Repeat with IDs 1, 2, and 3. The tool generates a complete EEPROM image containing
the CRC-protected identity and configuration defaults, sets external-crystal/BOD/
EEPROM-preserve fuses, writes the application, and verifies flash plus EEPROM. It
accepts `--port` and `--bitclock` for programmers that need them.

MiniCore Urboot is installed by default before the application so addressed OTA
entry remains available. `--no-bootloader` is a recovery/manufacturing escape
hatch and disables the OTA entry path. Never
attempt a serial bootloader upload with multiple quadrants on the shared UART:
they receive the same bytes and their responses would collide. Normal updates
should remain ISP-based until the separately specified managed update path lands.

Fuse programming is recoverability-sensitive. This setup is only for an
ATmega328PB with an external 16 MHz crystal and 2.7 V brown-out.

### Exercise application-to-bootloader entry

Start with one isolated quadrant and a logic analyzer. Provision it normally,
boot the application, then use the ESP console:

```text
fw-preflight 0
fw-enter 0 0x12345678 12000 0x89abcdef 0x42
```

Arguments are node, one-time token, proposed application size, application CRC32,
and update ID. Preflight prints the fuse, BOOTRST status, flash geometry, marker
state, reset cause, and supply. Entry broadcasts maintenance, persists prepare
metadata, commits with the repeated token, waits for the ACK, then stops polling.

There is not yet a page-programming client behind this handoff. Urboot will time
out and return to the existing application when it receives no synchronization.
Run `fw-end 0x12345678` to resume ESP polling after the isolated test. The future
STK500 writer/readback verifier belongs between entry ACK and `MAINTENANCE_END`.

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

1. Remove every piece and magnet. Run `raw 16` repeatedly. Empty readings should
   be away from ADC rails and stable around their own baselines.
2. Run `calibrate all`. Each AVR averages 128 scans, captures per-square baseline
   and peak-to-peak noise, validates them, and writes a CRC-protected EEPROM record.
3. Run `nodes`, then `raw 16`. Calibration rejects a baseline below 120 or above
   900, or noise above 40 ADC counts.
4. Walk a known positive magnet across all squares. Settled squares should report
   positive and illuminate green. Reverse it; settled negative squares are blue.
5. Tune enter threshold (key 1), exit threshold (2), debounce scans (3), mux
   settling time (4), and full-scan time (5). Change one factor at a time while
   retaining frontend raw-scan captures.
6. Use `identify` and walking-magnet tests to determine physical orientation.
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
