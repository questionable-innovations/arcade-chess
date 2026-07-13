#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

struct QuadrantState {
  bool online = false;
  bool calibrated = false;
  bool raw_valid = false;
  uint32_t last_seen_ms = 0;
  uint16_t timeouts = 0;
  uint16_t bad_frames = 0;
  uint16_t raw[16]{};
  uint16_t baseline[16]{};
  uint8_t noise[16]{};
  arcade::SensorState state[16]{};
};

struct BusCallbacks {
  void (*sensorChanged)(uint8_t global_square, arcade::SensorState state,
                        uint16_t raw, uint8_t node, uint8_t local_square) = nullptr;
  void (*rawScanReady)(bool complete, uint32_t scan_id) = nullptr;
  void (*commandComplete)(const char* correlation, bool ok, const char* reason) = nullptr;
};

class BusManager {
 public:
  void begin(HardwareSerial& serial, BusCallbacks callbacks);
  void tick(uint32_t now_ms);
  bool enqueue(uint8_t node, arcade::MessageType type, const uint8_t* payload,
               uint8_t length, const char* correlation = nullptr);
  bool requestRawScan(uint8_t samples, const char* correlation = nullptr);
  bool calibrate(uint8_t node, const char* correlation = nullptr);
  bool identify(uint8_t node, uint16_t duration_ms, const char* correlation = nullptr);
  bool setBrightness(uint8_t node, uint8_t value, const char* correlation = nullptr);
  bool setConfig(uint8_t node, uint8_t key, uint16_t value,
                 const char* correlation = nullptr);
  bool firmwarePreflight(uint8_t node, const char* correlation = nullptr);
  bool beginFirmwareHandoff(uint8_t node, uint32_t token, uint32_t update_id,
                            uint32_t image_size, uint32_t image_crc32,
                            const char* correlation = nullptr);
  void endFirmwareMaintenance(uint32_t token);
  bool setGlobalSquares(const uint8_t* squares, size_t count, uint8_t red,
                        uint8_t green, uint8_t blue, uint16_t duration_ms,
                        const char* correlation = nullptr);
  void setOrientation(uint8_t node, uint8_t orientation);
  void setRuntimeMode(arcade::RuntimeMode mode) { runtime_mode_ = mode; }
  uint8_t globalSquare(uint8_t node, uint8_t local) const;
  bool locateGlobal(uint8_t global, uint8_t& node, uint8_t& local) const;
  const QuadrantState& node(uint8_t index) const { return nodes_[index]; }
  uint32_t goodFrames() const { return good_frames_; }
  uint32_t badFrames() const { return bad_frames_; }
  uint32_t timeoutCount() const { return timeout_count_; }
  bool busy() const { return pending_ || queue_count_ || raw_active_; }

 private:
  struct QueuedCommand {
    uint8_t node = 0;
    arcade::MessageType type = arcade::MessageType::kPing;
    uint8_t payload[24]{};
    uint8_t length = 0;
    char correlation[33]{};
  };

  void receive(uint32_t now_ms);
  void send(uint8_t node, arcade::MessageType type, const uint8_t* payload,
            uint8_t length, const char* correlation, uint32_t now_ms);
  void handleResponse(const arcade::Frame& frame, uint32_t now_ms);
  void handleTimeout(uint32_t now_ms);
  void schedule(uint32_t now_ms);
  void startQueued(uint32_t now_ms);
  void parseRaw(uint8_t node, const arcade::Frame& frame);
  void finishRawIfReady();
  void openRenderWindow(uint32_t now_ms);
  void sendBroadcast(arcade::MessageType type, const uint8_t* payload, uint8_t length);

  HardwareSerial* serial_ = nullptr;
  BusCallbacks callbacks_{};
  arcade::StreamDecoder decoder_;
  QuadrantState nodes_[4]{};
  uint8_t orientation_[4]{};
  QueuedCommand queue_[8]{};
  uint8_t queue_head_ = 0;
  uint8_t queue_tail_ = 0;
  uint8_t queue_count_ = 0;
  bool pending_ = false;
  uint8_t pending_node_ = 0;
  uint8_t pending_sequence_ = 0;
  arcade::MessageType pending_type_ = arcade::MessageType::kPing;
  uint32_t deadline_ms_ = 0;
  char pending_correlation_[33]{};
  uint8_t sequence_ = 0;
  uint8_t poll_node_ = 0;
  uint8_t poll_count_[4]{};
  uint32_t next_poll_ms_ = 0;
  uint32_t next_render_ms_ = 0;
  uint32_t bus_quiet_until_ms_ = 0;
  uint32_t good_frames_ = 0;
  uint32_t bad_frames_ = 0;
  uint32_t timeout_count_ = 0;
  bool raw_active_ = false;
  uint8_t raw_samples_ = 1;
  uint8_t raw_next_node_ = 0;
  uint8_t raw_done_mask_ = 0;
  uint32_t raw_scan_id_ = 0;
  char raw_correlation_[33]{};
  bool programming_handoff_ = false;
  uint32_t maintenance_token_ = 0;
  arcade::RuntimeMode runtime_mode_ = arcade::RuntimeMode::kNormal;
};
