#!/usr/bin/env bash
set -euo pipefail

root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
id=""
programmer="usbasp"
port=""
bitclock="8"
with_bootloader=1
confirm=0

usage() {
  echo "usage: $0 --id 0..3 [--programmer usbasp] [--port PORT] [--bitclock 8] [--no-bootloader] --yes"
}

while (($#)); do
  case "$1" in
    --id) id="$2"; shift 2 ;;
    --programmer) programmer="$2"; shift 2 ;;
    --port) port="$2"; shift 2 ;;
    --bitclock) bitclock="$2"; shift 2 ;;
    --with-bootloader) with_bootloader=1; shift ;;
    --no-bootloader) with_bootloader=0; shift ;;
    --yes) confirm=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

if [[ ! "$id" =~ ^[0-3]$ ]]; then usage; exit 2; fi
if command -v pio >/dev/null 2>&1; then
  pio=(pio)
else
  pio=(uvx --from platformio platformio)
fi

eeprom="${TMPDIR:-/tmp}/arcade-quadrant-${id}.eeprom.bin"
python3 "$root/tools/make-quadrant-eeprom.py" --id "$id" --output "$eeprom"
"${pio[@]}" run -d "$root/firmware-atmega" -e ATmega328PB

echo "Target: ATmega328PB quadrant $id via $programmer${port:+ at $port}"
echo "This will erase/program flash, fuses, and EEPROM. Verify 5 V, 16 MHz crystal, pin 1, and ISP orientation."
if ((with_bootloader)); then
  echo "Urboot will also be installed. Do not serial-upload with multiple quadrants connected to the shared UART."
fi
if ((!confirm)); then echo "dry run only; repeat with --yes to program"; exit 0; fi

env_name="fuses_isp"
if ((with_bootloader)); then env_name="fuses_bootloader"; fi
pio_options=(--project-option "upload_protocol=$programmer")
if [[ -n "$port" ]]; then pio_options+=(--project-option "upload_port=$port"); fi
"${pio[@]}" run -d "$root/firmware-atmega" -e "$env_name" \
  "${pio_options[@]}" -t "$([[ $with_bootloader == 1 ]] && echo bootloader || echo fuses)"

avrdude="$HOME/.platformio/packages/tool-avrdude/avrdude"
avrdude_conf="$HOME/.platformio/packages/tool-avrdude/avrdude.conf"
firmware="$root/firmware-atmega/.pio/build/ATmega328PB/firmware.hex"
if [[ ! -x "$avrdude" ]]; then echo "avrdude not found at $avrdude" >&2; exit 1; fi
base_args=(-C "$avrdude_conf" -p m328pb -c "$programmer" -B "$bitclock")
if [[ -n "$port" ]]; then base_args+=(-P "$port"); fi
write_args=("${base_args[@]}")
if ((with_bootloader)); then write_args+=(-D); else write_args+=(-e); fi
"$avrdude" "${write_args[@]}" -U "flash:w:$firmware:i" -U "eeprom:w:$eeprom:r"
"$avrdude" "${base_args[@]}" -U "flash:v:$firmware:i" -U "eeprom:v:$eeprom:r"
echo "quadrant $id programmed and verified"
