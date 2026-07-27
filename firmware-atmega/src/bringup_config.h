#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

namespace bringup {

constexpr uint32_t kBusBaud = arcade::kBusBaud;
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
constexpr uint8_t kLedFramesPerSecond = arcade::kRenderFramesPerSecond;
constexpr uint16_t kDiagnosticDumpIntervalMs = 250;
constexpr uint8_t kMuxChannelCount = 8;
constexpr uint8_t kPixelsPerSquare = 2;
constexpr uint8_t kSquareStripPixels = arcade::kSquaresPerQuadrant * kPixelsPerSquare;
constexpr uint8_t kEdgeStripPixels = 8;
constexpr uint16_t kIdentifyBlinkMs = 180;
// With no ESP on the bus nothing ever opens a render window, so the strips would
// stay dark on an otherwise healthy node. After this much bus silence the
// quadrant idles on a shimmer instead.
constexpr uint16_t kBusIdleTimeoutMs = 3000;
constexpr uint8_t kIdentifyRed = 255;
constexpr uint8_t kIdentifyGreen = 72;
constexpr uint8_t kIdentifyBlue = 0;
// A node with no EEPROM identity can never be addressed, so it can never answer
// a scan or report its own state. The strips are the only channel left.
constexpr uint16_t kUnprovisionedBlinkMs = 500;
// Chance out of 255 that any given pixel is lit on a shimmer frame.
constexpr uint8_t kShimmerOnChance = 40;
constexpr uint16_t kDefaultAdcMidpoint = 512;
constexpr uint16_t kMinimumCalibrationBaseline = 120;
constexpr uint16_t kMaximumCalibrationBaseline = 900;
constexpr uint16_t kMinimumEnterThreshold = 10;
constexpr uint16_t kMaximumEnterThreshold = 400;
constexpr uint8_t kMinimumDebounceScans = 1;
constexpr uint8_t kMaximumDebounceScans = 20;
constexpr uint16_t kMinimumMuxSettleUs = 5;
constexpr uint16_t kMaximumMuxSettleUs = 500;
constexpr uint16_t kMinimumFullScanMs = 8;
constexpr uint16_t kMaximumFullScanMs = 200;
constexpr uint8_t kMaximumOrientation = 7;

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
