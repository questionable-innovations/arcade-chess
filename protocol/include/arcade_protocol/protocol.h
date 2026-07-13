#pragma once

#include <stddef.h>
#include <stdint.h>

namespace arcade {

constexpr uint8_t kProtocolVersion = 1;
constexpr uint8_t kEspAddress = 0x80;
constexpr uint8_t kBroadcastAddress = 0xff;
constexpr uint8_t kInvalidNodeAddress = 0xff;
constexpr uint8_t kQuadrantCount = 4;
constexpr uint8_t kQuadrantsPerBoardEdge = 2;
constexpr uint8_t kQuadrantWidth = 4;
constexpr uint8_t kBoardWidth = kQuadrantsPerBoardEdge * kQuadrantWidth;
constexpr uint8_t kSquaresPerQuadrant = kQuadrantWidth * kQuadrantWidth;
constexpr uint8_t kBoardSquareCount = kBoardWidth * kBoardWidth;
constexpr uint8_t kMaximumRawCaptureScans = 32;
constexpr uint8_t kRenderFramesPerSecond = 25;
constexpr uint32_t kBusBaud = 38400;
constexpr uint32_t kAvrFlashBytes = 32768;
constexpr uint16_t kAvrFlashPageBytes = 128;
constexpr uint32_t kAvrApplicationLimit = 32384;
constexpr size_t kHeaderSize = 8;
constexpr size_t kCrcSize = 2;
constexpr size_t kMaxPayload = 112;
constexpr size_t kMaxDecodedFrame = kHeaderSize + kMaxPayload + kCrcSize;
constexpr size_t kMaxEncodedFrame = kMaxDecodedFrame + (kMaxDecodedFrame / 254) + 2;

enum Flag : uint8_t {
  kResponse = 1u << 0,
  kEventPending = 1u << 1,
  kAckRequired = 1u << 2,
  kError = 1u << 3,
};

enum class MessageType : uint8_t {
  kPing = 0x01,
  kInfo = 0x02,
  kStatus = 0x03,
  kTimeSync = 0x04,
  kConfigGet = 0x05,
  kConfigSet = 0x06,
  kError = 0x0f,

  kPollEvents = 0x20,
  kEventBatch = 0x21,
  kGetSnapshot = 0x22,
  kSensorSnapshot = 0x23,
  kGetRawScan = 0x24,
  kRawScan = 0x25,

  kCalibrate = 0x30,
  kCalibrationResult = 0x31,

  kSetSquares = 0x40,
  kSetBrightness = 0x41,
  kIdentify = 0x42,
  kClearLighting = 0x43,
  kRenderWindow = 0x44,
  kSetDebug = 0x50,

  kFwPreflight = 0x60,
  kMaintenanceBegin = 0x61,
  kFwPrepare = 0x62,
  kFwEnterBootloader = 0x63,
  kMaintenanceEnd = 0x64,
  kFwHealth = 0x65,
  kFwConfirm = 0x66,
};

enum class SensorState : uint8_t {
  kEmpty = 0,
  kPositive = 1,
  kNegative = 2,
  kUncertain = 3,
};

enum class RuntimeMode : uint8_t {
  kNormal = 0,
  kBringup = 1,
};

enum class FirmwareState : uint8_t {
  kNone = 0,
  kRequested = 1,
  kProgramming = 2,
  kCandidate = 3,
  kValid = 4,
};

// Numeric values are part of the UART wire contract and the factory EEPROM
// tooling. Add new keys at the end so deployed tooling never changes meaning.
enum class ConfigKey : uint8_t {
  kEnterThreshold = 1,
  kExitThreshold = 2,
  kDebounceScans = 3,
  kMuxSettleUs = 4,
  kFullScanMs = 5,
  kBrightness = 6,
  kPositiveRgb565 = 7,
  kNegativeRgb565 = 8,
  kOrientation = 9,
  kRuntimeMode = 10,
};

constexpr uint8_t configKey(ConfigKey key) { return static_cast<uint8_t>(key); }

enum class DecodeResult : uint8_t {
  kNone,
  kFrame,
  kEmpty,
  kOverflow,
  kBadCobs,
  kBadLength,
  kBadCrc,
  kBadVersion,
};

struct Frame {
  uint8_t flags = 0;
  uint8_t source = 0;
  uint8_t destination = 0;
  MessageType type = MessageType::kError;
  uint8_t sequence = 0;
  uint16_t payload_length = 0;
  uint8_t payload[kMaxPayload]{};
};

uint16_t crc16Ccitt(const uint8_t* data, size_t length);

// Returns encoded byte count including the trailing zero delimiter, or zero when
// the frame/output is invalid.
size_t encodeFrame(const Frame& frame, uint8_t* output, size_t output_capacity);

// Input is one COBS packet without its trailing zero delimiter.
DecodeResult decodeFrame(const uint8_t* encoded, size_t encoded_length, Frame& output);

class StreamDecoder {
 public:
  DecodeResult push(uint8_t byte, Frame& output);
  void reset();
  uint32_t overflowCount() const { return overflow_count_; }

 private:
  uint8_t encoded_[kMaxEncodedFrame]{};
  size_t length_ = 0;
  bool dropping_ = false;
  uint32_t overflow_count_ = 0;
};

inline void putU16(uint8_t* output, uint16_t value) {
  output[0] = static_cast<uint8_t>(value);
  output[1] = static_cast<uint8_t>(value >> 8);
}

inline uint16_t getU16(const uint8_t* input) {
  return static_cast<uint16_t>(input[0]) |
         (static_cast<uint16_t>(input[1]) << 8);
}

inline void putU32(uint8_t* output, uint32_t value) {
  output[0] = static_cast<uint8_t>(value);
  output[1] = static_cast<uint8_t>(value >> 8);
  output[2] = static_cast<uint8_t>(value >> 16);
  output[3] = static_cast<uint8_t>(value >> 24);
}

inline uint32_t getU32(const uint8_t* input) {
  return static_cast<uint32_t>(input[0]) |
         (static_cast<uint32_t>(input[1]) << 8) |
         (static_cast<uint32_t>(input[2]) << 16) |
         (static_cast<uint32_t>(input[3]) << 24);
}

}  // namespace arcade
