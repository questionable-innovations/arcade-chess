# Arcade Chess

Arcade Chess is a modular, sensor-driven illuminated chessboard built around one
ESP32 controller and four ATmega328PB quadrant controllers.

Start with [`docs/project-overview.typ`](docs/project-overview.typ) for the product
goals, component responsibilities, electronics summary, bus invariants, module
safety model, and software architecture. The detailed implementation roadmap is in
[`docs/firmware-plan.md`](docs/firmware-plan.md). ATmega updates over the shared
UART, including the remote website workflow and observability contract, are defined
in [`docs/avr-ota.typ`](docs/avr-ota.typ).

Firmware entry points:

- [`docs/bringup.md`](docs/bringup.md) — build, ISP provisioning, console, raw scans,
  calibration, and hardware checks.
- [`docs/firmware-architecture.md`](docs/firmware-architecture.md) — permanent
  subsystem boundaries and normal/bring-up runtime modes.
- [`docs/uart-api.md`](docs/uart-api.md) — frozen bring-up UART wire contract.
- [`docs/websocket-api.md`](docs/websocket-api.md) — ESP/backend/frontend JSON API.
- `firmware-atmega/` and `firmware-esp/` — PlatformIO projects.
- `protocol/` — shared allocation-free framing and host tests.

Render the combined overview and protocol notes with:

```sh
typst compile docs/main.typ
```
