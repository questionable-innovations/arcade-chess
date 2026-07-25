#!/usr/bin/env bash
# Fast application update over ISP. Unlike provision-quadrant.sh this leaves the
# fuses and the EEPROM identity alone, runs a single avrdude session, and clocks
# the programmer as fast as the target allows.
#
# It still chip-erases: classic AVRs have no ISP page erase (only ATxmega and
# UPDI parts do), so avrdude's -D would AND the new image into the resident one.
# Urboot is therefore rewritten from the same pinned image the provisioning
# project installs, together with the boot-section lock bits it depends on.
# EESAVE is what carries the quadrant identity in EEPROM across the erase.
set -euo pipefail

root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
# shellcheck source=tools/isp-common.sh
source "$root/tools/isp-common.sh"

firmware_env="ATmega328PB"
provision_ini="$root/firmware-atmega/provisioning/platformio.ini"
programmer="auto"
port=""
programmer_baud=""
# ISP clock period in microseconds. USBasp rounds down to 750 kHz, comfortably
# under the f_cpu/4 ceiling of an 8 MHz part and ~8x the provisioning default.
bitclock="1"
hex=""
with_bootloader=1
verify=1
build=0

usage() {
  cat <<'EOF'
usage: flash-isp.sh [--hex FILE] [--env NAME] [--build]
                    [--programmer auto|usbasp|arduino-as-isp] [--port PORT]
                    [--bitclock US] [--baud BAUD] [--no-bootloader] [--no-verify]

Chip-erases the attached quadrant, writes the application and Urboot, and
restores the boot-section lock bits. Fuses and EEPROM are left as provisioned.
EOF
}

while (($#)); do
  case "$1" in
    --hex) hex="$2"; shift 2 ;;
    --env) firmware_env="$2"; shift 2 ;;
    --build) build=1; shift ;;
    --programmer) programmer="$2"; shift 2 ;;
    --port) port="$2"; shift 2 ;;
    --bitclock) bitclock="$2"; shift 2 ;;
    --baud) programmer_baud="$2"; shift 2 ;;
    --no-bootloader) with_bootloader=0; shift ;;
    --no-verify) verify=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

# Single source of truth for the bootloader image and lock bits: whatever the
# provisioning project would install is what this tool puts back.
ini_value() {
  awk -F' *= *' -v key="$1" '
    { sub(/^[[:space:]]+/, "", $1); sub(/[[:space:]]+$/, "", $2) }
    $1 == key { print $2; exit }
  ' "$provision_ini"
}
bootloader_hex="$(dirname "$provision_ini")/$(ini_value board_bootloader.file)"
lock_bits="$(ini_value board_bootloader.lock_bits)"
if ((with_bootloader)); then
  if [[ ! -f "$bootloader_hex" ]]; then
    echo "error: bootloader image not found at $bootloader_hex" >&2
    exit 1
  fi
  bootloader_hex="$(CDPATH='' cd -- "$(dirname -- "$bootloader_hex")" && pwd)/$(basename -- "$bootloader_hex")"
  if [[ ! "$lock_bits" =~ ^0x[0-9a-fA-F]{1,2}$ ]]; then
    echo "error: no board_bootloader.lock_bits in $provision_ini" >&2
    exit 1
  fi
fi

if ((build)); then
  find_pio
  "${pio[@]}" run -d "$root/firmware-atmega" -e "$firmware_env"
fi
if [[ -z "$hex" ]]; then
  hex="$root/firmware-atmega/.pio/build/$firmware_env/firmware.hex"
fi
if [[ ! -f "$hex" ]]; then
  echo "error: $hex not found; build first or pass --hex" >&2
  exit 1
fi

find_avrdude
resolve_programmer
avrdude_args=(-C "$avrdude_conf" -p m328pb -c "$programmer")
if [[ "$programmer" == "arduino_as_isp" ]]; then
  avrdude_args+=(-b "$programmer_baud")
else
  avrdude_args+=(-B "$bitclock")
fi
if [[ -n "$port" ]]; then avrdude_args+=(-P "$port"); fi

echo "Target: ATmega328PB via $programmer${port:+ at $port}"
echo "Image:  $hex"

# Pre-flight: proves the ISP link and reads EESAVE before anything is erased.
if ! hfuse="$("$avrdude" "${avrdude_args[@]}" -q -q -U hfuse:r:-:h)"; then
  echo "error: no ISP response; check the programmer, 5 V, pin 1, and orientation" >&2
  echo "hint: a part still on its 1 MHz factory clock needs a slower --bitclock 8" >&2
  exit 1
fi
hfuse="${hfuse//[[:space:]]/}"
eeprom_restore=1
if [[ "$hfuse" =~ ^0x[0-9a-fA-F]{1,2}$ ]] && (( (hfuse & 0x08) == 0 )); then
  eeprom_restore=0  # EESAVE programmed: the chip erase preserves EEPROM.
else
  echo "warning: EESAVE not programmed (hfuse $hfuse); saving and rewriting EEPROM"
fi

write_args=("${avrdude_args[@]}" -e -U "flash:w:$hex:i")
if ((eeprom_restore)); then
  eeprom_backup="$(mktemp "${TMPDIR:-/tmp}/arcade-eeprom.XXXXXX")"
  trap 'rm -f "$eeprom_backup"' EXIT
  "$avrdude" "${avrdude_args[@]}" -q -q -U "eeprom:r:$eeprom_backup:r"
  write_args+=(-U "eeprom:w:$eeprom_backup:r")
fi
if ((with_bootloader)); then
  # After the application, so an oversized image can never displace Urboot.
  write_args+=(-U "flash:w:$bootloader_hex:i" -U "lock:w:$lock_bits:m")
else
  echo "warning: --no-bootloader leaves no Urboot; fw-flash updates stop working"
fi
if ((!verify)); then write_args+=(-V); fi

started=$SECONDS
"$avrdude" "${write_args[@]}"
if ((verify)); then
  echo "flashed and verified in $((SECONDS - started))s"
else
  echo "flashed (unverified) in $((SECONDS - started))s"
fi
