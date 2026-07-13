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
  const Identity& identity_;
  Sensors& sensors_;
  Lighting& lighting_;
  UpdateMarker marker_{};
  bool reset_pending_ = false;
  bool maintenance_active_ = false;
  uint8_t maintenance_target_ = 0xff;
  uint32_t maintenance_token_ = 0;
  uint32_t maintenance_until_ms_ = 0;
};

}  // namespace quadrant
