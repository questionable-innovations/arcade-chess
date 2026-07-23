#pragma once

#include <Arduino.h>
#include <ArduinoJson.h>
#include <WebSocketsClient.h>

#include "app_config.h"
#include "bus_manager.h"

class NetworkManager {
 public:
  void begin(AppConfig& config, BusManager& bus);
  void tick(uint32_t now_ms);
  void publishSensor(uint8_t square, arcade::SensorState state, uint16_t raw,
                     uint8_t node, uint8_t local);
  void publishRawScan(bool complete, uint32_t scan_id);
  void publishNodeStatus(uint8_t node);
  void publishBusTrace(const char* direction, uint8_t node, uint8_t sequence,
                       arcade::MessageType type, const char* result,
                       const uint8_t* payload, uint8_t length);
  void publishCalibrationProgress(uint8_t node, uint8_t percent);
  void publishCalibrationResult(uint8_t node, bool ok, const char* reason);
  void commandComplete(const char* id, bool ok, const char* reason);
  void publishSnapshot();
  void setRuntimeMode(arcade::RuntimeMode mode) { runtime_mode_ = mode; }
  bool connected() const { return connected_; }
  uint32_t reconnects() const { return reconnects_; }

 private:
  static constexpr uint8_t kCommandHistorySize = 8;

  struct CommandRecord {
    char id[33]{};
    char status[12]{};
    char reason[28]{};
  };

  void connectWebSocket();
  void onEvent(WStype_t type, uint8_t* payload, size_t length);
  void handleCommand(const uint8_t* payload, size_t length);
  void sendHello();
  void sendResult(const char* id, const char* status, const char* reason = nullptr,
                  JsonVariantConst data = JsonVariantConst());
  void recordResult(const char* id, const char* status, const char* reason);
  CommandRecord* findResult(const char* id);
  void sendJson(String& json);
  const char* stateName(arcade::SensorState state) const;

  AppConfig* config_ = nullptr;
  BusManager* bus_ = nullptr;
  WebSocketsClient websocket_;
  bool websocket_started_ = false;
  bool connected_ = false;
  bool welcomed_ = false;
  uint32_t reconnects_ = 0;
  uint32_t reconnect_backoff_ms_ = 1000;
  uint32_t boot_id_ = 0;
  uint32_t event_sequence_ = 0;
  uint32_t status_interval_ms_ = 15000;
  uint32_t next_status_ms_ = 0;
  uint32_t next_snapshot_ms_ = 0;
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
  CommandRecord command_history_[kCommandHistorySize]{};
  uint8_t command_history_next_ = 0;
  String extra_headers_;
  arcade::RuntimeMode runtime_mode_ = arcade::RuntimeMode::kNormal;
};
