#include "bus_manager.h"

namespace {
constexpr uint16_t kMaintenanceLeaseMs = 60000;
constexpr uint8_t kMaintenanceBeginBytes = 7;
constexpr uint8_t kMaintenanceTokenOffset = 1;
constexpr uint8_t kMaintenanceLeaseOffset = 5;
constexpr uint8_t kPrepareBytes = 16;
constexpr uint8_t kPrepareUpdateIdOffset = 4;
constexpr uint8_t kPrepareImageSizeOffset = 8;
constexpr uint8_t kPrepareImageCrcOffset = 12;
constexpr uint8_t kEnterBootloaderBytes = 8;
constexpr uint8_t kBroadcastEnterBootloaderBytes = 10;
constexpr uint8_t kBroadcastLeaderOffset = 8;
constexpr uint8_t kBroadcastTargetMaskOffset = 9;
constexpr uint16_t kBroadcastProcessingDelayMs = 60;
}

bool BusManager::beginFirmwareHandoffAll(uint8_t leader, uint8_t target_mask,
                                         uint32_t token,
                                         uint32_t update_id,
                                         uint32_t image_size,
                                         uint32_t image_crc32) {
  if (!target_mask || !(target_mask & (1U << leader)) ||
      (target_mask & onlineMask()) != target_mask || !isOnline(leader) ||
      !token || !image_size ||
      image_size > arcade::kAvrApplicationLimit || raw_active_ || queue_count_ ||
      programming_handoff_) return false;

  // A normal poll may be in flight because polling is intentionally continuous.
  // Finish that one bounded transaction, then reserve the idle bus before the
  // scheduler can launch another.
  const uint32_t idle_deadline = millis() + 30;
  while (pending_ && static_cast<int32_t>(millis() - idle_deadline) < 0) {
    const uint32_t now = millis();
    receive(now);
    if (pending_ && static_cast<int32_t>(now - deadline_ms_) >= 0)
      handleTimeout(now);
    if (pending_) delay(1);
  }
  if (pending_) return false;

  uint8_t begin[kMaintenanceBeginBytes];
  begin[0] = arcade::kBroadcastAddress;
  arcade::putU32(begin + kMaintenanceTokenOffset, token);
  arcade::putU16(begin + kMaintenanceLeaseOffset, kMaintenanceLeaseMs);
  uint8_t prepare[kPrepareBytes];
  arcade::putU32(prepare, token);
  arcade::putU32(prepare + kPrepareUpdateIdOffset, update_id);
  arcade::putU32(prepare + kPrepareImageSizeOffset, image_size);
  arcade::putU32(prepare + kPrepareImageCrcOffset, image_crc32);
  uint8_t enter[kBroadcastEnterBootloaderBytes];
  arcade::putU32(enter, token);
  arcade::putU32(enter + kPrepareUpdateIdOffset, update_id);
  enter[kBroadcastLeaderOffset] = leader;
  enter[kBroadcastTargetMaskOffset] = target_mask;

  // EEPROM.put() blocks each AVR while committing the marker.  Space these
  // no-response broadcasts so the next frame cannot overrun a node's UART.
  sendBroadcast(arcade::MessageType::kMaintenanceBegin, begin, sizeof(begin));
  delay(kBroadcastProcessingDelayMs);
  sendBroadcast(arcade::MessageType::kFwPrepare, prepare, sizeof(prepare));
  delay(kBroadcastProcessingDelayMs);
  sendBroadcast(arcade::MessageType::kFwEnterBootloader, enter, sizeof(enter));
  serial_->flush();
  delay(kBroadcastProcessingDelayMs);
  maintenance_token_ = token;
  programming_handoff_ = true;
  Serial.printf("[%10lu][I][FW] all-node handoff leader=%u mask=0x%02x\n",
                millis(), leader, target_mask);
  return true;
}

bool BusManager::firmwarePreflight(uint8_t node, const char* correlation) {
  if (!isOnline(node)) return false;
  return enqueue(node, arcade::MessageType::kFwPreflight, nullptr, 0, correlation);
}

bool BusManager::beginFirmwareHandoff(uint8_t node, uint32_t token, uint32_t update_id,
                                      uint32_t image_size, uint32_t image_crc32,
                                      const char* correlation) {
  if (!isOnline(node) || !token || !image_size ||
      image_size > arcade::kAvrApplicationLimit || raw_active_ ||
      queue_count_ > 5 || programming_handoff_) return false;
  uint8_t begin[kMaintenanceBeginBytes];
  begin[0] = node;
  arcade::putU32(begin + kMaintenanceTokenOffset, token);
  // Lease must outlast full page programming + readback at the bootloader baud.
  arcade::putU16(begin + kMaintenanceLeaseOffset, kMaintenanceLeaseMs);
  uint8_t prepare[kPrepareBytes];
  arcade::putU32(prepare, token);
  arcade::putU32(prepare + kPrepareUpdateIdOffset, update_id);
  arcade::putU32(prepare + kPrepareImageSizeOffset, image_size);
  arcade::putU32(prepare + kPrepareImageCrcOffset, image_crc32);
  uint8_t enter[kEnterBootloaderBytes];
  arcade::putU32(enter, token);
  arcade::putU32(enter + kPrepareUpdateIdOffset, update_id);
  const bool queued = enqueue(arcade::kBroadcastAddress,
                              arcade::MessageType::kMaintenanceBegin,
                              begin, sizeof(begin)) &&
      enqueue(node, arcade::MessageType::kFwPrepare, prepare, sizeof(prepare)) &&
      enqueue(node, arcade::MessageType::kFwEnterBootloader,
              enter, sizeof(enter), correlation);
  if (queued) maintenance_token_ = token;
  return queued;
}

void BusManager::endFirmwareMaintenance(uint32_t token) {
  uint8_t payload[4];
  arcade::putU32(payload, token);
  programming_handoff_ = false;
  sendBroadcast(arcade::MessageType::kMaintenanceEnd, payload, sizeof(payload));
  maintenance_token_ = 0;
}
