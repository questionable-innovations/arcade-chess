#pragma once

#include <Arduino.h>
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
  void commandComplete(const char* id, bool ok, const char* reason);
  void publishSnapshot();
  bool connected() const { return connected_; }
  uint32_t reconnects() const { return reconnects_; }

 private:
  void connectWebSocket();
  void onEvent(WStype_t type, uint8_t* payload, size_t length);
  void handleCommand(const uint8_t* payload, size_t length);
  void sendHello();
  void sendResult(const char* id, const char* status, const char* reason = nullptr);
  void sendJson(String& json);
  const char* stateName(arcade::SensorState state) const;

  AppConfig* config_ = nullptr;
  BusManager* bus_ = nullptr;
  WebSocketsClient websocket_;
  bool websocket_started_ = false;
  bool connected_ = false;
  bool welcomed_ = false;
  uint32_t reconnects_ = 0;
  uint32_t boot_id_ = 0;
  uint32_t event_sequence_ = 0;
  uint32_t next_status_ms_ = 0;
  bool raw_stream_enabled_ = false;
  uint32_t raw_stream_interval_ms_ = 1000;
  uint32_t raw_stream_until_ms_ = 0;
  uint32_t next_raw_stream_ms_ = 0;
  uint8_t raw_stream_samples_ = 1;
  String extra_headers_;
};
