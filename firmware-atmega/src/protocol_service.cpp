#include "protocol_service.h"

#include <string.h>

#include "bringup_config.h"
#include "system_info.h"

namespace quadrant {
namespace {
void saturatingIncrement(uint16_t& value) {
  if (value != UINT16_MAX) ++value;
}
}

void ProtocolService::begin() { Serial.begin(bringup::kBusBaud); }

void ProtocolService::tick() {
  while (Serial.available()) {
    arcade::Frame request{};
    const auto result = decoder_.push(static_cast<uint8_t>(Serial.read()), request);
    if (result == arcade::DecodeResult::kFrame) {
      saturatingIncrement(rx_good_);
      handleRequest(request);
    } else if (result != arcade::DecodeResult::kNone &&
               result != arcade::DecodeResult::kEmpty) {
      saturatingIncrement(rx_bad_);
    }
  }
  serviceRawResponse();
}

void ProtocolService::sendFrame(arcade::Frame& response) {
  uint8_t wire[arcade::kMaxEncodedFrame];
  const size_t length = arcade::encodeFrame(response, wire, sizeof(wire));
  if (length) { Serial.write(wire, length); Serial.flush(); }
}

arcade::Frame ProtocolService::makeResponse(const arcade::Frame& request,
                                             arcade::MessageType type) const {
  arcade::Frame response{};
  response.flags = arcade::kResponse |
      (sensors_.eventDepth() ? arcade::kEventPending : 0);
  response.source = identity_.node_id;
  response.destination = request.source;
  response.type = type;
  response.sequence = request.sequence;
  return response;
}

void ProtocolService::sendError(const arcade::Frame& request, uint8_t code) {
  auto response = makeResponse(request, arcade::MessageType::kError);
  response.flags |= arcade::kError;
  response.payload[0] = static_cast<uint8_t>(request.type);
  response.payload[1] = code;
  response.payload_length = 2;
  sendFrame(response);
}

bool ProtocolService::applyConfig(uint8_t key, uint16_t value) {
  switch (key) {
    case 1:
      if (value < 10 || value > 400 || value <= settings_.exit_threshold) return false;
      settings_.enter_threshold = value; break;
    case 2:
      if (value >= settings_.enter_threshold) return false;
      settings_.exit_threshold = value; break;
    case 3:
      if (value < 1 || value > 20) return false;
      settings_.debounce_scans = static_cast<uint8_t>(value); break;
    case 4:
      if (value < 5 || value > 500) return false;
      settings_.mux_settle_us = value; break;
    case 5:
      if (value < 8 || value > 200) return false;
      settings_.full_scan_ms = value; break;
    case 6:
      lighting_.setBrightness(static_cast<uint8_t>(value > 255 ? 255 : value)); break;
    case 7: settings_.positive_rgb565 = value; break;
    case 8: settings_.negative_rgb565 = value; break;
    case 9:
      if (value > 7) return false;
      settings_.orientation = value; break;
    case 10:
      if (value > static_cast<uint8_t>(arcade::RuntimeMode::kBringup)) return false;
      settings_.runtime_mode = static_cast<arcade::RuntimeMode>(value); break;
    default: return false;
  }
  return true;
}

uint16_t ProtocolService::configValue(uint8_t key) const {
  switch (key) {
    case 1: return settings_.enter_threshold;
    case 2: return settings_.exit_threshold;
    case 3: return settings_.debounce_scans;
    case 4: return settings_.mux_settle_us;
    case 5: return settings_.full_scan_ms;
    case 6: return settings_.brightness;
    case 7: return settings_.positive_rgb565;
    case 8: return settings_.negative_rgb565;
    case 9: return settings_.orientation;
    case 10: return static_cast<uint8_t>(settings_.runtime_mode);
    default: return 0;
  }
}

void ProtocolService::handleRequest(const arcade::Frame& request) {
  if (firmware_update_.handleBroadcast(request)) return;
  if (request.destination == arcade::kBroadcastAddress &&
      request.type == arcade::MessageType::kRenderWindow &&
      !(request.flags & arcade::kResponse)) {
    if (!firmware_update_.responsesSuppressed()) lighting_.requestRender();
    return;
  }
  if (request.destination != identity_.node_id || request.flags & arcade::kResponse ||
      firmware_update_.responsesSuppressed()) return;

  auto response = makeResponse(request, request.type);
  switch (request.type) {
    case arcade::MessageType::kPing:
      arcade::putU32(response.payload, millis());
      response.payload_length = 4;
      break;
    case arcade::MessageType::kInfo:
      response.payload[0] = 0; response.payload[1] = 1; response.payload[2] = 0;
      response.payload[3] = 0x10; response.payload[4] = identity_.node_id;
      arcade::putU16(response.payload + 5, 0x000f);
      response.payload[7] = static_cast<uint8_t>(settings_.runtime_mode);
      response.payload_length = 8;
      break;
    case arcade::MessageType::kStatus:
      arcade::putU32(response.payload, millis());
      response.payload[4] = system_info::resetCause();
      response.payload[5] = settings_.calibrated;
      response.payload[6] = sensors_.eventDepth();
      arcade::putU16(response.payload + 7, sensors_.lastScanMs());
      arcade::putU16(response.payload + 9, rx_good_);
      arcade::putU16(response.payload + 11, rx_bad_);
      arcade::putU16(response.payload + 13, sensors_.overflowCount());
      arcade::putU16(response.payload + 15, system_info::supplyMillivolts());
      response.payload[17] = static_cast<uint8_t>(settings_.runtime_mode);
      response.payload_length = 18;
      break;
    case arcade::MessageType::kPollEvents: {
      const uint8_t requested = request.payload_length ? request.payload[0] : 4;
      const uint8_t maximum = requested > 8 ? 8 : requested;
      uint8_t count = 0, offset = 1;
      SensorEvent event{};
      while (count < maximum && sensors_.popEvent(event)) {
        response.payload[offset++] = event.square;
        response.payload[offset++] = static_cast<uint8_t>(event.state);
        arcade::putU16(response.payload + offset, event.raw); offset += 2;
        arcade::putU32(response.payload + offset, event.at_ms); offset += 4;
        ++count;
      }
      response.payload[0] = count;
      response.payload_length = offset;
      break;
    }
    case arcade::MessageType::kGetSnapshot: {
      uint8_t offset = 0;
      for (uint8_t i = 0; i < 16; ++i)
        response.payload[offset++] = static_cast<uint8_t>(sensors_.state(i));
      for (uint8_t i = 0; i < 16; ++i) {
        arcade::putU16(response.payload + offset, sensors_.raw(i)); offset += 2;
      }
      response.payload_length = offset;
      break;
    }
    case arcade::MessageType::kGetRawScan:
      if (raw_response_pending_ || !sensors_.startRawCapture(
          request.payload_length ? request.payload[0] : 1)) {
        sendError(request, 3); return;
      }
      raw_request_ = request;
      raw_response_pending_ = true;
      return;
    case arcade::MessageType::kCalibrate:
      if (request.payload_length != 1) { sendError(request, 1); return; }
      if (request.payload[0] == 1) sensors_.startCalibration();
      else if (request.payload[0] == 2) sensors_.cancelCalibration();
      else { sendError(request, 1); return; }
      response.type = arcade::MessageType::kCalibrationResult;
      response.payload[0] = sensors_.calibrating() ? 1 : 0;
      response.payload[1] = sensors_.calibrationPercent();
      arcade::putU16(response.payload + 2, sensors_.calibrationSamples());
      response.payload_length = 4;
      break;
    case arcade::MessageType::kSetSquares:
      if (request.payload_length != 7) { sendError(request, 1); return; }
      lighting_.setSquares(arcade::getU16(request.payload), request.payload[2],
          request.payload[3], request.payload[4],
          arcade::getU16(request.payload + 5), millis());
      arcade::putU16(response.payload, arcade::getU16(request.payload));
      response.payload_length = 2;
      break;
    case arcade::MessageType::kSetBrightness:
      if (request.payload_length != 1) { sendError(request, 1); return; }
      lighting_.setBrightness(request.payload[0]); saveSettings(settings_);
      response.payload[0] = settings_.brightness; response.payload_length = 1;
      break;
    case arcade::MessageType::kIdentify:
      if (request.payload_length != 2) { sendError(request, 1); return; }
      lighting_.identify(arcade::getU16(request.payload), millis());
      memcpy(response.payload, request.payload, 2); response.payload_length = 2;
      break;
    case arcade::MessageType::kClearLighting: {
      const uint16_t mask = request.payload_length == 2
          ? arcade::getU16(request.payload) : 0xffff;
      lighting_.clear(mask); arcade::putU16(response.payload, mask);
      response.payload_length = 2;
      break;
    }
    case arcade::MessageType::kConfigGet: {
      const uint8_t requested_key = request.payload_length ? request.payload[0] : 0;
      uint8_t offset = 0;
      for (uint8_t key = 1; key <= 10; ++key) {
        if (requested_key && requested_key != key) continue;
        response.payload[offset++] = key;
        arcade::putU16(response.payload + offset, configValue(key)); offset += 2;
      }
      response.payload_length = offset;
      break;
    }
    case arcade::MessageType::kConfigSet:
      if (!request.payload_length || request.payload_length % 3) {
        sendError(request, 1); return;
      }
      for (uint8_t offset = 0; offset < request.payload_length; offset += 3) {
        if (!applyConfig(request.payload[offset],
                         arcade::getU16(request.payload + offset + 1))) {
          sendError(request, 5); return;
        }
      }
      saveSettings(settings_);
      memcpy(response.payload, request.payload, request.payload_length);
      response.payload_length = request.payload_length;
      break;
    default: {
      uint8_t error_code = 0;
      if (!firmware_update_.handleRequest(request, response, error_code)) {
        sendError(request, 2); return;
      }
      if (error_code) { sendError(request, error_code); return; }
      break;
    }
  }
  sendFrame(response);
}

void ProtocolService::serviceRawResponse() {
  if (!raw_response_pending_ || !sensors_.rawCaptureReady()) return;
  auto response = makeResponse(raw_request_, arcade::MessageType::kRawScan);
  response.payload[0] = sensors_.rawCaptureSamples();
  arcade::putU16(response.payload + 1, system_info::supplyMillivolts());
  uint8_t offset = 3;
  for (uint8_t i = 0; i < 16; ++i) {
    arcade::putU16(response.payload + offset, sensors_.rawCaptureValue(i)); offset += 2;
    arcade::putU16(response.payload + offset, settings_.baseline[i]); offset += 2;
    response.payload[offset++] = settings_.noise[i];
    response.payload[offset++] = static_cast<uint8_t>(sensors_.state(i));
  }
  response.payload_length = offset;
  sensors_.consumeRawCapture();
  raw_response_pending_ = false;
  sendFrame(response);
}

}  // namespace quadrant
