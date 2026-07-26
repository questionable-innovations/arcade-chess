#include "avr_flasher.h"

#include <string.h>

namespace {
constexpr uint32_t kAppLimit = arcade::kAvrApplicationLimit;
constexpr uint8_t kHexMaximumDataBytes = UINT8_MAX;
constexpr uint8_t kHexFixedRecordBytes = 5;  // count, address, type, checksum
constexpr uint8_t kHexHeaderBytes = 4;

enum class HexRecordType : uint8_t {
  kData = 0x00,
  kEndOfFile = 0x01,
  kExtendedSegmentAddress = 0x02,
  kStartSegmentAddress = 0x03,
  kExtendedLinearAddress = 0x04,
  kStartLinearAddress = 0x05,
};

int hexNibble(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'A' && c <= 'F') return c - 'A' + 10;
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  return -1;
}

int hexByte(const char* s) {
  const int high = hexNibble(s[0]);
  const int low = hexNibble(s[1]);
  return high < 0 || low < 0 ? -1 : (high << 4) | low;
}
}  // namespace

bool AvrFlasher::parseHexLine(const char* line) {
  const size_t text_length = strlen(line + 1);
  if (text_length < kHexFixedRecordBytes * 2 || text_length % 2) {
    fail("short/odd hex record"); return false;
  }
  uint8_t record[kHexHeaderBytes + kHexMaximumDataBytes + 1];
  const size_t byte_count = text_length / 2;
  if (byte_count > sizeof(record)) { fail("oversized hex record"); return false; }
  uint8_t checksum = 0;
  for (size_t i = 0; i < byte_count; ++i) {
    const int value = hexByte(line + 1 + i * 2);
    if (value < 0) { fail("bad hex digit"); return false; }
    record[i] = static_cast<uint8_t>(value);
    checksum += record[i];
  }
  const uint8_t length = record[0];
  if (byte_count != kHexFixedRecordBytes + length) {
    fail("record length mismatch"); return false;
  }
  if (checksum) { fail("record checksum mismatch"); return false; }
  const uint16_t address = static_cast<uint16_t>(record[1]) << 8 | record[2];
  const uint8_t type = record[3];
  const uint8_t* data = record + 4;

  switch (static_cast<HexRecordType>(type)) {
    case HexRecordType::kData: {
      const uint32_t absolute = ext_base_ + address;
      if (absolute >= kAppLimit || length > kAppLimit - absolute) {
        fail("record beyond application limit"); return false;
      }
      memcpy(image_ + absolute, data, length);
      if (absolute + length > image_size_) image_size_ = absolute + length;
      return true;
    }
    case HexRecordType::kEndOfFile:
      eof_seen_ = true;
      return true;
    case HexRecordType::kExtendedSegmentAddress:
      if (length != 2) { fail("bad extended address record"); return false; }
      ext_base_ = (static_cast<uint32_t>(data[0]) << 8 | data[1]) << 4;
      return true;
    case HexRecordType::kExtendedLinearAddress:
      if (length != 2) { fail("bad extended address record"); return false; }
      ext_base_ = (static_cast<uint32_t>(data[0]) << 8 | data[1]) << 16;
      return true;
    case HexRecordType::kStartSegmentAddress:
    case HexRecordType::kStartLinearAddress:  // start records carry no flash data
      return true;
    default:
      fail("unsupported hex record type");
      return false;
  }
}
