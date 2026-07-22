# Arcade Chess Urboot

This directory contains the manufacturing-ready bootloader image used by every
ATmega328PB quadrant. It is based on upstream Urboot commit
`e313db2e7df1e0273e2d62f9dba769fea81c7a8d` (2026-04-21, Urboot 8.0) with the
small project patch in `urboot-arcade.patch`.

The patch adds two things:

- a direct application handoff at the existing hardware boot address `0x7E00`;
- a silent participant mode, so multiple AVRs program from the same Urprotocol
  byte stream while only the selected leader sends replies.

`arcade-chess-urboot.hex` is the reviewed build artifact. Its SHA-256 is:

```text
f77ca7b03299fbd80c779764d4b6ac4cf1303e18e7c1e9ad1f3bcf7923158347
```

It occupies 444 bytes and fits the existing 512-byte boot section. The build is
for `atmega328pb`, UART0 autobaud, two-second watchdog window, hardware
boot mode, EEPROM support, chip erase, and Urprotocol. The response bytes for
this exact feature build are `UR_INSYNC=0xE0` and `UR_OK=0x78`.

Urboot's autobaud implementation makes this same reviewed image usable with the
project's 8 MHz internal-RC and 16 MHz external-crystal clock settings.
`F_CPU=16000000L` below records the value used to reproduce the reviewed
artifact; application firmware must still be compiled for its real clock.

To reproduce it, check out the pinned upstream commit, apply the patch, then use
the PlatformIO AVR toolchain to compile `src/urboot.c` with:

```sh
avr-gcc -DSTART=0x7e00UL \
  -Wl,--section-start=.text=0x7e00 \
  -Wl,--section-start=.version=0x7ffa \
  -D_urboot_AVAILABLE=14 -g -Wundef -Wall -Os -fno-split-wide-types \
  -mrelax -mmcu=atmega328pb -DF_CPU=16000000L -Wno-clobbered \
  -DWDTO=2S -DAUTOBAUD=1 -DDUAL=0 -DEEPROM=1 -DVBL=0 \
  -DPGMWRITEPAGE=0 -DFRILLS=7 -Wl,--relax -nostartfiles -nostdlib \
  -o arcade-chess-urboot.elf src/urboot.c
avr-objcopy -j .text -j .data -j .version \
  --set-section-flags .version=alloc,load -O ihex \
  arcade-chess-urboot.elf arcade-chess-urboot.hex
```

Urboot is GPL-3.0 licensed. The pinned source and license are available from the
[upstream repository](https://github.com/stefanrueger/urboot).
