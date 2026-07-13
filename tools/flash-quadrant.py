#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["pyserial"]
# ///
"""Flash a quadrant ATmega over the shared bus via the ESP32 USB console.

The ESP's `fw-flash <node>` command stages an Intel HEX image in RAM, walks the
target into its resident urboot bootloader, programs and verifies every page,
then health-checks and confirms the new application. This script feeds it the
hex file with per-line flow control and relays progress.

Examples:
  tools/flash-quadrant.py --port /dev/cu.usbserial-0001 --node 0 --build
  tools/flash-quadrant.py --port /dev/cu.usbserial-0001 --node 2 \
      --hex firmware-atmega/.pio/build/ATmega328PB/firmware.hex
"""

import argparse
import pathlib
import subprocess
import sys
import time

import serial
from serial.tools import list_ports

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_ENV = "ATmega328PB"
USB_SERIAL_HINTS = ("usbserial", "usbmodem", "slab", "wchusb", "ttyusb", "ttyacm")


def fatal(message: str) -> "NoReturn":  # noqa: F821
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)


def detect_port() -> str:
    candidates = [
        p.device for p in list_ports.comports()
        if any(hint in p.device.lower() for hint in USB_SERIAL_HINTS)
    ]
    if not candidates:
        fatal("no USB serial port found; pass --port")
    if len(candidates) > 1:
        fatal("multiple USB serial ports found; pass --port one of: "
              + ", ".join(candidates))
    print(f"using port {candidates[0]}")
    return candidates[0]


def build_firmware(env: str) -> None:
    print(f"building firmware-atmega -e {env} ...")
    result = subprocess.run(
        ["uvx", "--from", "platformio", "platformio", "run",
         "-d", str(REPO_ROOT / "firmware-atmega"), "-e", env],
        check=False,
    )
    if result.returncode:
        fatal("PlatformIO build failed")


def read_line(port: serial.Serial, timeout_s: float) -> str:
    deadline = time.monotonic() + timeout_s
    buffer = bytearray()
    while time.monotonic() < deadline:
        byte = port.read(1)
        if not byte:
            continue
        if byte == b"\n":
            return buffer.decode(errors="replace").strip()
        if byte != b"\r":
            buffer += byte
    return ""


def expect(port: serial.Serial, token: str, timeout_s: float) -> str:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        line = read_line(port, deadline - time.monotonic())
        if not line:
            continue
        if token in line:
            return line
        if "FLASH FAIL" in line:
            fatal(line)
        print(f"  esp: {line}")
    fatal(f"timed out waiting for '{token}'")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", help="ESP32 USB serial port (default: auto-detect)")
    parser.add_argument("--node", required=True, type=int, choices=range(4),
                        help="target quadrant node id")
    parser.add_argument("--hex", type=pathlib.Path,
                        help="Intel HEX image (default: PlatformIO build output)")
    parser.add_argument("--env", default=DEFAULT_ENV,
                        help=f"PlatformIO environment (default {DEFAULT_ENV})")
    parser.add_argument("--build", action="store_true",
                        help="run the PlatformIO build before flashing")
    args = parser.parse_args()

    if args.build:
        build_firmware(args.env)
    hex_path = args.hex or (
        REPO_ROOT / "firmware-atmega" / ".pio" / "build" / args.env / "firmware.hex")
    if not hex_path.is_file():
        fatal(f"{hex_path} not found; build first or pass --hex")
    lines = [l.strip() for l in hex_path.read_text().splitlines() if l.strip()]
    if not lines or not all(l.startswith(":") for l in lines):
        fatal(f"{hex_path} is not an Intel HEX file")

    with serial.Serial(args.port or detect_port(), 115200, timeout=0.1) as port:
        time.sleep(0.3)
        port.reset_input_buffer()
        port.write(f"fw-flash {args.node}\n".encode())
        expect(port, "HEX-READY", 5)

        print(f"uploading {hex_path.name}: {len(lines)} records")
        for index, line in enumerate(lines[:-1]):
            port.write(line.encode() + b"\n")
            reply = read_line(port, 5)
            if reply != "+":
                if "FLASH FAIL" in reply:
                    fatal(reply)
                fatal(f"record {index + 1}/{len(lines)}: expected '+', got '{reply}'")
            if (index + 1) % 200 == 0:
                print(f"  {index + 1}/{len(lines)} records")
        port.write(lines[-1].encode() + b"\n")  # EOF record triggers the handoff
        expect(port, "IMAGE ", 10)

        # Programming + verify + health + confirm; relay ESP progress lines.
        line = expect(port, "FLASH OK", 120)
        print(line)


if __name__ == "__main__":
    main()
