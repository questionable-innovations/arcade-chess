#include "system_info.h"

#include <avr/boot.h>
#include <avr/wdt.h>

namespace quadrant::system_info {
namespace {
constexpr uint8_t kInternalBandgapAdcChannel = 0x0e;
constexpr uint16_t kAdcReferenceSettleUs = 250;
constexpr uint32_t kBandgapMillivoltAdcScale = 1125300UL;
constexpr uint8_t kBootResetVectorFuseMask = 0x01U;

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
  // Measuring the internal 1.1 V bandgap against AVCC lets us estimate the
  // supply without consuming a board ADC input. The scale includes 1023 ADC counts.
  const uint8_t previous_admux = ADMUX;
  ADMUX = _BV(REFS0) | kInternalBandgapAdcChannel;
  delayMicroseconds(kAdcReferenceSettleUs);
  ADCSRA |= _BV(ADSC);
  while (bit_is_set(ADCSRA, ADSC)) {}
  const uint16_t reading = ADC;
  ADMUX = previous_admux;
  return reading ? static_cast<uint16_t>(kBandgapMillivoltAdcScale / reading) : 0;
}

uint8_t highFuse() { return boot_lock_fuse_bits_get(GET_HIGH_FUSE_BITS); }

bool residentBootloaderEnabled() {
  return (highFuse() & kBootResetVectorFuseMask) == 0;
}

}  // namespace quadrant::system_info
