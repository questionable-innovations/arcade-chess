#include "system_info.h"

#include <avr/boot.h>
#include <avr/wdt.h>

namespace quadrant::system_info {
namespace {
uint8_t reset_cause __attribute__((section(".noinit")));
void captureResetCause() __attribute__((naked, used, section(".init3")));
void captureResetCause() {
  reset_cause = MCUSR;
  MCUSR = 0;
  wdt_disable();
}
}  // namespace

uint8_t resetCause() { return reset_cause; }

uint16_t supplyMillivolts() {
  const uint8_t previous_admux = ADMUX;
  ADMUX = _BV(REFS0) | 0x0e;
  delayMicroseconds(250);
  ADCSRA |= _BV(ADSC);
  while (bit_is_set(ADCSRA, ADSC)) {}
  const uint16_t reading = ADC;
  ADMUX = previous_admux;
  return reading ? static_cast<uint16_t>(1125300UL / reading) : 0;
}

uint8_t highFuse() { return boot_lock_fuse_bits_get(GET_HIGH_FUSE_BITS); }

bool residentBootloaderEnabled() { return (highFuse() & 0x01U) == 0; }

}  // namespace quadrant::system_info
