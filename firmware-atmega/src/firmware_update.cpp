#include "firmware_update.h"

#include <avr/wdt.h>

#include "system_info.h"

namespace quadrant {

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
      request.payload_length == 7) {
    maintenance_target_ = request.payload[0];
    maintenance_token_ = arcade::getU32(request.payload + 1);
    maintenance_until_ms_ = millis() + arcade::getU16(request.payload + 5);
    maintenance_active_ = maintenance_target_ < 4 && maintenance_token_ != 0;
    return true;
  }
  if (request.type == arcade::MessageType::kMaintenanceEnd &&
      request.payload_length == 4 &&
      arcade::getU32(request.payload) == maintenance_token_) {
    maintenance_active_ = false;
    maintenance_target_ = 0xff;
    maintenance_token_ = 0;
    return true;
  }
  return false;
}

bool FirmwareUpdate::handleRequest(const arcade::Frame& request,
                                   arcade::Frame& response, uint8_t& error_code) {
  switch (request.type) {
    case arcade::MessageType::kFwPreflight:
      response.payload[0] = identity_.node_id;
      response.payload[1] = system_info::highFuse();
      response.payload[2] = system_info::residentBootloaderEnabled() ? 1 : 0;
      response.payload[3] = 1;
      arcade::putU16(response.payload + 4, 128);
      arcade::putU32(response.payload + 6, 32768UL);
      arcade::putU32(response.payload + 10, 32384UL);
      response.payload[14] = static_cast<uint8_t>(marker_.state);
      response.payload[15] = system_info::resetCause();
      arcade::putU16(response.payload + 16, system_info::supplyMillivolts());
      response.payload_length = 18;
      return true;

    case arcade::MessageType::kFwPrepare: {
      if (request.payload_length != 16) { error_code = 1; return true; }
      if (!system_info::residentBootloaderEnabled()) { error_code = 6; return true; }
      if (!maintenance_active_ || maintenance_target_ != identity_.node_id) {
        error_code = 7; return true;
      }
      const uint32_t token = arcade::getU32(request.payload);
      const uint32_t image_size = arcade::getU32(request.payload + 8);
      if (!token || token != maintenance_token_ || !image_size || image_size > 32384UL ||
          sensors_.calibrating()) { error_code = 3; return true; }
      marker_.state = UpdateState::kRequested;
      marker_.node_id = identity_.node_id;
      marker_.token = token;
      marker_.update_id = arcade::getU32(request.payload + 4);
      marker_.image_size = image_size;
      marker_.image_crc32 = arcade::getU32(request.payload + 12);
      saveUpdateMarker(marker_);
      arcade::putU32(response.payload, marker_.token);
      arcade::putU32(response.payload + 4, marker_.update_id);
      response.payload_length = 8;
      return true;
    }

    case arcade::MessageType::kFwEnterBootloader: {
      if (request.payload_length != 8) { error_code = 1; return true; }
      const uint32_t token = arcade::getU32(request.payload);
      const uint32_t update_id = arcade::getU32(request.payload + 4);
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
      response.payload_length = 8;
      reset_pending_ = true;
      return true;
    }

    case arcade::MessageType::kFwHealth:
      response.payload[0] = static_cast<uint8_t>(marker_.state);
      response.payload[1] = system_info::resetCause();
      arcade::putU32(response.payload + 2, millis());
      arcade::putU32(response.payload + 6, marker_.update_id);
      arcade::putU32(response.payload + 10, marker_.image_crc32);
      response.payload_length = 14;
      return true;

    case arcade::MessageType::kFwConfirm: {
      if (request.payload_length != 4) { error_code = 1; return true; }
      const uint32_t update_id = arcade::getU32(request.payload);
      const bool candidate = marker_.state == UpdateState::kCandidate ||
                             marker_.state == UpdateState::kValid;
      if (!candidate || marker_.update_id != update_id) { error_code = 8; return true; }
      if (marker_.state != UpdateState::kValid) {
        marker_.state = UpdateState::kValid;
        saveUpdateMarker(marker_);
      }
      arcade::putU32(response.payload, marker_.update_id);
      response.payload[4] = static_cast<uint8_t>(marker_.state);
      response.payload_length = 5;
      return true;
    }

    default: return false;
  }
}

void FirmwareUpdate::tick(uint32_t now_ms) {
  if (maintenance_active_ && static_cast<int32_t>(now_ms - maintenance_until_ms_) >= 0) {
    maintenance_active_ = false;
    maintenance_target_ = 0xff;
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
