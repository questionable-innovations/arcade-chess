#!/usr/bin/env bash
set -euo pipefail

root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
id=""
programmer="auto"
port=""
bitclock="8"
with_bootloader=1
confirm=0
programmer_baud=""

usage() {
  echo "usage: $0 --id 0..3 [--programmer auto|usbasp|arduino-as-isp] [--port PORT] [--bitclock 8] [--no-bootloader] --yes"
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
clock="$(awk -F'"' '/^QUADRANT_CLOCK = "(internal|external)"$/ { print $2; exit }' "$root/tools/pio-targets.py")"
case "$clock" in
  internal)
    firmware_env="ATmega328PB"
    clock_description="internal 8 MHz RC"
    ;;
  external)
    firmware_env="ATmega328PB"
    clock_description="external 16 MHz crystal"
    ;;
  *) usage; exit 2 ;;
esac
if command -v pio >/dev/null 2>&1; then
  pio=(pio)
elif [[ -x "$HOME/.platformio/penv/bin/pio" ]]; then
  pio=("$HOME/.platformio/penv/bin/pio")
elif command -v uvx >/dev/null 2>&1; then
  pio=(uvx --from platformio platformio)
else
  echo "PlatformIO not found; install pio or uvx first" >&2
  exit 1
fi

usbasp_attached() {
  if command -v system_profiler >/dev/null 2>&1; then
    system_profiler SPUSBDataType 2>/dev/null |
      grep -Eiq 'USBasp|16c0[^[:alnum:]]+05dc'
  elif command -v lsusb >/dev/null 2>&1; then
    lsusb 2>/dev/null | grep -Eiq 'USBasp|16c0:05dc'
  else
    return 1
  fi
}

detect_arduino_isp_port() {
  local candidates=()
  local pattern candidate
  for pattern in \
      '/dev/cu.usbmodem*' '/dev/cu.usbserial*' \
      '/dev/ttyACM*' '/dev/ttyUSB*'; do
    while IFS= read -r candidate; do
      [[ -n "$candidate" ]] && candidates+=("$candidate")
    done < <(compgen -G "$pattern" || true)
  done
  if ((${#candidates[@]} == 1)); then
    port="${candidates[0]}"
  elif ((${#candidates[@]} == 0)); then
    echo "no Arduino-as-ISP serial port found; connect it or pass --port" >&2
    exit 1
  else
    echo "multiple possible Arduino-as-ISP ports found; pass --port:" >&2
    printf '  %s\n' "${candidates[@]}" >&2
    exit 1
  fi
}

if [[ "$programmer" == "auto" ]]; then
  if [[ -n "$port" ]]; then
    programmer="arduino_as_isp"
  elif usbasp_attached; then
    programmer="usbasp"
  else
    programmer="arduino_as_isp"
  fi
fi
case "$programmer" in
  arduino-as-isp|arduinoasisp|stk500v1)
    programmer="arduino_as_isp"
    ;;
esac
if [[ "$programmer" == "arduino_as_isp" ]]; then
  programmer_baud="19200"
  if [[ -z "$port" ]]; then detect_arduino_isp_port; fi
fi

eeprom="${TMPDIR:-/tmp}/arcade-quadrant-${id}.eeprom.bin"
python3 "$root/tools/make-quadrant-eeprom.py" --id "$id" --output "$eeprom"
"${pio[@]}" run -d "$root/firmware-atmega" -e "$firmware_env"

echo "Target: ATmega328PB quadrant $id via $programmer${port:+ at $port}"
echo "Clock: $clock_description (firmware environment $firmware_env)"
echo "This will erase/program flash, fuses, and EEPROM. Verify 5 V, pin 1, and ISP orientation."
if [[ "$clock" == "external" ]]; then
  echo "External mode requires the 16 MHz crystal to be fitted before fuse programming."
fi
if ((with_bootloader)); then
  echo "Urboot will also be installed. Do not serial-upload with multiple quadrants connected to the shared UART."
fi
if ((!confirm)); then echo "dry run only; repeat with --yes to program"; exit 0; fi

env_name="fuses_isp"
if ((with_bootloader)); then env_name="fuses_bootloader"; fi
# The fuse environments live in the separate provisioning/ project (kept out of
# the IDE sidebar of the main firmware project).
provision_dir="$root/firmware-atmega/provisioning"
provision_conf="$(mktemp "$provision_dir/.platformio-provision.XXXXXX")"
trap 'rm -f "$provision_conf"' EXIT
# `platformio run` has no --project-option flag. Generate a short-lived copy of
# the checked-in provisioning config for the two options exposed by this wrapper;
# keeping it beside platformio.ini also preserves relative bootloader paths.
if [[ "$programmer" == "arduino_as_isp" ]]; then
  upload_flags="-P$port|-b$programmer_baud"
else
  upload_flags="-B$bitclock"
  if [[ -n "$port" ]]; then
    upload_flags="$upload_flags|-P$port"
  fi
fi
awk -v protocol="$programmer" -v flags="$upload_flags" '
  /^upload_protocol =/ {
    print "upload_protocol = " protocol
    next
  }
  /^upload_flags =/ {
    print "upload_flags ="
    count = split(flags, values, "|")
    for (flag_index = 1; flag_index <= count; ++flag_index)
      print "  " values[flag_index]
    next
  }
  { print }
' "$provision_dir/platformio.ini" > "$provision_conf"
pio_options=(--project-conf "$provision_conf")
if [[ -n "$port" ]]; then pio_options+=(--upload-port "$port"); fi
"${pio[@]}" run -d "$provision_dir" -e "$env_name" \
  "${pio_options[@]}" -t "$([[ $with_bootloader == 1 ]] && echo bootloader || echo fuses)"

avrdude="$HOME/.platformio/packages/tool-avrdude/avrdude"
avrdude_conf="$HOME/.platformio/packages/tool-avrdude/avrdude.conf"
firmware="$root/firmware-atmega/.pio/build/$firmware_env/firmware.hex"
if [[ ! -x "$avrdude" ]]; then echo "avrdude not found at $avrdude" >&2; exit 1; fi
base_args=(-C "$avrdude_conf" -p m328pb -c "$programmer")
if [[ "$programmer" == "arduino_as_isp" ]]; then
  base_args+=(-b "$programmer_baud")
else
  base_args+=(-B "$bitclock")
fi
if [[ -n "$port" ]]; then base_args+=(-P "$port"); fi
write_args=("${base_args[@]}")
if ((with_bootloader)); then write_args+=(-D); else write_args+=(-e); fi
"$avrdude" "${write_args[@]}" -U "flash:w:$firmware:i" -U "eeprom:w:$eeprom:r"
"$avrdude" "${base_args[@]}" -U "flash:v:$firmware:i" -U "eeprom:v:$eeprom:r"
echo "quadrant $id programmed and verified"
