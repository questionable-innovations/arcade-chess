#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

#include "firmware_update.h"
#include "lighting.h"
#include "persistent.h"
#include "sensors.h"

namespace quadrant {

class ProtocolService {
 public:
  ProtocolService(const Identity& identity, Settings& settings, Sensors& sensors,
                  Lighting& lighting, FirmwareUpdate& firmware_update)
      : identity_(identity), settings_(settings), sensors_(sensors),
        lighting_(lighting), firmware_update_(firmware_update) {}

  void begin();
  void tick();
  uint16_t goodFrames() const { return rx_good_; }
  uint16_t badFrames() const { return rx_bad_; }

 private:
  void handleRequest(const arcade::Frame& request);
  void serviceRawResponse();
  void sendFrame(arcade::Frame& response);
  void sendError(const arcade::Frame& request, uint8_t code);
  arcade::Frame makeResponse(const arcade::Frame& request,
                             arcade::MessageType type) const;
  bool applyConfig(uint8_t key, uint16_t value);
  uint16_t configValue(uint8_t key) const;

  const Identity& identity_;
  Settings& settings_;
  Sensors& sensors_;
  Lighting& lighting_;
  FirmwareUpdate& firmware_update_;
  arcade::StreamDecoder decoder_;
  uint16_t rx_good_ = 0;
  uint16_t rx_bad_ = 0;
  uint8_t debug_flags_ = 0;
  uint16_t debug_raw_interval_ms_ = 0;
  bool raw_response_pending_ = false;
  arcade::Frame raw_request_{};
};

}  // namespace quadrant
