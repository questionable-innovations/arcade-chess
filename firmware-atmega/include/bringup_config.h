#pragma once

#include <Arduino.h>

namespace bringup {

constexpr uint32_t kBusBaud = 38400;
constexpr uint16_t kDefaultEnterThreshold = 70;
constexpr uint16_t kDefaultExitThreshold = 42;
constexpr uint8_t kDefaultDebounceScans = 3;
constexpr uint16_t kDefaultMuxSettleUs = 25;
constexpr uint16_t kDefaultFullScanMs = 16;
constexpr uint8_t kDefaultBrightness = 48;
constexpr uint16_t kDefaultPositiveRgb565 = 0x07e0;  // green
constexpr uint16_t kDefaultNegativeRgb565 = 0x001f;  // blue
constexpr uint8_t kCalibrationScans = 128;
constexpr uint16_t kMaximumCalibrationNoise = 40;
constexpr uint8_t kEventQueueSize = 16;
constexpr uint8_t kLedFramesPerSecond = 25;

// MiniCore's ATmega328PB variant exposes all four PE pins with these macros.
// Failing at compile time is safer than silently driving a guessed pin.
#if !defined(PIN_PE0) || !defined(PIN_PE1) || !defined(PIN_PE2) || !defined(PIN_PE3)
#error "ATmega328PB MiniCore pin macros are required (PIN_PE0..PIN_PE3)"
#endif

constexpr uint8_t kLedEdgeA = PIN_PE0;
constexpr uint8_t kLedEdgeB = PIN_PE1;
constexpr uint8_t kLedSecondary = PIN_PE2;
constexpr uint8_t kLedPrimary = PIN_PE3;

}  // namespace bringup
