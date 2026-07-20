#include "network_manager.h"

#include <ArduinoJson.h>
#include <WiFi.h>
#include <esp_system.h>

namespace {
constexpr uint32_t kWebSocketReconnectMs = 1000;
constexpr uint32_t kHeartbeatPingIntervalMs = 15000;
constexpr uint32_t kHeartbeatPongTimeoutMs = 3000;
constexpr uint8_t kHeartbeatMissLimit = 2;
constexpr uint32_t kStatusPublishIntervalMs = 15000;
}

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
  websocket_.setReconnectInterval(kWebSocketReconnectMs);
  websocket_.enableHeartbeat(kHeartbeatPingIntervalMs, kHeartbeatPongTimeoutMs,
                             kHeartbeatMissLimit);
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
    doc["at_ms"] = now_ms; doc["data"]["rssi"] = WiFi.RSSI();
    doc["data"]["heap"] = ESP.getFreeHeap();
    doc["data"]["uptime"] = now_ms;
    doc["data"]["websocket_reconnects"] = reconnects_;
    doc["data"]["uart_good"] = bus_->goodFrames();
    doc["data"]["uart_bad"] = bus_->badFrames();
    doc["data"]["uart_timeouts"] = bus_->timeoutCount();
    doc["data"]["quadrant_mask"] = bus_->onlineMask();
    doc["data"]["quadrant_count"] = bus_->onlineCount();
    doc["data"]["mode"] = runtime_mode_ == arcade::RuntimeMode::kBringup
        ? "bringup" : "normal";
    String json; serializeJson(doc, json); sendJson(json);
    next_status_ms_ = now_ms + kStatusPublishIntervalMs;
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
  doc["mode"] = runtime_mode_ == arcade::RuntimeMode::kBringup ? "bringup" : "normal";
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

void NetworkManager::publishNodeStatus(uint8_t node) {
  if (!welcomed_ || node >= arcade::kQuadrantCount) return;
  const QuadrantState& q = bus_->node(node);
  JsonDocument doc;
  doc["v"] = 1; doc["type"] = "node.status"; doc["device_id"] = config_->device_id;
  doc["boot_id"] = String(boot_id_, HEX); doc["seq"] = ++event_sequence_;
  doc["at_ms"] = millis(); JsonObject data = doc["data"].to<JsonObject>();
  data["node"] = node; data["online"] = q.online; data["calibrated"] = q.calibrated;
  data["reset_cause"] = q.reset_cause; data["timeouts"] = q.timeouts;
  data["consecutive_timeouts"] = q.consecutive_timeouts;
  data["last_seen_ms"] = q.last_seen_ms;
  if (q.fw_known) {
    char firmware[16];
    snprintf(firmware, sizeof(firmware), "%u.%u.%u",
             q.fw_version[0], q.fw_version[1], q.fw_version[2]);
    data["firmware"] = firmware;
  }
  String json; serializeJson(doc, json); sendJson(json);
}

void NetworkManager::publishRawScan(bool complete, uint32_t scan_id) {
  if (!welcomed_) return;
  JsonDocument doc;
  doc["v"] = 1; doc["type"] = "sensor.raw_scan"; doc["device_id"] = config_->device_id;
  doc["boot_id"] = String(boot_id_, HEX); doc["seq"] = ++event_sequence_;
  doc["at_ms"] = millis(); JsonObject data = doc["data"].to<JsonObject>();
  data["scan_id"] = scan_id; data["complete"] = complete; data["captured_ms"] = millis();
  data["target_node_mask"] = bus_->rawTargetMask();
  data["response_node_mask"] = bus_->rawResponseMask();
  data["online_node_mask"] = bus_->onlineMask();
  JsonArray raw = data["raw_adc"].to<JsonArray>();
  JsonArray baseline = data["baseline_adc"].to<JsonArray>();
  JsonArray noise = data["noise_adc"].to<JsonArray>();
  JsonArray states = data["state"].to<JsonArray>();
  for (uint8_t global = 0; global < arcade::kBoardSquareCount; ++global) {
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
  data["online_node_mask"] = bus_->onlineMask();
  data["online_node_count"] = bus_->onlineCount();
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    const QuadrantState& q = bus_->node(node);
    JsonObject summary = nodes.add<JsonObject>();
    summary["node"] = node; summary["online"] = q.online;
    summary["calibrated"] = q.calibrated; summary["timeouts"] = q.timeouts;
  }
  for (uint8_t global = 0; global < arcade::kBoardSquareCount; ++global) {
    uint8_t node = 0, local = 0; bus_->locateGlobal(global, node, local);
    const QuadrantState& q = bus_->node(node);
    const auto state = q.state[local];
    squares.add(state == arcade::SensorState::kPositive ? 1 :
                state == arcade::SensorState::kNegative ? -1 : 0);
    valid.add(q.online && state != arcade::SensorState::kUncertain);
  }
  String json; serializeJson(doc, json); sendJson(json);
}

void NetworkManager::sendJson(String& json) {
  websocket_.sendTXT(json);
  if (runtime_mode_ == arcade::RuntimeMode::kBringup)
    Serial.printf("[%10lu][D][WS>] type-bytes=%u\n", millis(), json.length());
}
