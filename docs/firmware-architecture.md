# Firmware architecture and runtime modes

Both controllers use a small composition root and permanent subsystems. Bring-up
features are part of the shipped firmware; they are not a temporary fork.

## Runtime modes

`normal` is the factory default. It runs sensing, settled-piece lighting, event
polling, snapshots, WebSocket connectivity, calibration, raw scans, configuration,
and firmware-update recovery, but avoids high-volume trace output.

`bringup` enables operator-oriented trace detail such as per-transition ESP logs
and WebSocket transmission summaries. Explicit diagnostic operations remain
available in normal mode so field servicing never requires a special firmware
image. The mode is persisted on ESP NVS and in each quadrant's CRC-protected
settings. Set it from ESP serial with:

```text
mode normal
mode bringup
```

The ESP persists its mode and queues configuration key 10 to all quadrants. The
backend can do the same with `device.mode.set`. The isolated `bringup_ascii`
PlatformIO environment is the sole compile-time exception: it forces bring-up mode
for CSV output and deliberately replaces the shared binary UART for bench work.

## ATmega328PB composition

`src/main.cpp` is the small composition root: it constructs the subsystems and
keeps their top-level scheduling visible in one place:

| Subsystem | Responsibility |
| --- | --- |
| `Sensors` | Mux/ADC scanning, filtering, classification, calibration, event queue, averaged raw capture |
| `Lighting` | Independent FastLED surfaces for settled pieces, overrides, identify, and board-wide animations |
| `ProtocolService` | COBS stream parsing, request dispatch, configuration, snapshots and event responses |
| `FirmwareUpdate` | Maintenance leases, preflight, update marker, and validated direct bootloader handoff |
| `Diagnostics` | Isolated ASCII/CSV diagnostic presentation |
| `system_info` | Early reset-cause capture, AVCC measurement, fuse and bootloader inspection |
| `persistent` | Identity, configuration, calibration and alternating OTA marker slots |

The cooperative application loop remains explicit: sample sensors, advance
lighting, service one communications mode, service firmware maintenance, reset the
watchdog. Subsystems do not own one another; references are wired once in
`main.cpp`.

## ESP32 composition

The ESP `src/main.cpp` likewise wires and schedules the permanent subsystems:

| Subsystem | Responsibility |
| --- | --- |
| `AppConfig` | NVS-backed Wi-Fi, backend, identity, orientation and runtime mode |
| `BusManager` | Exclusive UART ownership, polling, mapping, render windows, raw aggregation and OTA handoff |
| `NetworkManager` | Wi-Fi and device WebSocket session, semantic events and remote commands |
| `Console` | Human operator commands and persisted configuration changes |

Quadrant population is dynamic. Each fixed address is discovered independently;
online nodes receive normal polling, offline sockets use bounded probe backoff, and
reappearing nodes are resynchronized. Aggregate operations target the online mask,
so no control path treats four responses as a prerequisite for normal operation.

Normal product behavior should be added at a meaningful subsystem boundary and
scheduled from `main.cpp`, not hidden behind the bring-up mode.
Diagnostics should observe or command stable subsystem interfaces so the same
instrumentation remains useful during manufacturing and field service.

## Persistence compatibility

Quadrant settings use factory schema v1, including runtime mode. There is
deliberately no compatibility migration: no boards have been provisioned yet, so
the first programmed image defines the persistent layout. Blank or invalid EEPROM
loads safe defaults and can then be provisioned with the ISP tooling. ESP NVS also
defaults directly to the current backend and normal mode.
