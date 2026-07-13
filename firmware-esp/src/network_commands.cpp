#include "network_manager.h"

#include <ArduinoJson.h>
#include <Preferences.h>
#include <string.h>

void NetworkManager::sendResult(const char* id, const char* status, const char* reason) {
  if (!welcomed_) return;
  JsonDocument doc;
  doc["v"] = 1; doc["type"] = "command.result"; doc["device_id"] = config_->device_id;
  doc["id"] = id; doc["status"] = status;
  if (reason) doc["reason"] = reason; else doc["reason"] = nullptr;
  String json; serializeJson(doc, json); sendJson(json);
}

void NetworkManager::commandComplete(const char* id, bool ok, const char* reason) {
  const bool rejected = !ok && reason && strcmp(reason, "timeout") != 0 &&
                        strcmp(reason, "partial_scan") != 0;
  sendResult(id, ok ? "applied" : (rejected ? "rejected" : "timeout"), reason);
}

void NetworkManager::handleCommand(const uint8_t* payload, size_t length) {
  if (length > 2048) return;
  JsonDocument doc;
  if (deserializeJson(doc, payload, length) || doc["v"].as<int>() != 1) return;
  const char* message_type = doc["type"] | "";
  if (!strcmp(message_type, "welcome")) {
    welcomed_ = true;
    next_status_ms_ = millis();
    publishSnapshot();
    return;
  }
  if (strcmp(message_type, "command") != 0 || !welcomed_) return;
  const char* id = doc["id"] | "";
  const char* name = doc["name"] | "";
  JsonObject args = doc["args"].as<JsonObject>();
  bool accepted = false;

  if (!strcmp(name, "board.snapshot.get")) {
    publishSnapshot(); accepted = true;
  } else if (!strcmp(name, "sensor.raw_scan.get")) {
    accepted = bus_->requestRawScan(args["samples_per_square"] | 1, id);
  } else if (!strcmp(name, "sensor.raw_stream.set")) {
    raw_stream_enabled_ = args["enabled"] | false;
    const uint32_t requested_interval = args["interval_ms"] | 1000U;
    raw_stream_interval_ms_ = constrain(requested_interval, 250U, 10000U);
    const uint8_t requested_samples = args["samples_per_square"] | 1;
    raw_stream_samples_ = constrain(requested_samples, static_cast<uint8_t>(1),
                                    static_cast<uint8_t>(8));
    const uint32_t requested_duration = args["duration_ms"] | 600000U;
    const uint32_t duration = requested_duration > 600000U ? 600000U : requested_duration;
    raw_stream_until_ms_ = duration ? millis() + duration : 0;
    next_raw_stream_ms_ = millis(); accepted = true;
  } else if (!strcmp(name, "device.mode.set")) {
    const char* requested_mode = args["mode"] | "";
    arcade::RuntimeMode mode;
    if (!strcmp(requested_mode, "normal")) mode = arcade::RuntimeMode::kNormal;
    else if (!strcmp(requested_mode, "bringup")) mode = arcade::RuntimeMode::kBringup;
    else { sendResult(id, "rejected", "invalid_args"); return; }
    runtime_mode_ = mode;
    config_->runtime_mode = mode;
    Preferences preferences;
    config_->save(preferences);
    bus_->setRuntimeMode(mode);
    accepted = true;
    for (uint8_t node = 0; node < 4; ++node)
      accepted &= bus_->setConfig(node, 10, static_cast<uint8_t>(mode),
                                  node == 3 ? id : nullptr);
  } else if (!strcmp(name, "calibration.start")) {
    if (args["node"].is<const char*>() && !strcmp(args["node"], "all")) {
      accepted = true;
      for (uint8_t node = 0; node < 4; ++node)
        accepted &= bus_->calibrate(node, node == 3 ? id : nullptr);
    } else accepted = bus_->calibrate(args["node"] | 0xff, id);
  } else if (!strcmp(name, "node.identify")) {
    accepted = bus_->identify(args["node"] | 0xff, args["duration_ms"] | 3000, id);
  } else if (!strcmp(name, "lighting.set")) {
    uint8_t squares[64]; size_t count = 0;
    for (JsonVariant square : args["squares"].as<JsonArray>())
      if (count < 64) squares[count++] = square.as<uint8_t>();
    const uint32_t colour = strtoul(args["colour"] | "000000", nullptr, 16);
    accepted = bus_->setGlobalSquares(squares, count, colour >> 16, colour >> 8, colour,
                                      args["duration_ms"] | 0, id);
  } else if (!strcmp(name, "device.restart") &&
             !strcmp(args["confirm"] | "", "restart")) {
    sendResult(id, "accepted"); delay(50); ESP.restart();
  }

  if (accepted) sendResult(id, "accepted");
  else sendResult(id, "rejected", "invalid_args_or_busy");
}
