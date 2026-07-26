#!/usr/bin/env python3
"""Build a complete, reproducible ATmega328PB EEPROM image."""

import argparse
import struct
from pathlib import Path

EEPROM_SIZE = 1024
ERASED_BYTE = 0xFF

# Addresses, magics and record layouts mirror firmware-atmega/src/persistent.h.
IDENTITY_ADDRESS = 0
SETTINGS_ADDRESS = 16
IDENTITY_MAGIC = 0x51434944
SETTINGS_MAGIC = 0x51434346
SQUARES_PER_QUADRANT = 16
IDENTITY_FORMAT = "<IBB"
SETTINGS_FORMAT = f"<IBHHBHHBHHB{SQUARES_PER_QUADRANT}H{SQUARES_PER_QUADRANT}BBB"

# Settings carry their own version (2: baseline/noise indexed by board
# geometry); identity and update markers stay on storage version 1.
IDENTITY_VERSION = 1
SETTINGS_VERSION = 2


def crc16(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def with_crc(payload: bytes) -> bytes:
    """Append the trailing storage CRC every persistent record carries."""
    return payload + struct.pack("<H", crc16(payload))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--id", type=int, required=True, choices=range(4), metavar="0-3")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    image = bytearray([ERASED_BYTE] * EEPROM_SIZE)
    identity = with_crc(struct.pack(
        IDENTITY_FORMAT, IDENTITY_MAGIC, IDENTITY_VERSION, args.id))
    image[IDENTITY_ADDRESS : IDENTITY_ADDRESS + len(identity)] = identity

    # Values follow the quadrant::Settings field order: enter/exit threshold,
    # debounce scans, mux settle us, full scan ms, brightness, positive/negative
    # RGB565, orientation, per-square baseline and noise, runtime mode, calibrated.
    settings = with_crc(struct.pack(
        SETTINGS_FORMAT,
        SETTINGS_MAGIC, SETTINGS_VERSION, 70, 42, 3, 25, 16, 48, 0x07E0, 0x001F, 0,
        *([512] * SQUARES_PER_QUADRANT), *([4] * SQUARES_PER_QUADRANT), 0, 0,
    ))
    image[SETTINGS_ADDRESS : SETTINGS_ADDRESS + len(settings)] = settings
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(image)
    print(f"wrote {args.output} (quadrant id {args.id}, {len(image)} bytes)")


if __name__ == "__main__":
    main()
