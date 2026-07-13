#include "avr_flasher.h"

#include <esp_system.h>
#include <string.h>

namespace {
constexpr uint32_t kAppLimit = arcade::kAvrApplicationLimit;
constexpr uint16_t kPageSize = arcade::kAvrFlashPageBytes;
constexpr uint32_t kBusBaud = arcade::kBusBaud;
// Must match board_bootloader.speed in firmware-atmega/platformio.ini. If sync
// proves unreliable on the diode-OR return at this rate, rebuild urboot slower.
constexpr uint32_t kBootloaderBaud = 115200;
// This provisioned urboot u8.0 build encodes its MCU/features in response bytes.
constexpr uint8_t kUrInSync = 0xa0;
constexpr uint8_t kUrOk = 0x78;
constexpr uint8_t kCrcEop = 0x20;
constexpr uint8_t kUrGetSync = 0x30;
constexpr uint8_t kUrProgramFlashPage = 0x02;
constexpr uint8_t kUrReadFlashPage = 0x03;
constexpr uint8_t kUrLeaveProgmode = 0x51;
constexpr uint8_t kUrResponseOverhead = 2;
constexpr uint8_t kMaximumPageRetries = 3;
constexpr uint8_t kMaximumHealthPollRetries = 4;
constexpr uint8_t kMaximumSyncAttempts = 80;
constexpr uint8_t kProgressPageInterval = 32;
constexpr uint32_t kHandoffTimeoutMs = 8000;
constexpr uint32_t kBootResetDelayMs = 50;
constexpr uint32_t kApplicationBootDelayMs = 1500;
constexpr uint32_t kHealthResponseTimeoutMs = 3000;
constexpr uint32_t kSyncCommandTimeoutMs = 80;
constexpr uint32_t kPageCommandTimeoutMs = 400;
constexpr uint32_t kLeaveProgrammingTimeoutMs = 300;
constexpr uint16_t kSerialPollDelayUs = 200;
constexpr uint8_t kHexMaximumDataBytes = UINT8_MAX;
constexpr uint8_t kHexFixedRecordBytes = 5;  // count, address, type, checksum
constexpr uint8_t kHexHeaderBytes = 4;
constexpr uint8_t kHealthPayloadBytes = 14;
constexpr uint8_t kHealthMarkerOffset = 0;
constexpr uint8_t kHealthResetCauseOffset = 1;
constexpr uint8_t kHealthUpdateIdOffset = 6;
constexpr uint8_t kHealthCrcOffset = 10;

enum class HexRecordType : uint8_t {
  kData = 0x00,
  kEndOfFile = 0x01,
  kExtendedSegmentAddress = 0x02,
  kStartSegmentAddress = 0x03,
  kExtendedLinearAddress = 0x04,
  kStartLinearAddress = 0x05,
};

uint32_t crc32Update(uint32_t crc, const uint8_t* data, size_t length) {
  crc = ~crc;
  for (size_t i = 0; i < length; ++i) {
    crc ^= data[i];
    for (uint8_t bit = 0; bit < 8; ++bit) {
      crc = (crc >> 1) ^ (0xEDB88320UL & (-(crc & 1)));
    }
  }
  return ~crc;
}

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

uint32_t nonzeroRandom() {
  uint32_t value;
  do { value = esp_random(); } while (!value);
  return value;
}
}  // namespace

void AvrFlasher::begin(BusManager& bus, HardwareSerial& bus_serial) {
  bus_ = &bus;
  serial_ = &bus_serial;
}

bool AvrFlasher::start(uint8_t node) {
  if (phase_ != Phase::kIdle || node >= arcade::kQuadrantCount ||
      bus_->programmingHandoff()) return false;
  if (!bus_->isOnline(node)) {
    Serial.printf("FLASH FAIL node=%u offline\n", node);
    return false;
  }
  image_ = static_cast<uint8_t*>(malloc(kAppLimit));
  if (!image_) {
    Serial.println(F("FLASH FAIL no memory for image staging"));
    return false;
  }
  memset(image_, 0xff, kAppLimit);
  node_ = node;
  image_size_ = 0;
  ext_base_ = 0;
  eof_seen_ = false;
  phase_ = Phase::kReceiveHex;
  started_ms_ = millis();
  Serial.printf("HEX-READY node=%u\n", node);
  return true;
}

void AvrFlasher::abort(const char* reason) {
  if (phase_ == Phase::kIdle) return;
  fail(reason);
}

bool AvrFlasher::consumeLine(const char* line) {
  if (phase_ != Phase::kReceiveHex) return false;
  if (!line[0]) return true;
  if (line[0] != ':') {
    fail("not an Intel HEX record");
    return true;
  }
  if (!parseHexLine(line)) return true;  // parse failure already reported
  if (eof_seen_) finishReceive();
  else Serial.println(F("+"));
  return true;
}

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

void AvrFlasher::finishReceive() {
  if (!image_size_) { fail("empty image"); return; }
  image_crc32_ = crc32Update(0, image_, image_size_);
  page_count_ = static_cast<uint16_t>((image_size_ + kPageSize - 1) / kPageSize);
  token_ = nonzeroRandom();
  update_id_ = nonzeroRandom();
  Serial.printf("IMAGE size=%u crc32=0x%08x pages=%u\n", image_size_, image_crc32_,
                page_count_);
  if (!bus_->beginFirmwareHandoff(node_, token_, update_id_, image_size_,
                                  image_crc32_)) {
    fail("handoff rejected (bus busy?)");
    return;
  }
  phase_ = Phase::kAwaitHandoff;
  deadline_ms_ = millis() + kHandoffTimeoutMs;
}

void AvrFlasher::tick(uint32_t now_ms) {
  switch (phase_) {
    case Phase::kIdle:
    case Phase::kReceiveHex:
      return;

    case Phase::kAwaitHandoff:
      if (bus_->programmingHandoff()) {
        serial_->updateBaudRate(kBootloaderBaud);
        while (serial_->available()) serial_->read();
        phase_ = Phase::kSync;
        sync_attempts_ = 0;
        deadline_ms_ = now_ms + kBootResetDelayMs;
      } else if (static_cast<int32_t>(now_ms - deadline_ms_) >= 0) {
        fail("bootloader entry not acknowledged");
      }
      return;

    case Phase::kSync:
      if (static_cast<int32_t>(now_ms - deadline_ms_) < 0) return;
      if (urSync()) {
        Serial.printf("SYNC ok after %u attempt(s)\n", sync_attempts_ + 1);
        phase_ = Phase::kProgram;
        page_ = 0;
        page_retries_ = 0;
        return;
      }
      if (++sync_attempts_ >= kMaximumSyncAttempts) {
        fail("bootloader sync timeout"); return;
      }
      deadline_ms_ = now_ms + kBootResetDelayMs;
      return;

    case Phase::kProgram: {
      const uint32_t address = static_cast<uint32_t>(page_) * kPageSize;
      if (urProgramPage(address)) {
        page_retries_ = 0;
        if (++page_ >= page_count_) { phase_ = Phase::kVerify; page_ = 0; return; }
        if (!(page_ % kProgressPageInterval)) {
          Serial.printf("PROG %u/%u pages\n", page_, page_count_);
        }
      } else if (++page_retries_ > kMaximumPageRetries) {
        fail("page program retries exhausted");
      }
      return;
    }

    case Phase::kVerify: {
      const uint32_t address = static_cast<uint32_t>(page_) * kPageSize;
      if (urVerifyPage(address)) {
        page_retries_ = 0;
        if (++page_ < page_count_) {
          if (!(page_ % kProgressPageInterval)) {
            Serial.printf("VRFY %u/%u pages\n", page_, page_count_);
          }
          return;
        }
        Serial.println(F("VERIFY ok; leaving bootloader"));
        urLeaveProgmode();
        restoreBusBaud();
        bus_->endFirmwareMaintenance(token_);
        phase_ = Phase::kAwaitBoot;
        poll_retries_ = 0;
        deadline_ms_ = millis() + kApplicationBootDelayMs;
      } else if (++page_retries_ > kMaximumPageRetries) {
        fail("page verify failed");
      }
      return;
    }

    case Phase::kAwaitBoot:
      if (static_cast<int32_t>(now_ms - deadline_ms_) < 0) return;
      if (!bus_->enqueue(node_, arcade::MessageType::kFwHealth, nullptr, 0)) return;
      phase_ = Phase::kHealth;
      deadline_ms_ = now_ms + kHealthResponseTimeoutMs;
      return;

    case Phase::kHealth:
    case Phase::kConfirm:
      if (static_cast<int32_t>(now_ms - deadline_ms_) < 0) return;
      if (++poll_retries_ > kMaximumHealthPollRetries) {
        fail(phase_ == Phase::kHealth ? "no application health response"
                                      : "no confirm response");
        return;
      }
      if (phase_ == Phase::kHealth) {
        bus_->enqueue(node_, arcade::MessageType::kFwHealth, nullptr, 0);
      } else {
        uint8_t payload[4];
        arcade::putU32(payload, update_id_);
        bus_->enqueue(node_, arcade::MessageType::kFwConfirm, payload, sizeof(payload));
      }
      deadline_ms_ = now_ms + kHealthResponseTimeoutMs;
      return;
  }
}

void AvrFlasher::onFwResponse(uint8_t node, arcade::MessageType type, bool ok,
                              const uint8_t* payload, uint8_t length) {
  if (node != node_) return;
  if (phase_ == Phase::kAwaitHandoff && !ok &&
      (type == arcade::MessageType::kFwPrepare ||
       type == arcade::MessageType::kFwEnterBootloader)) {
    fail(type == arcade::MessageType::kFwPrepare ? "prepare rejected"
                                                 : "enter rejected");
    return;
  }
  if (phase_ == Phase::kHealth && type == arcade::MessageType::kFwHealth) {
    if (!ok || length < kHealthPayloadBytes) { fail("health request rejected"); return; }
    const uint8_t marker_state = payload[kHealthMarkerOffset];
    const uint32_t update_id = arcade::getU32(payload + kHealthUpdateIdOffset);
    const uint32_t crc32 = arcade::getU32(payload + kHealthCrcOffset);
    Serial.printf("HEALTH marker=%u reset=0x%02x update_id=0x%08x crc32=0x%08x\n",
                  marker_state, payload[kHealthResetCauseOffset], update_id, crc32);
    if (marker_state != static_cast<uint8_t>(arcade::FirmwareState::kCandidate) ||
        update_id != update_id_ || crc32 != image_crc32_) {
      fail("health/marker mismatch");
      return;
    }
    uint8_t confirm[4];
    arcade::putU32(confirm, update_id_);
    bus_->enqueue(node_, arcade::MessageType::kFwConfirm, confirm, sizeof(confirm));
    phase_ = Phase::kConfirm;
    poll_retries_ = 0;
    deadline_ms_ = millis() + kHealthResponseTimeoutMs;
  } else if (phase_ == Phase::kConfirm && type == arcade::MessageType::kFwConfirm) {
    if (!ok) { fail("confirm rejected"); return; }
    finishSuccess();
  }
}

void AvrFlasher::finishSuccess() {
  Serial.printf("FLASH OK node=%u size=%u crc32=0x%08x pages=%u elapsed_ms=%lu\n",
                node_, image_size_, image_crc32_, page_count_,
                millis() - started_ms_);
  free(image_);
  image_ = nullptr;
  phase_ = Phase::kIdle;
}

void AvrFlasher::fail(const char* reason) {
  Serial.printf("FLASH FAIL node=%u phase=%u reason=%s\n", node_,
                static_cast<unsigned>(phase_), reason);
  if (bus_->programmingHandoff()) restoreBusBaud();
  if (phase_ >= Phase::kAwaitHandoff) {
    // Ends the quiet lease for the other quadrants even when bootloader entry
    // was never acknowledged; the target's urboot times out back to whatever
    // application is present. Re-run fw-flash to retry.
    bus_->endFirmwareMaintenance(token_);
  }
  free(image_);
  image_ = nullptr;
  phase_ = Phase::kIdle;
}

void AvrFlasher::restoreBusBaud() {
  serial_->flush();
  serial_->updateBaudRate(kBusBaud);
  while (serial_->available()) serial_->read();
}

bool AvrFlasher::urCommand(const uint8_t* request, size_t request_length,
                           uint8_t* response, size_t response_length,
                           uint32_t timeout_ms) {
  while (serial_->available()) serial_->read();
  serial_->write(request, request_length);
  serial_->flush();
  uint8_t framed[kUrResponseOverhead + kPageSize];
  const size_t total = response_length + kUrResponseOverhead;
  if (total > sizeof(framed)) return false;
  const uint32_t deadline = millis() + timeout_ms;
  size_t received = 0;
  while (received < total) {
    if (static_cast<int32_t>(millis() - deadline) >= 0) return false;
    const int value = serial_->read();
    if (value < 0) { delayMicroseconds(kSerialPollDelayUs); continue; }
    framed[received++] = static_cast<uint8_t>(value);
  }
  if (framed[0] != kUrInSync || framed[total - 1] != kUrOk) return false;
  if (response_length) memcpy(response, framed + 1, response_length);
  return true;
}

bool AvrFlasher::urSync() {
  const uint8_t request[] = {kUrGetSync, kCrcEop};
  return urCommand(request, sizeof(request), nullptr, 0, kSyncCommandTimeoutMs);
}

bool AvrFlasher::urProgramPage(uint32_t byte_address) {
  uint8_t request[4 + kPageSize + 1];
  request[0] = kUrProgramFlashPage;
  // Urprotocol carries the direct byte address, low byte first.
  request[1] = static_cast<uint8_t>(byte_address);
  request[2] = static_cast<uint8_t>(byte_address >> 8);
  request[3] = static_cast<uint8_t>(kPageSize);
  memcpy(request + 4, image_ + byte_address, kPageSize);
  request[4 + kPageSize] = kCrcEop;
  return urCommand(request, sizeof(request), nullptr, 0, kPageCommandTimeoutMs);
}

bool AvrFlasher::urVerifyPage(uint32_t byte_address) {
  const uint8_t request[] = {kUrReadFlashPage,
                             static_cast<uint8_t>(byte_address),
                             static_cast<uint8_t>(byte_address >> 8),
                             static_cast<uint8_t>(kPageSize), kCrcEop};
  uint8_t page[kPageSize];
  if (!urCommand(request, sizeof(request), page, kPageSize,
                 kPageCommandTimeoutMs)) return false;
  return memcmp(page, image_ + byte_address, kPageSize) == 0;
}

bool AvrFlasher::urLeaveProgmode() {
  const uint8_t request[] = {kUrLeaveProgmode, kCrcEop};
  return urCommand(request, sizeof(request), nullptr, 0,
                   kLeaveProgrammingTimeoutMs);
}
