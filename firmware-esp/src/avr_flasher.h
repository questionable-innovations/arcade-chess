#pragma once

#include <Arduino.h>

#include "bus_manager.h"

// Flashes a quadrant application over the shared bus UART using the resident
// urboot urprotocol. The image arrives as Intel HEX lines on the USB
// console (see tools/flash-quadrant.py), is staged in RAM, then programmed and
// read back after the existing FW_PREPARE/FW_ENTER_BOOTLOADER handoff.
class AvrFlasher {
 public:
  void begin(BusManager& bus, HardwareSerial& bus_serial);
  bool start(uint8_t node);
  void abort(const char* reason);
  bool active() const { return phase_ != Phase::kIdle; }
  bool receiving() const { return phase_ == Phase::kReceiveHex; }
  // Returns true when the line was consumed as upload input.
  bool consumeLine(const char* line);
  void tick(uint32_t now_ms);
  void onFwResponse(uint8_t node, arcade::MessageType type, bool ok,
                    const uint8_t* payload, uint8_t length);

 private:
  enum class Phase : uint8_t {
    kIdle,
    kReceiveHex,
    kAwaitHandoff,
    kSync,
    kProgram,
    kVerify,
    kAwaitBoot,
    kHealth,
    kConfirm,
  };

  bool parseHexLine(const char* line);
  void finishReceive();
  void fail(const char* reason);
  void finishSuccess();
  void restoreBusBaud();

  bool urCommand(const uint8_t* request, size_t request_length,
                 uint8_t* response, size_t response_length, uint32_t timeout_ms);
  bool urSync();
  bool urProgramPage(uint32_t byte_address);
  bool urVerifyPage(uint32_t byte_address);
  bool urLeaveProgmode();

  BusManager* bus_ = nullptr;
  HardwareSerial* serial_ = nullptr;
  Phase phase_ = Phase::kIdle;
  uint8_t node_ = 0;
  uint8_t* image_ = nullptr;
  uint32_t image_size_ = 0;
  uint32_t image_crc32_ = 0;
  uint32_t ext_base_ = 0;
  bool eof_seen_ = false;
  uint32_t token_ = 0;
  uint32_t update_id_ = 0;
  uint32_t deadline_ms_ = 0;
  uint32_t started_ms_ = 0;
  uint16_t page_ = 0;
  uint16_t page_count_ = 0;
  uint8_t page_retries_ = 0;
  uint8_t sync_attempts_ = 0;
  uint8_t poll_retries_ = 0;
};
