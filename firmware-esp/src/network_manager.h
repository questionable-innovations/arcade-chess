#pragma once

#include <Arduino.h>
#include <ArduinoJson.h>
#include <WebSocketsClient.h>

#include "app_config.h"
#include "bus_manager.h"

// Named `ArcadeNetwork` rather than the obvious `NetworkManager`: arduino-esp32
// 3.x added a global `class NetworkManager` in its own Network library, which
// every translation unit picks up through Arduino.h. Two global classes of the
// same name is a hard build failure, and this side is the one that can move.
class ArcadeNetwork {
 public:
  void begin(AppConfig& config, BusManager& bus);
  void tick(uint32_t now_ms);
  void publishSensor(uint8_t square, arcade::SensorState state, uint16_t raw,
                     uint8_t node, uint8_t local);
  void publishRawScan(bool complete, uint32_t scan_id);
  void publishNodeStatus(uint8_t node);
  void publishBusTrace(const char* direction, uint8_t node, uint8_t sequence,
                       arcade::MessageType type, const char* result,
                       const uint8_t* payload, uint8_t length,
                       uint8_t node_error_code);
  void publishCalibrationProgress(uint8_t node, uint8_t percent);
  void publishCalibrationResult(uint8_t node, bool ok, const char* reason);
  void publishLog(const char* level, const char* component, const char* message,
                  uint8_t node = arcade::kInvalidNodeAddress);
  void commandComplete(const char* id, bool ok, const char* reason, uint8_t node,
                       uint8_t node_error_code);
  void publishSnapshot();
  void setRuntimeMode(arcade::RuntimeMode mode) { runtime_mode_ = mode; }
  bool connected() const { return connected_; }
  uint32_t reconnects() const { return reconnects_; }

 private:
  static constexpr uint8_t kLogGateCount = 6;

  // One repeat gate per (component, level) so a storm on one channel cannot
  // starve every other channel out of the shared per-second budget.
  struct LogGate {
    char key[20]{};
    uint32_t next_ms = 0;
  };

  void connectWebSocket();
  void onEvent(WStype_t type, uint8_t* payload, size_t length);
  void handleCommand(const uint8_t* payload, size_t length);
  void sendHello();
  void sendResult(const char* id, const char* status, const char* reason = nullptr,
                  JsonVariantConst data = JsonVariantConst());
  // Writes the envelope every published event shares and returns its `data`
  // object. Key insertion order is the wire order, so nothing may be added
  // between the call and the fields the caller then writes into `data`.
  JsonObject beginEvent(JsonDocument& doc, const char* type, uint32_t at_ms);
  void sendJson(JsonDocument& doc);
  bool logAllowed(const char* component, const char* level, uint32_t now_ms);
  const char* stateName(arcade::SensorState state) const;

  AppConfig* config_ = nullptr;
  BusManager* bus_ = nullptr;
  WebSocketsClient websocket_;
  bool websocket_started_ = false;
  bool connected_ = false;
  bool ever_connected_ = false;
  bool welcomed_ = false;
  uint32_t reconnects_ = 0;
  uint32_t reconnect_backoff_ms_ = 1000;
  uint32_t next_never_connected_warning_ms_ = 0;
  uint32_t ws_send_failed_ = 0;
  uint32_t events_dropped_offline_ = 0;
  uint32_t boot_id_ = 0;
  uint32_t event_sequence_ = 0;
  uint32_t next_status_ms_ = 0;
  bool raw_stream_enabled_ = false;
  uint32_t raw_stream_interval_ms_ = 1000;
  uint32_t raw_stream_until_ms_ = 0;
  uint32_t next_raw_stream_ms_ = 0;
  uint8_t raw_stream_samples_ = 1;
  bool trace_enabled_ = false;
  bool trace_raw_frames_ = false;
  uint32_t trace_until_ms_ = 0;
  uint32_t trace_window_ms_ = 0;
  uint8_t trace_window_count_ = 0;
  uint16_t trace_dropped_ = 0;
  LogGate log_gates_[kLogGateCount]{};
  uint8_t log_gate_next_ = 0;
  uint32_t log_window_ms_ = 0;
  uint8_t log_window_count_ = 0;
  uint16_t log_suppressed_ = 0;
  String extra_headers_;
  arcade::RuntimeMode runtime_mode_ = arcade::RuntimeMode::kNormal;
};
