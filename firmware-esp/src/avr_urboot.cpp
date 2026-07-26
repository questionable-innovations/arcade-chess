#include "avr_flasher.h"

#include <string.h>

namespace {
constexpr uint16_t kPageSize = arcade::kAvrFlashPageBytes;
// This provisioned urboot u8.0 build encodes its MCU/features in response bytes.
constexpr uint8_t kUrInSync = 0xe0;
constexpr uint8_t kUrOk = 0x78;
constexpr uint8_t kCrcEop = 0x20;
constexpr uint8_t kUrGetSync = 0x30;
constexpr uint8_t kUrProgramFlashPage = 0x02;
constexpr uint8_t kUrReadFlashPage = 0x03;
constexpr uint8_t kUrLeaveProgmode = 0x51;
constexpr uint8_t kUrResponseOverhead = 2;
constexpr uint32_t kSyncCommandTimeoutMs = 80;
constexpr uint32_t kPageCommandTimeoutMs = 400;
constexpr uint32_t kLeaveProgrammingTimeoutMs = 300;
constexpr uint16_t kSerialPollDelayUs = 200;
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
