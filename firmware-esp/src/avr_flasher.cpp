#include "avr_flasher.h"

#include <esp_system.h>
#include <string.h>

namespace {
constexpr uint32_t kAppLimit = arcade::kAvrApplicationLimit;
constexpr uint16_t kPageSize = arcade::kAvrFlashPageBytes;
constexpr uint32_t kBusBaud = arcade::kBusBaud;
// Urboot autobauds, but it can only pick a UBRR divisor: on the 8 MHz internal
// RC the closest divisor for 115200 is 111111 (-3.6%), past the UART's tolerance
// before the RC's own error is counted. 76800 quantises to +0.16% (UBRR=12), the
// same margin as the 38400 bus rate, and halves the programming time. Going
// faster is limited by the return path, not the divisor: quadrant TXD reaches
// the ESP through D8 with only R1=10k pulling up, so rising edges take ~1us.
constexpr uint32_t kBootloaderBaud = 76800;
constexpr uint8_t kMaximumPageRetries = 3;
constexpr uint8_t kMaximumHealthPollRetries = 4;
constexpr uint8_t kMaximumSyncAttempts = 80;
constexpr uint8_t kProgressPageInterval = 32;
constexpr uint32_t kHandoffTimeoutMs = 8000;
constexpr uint32_t kBootResetDelayMs = 50;
constexpr uint32_t kApplicationBootDelayMs = 1500;
constexpr uint32_t kHealthResponseTimeoutMs = 3000;
constexpr uint8_t kHealthPayloadBytes = 14;
constexpr uint8_t kHealthMarkerOffset = 0;
constexpr uint8_t kHealthResetCauseOffset = 1;
constexpr uint8_t kHealthUpdateIdOffset = 6;
constexpr uint8_t kHealthCrcOffset = 10;

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
  return beginReceive(node, static_cast<uint8_t>(1U << node), false);
}

bool AvrFlasher::startAll() {
  if (phase_ != Phase::kIdle || bus_->programmingHandoff()) return false;
  const uint8_t mask = bus_->onlineMask();
  if (!mask) return false;
  uint8_t leader = 0;
  while (!(mask & (1U << leader))) ++leader;
  return beginReceive(leader, mask, true);
}

bool AvrFlasher::beginReceive(uint8_t node, uint8_t target_mask,
                              bool simultaneous) {
  image_ = static_cast<uint8_t*>(malloc(kAppLimit));
  if (!image_) {
    Serial.println(F("FLASH FAIL no memory for image staging"));
    return false;
  }
  memset(image_, 0xff, kAppLimit);
  // A raw sweep starting while the hex streams makes beginFirmwareHandoff()
  // reject at finishReceive(), after the whole image has been staged. Hold them
  // off now; an already-running sweep is bounded and drains during the stream.
  bus_->setRawScansBlocked(true);
  if (bus_->rawActive()) Serial.println(F("waiting out an in-flight raw scan"));
  node_ = node;
  target_mask_ = target_mask;
  confirmed_mask_ = 0;
  simultaneous_ = simultaneous;
  image_size_ = 0;
  ext_base_ = 0;
  eof_seen_ = false;
  phase_ = Phase::kReceiveHex;
  started_ms_ = millis();
  if (simultaneous_) Serial.printf("HEX-READY all mask=0x%02x leader=%u\n", target_mask_, node_);
  else Serial.printf("HEX-READY node=%u\n", node_);
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

void AvrFlasher::finishReceive() {
  if (!image_size_) { fail("empty image"); return; }
  if (simultaneous_) {
    target_mask_ = bus_->onlineMask();
    if (!target_mask_) { fail("no online nodes at handoff"); return; }
    node_ = 0;
    while (!(target_mask_ & (1U << node_))) ++node_;
  }
  image_crc32_ = crc32Update(0, image_, image_size_);
  page_count_ = static_cast<uint16_t>((image_size_ + kPageSize - 1) / kPageSize);
  token_ = nonzeroRandom();
  update_id_ = nonzeroRandom();
  Serial.printf("IMAGE size=%u crc32=0x%08x pages=%u\n", image_size_, image_crc32_,
                page_count_);
  const bool handoff_started = simultaneous_
      ? bus_->beginFirmwareHandoffAll(node_, target_mask_, token_, update_id_,
                                      image_size_, image_crc32_)
      : bus_->beginFirmwareHandoff(node_, token_, update_id_, image_size_, image_crc32_);
  if (!handoff_started) {
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
        bus_baud_switched_ = true;
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
        // The broadcast prepare is unacknowledged, so a node that refused looks
        // identical to a baud problem from here. Point at the one place that knows.
        if (simultaneous_) {
          Serial.println(F("hint: run fw-preflight <node> and read "
                           "last_broadcast_refusal before suspecting the baud rate"));
        }
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
      if (!queueNextHealth()) return;
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
    // Error frames carry [0]=request type, [1]=node error code; without the code
    // "rejected" cannot be told apart from a lost maintenance lease.
    char detail[48];
    snprintf(detail, sizeof(detail), "%s rejected code=%u",
             type == arcade::MessageType::kFwPrepare ? "prepare" : "enter",
             length >= 2 ? payload[1] : 0);
    fail(detail);
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
    confirmed_mask_ |= static_cast<uint8_t>(1U << node_);
    if ((confirmed_mask_ & target_mask_) == target_mask_) finishSuccess();
    else {
      phase_ = Phase::kAwaitBoot;
      poll_retries_ = 0;
      deadline_ms_ = millis();
    }
  }
}

bool AvrFlasher::queueNextHealth() {
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    const uint8_t bit = static_cast<uint8_t>(1U << node);
    if ((target_mask_ & bit) && !(confirmed_mask_ & bit)) {
      node_ = node;
      return bus_->enqueue(node_, arcade::MessageType::kFwHealth, nullptr, 0);
    }
  }
  return false;
}

void AvrFlasher::finishSuccess() {
  Serial.printf("FLASH OK nodes=0x%02x size=%u crc32=0x%08x pages=%u elapsed_ms=%lu\n",
                target_mask_, image_size_, image_crc32_, page_count_,
                millis() - started_ms_);
  bus_->setRawScansBlocked(false);
  free(image_);
  image_ = nullptr;
  phase_ = Phase::kIdle;
}

void AvrFlasher::fail(const char* reason) {
  Serial.printf("FLASH FAIL node=%u phase=%u reason=%s\n", node_,
                static_cast<unsigned>(phase_), reason);
  char message[64];
  snprintf(message, sizeof(message), "flash failed in phase=%u: %s",
           static_cast<unsigned>(phase_), reason);
  bus_->log("error", "flash", message, node_);
  bus_->setRawScansBlocked(false);
  if (bus_baud_switched_) restoreBusBaud();
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
  bus_baud_switched_ = false;
}
