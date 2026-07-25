# shellcheck shell=bash
# Shared ISP plumbing for provision-quadrant.sh and flash-isp.sh.
#
# Sourced, never executed. Callers declare `programmer`, `port` and
# `programmer_baud` as globals, then call resolve_programmer to fill them in.
#
# shellcheck disable=SC2034  # the globals set here are read by the callers

ARDUINO_ISP_BAUD=19200

find_pio() {
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
}

find_avrdude() {
  avrdude="$HOME/.platformio/packages/tool-avrdude/avrdude"
  avrdude_conf="$HOME/.platformio/packages/tool-avrdude/avrdude.conf"
  if [[ ! -x "$avrdude" ]]; then echo "avrdude not found at $avrdude" >&2; exit 1; fi
}

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

# Normalises $programmer (auto/aliases -> an avrdude programmer id) and, for
# Arduino as ISP, fills in the serial port and its fixed sketch baud rate.
resolve_programmer() {
  if [[ "$programmer" == "auto" ]]; then
    if [[ -n "${port:-}" ]]; then
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
    programmer_baud="${programmer_baud:-$ARDUINO_ISP_BAUD}"
    if [[ -z "${port:-}" ]]; then detect_arduino_isp_port; fi
  fi
}
