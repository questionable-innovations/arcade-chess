#include "network_manager.h"

#include <ArduinoJson.h>
#include <WiFi.h>
#include <esp_system.h>

void NetworkManager::begin(AppConfig& config, BusManager& bus) {
  config_ = &config;
  bus_ = &bus;
  boot_id_ = esp_random();
  if (config.wifi_ssid.isEmpty()) {
    Serial.println(F("[         0][W][NET] Wi-Fi not configured; use: wifi <ssid> <password>"));
    return;
  }
  WiFi.mode(WIFI_STA);
  WiFi.setAutoReconnect(true);
  WiFi.begin(config.wifi_ssid.c_str(), config.wifi_password.c_str());
  Serial.printf("[%10lu][I][NET] connecting SSID=%s\n", millis(), config.wifi_ssid.c_str());
}

void NetworkManager::connectWebSocket() {
  if (websocket_started_ || WiFi.status() != WL_CONNECTED) return;
  websocket_.beginSSL(config_->websocket_host.c_str(), config_->websocket_port,
                      config_->websocket_path.c_str());
  if (!config_->bearer_token.isEmpty()) {
    extra_headers_ = "Authorization: Bearer " + config_->bearer_token + "\r\n";
    websocket_.setExtraHeaders(extra_headers_.c_str());
  }
  websocket_.onEvent([this](WStype_t type, uint8_t* payload, size_t length) {
    onEvent(type, payload, length);
  });
  websocket_.setReconnectInterval(1000);
  websocket_.enableHeartbeat(15000, 3000, 2);
  websocket_started_ = true;
  Serial.printf("[%10lu][I][WS] connecting wss://%s:%u%s\n", millis(),
                config_->websocket_host.c_str(), config_->websocket_port,
                config_->websocket_path.c_str());
}

void NetworkManager::tick(uint32_t now_ms) {
  if (!websocket_started_ && WiFi.status() == WL_CONNECTED) {
    Serial.printf("[%10u][I][NET] Wi-Fi connected ip=%s rssi=%d\n", now_ms,
                  WiFi.localIP().toString().c_str(), WiFi.RSSI());
    connectWebSocket();
  }
  if (websocket_started_) websocket_.loop();
  if (welcomed_ && static_cast<int32_t>(now_ms - next_status_ms_) >= 0) {
    JsonDocument doc;
    doc["v"] = 1; doc["type"] = "device.status"; doc["device_id"] = config_->device_id;
    doc["boot_id"] = String(boot_id_, HEX); doc["seq"] = ++event_sequence_;
    doc["at_ms"] = now_ms; doc["data"]["wifi_rssi"] = WiFi.RSSI();
    doc["data"]["free_heap"] = ESP.getFreeHeap();
    doc["data"]["uart_good"] = bus_->goodFrames();
    doc["data"]["uart_bad"] = bus_->badFrames();
    doc["data"]["uart_timeouts"] = bus_->timeoutCount();
    String json; serializeJson(doc, json); sendJson(json);
    next_status_ms_ = now_ms + 15000;
  }
  if (raw_stream_enabled_ && (!raw_stream_until_ms_ ||
      static_cast<int32_t>(raw_stream_until_ms_ - now_ms) > 0) &&
      static_cast<int32_t>(now_ms - next_raw_stream_ms_) >= 0) {
    if (bus_->requestRawScan(raw_stream_samples_)) next_raw_stream_ms_ = now_ms + raw_stream_interval_ms_;
  } else if (raw_stream_until_ms_ && static_cast<int32_t>(now_ms - raw_stream_until_ms_) >= 0) {
    raw_stream_enabled_ = false;
  }
}

void NetworkManager::onEvent(WStype_t type, uint8_t* payload, size_t length) {
  switch (type) {
    case WStype_CONNECTED:
      connected_ = true; ++reconnects_;
      welcomed_ = false;
      Serial.printf("[%10lu][I][WS] connected\n", millis());
      sendHello();
      break;
    case WStype_DISCONNECTED:
      if (connected_) Serial.printf("[%10lu][W][WS] disconnected\n", millis());
      connected_ = false;
      welcomed_ = false;
      break;
    case WStype_TEXT: handleCommand(payload, length); break;
    case WStype_ERROR: Serial.printf("[%10lu][W][WS] transport error\n", millis()); break;
    default: break;
  }
}

void NetworkManager::sendHello() {
  JsonDocument doc;
  doc["v"] = 1; doc["type"] = "hello"; doc["device_id"] = config_->device_id;
  doc["boot_id"] = String(boot_id_, HEX); doc["firmware"] = "0.1.0";
  doc["hardware"] = "esp32-main-1R0";
  doc["last_server_seq"] = 0;
  doc["protocols"]["uart"] = 1; doc["protocols"]["websocket"] = 1;
  JsonArray caps = doc["capabilities"].to<JsonArray>();
  caps.add("board.snapshot"); caps.add("sensor.events"); caps.add("sensor.raw_scan");
  caps.add("lighting.basic"); caps.add("diagnostics");
  String json; serializeJson(doc, json); sendJson(json);
}

const char* NetworkManager::stateName(arcade::SensorState state) const {
  switch (state) {
    case arcade::SensorState::kEmpty: return "empty";
    case arcade::SensorState::kPositive: return "positive";
    case arcade::SensorState::kNegative: return "negative";
    default: return "uncertain";
  }
}

void NetworkManager::publishSensor(uint8_t square, arcade::SensorState state,
                                   uint16_t raw, uint8_t node, uint8_t local) {
  if (!welcomed_) return;
  JsonDocument doc;
  doc["v"] = 1; doc["type"] = "sensor.changed"; doc["device_id"] = config_->device_id;
  doc["boot_id"] = String(boot_id_, HEX); doc["seq"] = ++event_sequence_;
  doc["at_ms"] = millis(); doc["data"]["square"] = square;
  doc["data"]["state"] = stateName(state); doc["data"]["raw"] = raw;
  doc["data"]["node"] = node; doc["data"]["local_square"] = local;
  doc["data"]["baseline"] = bus_->node(node).baseline[local];
  String json; serializeJson(doc, json); sendJson(json);
}

void NetworkManager::publishRawScan(bool complete, uint32_t scan_id) {
  if (!welcomed_) return;
  JsonDocument doc;
  doc["v"] = 1; doc["type"] = "sensor.raw_scan"; doc["device_id"] = config_->device_id;
  doc["boot_id"] = String(boot_id_, HEX); doc["seq"] = ++event_sequence_;
  doc["at_ms"] = millis(); JsonObject data = doc["data"].to<JsonObject>();
  data["scan_id"] = scan_id; data["complete"] = complete; data["captured_ms"] = millis();
  JsonArray raw = data["raw_adc"].to<JsonArray>();
  JsonArray baseline = data["baseline_adc"].to<JsonArray>();
  JsonArray noise = data["noise_adc"].to<JsonArray>();
  JsonArray states = data["state"].to<JsonArray>();
  for (uint8_t global = 0; global < 64; ++global) {
    uint8_t node = 0, local = 0;
    bus_->locateGlobal(global, node, local);
    const QuadrantState& q = bus_->node(node);
    if (q.raw_valid) {
      raw.add(q.raw[local]); baseline.add(q.baseline[local]); noise.add(q.noise[local]);
      states.add(stateName(q.state[local]));
    } else {
      raw.add(nullptr); baseline.add(nullptr); noise.add(nullptr); states.add(nullptr);
    }
  }
  String json; serializeJson(doc, json); sendJson(json);
}

void NetworkManager::publishSnapshot() {
  if (!welcomed_) return;
  JsonDocument doc;
  doc["v"] = 1; doc["type"] = "board.snapshot"; doc["device_id"] = config_->device_id;
  doc["boot_id"] = String(boot_id_, HEX); doc["seq"] = ++event_sequence_;
  doc["at_ms"] = millis(); JsonObject data = doc["data"].to<JsonObject>();
  JsonArray squares = data["squares"].to<JsonArray>();
  JsonArray valid = data["valid"].to<JsonArray>();
  JsonArray nodes = data["nodes"].to<JsonArray>();
  for (uint8_t node = 0; node < 4; ++node) {
    const QuadrantState& q = bus_->node(node);
    JsonObject summary = nodes.add<JsonObject>();
    summary["node"] = node; summary["online"] = q.online;
    summary["calibrated"] = q.calibrated; summary["timeouts"] = q.timeouts;
  }
  for (uint8_t global = 0; global < 64; ++global) {
    uint8_t node = 0, local = 0; bus_->locateGlobal(global, node, local);
    const QuadrantState& q = bus_->node(node);
    const auto state = q.state[local];
    squares.add(state == arcade::SensorState::kPositive ? 1 :
                state == arcade::SensorState::kNegative ? -1 : 0);
    valid.add(q.online && state != arcade::SensorState::kUncertain);
  }
  String json; serializeJson(doc, json); sendJson(json);
}

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
    raw_stream_samples_ = constrain(requested_samples, static_cast<uint8_t>(1), static_cast<uint8_t>(8));
    const uint32_t requested_duration = args["duration_ms"] | 600000U;
    const uint32_t duration = requested_duration > 600000U ? 600000U : requested_duration;
    raw_stream_until_ms_ = duration ? millis() + duration : 0;
    next_raw_stream_ms_ = millis(); accepted = true;
  } else if (!strcmp(name, "calibration.start")) {
    if (args["node"].is<const char*>() && !strcmp(args["node"], "all")) {
      accepted = true;
      for (uint8_t node = 0; node < 4; ++node) accepted &= bus_->calibrate(node, node == 3 ? id : nullptr);
    } else accepted = bus_->calibrate(args["node"] | 0xff, id);
  } else if (!strcmp(name, "node.identify")) {
    accepted = bus_->identify(args["node"] | 0xff, args["duration_ms"] | 3000, id);
  } else if (!strcmp(name, "lighting.set")) {
    uint8_t squares[64]; size_t count = 0;
    for (JsonVariant square : args["squares"].as<JsonArray>()) {
      if (count < 64) squares[count++] = square.as<uint8_t>();
    }
    const char* hex = args["colour"] | "000000";
    const uint32_t colour = strtoul(hex, nullptr, 16);
    accepted = bus_->setGlobalSquares(squares, count, colour >> 16, colour >> 8, colour,
        args["duration_ms"] | 0, id);
  } else if (!strcmp(name, "device.restart") &&
             !strcmp(args["confirm"] | "", "restart")) {
    sendResult(id, "accepted"); delay(50); ESP.restart();
  }

  if (accepted) sendResult(id, "accepted");
  else sendResult(id, "rejected", "invalid_args_or_busy");
}

void NetworkManager::sendJson(String& json) {
  websocket_.sendTXT(json);
  Serial.printf("[%10lu][D][WS>] type-bytes=%u\n", millis(), json.length());
}
