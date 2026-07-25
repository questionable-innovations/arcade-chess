#!/usr/bin/env python3
"""Build a complete, reproducible ATmega328PB EEPROM image."""

import argparse
import struct
from pathlib import Path


def crc16(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--id", type=int, required=True, choices=range(4), metavar="0-3")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    image = bytearray([0xFF] * 1024)
    identity_without_crc = struct.pack("<IBB", 0x51434944, 1, args.id)
    identity = identity_without_crc + struct.pack("<H", crc16(identity_without_crc))
    image[0 : len(identity)] = identity

    settings_without_crc = struct.pack(
        "<IBHHBHHBHHB16H16BBB",
        0x51434346, 1, 70, 42, 3, 25, 16, 48, 0x07E0, 0x001F, 0,
        *([512] * 16), *([4] * 16), 0, 0,
    )
    settings = settings_without_crc + struct.pack("<H", crc16(settings_without_crc))
    image[16 : 16 + len(settings)] = settings
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(image)
    print(f"wrote {args.output} (quadrant id {args.id}, {len(image)} bytes)")


if __name__ == "__main__":
    main()
