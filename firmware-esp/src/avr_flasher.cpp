#include "avr_flasher.h"

#include <esp_system.h>
#include <string.h>

namespace {
constexpr uint32_t kAppLimit = 32384;
constexpr uint32_t kPageSize = 128;
constexpr uint32_t kBusBaud = 38400;
// Must match board_bootloader.speed in firmware-atmega/platformio.ini. If sync
// proves unreliable on the diode-OR return at this rate, rebuild urboot slower.
constexpr uint32_t kBootloaderBaud = 115200;
constexpr uint8_t kStkInSync = 0x14;
constexpr uint8_t kStkOk = 0x10;
constexpr uint8_t kCrcEop = 0x20;

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
  if (phase_ != Phase::kIdle || node > 3 || bus_->programmingHandoff()) return false;
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
  if (text_length < 10 || text_length % 2) { fail("short/odd hex record"); return false; }
  uint8_t record[4 + 255 + 1];
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
  if (byte_count != 5u + length) { fail("record length mismatch"); return false; }
  if (checksum) { fail("record checksum mismatch"); return false; }
  const uint16_t address = static_cast<uint16_t>(record[1]) << 8 | record[2];
  const uint8_t type = record[3];
  const uint8_t* data = record + 4;

  switch (type) {
    case 0x00: {  // data
      const uint32_t absolute = ext_base_ + address;
      if (absolute + length > kAppLimit) { fail("record beyond application limit"); return false; }
      memcpy(image_ + absolute, data, length);
      if (absolute + length > image_size_) image_size_ = absolute + length;
      return true;
    }
    case 0x01:  // end of file
      eof_seen_ = true;
      return true;
    case 0x02:  // extended segment address
      ext_base_ = (static_cast<uint32_t>(data[0]) << 8 | data[1]) << 4;
      return true;
    case 0x04:  // extended linear address
      ext_base_ = (static_cast<uint32_t>(data[0]) << 8 | data[1]) << 16;
      return true;
    case 0x03:
    case 0x05:  // start address records carry no flash data
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
  deadline_ms_ = millis() + 8000;
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
        deadline_ms_ = now_ms + 50;  // let the AVR reset into urboot
      } else if (static_cast<int32_t>(now_ms - deadline_ms_) >= 0) {
        fail("bootloader entry not acknowledged");
      }
      return;

    case Phase::kSync:
      if (static_cast<int32_t>(now_ms - deadline_ms_) < 0) return;
      if (stkSync()) {
        Serial.printf("SYNC ok after %u attempt(s)\n", sync_attempts_ + 1);
        phase_ = Phase::kProgram;
        page_ = 0;
        page_retries_ = 0;
        return;
      }
      if (++sync_attempts_ >= 80) { fail("bootloader sync timeout"); return; }
      deadline_ms_ = now_ms + 50;
      return;

    case Phase::kProgram: {
      const uint32_t address = static_cast<uint32_t>(page_) * kPageSize;
      if (stkLoadAddress(static_cast<uint16_t>(address >> 1)) &&
          stkProgramPage(address)) {
        page_retries_ = 0;
        if (++page_ >= page_count_) { phase_ = Phase::kVerify; page_ = 0; return; }
        if (!(page_ % 32)) {
          Serial.printf("PROG %u/%u pages\n", page_, page_count_);
        }
      } else if (++page_retries_ > 3) {
        fail("page program retries exhausted");
      }
      return;
    }

    case Phase::kVerify: {
      const uint32_t address = static_cast<uint32_t>(page_) * kPageSize;
      if (stkLoadAddress(static_cast<uint16_t>(address >> 1)) &&
          stkVerifyPage(address)) {
        page_retries_ = 0;
        if (++page_ < page_count_) {
          if (!(page_ % 32)) Serial.printf("VRFY %u/%u pages\n", page_, page_count_);
          return;
        }
        Serial.println(F("VERIFY ok; leaving bootloader"));
        stkLeaveProgmode();
        restoreBusBaud();
        bus_->endFirmwareMaintenance(token_);
        phase_ = Phase::kAwaitBoot;
        poll_retries_ = 0;
        deadline_ms_ = millis() + 1500;
      } else if (++page_retries_ > 3) {
        fail("page verify failed");
      }
      return;
    }

    case Phase::kAwaitBoot:
      if (static_cast<int32_t>(now_ms - deadline_ms_) < 0) return;
      if (!bus_->enqueue(node_, arcade::MessageType::kFwHealth, nullptr, 0)) return;
      phase_ = Phase::kHealth;
      deadline_ms_ = now_ms + 3000;
      return;

    case Phase::kHealth:
    case Phase::kConfirm:
      if (static_cast<int32_t>(now_ms - deadline_ms_) < 0) return;
      if (++poll_retries_ > 4) {
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
      deadline_ms_ = now_ms + 3000;
      return;
  }
}

void AvrFlasher::onFwResponse(uint8_t node, arcade::MessageType type, bool ok,
                              const uint8_t* payload, uint8_t length) {
  if (node != node_) return;
  if (phase_ == Phase::kHealth && type == arcade::MessageType::kFwHealth) {
    if (!ok || length < 14) { fail("health request rejected"); return; }
    const uint8_t marker_state = payload[0];
    const uint32_t update_id = arcade::getU32(payload + 6);
    const uint32_t crc32 = arcade::getU32(payload + 10);
    Serial.printf("HEALTH marker=%u reset=0x%02x update_id=0x%08x crc32=0x%08x\n",
                  marker_state, payload[1], update_id, crc32);
    // 3 = candidate awaiting confirm (see quadrant UpdateState).
    if (marker_state != 3 || update_id != update_id_ || crc32 != image_crc32_) {
      fail("health/marker mismatch");
      return;
    }
    uint8_t confirm[4];
    arcade::putU32(confirm, update_id_);
    bus_->enqueue(node_, arcade::MessageType::kFwConfirm, confirm, sizeof(confirm));
    phase_ = Phase::kConfirm;
    poll_retries_ = 0;
    deadline_ms_ = millis() + 3000;
  } else if (phase_ == Phase::kConfirm && type == arcade::MessageType::kFwConfirm) {
    if (!ok) { fail("confirm rejected"); return; }
    finishSuccess();
  }
}

void AvrFlasher::finishSuccess() {
  Serial.printf("FLASH OK node=%u size=%u crc32=0x%08x pages=%u elapsed_ms=%u\n",
                node_, image_size_, image_crc32_, page_count_,
                millis() - started_ms_);
  free(image_);
  image_ = nullptr;
  phase_ = Phase::kIdle;
}

void AvrFlasher::fail(const char* reason) {
  Serial.printf("FLASH FAIL node=%u phase=%u reason=%s\n", node_,
                static_cast<unsigned>(phase_), reason);
  if (bus_->programmingHandoff()) {
    restoreBusBaud();
    // Ends the quiet lease for the other quadrants; the target's urboot times
    // out back to whatever application is present. Re-run fw-flash to retry.
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

bool AvrFlasher::stkCommand(const uint8_t* request, size_t request_length,
                            uint8_t* response, size_t response_length,
                            uint32_t timeout_ms) {
  while (serial_->available()) serial_->read();
  serial_->write(request, request_length);
  serial_->flush();
  uint8_t framed[2 + 128];
  const size_t total = response_length + 2;
  if (total > sizeof(framed)) return false;
  const uint32_t deadline = millis() + timeout_ms;
  size_t received = 0;
  while (received < total) {
    if (static_cast<int32_t>(millis() - deadline) >= 0) return false;
    const int value = serial_->read();
    if (value < 0) { delayMicroseconds(200); continue; }
    framed[received++] = static_cast<uint8_t>(value);
  }
  if (framed[0] != kStkInSync || framed[total - 1] != kStkOk) return false;
  if (response_length) memcpy(response, framed + 1, response_length);
  return true;
}

bool AvrFlasher::stkSync() {
  const uint8_t request[] = {0x30, kCrcEop};  // STK_GET_SYNC
  return stkCommand(request, sizeof(request), nullptr, 0, 80);
}

bool AvrFlasher::stkLoadAddress(uint16_t word_address) {
  const uint8_t request[] = {0x55, static_cast<uint8_t>(word_address),
                             static_cast<uint8_t>(word_address >> 8), kCrcEop};
  return stkCommand(request, sizeof(request), nullptr, 0, 150);
}

bool AvrFlasher::stkProgramPage(uint32_t byte_address) {
  uint8_t request[4 + kPageSize + 1];
  request[0] = 0x64;  // STK_PROG_PAGE
  request[1] = static_cast<uint8_t>(kPageSize >> 8);
  request[2] = static_cast<uint8_t>(kPageSize);
  request[3] = 'F';
  memcpy(request + 4, image_ + byte_address, kPageSize);
  request[4 + kPageSize] = kCrcEop;
  return stkCommand(request, sizeof(request), nullptr, 0, 400);
}

bool AvrFlasher::stkVerifyPage(uint32_t byte_address) {
  const uint8_t request[] = {0x74,  // STK_READ_PAGE
                             static_cast<uint8_t>(kPageSize >> 8),
                             static_cast<uint8_t>(kPageSize), 'F', kCrcEop};
  uint8_t page[kPageSize];
  if (!stkCommand(request, sizeof(request), page, kPageSize, 400)) return false;
  return memcmp(page, image_ + byte_address, kPageSize) == 0;
}

bool AvrFlasher::stkLeaveProgmode() {
  const uint8_t request[] = {0x51, kCrcEop};  // STK_LEAVE_PROGMODE
  return stkCommand(request, sizeof(request), nullptr, 0, 300);
}
