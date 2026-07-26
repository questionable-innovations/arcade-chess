#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

#include "lighting.h"
#include "persistent.h"
#include "sensors.h"

namespace quadrant {

class FirmwareUpdate {
 public:
  FirmwareUpdate(const Identity& identity, Sensors& sensors, Lighting& lighting)
      : identity_(identity), sensors_(sensors), lighting_(lighting) {}

  void begin();
  void tick(uint32_t now_ms);
  bool handleBroadcast(const arcade::Frame& request);
  bool handleRequest(const arcade::Frame& request, arcade::Frame& response,
                     uint8_t& error_code);
  bool responsesSuppressed() const {
    return maintenance_active_ && maintenance_target_ != identity_.node_id;
  }
  UpdateState markerState() const { return marker_.state; }

 private:
  uint8_t stagingRefusal(uint32_t token, uint32_t image_size) const;
  void stagePrepare(const uint8_t* payload, uint32_t token, uint32_t image_size);
  bool handoffAuthorised(uint32_t token, uint32_t update_id) const;
  void clearMaintenance();

  const Identity& identity_;
  Sensors& sensors_;
  Lighting& lighting_;
  UpdateMarker marker_{};
  // Why the last broadcast FW_PREPARE was refused (0 = accepted). Broadcasts get
  // no reply, so this is the only channel; FW_PREFLIGHT reports it.
  uint8_t broadcast_refusal_ = 0;
  bool reset_pending_ = false;
  bool bootloader_responder_ = true;
  bool maintenance_active_ = false;
  uint8_t maintenance_target_ = arcade::kInvalidNodeAddress;
  uint32_t maintenance_token_ = 0;
  uint32_t maintenance_until_ms_ = 0;
};

}  // namespace quadrant
