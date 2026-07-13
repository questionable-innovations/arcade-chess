#include "bus_manager.h"

bool BusManager::firmwarePreflight(uint8_t node, const char* correlation) {
  if (!isOnline(node)) return false;
  return enqueue(node, arcade::MessageType::kFwPreflight, nullptr, 0, correlation);
}

bool BusManager::beginFirmwareHandoff(uint8_t node, uint32_t token, uint32_t update_id,
                                      uint32_t image_size, uint32_t image_crc32,
                                      const char* correlation) {
  if (!isOnline(node) || !token || !image_size || image_size > 32384UL || raw_active_ ||
      queue_count_ > 5 || programming_handoff_) return false;
  uint8_t begin[7];
  begin[0] = node;
  arcade::putU32(begin + 1, token);
  arcade::putU16(begin + 5, 30000);
  uint8_t prepare[16];
  arcade::putU32(prepare, token);
  arcade::putU32(prepare + 4, update_id);
  arcade::putU32(prepare + 8, image_size);
  arcade::putU32(prepare + 12, image_crc32);
  uint8_t enter[8];
  arcade::putU32(enter, token);
  arcade::putU32(enter + 4, update_id);
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
