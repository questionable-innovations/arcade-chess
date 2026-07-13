#include "firmware_update.h"

#include <avr/wdt.h>

#include "system_info.h"

namespace quadrant {
namespace {
constexpr uint8_t kMaintenanceBeginBytes = 7;
constexpr uint8_t kMaintenanceTargetOffset = 0;
constexpr uint8_t kMaintenanceTokenOffset = 1;
constexpr uint8_t kMaintenanceLeaseOffset = 5;
constexpr uint8_t kTokenBytes = sizeof(uint32_t);
constexpr uint8_t kPreflightBytes = 18;
constexpr uint8_t kPreflightNodeOffset = 0;
constexpr uint8_t kPreflightFuseOffset = 1;
constexpr uint8_t kPreflightBootloaderOffset = 2;
constexpr uint8_t kPreflightProtocolOffset = 3;
constexpr uint8_t kPreflightPageSizeOffset = 4;
constexpr uint8_t kPreflightFlashSizeOffset = 6;
constexpr uint8_t kPreflightApplicationLimitOffset = 10;
constexpr uint8_t kPreflightStateOffset = 14;
constexpr uint8_t kPreflightResetCauseOffset = 15;
constexpr uint8_t kPreflightSupplyMvOffset = 16;
constexpr uint8_t kPrepareBytes = 16;
constexpr uint8_t kPrepareTokenOffset = 0;
constexpr uint8_t kPrepareUpdateIdOffset = 4;
constexpr uint8_t kPrepareImageSizeOffset = 8;
constexpr uint8_t kPrepareImageCrcOffset = 12;
constexpr uint8_t kEnterBootloaderBytes = 8;
constexpr uint8_t kHandoffProtocolVersion = 1;
constexpr uint8_t kHealthBytes = 14;
constexpr uint8_t kHealthStateOffset = 0;
constexpr uint8_t kHealthResetCauseOffset = 1;
constexpr uint8_t kHealthUptimeOffset = 2;
constexpr uint8_t kHealthUpdateIdOffset = 6;
constexpr uint8_t kHealthImageCrcOffset = 10;
constexpr uint8_t kConfirmRequestBytes = sizeof(uint32_t);
constexpr uint8_t kConfirmResponseBytes = sizeof(uint32_t) + sizeof(uint8_t);
}

void FirmwareUpdate::begin() {
  // A kProgramming marker surviving into a running application means the
  // bootloader handoff completed and this image booted; it becomes the
  // candidate awaiting an explicit FW_CONFIRM from the ESP.
  if (loadUpdateMarker(marker_) && marker_.state == UpdateState::kProgramming &&
      marker_.node_id == identity_.node_id) {
    marker_.state = UpdateState::kCandidate;
    saveUpdateMarker(marker_);
  }
}

bool FirmwareUpdate::handleBroadcast(const arcade::Frame& request) {
  if (request.destination != arcade::kBroadcastAddress ||
      request.flags & arcade::kResponse) return false;
  if (request.type == arcade::MessageType::kMaintenanceBegin &&
      request.payload_length == kMaintenanceBeginBytes) {
    maintenance_target_ = request.payload[kMaintenanceTargetOffset];
    maintenance_token_ = arcade::getU32(request.payload + kMaintenanceTokenOffset);
    maintenance_until_ms_ = millis() +
        arcade::getU16(request.payload + kMaintenanceLeaseOffset);
    maintenance_active_ = maintenance_target_ < arcade::kQuadrantCount &&
                          maintenance_token_ != 0;
    return true;
  }
  if (request.type == arcade::MessageType::kMaintenanceEnd &&
      request.payload_length == kTokenBytes &&
      arcade::getU32(request.payload) == maintenance_token_) {
    maintenance_active_ = false;
    maintenance_target_ = arcade::kInvalidNodeAddress;
    maintenance_token_ = 0;
    return true;
  }
  return false;
}

bool FirmwareUpdate::handleRequest(const arcade::Frame& request,
                                   arcade::Frame& response, uint8_t& error_code) {
  switch (request.type) {
    case arcade::MessageType::kFwPreflight:
      response.payload[kPreflightNodeOffset] = identity_.node_id;
      response.payload[kPreflightFuseOffset] = system_info::highFuse();
      response.payload[kPreflightBootloaderOffset] =
          system_info::residentBootloaderEnabled() ? 1 : 0;
      response.payload[kPreflightProtocolOffset] = kHandoffProtocolVersion;
      arcade::putU16(response.payload + kPreflightPageSizeOffset,
                     arcade::kAvrFlashPageBytes);
      arcade::putU32(response.payload + kPreflightFlashSizeOffset,
                     arcade::kAvrFlashBytes);
      arcade::putU32(response.payload + kPreflightApplicationLimitOffset,
                     arcade::kAvrApplicationLimit);
      response.payload[kPreflightStateOffset] = static_cast<uint8_t>(marker_.state);
      response.payload[kPreflightResetCauseOffset] = system_info::resetCause();
      arcade::putU16(response.payload + kPreflightSupplyMvOffset,
                     system_info::supplyMillivolts());
      response.payload_length = kPreflightBytes;
      return true;

    case arcade::MessageType::kFwPrepare: {
      if (request.payload_length != kPrepareBytes) { error_code = 1; return true; }
      if (!system_info::residentBootloaderEnabled()) { error_code = 6; return true; }
      if (!maintenance_active_ || maintenance_target_ != identity_.node_id) {
        error_code = 7; return true;
      }
      const uint32_t token = arcade::getU32(request.payload + kPrepareTokenOffset);
      const uint32_t image_size = arcade::getU32(request.payload + kPrepareImageSizeOffset);
      if (!token || token != maintenance_token_ || !image_size ||
          image_size > arcade::kAvrApplicationLimit ||
          sensors_.calibrating()) { error_code = 3; return true; }
      marker_.state = UpdateState::kRequested;
      marker_.node_id = identity_.node_id;
      marker_.token = token;
      marker_.update_id = arcade::getU32(request.payload + kPrepareUpdateIdOffset);
      marker_.image_size = image_size;
      marker_.image_crc32 = arcade::getU32(request.payload + kPrepareImageCrcOffset);
      saveUpdateMarker(marker_);
      arcade::putU32(response.payload, marker_.token);
      arcade::putU32(response.payload + 4, marker_.update_id);
      response.payload_length = kEnterBootloaderBytes;
      return true;
    }

    case arcade::MessageType::kFwEnterBootloader: {
      if (request.payload_length != kEnterBootloaderBytes) { error_code = 1; return true; }
      const uint32_t token = arcade::getU32(request.payload);
      const uint32_t update_id = arcade::getU32(request.payload + kPrepareUpdateIdOffset);
      if (!maintenance_active_ || maintenance_target_ != identity_.node_id) {
        error_code = 7; return true;
      }
      if (marker_.state != UpdateState::kRequested || marker_.token != token ||
          marker_.update_id != update_id || token != maintenance_token_) {
        error_code = 8; return true;
      }
      marker_.state = UpdateState::kProgramming;
      saveUpdateMarker(marker_);
      arcade::putU32(response.payload, token);
      arcade::putU32(response.payload + 4, update_id);
      response.payload_length = kEnterBootloaderBytes;
      reset_pending_ = true;
      return true;
    }

    case arcade::MessageType::kFwHealth:
      response.payload[kHealthStateOffset] = static_cast<uint8_t>(marker_.state);
      response.payload[kHealthResetCauseOffset] = system_info::resetCause();
      arcade::putU32(response.payload + kHealthUptimeOffset, millis());
      arcade::putU32(response.payload + kHealthUpdateIdOffset, marker_.update_id);
      arcade::putU32(response.payload + kHealthImageCrcOffset, marker_.image_crc32);
      response.payload_length = kHealthBytes;
      return true;

    case arcade::MessageType::kFwConfirm: {
      if (request.payload_length != kConfirmRequestBytes) { error_code = 1; return true; }
      const uint32_t update_id = arcade::getU32(request.payload);
      const bool candidate = marker_.state == UpdateState::kCandidate ||
                             marker_.state == UpdateState::kValid;
      if (!candidate || marker_.update_id != update_id) { error_code = 8; return true; }
      if (marker_.state != UpdateState::kValid) {
        marker_.state = UpdateState::kValid;
        saveUpdateMarker(marker_);
      }
      arcade::putU32(response.payload, marker_.update_id);
      response.payload[sizeof(marker_.update_id)] = static_cast<uint8_t>(marker_.state);
      response.payload_length = kConfirmResponseBytes;
      return true;
    }

    default: return false;
  }
}

void FirmwareUpdate::tick(uint32_t now_ms) {
  if (maintenance_active_ && static_cast<int32_t>(now_ms - maintenance_until_ms_) >= 0) {
    maintenance_active_ = false;
    maintenance_target_ = arcade::kInvalidNodeAddress;
    maintenance_token_ = 0;
  }
  if (!reset_pending_) return;
  lighting_.shutdownNow();
  Serial.flush();
  cli();
  wdt_enable(WDTO_15MS);
  while (true) {}
}

}  // namespace quadrant
