#include "firmware_update.h"

#include <avr/pgmspace.h>

#include "bootloader_handoff.h"
#include "system_info.h"

namespace quadrant {
namespace {
constexpr uint8_t kMaintenanceBeginBytes = 7;
constexpr uint8_t kMaintenanceTargetOffset = 0;
constexpr uint8_t kMaintenanceTokenOffset = 1;
constexpr uint8_t kMaintenanceLeaseOffset = 5;
constexpr uint8_t kTokenBytes = sizeof(uint32_t);
constexpr uint8_t kPreflightBytes = 19;
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
constexpr uint8_t kPreflightBroadcastRefusalOffset = 18;
constexpr uint8_t kPrepareBytes = 16;
constexpr uint8_t kPrepareTokenOffset = 0;
constexpr uint8_t kPrepareUpdateIdOffset = 4;
constexpr uint8_t kPrepareImageSizeOffset = 8;
constexpr uint8_t kPrepareImageCrcOffset = 12;
constexpr uint8_t kEnterBootloaderBytes = 8;
constexpr uint8_t kBroadcastEnterBootloaderBytes = 10;
constexpr uint8_t kBroadcastLeaderOffset = 8;
constexpr uint8_t kBroadcastTargetMaskOffset = 9;
constexpr uint8_t kHandoffProtocolVersion = 1;
constexpr uint8_t kHealthBytes = 14;
constexpr uint8_t kHealthStateOffset = 0;
constexpr uint8_t kHealthResetCauseOffset = 1;
constexpr uint8_t kHealthUptimeOffset = 2;
constexpr uint8_t kHealthUpdateIdOffset = 6;
constexpr uint8_t kHealthImageCrcOffset = 10;
constexpr uint8_t kConfirmRequestBytes = sizeof(uint32_t);
constexpr uint8_t kConfirmResponseBytes = sizeof(uint32_t) + sizeof(uint8_t);

// Mirrors the ESP staging CRC (reflected 0xEDB88320) over the flashed image.
uint32_t applicationCrc32(uint32_t length) {
  uint32_t crc = UINT32_MAX;
  for (uint32_t address = 0; address < length; ++address) {
    crc ^= pgm_read_byte(static_cast<uint16_t>(address));
    for (uint8_t bit = 0; bit < 8; ++bit) {
      crc = (crc >> 1) ^ (0xEDB88320UL & (-(crc & 1)));
    }
  }
  return ~crc;
}
}

void FirmwareUpdate::begin() {
  // A kProgramming marker also survives a failed handoff that rebooted the old
  // application, so only a flash CRC matching the staged image proves the new
  // image booted and may become the candidate awaiting FW_CONFIRM.
  if (loadUpdateMarker(marker_) && marker_.state == UpdateState::kProgramming &&
      marker_.node_id == identity_.node_id) {
    if (applicationCrc32(marker_.image_size) == marker_.image_crc32) {
      marker_.state = UpdateState::kCandidate;
    } else {
      marker_.state = UpdateState::kNone;
      marker_.token = 0;
      marker_.update_id = 0;
    }
    saveUpdateMarker(marker_);
  }
}

// Refusal code for the preconditions the addressed and broadcast FW_PREPARE
// paths share, or 0 when the node may stage the image. Distinct codes: a single
// "busy" hides which precondition actually failed, and the ESP has no other way
// to see this node's sensor state.
uint8_t FirmwareUpdate::stagingRefusal(uint32_t token, uint32_t image_size) const {
  if (!token || token != maintenance_token_) return 8;
  if (!image_size || image_size > arcade::kAvrApplicationLimit) return 9;
  if (sensors_.calibrating()) return 10;
  // Refuse only while the ADC is mid-sweep. A finished capture is discarded:
  // its response is worthless across the reboot into the bootloader, and an
  // unclaimed one would otherwise veto every future update.
  if (sensors_.rawCaptureSampling()) return 11;
  return 0;
}

void FirmwareUpdate::stagePrepare(const uint8_t* payload, uint32_t token,
                                  uint32_t image_size) {
  sensors_.abortRawCapture();
  marker_.state = UpdateState::kRequested;
  marker_.node_id = identity_.node_id;
  marker_.token = token;
  marker_.update_id = arcade::getU32(payload + kPrepareUpdateIdOffset);
  marker_.image_size = image_size;
  marker_.image_crc32 = arcade::getU32(payload + kPrepareImageCrcOffset);
  saveUpdateMarker(marker_);
}

// A handoff is only honoured against the exact image this node staged under the
// still-current maintenance lease.
bool FirmwareUpdate::handoffAuthorised(uint32_t token, uint32_t update_id) const {
  return marker_.state == UpdateState::kRequested && marker_.token == token &&
         marker_.update_id == update_id && token == maintenance_token_;
}

void FirmwareUpdate::clearMaintenance() {
  maintenance_active_ = false;
  maintenance_target_ = arcade::kInvalidNodeAddress;
  maintenance_token_ = 0;
}

bool FirmwareUpdate::handleBroadcast(const arcade::Frame& request) {
  if (request.destination != arcade::kBroadcastAddress ||
      request.flags & arcade::kResponse) return false;
  // An unprovisioned node_id is 0xff, so `1U << identity_.node_id` below would be
  // undefined and the maintenance target checks would alias kBroadcastAddress. It
  // can never be a flash target anyway, so opt it out of the whole path.
  if (identity_.node_id >= arcade::kQuadrantCount) return false;
  switch (request.type) {
    case arcade::MessageType::kMaintenanceBegin: {
      if (request.payload_length != kMaintenanceBeginBytes) return false;
      maintenance_target_ = request.payload[kMaintenanceTargetOffset];
      maintenance_token_ = arcade::getU32(request.payload + kMaintenanceTokenOffset);
      maintenance_until_ms_ = millis() +
          arcade::getU16(request.payload + kMaintenanceLeaseOffset);
      maintenance_active_ = (maintenance_target_ < arcade::kQuadrantCount ||
                             maintenance_target_ == arcade::kBroadcastAddress) &&
                            maintenance_token_ != 0;
      broadcast_refusal_ = 0;  // per-attempt; a stale code would misdirect the next
      return true;
    }

    case arcade::MessageType::kFwPrepare: {
      if (request.payload_length != kPrepareBytes || !maintenance_active_ ||
          maintenance_target_ != arcade::kBroadcastAddress) return false;
      const uint32_t token = arcade::getU32(request.payload + kPrepareTokenOffset);
      const uint32_t image_size =
          arcade::getU32(request.payload + kPrepareImageSizeOffset);
      // A broadcast carries no response, so a refusal here is invisible on the bus
      // and surfaces ~10 s later as a bootloader sync timeout that points at the
      // baud rate rather than at whatever actually refused. Latch the same code the
      // addressed path returns; FW_PREFLIGHT reports it.
      broadcast_refusal_ = system_info::residentBootloaderEnabled()
          ? stagingRefusal(token, image_size) : 6;
      if (broadcast_refusal_) return true;
      stagePrepare(request.payload, token, image_size);
      return true;
    }

    case arcade::MessageType::kFwEnterBootloader: {
      if (request.payload_length != kBroadcastEnterBootloaderBytes ||
          !maintenance_active_ ||
          maintenance_target_ != arcade::kBroadcastAddress) return false;
      const uint8_t target_mask = request.payload[kBroadcastTargetMaskOffset];
      const uint8_t leader = request.payload[kBroadcastLeaderOffset];
      if (leader >= arcade::kQuadrantCount || !(target_mask & (1U << leader)))
        return true;
      if (!(target_mask & (1U << identity_.node_id))) return true;
      if (!handoffAuthorised(arcade::getU32(request.payload),
                             arcade::getU32(request.payload + kPrepareUpdateIdOffset)))
        return true;
      marker_.state = UpdateState::kProgramming;
      saveUpdateMarker(marker_);
      bootloader_responder_ = leader == identity_.node_id;
      reset_pending_ = true;
      return true;
    }

    case arcade::MessageType::kMaintenanceEnd: {
      if (request.payload_length != kTokenBytes ||
          arcade::getU32(request.payload) != maintenance_token_) return false;
      clearMaintenance();
      return true;
    }

    default: return false;
  }
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
      response.payload[kPreflightBroadcastRefusalOffset] = broadcast_refusal_;
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
      error_code = stagingRefusal(token, image_size);
      if (error_code) return true;
      stagePrepare(request.payload, token, image_size);
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
      if (!handoffAuthorised(token, update_id)) { error_code = 8; return true; }
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
    clearMaintenance();
  }
  if (!reset_pending_) return;
  lighting_.shutdownNow();
  Serial.flush();
  cli();
  enterBootloader(bootloader_responder_);
}

}  // namespace quadrant
