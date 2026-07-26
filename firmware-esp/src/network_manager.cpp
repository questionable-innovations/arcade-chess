#include "network_manager.h"

#include <ArduinoJson.h>
#include <WiFi.h>
#include <esp_system.h>
#include <string.h>

namespace {
constexpr uint32_t kWebSocketReconnectMs = 1000;
constexpr uint32_t kWebSocketReconnectMaximumMs = 60000;
constexpr uint32_t kNeverConnectedWarningMs = 10000;
constexpr uint32_t kHeartbeatPingIntervalMs = 15000;
constexpr uint32_t kHeartbeatPongTimeoutMs = 3000;
constexpr uint8_t kHeartbeatMissLimit = 2;
constexpr uint32_t kStatusPublishIntervalMs = 15000;
// Bus trace budget: the bus runs ~50-100 frames/s, which would swamp the
// WebSocket and every viewer; cap events per rolling second and count drops.
constexpr uint8_t kTraceEventsPerSecond = 40;
constexpr uint32_t kTraceDefaultDurationMs = 60000;
constexpr uint32_t kTraceMaximumDurationMs = 600000;
// Loud-failure budget: a fault that repeats every bus poll must never saturate
// the link, so cap the rolling second and hold each (component, level) off for
// kLogRepeatMs; everything dropped is folded into the next line's `suppressed`.
constexpr uint8_t kLogEventsPerSecond = 5;
constexpr uint32_t kLogRepeatMs = 10000;

void hexEncode(const uint8_t* data, uint8_t length, char* output) {
  static const char digits[] = "0123456789abcdef";
  for (uint8_t i = 0; i < length; ++i) {
    output[i * 2] = digits[data[i] >> 4];
    output[i * 2 + 1] = digits[data[i] & 0x0f];
  }
  output[length * 2] = 0;
}
}

void ArcadeNetwork::begin(AppConfig& config, BusManager& bus) {
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

void ArcadeNetwork::connectWebSocket() {
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

void ArcadeNetwork::tick(uint32_t now_ms) {
  if (!websocket_started_ && WiFi.status() == WL_CONNECTED) {
    Serial.printf("[%10u][I][NET] Wi-Fi connected ip=%s rssi=%d\n", now_ms,
                  WiFi.localIP().toString().c_str(), WiFi.RSSI());
    connectWebSocket();
  }
  if (websocket_started_) websocket_.loop();
  // A socket that never came up printed nothing at all, which is the most
  // confusing bring-up failure there is; repeat until it connects.
  if (websocket_started_ && !ever_connected_ &&
      static_cast<int32_t>(now_ms - next_never_connected_warning_ms_) >= 0) {
    Serial.printf("[%10u][W][WS] no connection yet to wss://%s:%u%s (token set=%u)\n",
                  now_ms, config_->websocket_host.c_str(), config_->websocket_port,
                  config_->websocket_path.c_str(), !config_->bearer_token.isEmpty());
    next_never_connected_warning_ms_ = now_ms + kNeverConnectedWarningMs;
  }
  if (welcomed_ && static_cast<int32_t>(now_ms - next_status_ms_) >= 0) {
    JsonDocument doc;
    JsonObject data = beginEvent(doc, "device.status", now_ms);
    data["rssi"] = WiFi.RSSI();
    data["heap"] = ESP.getFreeHeap();
    // `uptime` is the misnamed original; keep it one release while consumers
    // move to uptime_ms.
    data["uptime"] = now_ms;
    data["uptime_ms"] = now_ms;
    data["websocket_reconnects"] = reconnects_;
    data["ws_send_failed"] = ws_send_failed_;
    data["events_dropped_offline"] = events_dropped_offline_;
    data["snapshot_repairs"] = bus_->snapshotRepairs();
    data["raw_stream"] = raw_stream_enabled_;
    data["trace"] = trace_enabled_;
    data["reset_reason"] = static_cast<uint8_t>(esp_reset_reason());
    data["uart_good"] = bus_->goodFrames();
    data["uart_bad"] = bus_->badFrames();
    data["uart_timeouts"] = bus_->timeoutCount();
    data["quadrant_mask"] = bus_->onlineMask();
    data["quadrant_count"] = bus_->onlineCount();
    data["mode"] = runtime_mode_ == arcade::RuntimeMode::kBringup
        ? "bringup" : "normal";
    sendJson(doc);
    next_status_ms_ = now_ms + kStatusPublishIntervalMs;
  }
  if (raw_stream_enabled_ && (!raw_stream_until_ms_ ||
      static_cast<int32_t>(raw_stream_until_ms_ - now_ms) > 0) &&
      static_cast<int32_t>(now_ms - next_raw_stream_ms_) >= 0) {
    // Advance unconditionally. Re-arming only on success meant a sweep that
    // outlasted the interval retried every loop pass, holding raw_active_ true
    // back to back — which starves all node polling and blocks firmware uploads.
    bus_->requestRawScan(raw_stream_samples_);
    next_raw_stream_ms_ = now_ms + raw_stream_interval_ms_;
  } else if (raw_stream_until_ms_ && static_cast<int32_t>(now_ms - raw_stream_until_ms_) >= 0) {
    raw_stream_enabled_ = false;
  }
}

void ArcadeNetwork::onEvent(WStype_t type, uint8_t* payload, size_t length) {
  switch (type) {
    case WStype_CONNECTED:
      connected_ = true; ever_connected_ = true; ++reconnects_;
      welcomed_ = false;
      reconnect_backoff_ms_ = kWebSocketReconnectMs;
      websocket_.setReconnectInterval(kWebSocketReconnectMs);
      Serial.printf("[%10lu][I][WS] connected\n", millis());
      sendHello();
      break;
    case WStype_DISCONNECTED:
      // No diagnostic.log here: the socket is already torn down, so the send can
      // only fail and would charge ws_send_failed_ for every clean disconnect.
      // websocket_reconnects carries the same signal.
      if (connected_) Serial.printf("[%10lu][W][WS] disconnected\n", millis());
      connected_ = false;
      welcomed_ = false;
      // A flat 1 Hz retry against a bad bearer token or a down server is a
      // permanent TLS handshake storm; docs/websocket-api.md promises 1..60 s
      // jittered backoff.
      websocket_.setReconnectInterval(reconnect_backoff_ms_ +
                                      random(reconnect_backoff_ms_ / 4 + 1));
      reconnect_backoff_ms_ = reconnect_backoff_ms_ >= kWebSocketReconnectMaximumMs / 2
          ? kWebSocketReconnectMaximumMs
          : reconnect_backoff_ms_ * 2;
      break;
    case WStype_TEXT: handleCommand(payload, length); break;
    case WStype_ERROR: Serial.printf("[%10lu][W][WS] transport error\n", millis()); break;
    default: break;
  }
}

void ArcadeNetwork::sendHello() {
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
  sendJson(doc);
}

JsonObject ArcadeNetwork::beginEvent(JsonDocument& doc, const char* type,
                                      uint32_t at_ms) {
  doc["v"] = 1; doc["type"] = type; doc["device_id"] = config_->device_id;
  doc["boot_id"] = String(boot_id_, HEX); doc["seq"] = ++event_sequence_;
  doc["at_ms"] = at_ms;
  return doc["data"].to<JsonObject>();
}

const char* ArcadeNetwork::stateName(arcade::SensorState state) const {
  switch (state) {
    case arcade::SensorState::kEmpty: return "empty";
    case arcade::SensorState::kPositive: return "positive";
    case arcade::SensorState::kNegative: return "negative";
    default: return "uncertain";
  }
}

void ArcadeNetwork::publishSensor(uint8_t square, arcade::SensorState state,
                                   uint16_t raw, uint8_t node, uint8_t local) {
  if (!welcomed_) { ++events_dropped_offline_; return; }
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "sensor.changed", millis());
  data["square"] = square;
  data["state"] = stateName(state); data["raw"] = raw;
  data["node"] = node; data["local_square"] = local;
  data["baseline"] = bus_->node(node).baseline[local];
  sendJson(doc);
}

void ArcadeNetwork::publishNodeStatus(uint8_t node) {
  if (node >= arcade::kQuadrantCount) return;
  if (!welcomed_) { ++events_dropped_offline_; return; }
  const QuadrantState& q = bus_->node(node);
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "node.status", millis());
  data["node"] = node; data["online"] = q.online; data["calibrated"] = q.calibrated;
  data["reset_cause"] = q.reset_cause; data["timeouts"] = q.timeouts;
  data["consecutive_timeouts"] = q.consecutive_timeouts;
  data["last_seen_ms"] = q.last_seen_ms;
  data["reboots"] = q.reboots;
  // Absent rather than zero on pre-extension firmware: a real zero and "the node
  // cannot tell you" must not read the same downstream.
  if (q.status_extended) {
    data["node_uptime_ms"] = q.last_uptime_ms;
    data["event_depth"] = q.event_depth;
    data["last_scan_ms"] = q.last_scan_ms;
    data["node_rx_good"] = q.rx_good;
    data["node_rx_bad"] = q.rx_bad;
    data["node_event_overflow"] = q.event_overflow;
    data["supply_mv"] = q.supply_mv;
  }
  if (q.fw_known) {
    char firmware[16];
    snprintf(firmware, sizeof(firmware), "%u.%u.%u",
             q.fw_version[0], q.fw_version[1], q.fw_version[2]);
    data["firmware"] = firmware;
  }
  sendJson(doc);
}

void ArcadeNetwork::publishRawScan(bool complete, uint32_t scan_id) {
  if (!welcomed_) { ++events_dropped_offline_; return; }
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "sensor.raw_scan", millis());
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
  sendJson(doc);
}

void ArcadeNetwork::publishBusTrace(const char* direction, uint8_t node,
                                     uint8_t sequence, arcade::MessageType type,
                                     const char* result, const uint8_t* payload,
                                     uint8_t length, uint8_t node_error_code) {
  if (!welcomed_ || !trace_enabled_) return;
  const uint32_t now = millis();
  if (static_cast<int32_t>(now - trace_until_ms_) >= 0) {
    trace_enabled_ = false;
    return;
  }
  if (static_cast<int32_t>(now - trace_window_ms_) >= 1000) {
    trace_window_ms_ = now;
    trace_window_count_ = 0;
  }
  if (trace_window_count_ >= kTraceEventsPerSecond) {
    if (trace_dropped_ != UINT16_MAX) ++trace_dropped_;
    return;
  }
  ++trace_window_count_;
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "diagnostic.bus", now);
  data["direction"] = direction;
  data["node"] = node;
  data["uart_seq"] = sequence;
  data["message_type"] = static_cast<uint8_t>(type);
  data["result"] = result;
  if (!strcmp(result, "error")) data["code"] = node_error_code;
  if (trace_raw_frames_ && payload && length) {
    char hex[2 * arcade::kMaxPayload + 1];
    hexEncode(payload, length, hex);
    data["raw_hex"] = hex;
  }
  if (trace_dropped_) {
    data["dropped"] = trace_dropped_;
    trace_dropped_ = 0;
  }
  sendJson(doc);
}

void ArcadeNetwork::publishCalibrationProgress(uint8_t node, uint8_t percent) {
  if (!welcomed_) { ++events_dropped_offline_; return; }
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "calibration.progress", millis());
  data["node"] = node; data["phase"] = "sampling"; data["percent"] = percent;
  sendJson(doc);
}

void ArcadeNetwork::publishCalibrationResult(uint8_t node, bool ok, const char* reason) {
  if (!welcomed_) { ++events_dropped_offline_; return; }
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "calibration.result", millis());
  data["node"] = node; data["ok"] = ok;
  if (reason) data["reason"] = reason;
  sendJson(doc);
}

bool ArcadeNetwork::logAllowed(const char* component, const char* level,
                                uint32_t now_ms) {
  if (static_cast<int32_t>(now_ms - log_window_ms_) >= 1000) {
    log_window_ms_ = now_ms;
    log_window_count_ = 0;
  }
  if (log_window_count_ >= kLogEventsPerSecond) return false;
  char key[sizeof(LogGate::key)];
  snprintf(key, sizeof(key), "%s:%s", component, level);
  LogGate* gate = nullptr;
  for (auto& candidate : log_gates_) {
    if (!strcmp(candidate.key, key)) { gate = &candidate; break; }
  }
  if (!gate) {
    gate = &log_gates_[log_gate_next_];
    log_gate_next_ = static_cast<uint8_t>((log_gate_next_ + 1) % kLogGateCount);
    strncpy(gate->key, key, sizeof(gate->key) - 1);
    gate->key[sizeof(gate->key) - 1] = 0;
  } else if (static_cast<int32_t>(now_ms - gate->next_ms) < 0) {
    return false;
  }
  gate->next_ms = now_ms + kLogRepeatMs;
  ++log_window_count_;
  return true;
}

void ArcadeNetwork::publishLog(const char* level, const char* component,
                                const char* message, uint8_t node) {
  if (!welcomed_) { ++events_dropped_offline_; return; }
  const uint32_t now = millis();
  // `suppressed` counts only rate-limit drops; an offline drop is not a gap in
  // an otherwise live stream.
  if (!logAllowed(component, level, now)) {
    if (log_suppressed_ != UINT16_MAX) ++log_suppressed_;
    return;
  }
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "diagnostic.log", now);
  data["level"] = level; data["component"] = component; data["message"] = message;
  if (node < arcade::kQuadrantCount) data["node"] = node;
  if (log_suppressed_) {
    data["suppressed"] = log_suppressed_;
    log_suppressed_ = 0;
  }
  sendJson(doc);
}

void ArcadeNetwork::publishSnapshot() {
  if (!welcomed_) { ++events_dropped_offline_; return; }
  JsonDocument doc;
  JsonObject data = beginEvent(doc, "board.snapshot", millis());
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
  sendJson(doc);
}

void ArcadeNetwork::sendJson(JsonDocument& doc) {
  String json;
  serializeJson(doc, json);
  if (!websocket_.sendTXT(json) && ws_send_failed_ != UINT32_MAX) ++ws_send_failed_;
  if (runtime_mode_ == arcade::RuntimeMode::kBringup)
    Serial.printf("[%10lu][D][WS>] type-bytes=%u\n", millis(), json.length());
}
