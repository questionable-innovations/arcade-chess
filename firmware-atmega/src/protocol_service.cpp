#include "protocol_service.h"

#include <string.h>

#include "bringup_config.h"
#include "system_info.h"

namespace quadrant {
namespace {
constexpr uint8_t kMaximumEventsPerResponse = 8;
constexpr uint8_t kRawHeaderBytes = 3;
constexpr uint8_t kRawSquareBytes = 6;
constexpr uint8_t kConfigEntryBytes = 3;
constexpr uint16_t kAllSquaresMask = UINT16_MAX;
static_assert(kRawSquareBytes == sizeof(uint16_t) * 2 + sizeof(uint8_t) * 2,
              "raw square wire layout changed");

void saturatingIncrement(uint16_t& value) {
  if (value != UINT16_MAX) ++value;
}
}

void ProtocolService::begin() { Serial.begin(bringup::kBusBaud); }

void ProtocolService::tick() {
  // Any inbound byte — frame, garbage, or flashing traffic — counts as an ESP
  // being present, which suppresses the idle animation.
  if (Serial.available()) lighting_.noteBusActivity(millis());
  arcade::Frame request{};
  while (Serial.available()) {
    const auto result = decoder_.push(static_cast<uint8_t>(Serial.read()), request);
    if (result == arcade::DecodeResult::kFrame) {
      saturatingIncrement(rx_good_);
      // Broadcasts carry no reply, so they always run in arrival order. An
      // addressed request only runs when nothing is queued behind it: the ESP
      // keeps one transaction outstanding and sends nothing — not even a render
      // marker — until it is answered or timed out, so anything already buffered
      // behind a request proves the ESP abandoned it and re-armed the bus for
      // another node. Timestamps cannot decide this instead: after a stall (the
      // ~240 ms first CONFIG_SET erase) every buffered frame decodes at once.
      if (request.destination == arcade::kBroadcastAddress) handleBroadcast(request);
      else if (!Serial.available()) handleRequest(request);
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

void ProtocolService::beginResponse(arcade::Frame& response, uint8_t destination,
                                    uint8_t sequence, arcade::MessageType type) const {
  response.flags = arcade::kResponse |
      (sensors_.eventDepth() ? arcade::kEventPending : 0);
  response.source = identity_.node_id;
  response.destination = destination;
  response.type = type;
  response.sequence = sequence;
  response.payload_length = 0;
}

// Re-uses the response the caller already built: a second stack Frame here is
// 119 bytes on the deepest path there is.
void ProtocolService::sendError(arcade::Frame& response, const arcade::Frame& request,
                                uint8_t code) {
  response.type = arcade::MessageType::kError;
  response.flags |= arcade::kError;
  response.payload[0] = static_cast<uint8_t>(request.type);
  response.payload[1] = code;
  response.payload_length = 2;
  sendFrame(response);
}

bool ProtocolService::applyConfig(uint8_t key, uint16_t value) {
  switch (static_cast<arcade::ConfigKey>(key)) {
    case arcade::ConfigKey::kEnterThreshold:
      if (value < bringup::kMinimumEnterThreshold ||
          value > bringup::kMaximumEnterThreshold ||
          value <= settings_.exit_threshold) return false;
      settings_.enter_threshold = value; break;
    case arcade::ConfigKey::kExitThreshold:
      if (value >= settings_.enter_threshold) return false;
      settings_.exit_threshold = value; break;
    case arcade::ConfigKey::kDebounceScans:
      if (value < bringup::kMinimumDebounceScans ||
          value > bringup::kMaximumDebounceScans) return false;
      settings_.debounce_scans = static_cast<uint8_t>(value); break;
    case arcade::ConfigKey::kMuxSettleUs:
      if (value < bringup::kMinimumMuxSettleUs ||
          value > bringup::kMaximumMuxSettleUs) return false;
      settings_.mux_settle_us = value; break;
    case arcade::ConfigKey::kFullScanMs:
      if (value < bringup::kMinimumFullScanMs ||
          value > bringup::kMaximumFullScanMs) return false;
      settings_.full_scan_ms = value; break;
    case arcade::ConfigKey::kBrightness:
      lighting_.setBrightness(static_cast<uint8_t>(value > UINT8_MAX ? UINT8_MAX : value));
      break;
    case arcade::ConfigKey::kPositiveRgb565: settings_.positive_rgb565 = value; break;
    case arcade::ConfigKey::kNegativeRgb565: settings_.negative_rgb565 = value; break;
    case arcade::ConfigKey::kOrientation:
      if (value > bringup::kMaximumOrientation) return false;
      settings_.orientation = value; break;
    case arcade::ConfigKey::kRuntimeMode:
      if (value > static_cast<uint8_t>(arcade::RuntimeMode::kBringup)) return false;
      settings_.runtime_mode = static_cast<arcade::RuntimeMode>(value); break;
    default: return false;
  }
  return true;
}

uint16_t ProtocolService::configValue(uint8_t key) const {
  switch (static_cast<arcade::ConfigKey>(key)) {
    case arcade::ConfigKey::kEnterThreshold: return settings_.enter_threshold;
    case arcade::ConfigKey::kExitThreshold: return settings_.exit_threshold;
    case arcade::ConfigKey::kDebounceScans: return settings_.debounce_scans;
    case arcade::ConfigKey::kMuxSettleUs: return settings_.mux_settle_us;
    case arcade::ConfigKey::kFullScanMs: return settings_.full_scan_ms;
    case arcade::ConfigKey::kBrightness: return settings_.brightness;
    case arcade::ConfigKey::kPositiveRgb565: return settings_.positive_rgb565;
    case arcade::ConfigKey::kNegativeRgb565: return settings_.negative_rgb565;
    case arcade::ConfigKey::kOrientation: return settings_.orientation;
    case arcade::ConfigKey::kRuntimeMode: return static_cast<uint8_t>(settings_.runtime_mode);
    default: return 0;
  }
}

void ProtocolService::handleBroadcast(const arcade::Frame& request) {
  if (firmware_update_.handleBroadcast(request)) return;
  if (request.type == arcade::MessageType::kRenderWindow &&
      !(request.flags & arcade::kResponse) &&
      !firmware_update_.responsesSuppressed()) {
    lighting_.requestRender();
  }
}

void ProtocolService::handleRequest(const arcade::Frame& request) {
  // An unprovisioned node_id is kInvalidNodeAddress, which is the same 0xff as
  // kBroadcastAddress; compare against the valid range so a blank EEPROM cannot
  // make broadcasts look like addressed traffic.
  if (identity_.node_id >= arcade::kQuadrantCount ||
      request.destination != identity_.node_id || request.flags & arcade::kResponse ||
      firmware_update_.responsesSuppressed()) return;

  arcade::Frame response{};
  beginResponse(response, request.source, request.sequence, request.type);
  switch (request.type) {
    case arcade::MessageType::kPing:
      arcade::putU32(response.payload, millis());
      response.payload_length = 4;
      break;
    case arcade::MessageType::kInfo:
      response.payload[0] = ARCADE_FW_MAJOR; response.payload[1] = ARCADE_FW_MINOR;
      response.payload[2] = ARCADE_FW_PATCH;
      response.payload[3] = 0x10; response.payload[4] = identity_.node_id;
      arcade::putU16(response.payload + 5, 0x000f);
      response.payload_length = 7;
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
      response.payload[17] = sensors_.calibrationPhaseCode();
      response.payload[18] = sensors_.calibrationPercent();
      response.payload_length = 19;
      break;
    case arcade::MessageType::kPollEvents: {
      const uint8_t requested = request.payload_length ? request.payload[0] : 4;
      const uint8_t maximum = requested > kMaximumEventsPerResponse
          ? kMaximumEventsPerResponse : requested;
      uint8_t count = 0, offset = 1;
      SensorEvent event{};
      while (count < maximum && sensors_.popEvent(event)) {
        response.payload[offset++] = event.square;
        response.payload[offset++] = static_cast<uint8_t>(event.state);
        arcade::putU16(response.payload + offset, event.raw); offset += 2;
        arcade::putU32(response.payload + offset, event.at_ms);
        offset += sizeof(event.at_ms);
        ++count;
      }
      response.payload[0] = count;
      response.payload_length = offset;
      break;
    }
    case arcade::MessageType::kGetSnapshot: {
      uint8_t offset = 0;
      for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i)
        response.payload[offset++] = static_cast<uint8_t>(sensors_.state(i));
      for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
        arcade::putU16(response.payload + offset, sensors_.raw(i)); offset += 2;
      }
      response.payload_length = offset;
      break;
    }
    case arcade::MessageType::kGetRawScan:
      if (raw_response_pending_) { sendError(response, request, 12); return; }
      // Belt and braces: only serviceRawResponse() consumes a finished capture,
      // so an unclaimed one would refuse every later scan. Discard, never refuse.
      if (sensors_.rawCaptureReady()) sensors_.consumeRawCapture();
      if (!sensors_.startRawCapture(request.payload_length ? request.payload[0] : 1)) {
        sendError(response, request, 11); return;
      }
      raw_request_source_ = request.source;
      raw_request_sequence_ = request.sequence;
      raw_response_pending_ = true;
      return;
    case arcade::MessageType::kCalibrate:
      if (request.payload_length != 1) { sendError(response, request, 1); return; }
      if (request.payload[0] == 1) sensors_.startCalibration();
      else if (request.payload[0] == 2) sensors_.cancelCalibration();
      else { sendError(response, request, 1); return; }
      response.type = arcade::MessageType::kCalibrationResult;
      response.payload[0] = sensors_.calibrating() ? 1 : 0;
      response.payload[1] = sensors_.calibrationPercent();
      arcade::putU16(response.payload + 2, sensors_.calibrationSamples());
      response.payload_length = 4;
      break;
    case arcade::MessageType::kSetSquares:
      if (request.payload_length != 7) { sendError(response, request, 1); return; }
      lighting_.setSquares(arcade::getU16(request.payload), request.payload[2],
          request.payload[3], request.payload[4],
          arcade::getU16(request.payload + 5), millis());
      arcade::putU16(response.payload, arcade::getU16(request.payload));
      response.payload_length = 2;
      break;
    case arcade::MessageType::kSetBrightness:
      if (request.payload_length != 1) { sendError(response, request, 1); return; }
      lighting_.setBrightness(request.payload[0]); saveSettings(settings_);
      response.payload[0] = settings_.brightness; response.payload_length = 1;
      break;
    case arcade::MessageType::kIdentify:
      if (request.payload_length != 2) { sendError(response, request, 1); return; }
      lighting_.identify(arcade::getU16(request.payload), millis());
      memcpy(response.payload, request.payload, 2); response.payload_length = 2;
      break;
    case arcade::MessageType::kClearLighting: {
      const uint16_t mask = request.payload_length == 2
          ? arcade::getU16(request.payload) : kAllSquaresMask;
      lighting_.clear(mask); arcade::putU16(response.payload, mask);
      response.payload_length = 2;
      break;
    }
    case arcade::MessageType::kConfigGet: {
      const uint8_t requested_key = request.payload_length ? request.payload[0] : 0;
      uint8_t offset = 0;
      for (uint8_t key = arcade::configKey(arcade::ConfigKey::kEnterThreshold);
           key <= arcade::configKey(arcade::ConfigKey::kRuntimeMode); ++key) {
        if (requested_key && requested_key != key) continue;
        response.payload[offset++] = key;
        arcade::putU16(response.payload + offset, configValue(key)); offset += 2;
      }
      response.payload_length = offset;
      break;
    }
    case arcade::MessageType::kConfigSet:
      if (!request.payload_length || request.payload_length % kConfigEntryBytes) {
        sendError(response, request, 1); return;
      }
      for (uint8_t offset = 0; offset < request.payload_length;
           offset += kConfigEntryBytes) {
        if (!applyConfig(request.payload[offset],
                         arcade::getU16(request.payload + offset + 1))) {
          sendError(response, request, 5); return;
        }
      }
      saveSettings(settings_);
      memcpy(response.payload, request.payload, request.payload_length);
      response.payload_length = request.payload_length;
      break;
    case arcade::MessageType::kSetDebug:
      if (request.payload_length != 3) { sendError(response, request, 1); return; }
      debug_flags_ = request.payload[0];
      debug_raw_interval_ms_ = arcade::getU16(request.payload + 1);
      response.payload[0] = debug_flags_;
      arcade::putU16(response.payload + 1, debug_raw_interval_ms_);
      response.payload_length = 3;
      break;
    default: {
      uint8_t error_code = 0;
      if (!firmware_update_.handleRequest(request, response, error_code)) {
        sendError(response, request, 2); return;
      }
      if (error_code) { sendError(response, request, error_code); return; }
      break;
    }
  }
  sendFrame(response);
}

void ProtocolService::serviceRawResponse() {
  if (!raw_response_pending_) return;
  // Two drops, one test. A suppressed node must not put a 99-byte frame on the
  // wire during another node's bootloader session, and a firmware handoff calls
  // abortRawCapture() underneath an in-flight request — without clearing the
  // pending flag here it latches and every later raw scan is refused with 12.
  if (firmware_update_.responsesSuppressed() || !sensors_.rawCaptureBusy()) {
    sensors_.abortRawCapture();
    raw_response_pending_ = false;
    return;
  }
  if (!sensors_.rawCaptureReady()) return;
  arcade::Frame response{};
  beginResponse(response, raw_request_source_, raw_request_sequence_,
                arcade::MessageType::kRawScan);
  response.payload[0] = sensors_.rawCaptureSamples();
  arcade::putU16(response.payload + 1, system_info::supplyMillivolts());
  uint8_t offset = kRawHeaderBytes;
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
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
