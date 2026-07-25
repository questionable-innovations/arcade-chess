#pragma once

#include <Arduino.h>
#include <FastLED.h>

// FastLED 3.10 groups the 328PB with the 328P and omits its extra PORTE pins.
// MiniCore maps PE0..PE3 to 23..26, so supply the missing direct-I/O entries.
#if defined(__AVR_ATmega328PB__)
FASTLED_NAMESPACE_BEGIN
_FL_DEFPIN(PIN_PE0, 0, E);
_FL_DEFPIN(PIN_PE1, 1, E);
_FL_DEFPIN(PIN_PE2, 2, E);
_FL_DEFPIN(PIN_PE3, 3, E);
FASTLED_NAMESPACE_END
#endif
