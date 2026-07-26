#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

struct QuadrantState {
  bool online = false;
  bool calibrated = false;
  bool raw_valid = false;
  bool needs_sync = false;
  bool fw_known = false;
  uint8_t reset_cause = 0;
  uint8_t fw_version[3]{};
  uint32_t last_seen_ms = 0;
  uint16_t timeouts = 0;
  uint16_t measured_avcc_mv = 0;
  uint8_t consecutive_timeouts = 0;
  // Extended STATUS counters (payload >= 17 bytes). Pre-extension firmware never
  // sets status_extended, so these stay absent instead of publishing zeroes.
  bool status_extended = false;
  uint8_t event_depth = 0;
  uint16_t last_scan_ms = 0;
  uint16_t rx_good = 0;
  uint16_t rx_bad = 0;
  uint16_t event_overflow = 0;
  uint16_t supply_mv = 0;
  // ESP-side: how many times this node's uptime has stepped backwards.
  uint16_t reboots = 0;
  // Calibration wire codes from the extended status payload; 0xff = unknown
  // (offline or pre-extension firmware).
  uint8_t cal_phase = 0xff;
  uint8_t cal_percent = 0xff;
  bool calibration_watch = false;
  uint32_t calibration_deadline_ms = 0;
  uint32_t calibration_started_ms = 0;
  uint32_t last_uptime_ms = 0;
  uint16_t raw[arcade::kSquaresPerQuadrant]{};
  uint16_t baseline[arcade::kSquaresPerQuadrant]{};
  uint8_t noise[arcade::kSquaresPerQuadrant]{};
  arcade::SensorState state[arcade::kSquaresPerQuadrant]{};
};

struct BusCallbacks {
  void (*sensorChanged)(uint8_t global_square, arcade::SensorState state,
                        uint16_t raw, uint8_t node, uint8_t local_square) = nullptr;
  void (*rawScanReady)(bool complete, uint32_t scan_id) = nullptr;
  // node/node_error_code are meaningful only when reason is "node_error".
  void (*commandComplete)(const char* correlation, bool ok, const char* reason,
                          uint8_t node, uint8_t node_error_code) = nullptr;
  void (*nodePresenceChanged)(uint8_t node, bool online) = nullptr;
  void (*nodeStatusChanged)(uint8_t node) = nullptr;
  void (*fwResponse)(uint8_t node, arcade::MessageType type, bool ok,
                     const uint8_t* payload, uint8_t length) = nullptr;
  void (*calibrationProgress)(uint8_t node, uint8_t percent) = nullptr;
  void (*calibrationResult)(uint8_t node, bool ok, const char* reason) = nullptr;
  void (*busTrace)(const char* direction, uint8_t node, uint8_t sequence,
                   arcade::MessageType type, const char* result,
                   const uint8_t* payload, uint8_t length,
                   uint8_t node_error_code) = nullptr;
  void (*log)(const char* level, const char* component, const char* message,
              uint8_t node) = nullptr;
};

class BusManager {
 public:
  // A snapshot reply is ~60 encoded bytes = 16 ms of wire time at 38400, on top
  // of the request and the node's scan-loop latency; 20 ms timed out every one
  // while the reply was still arriving. Covers the largest polled response.
  // Public because any code that drains the bus must outwait a whole response.
  static constexpr uint32_t kResponseTimeoutMs = 50;

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
  bool beginFirmwareHandoffAll(uint8_t leader, uint8_t target_mask, uint32_t token,
                               uint32_t update_id, uint32_t image_size,
                               uint32_t image_crc32);
  void endFirmwareMaintenance(uint32_t token);
  bool setGlobalSquares(const uint8_t* squares, size_t count, uint8_t red,
                        uint8_t green, uint8_t blue, uint16_t duration_ms,
                        const char* correlation = nullptr);
  bool clearGlobalSquares(const uint8_t* squares, size_t count,
                          const char* correlation = nullptr);
  // Per-pixel colour for one edge half-bar. Quadrants running older firmware
  // answer error code 2, which the server reads as "no bars here" and stops
  // asking. See docs/uart-api.md SET_PIXELS.
  bool setPixels(uint8_t node, uint8_t zone, uint16_t mask,
                 const uint16_t* colours_rgb565, uint8_t count,
                 const char* correlation = nullptr);
  void setOrientation(uint8_t node, uint8_t orientation);
  void setRuntimeMode(arcade::RuntimeMode mode) { runtime_mode_ = mode; }
  uint8_t globalSquare(uint8_t node, uint8_t local) const;
  bool locateGlobal(uint8_t global, uint8_t& node, uint8_t& local) const;
  const QuadrantState& node(uint8_t index) const { return nodes_[index]; }
  bool isOnline(uint8_t node) const {
    return node < arcade::kQuadrantCount && nodes_[node].online;
  }
  uint8_t onlineMask() const;
  uint8_t onlineCount() const;
  uint8_t rawTargetMask() const { return raw_target_mask_; }
  uint8_t rawResponseMask() const { return raw_response_mask_; }
  bool rawActive() const { return raw_active_; }
  // Held for the duration of a firmware upload: a raw sweep starting mid-stream
  // makes beginFirmwareHandoff() reject after the whole image has been staged.
  void setRawScansBlocked(bool blocked) { raw_scans_blocked_ = blocked; }
  uint32_t goodFrames() const { return good_frames_; }
  uint32_t badFrames() const { return bad_frames_; }
  uint32_t timeoutCount() const { return timeout_count_; }
  uint32_t snapshotRepairs() const { return snapshot_repairs_; }
  // Lets code that only holds the bus (the flasher) reach the diagnostic channel.
  void log(const char* level, const char* component, const char* message,
           uint8_t node = arcade::kInvalidNodeAddress) const {
    if (callbacks_.log) callbacks_.log(level, component, message, node);
  }
  bool busy() const { return pending_ || queue_count_ || raw_active_; }
  bool programmingHandoff() const { return programming_handoff_; }
  uint32_t maintenanceToken() const { return maintenance_token_; }

 private:
  static constexpr uint8_t kQueueCapacity = 8;
  // A full eight-pixel SET_PIXELS bar frame is 3 + 16 = 19 bytes, so the
  // original cap still covers the longest message on the wire.
  static constexpr uint8_t kMaximumQueuedPayload = 24;
  static constexpr uint8_t kMaximumCorrelationLength = 32;
  // Shared by the POLL_EVENTS request and the reply parser that trusts its cap.
  static constexpr uint8_t kMaximumEventsPerPoll = 8;

  struct QueuedCommand {
    uint8_t node = 0;
    arcade::MessageType type = arcade::MessageType::kPing;
    uint8_t payload[kMaximumQueuedPayload]{};
    uint8_t length = 0;
    char correlation[kMaximumCorrelationLength + 1]{};
  };

  void receive(uint32_t now_ms);
  void send(uint8_t node, arcade::MessageType type, const uint8_t* payload,
            uint8_t length, const char* correlation, uint32_t now_ms);
  // handleResponse() dispatches to one of these per pending request type; the
  // order and the fall-through of that chain are load-bearing, see bus_response.cpp.
  void handleResponse(const arcade::Frame& frame, uint32_t now_ms);
  void handlePollEvents(const arcade::Frame& frame, QuadrantState& node, uint32_t now_ms);
  void handleRawScan(const arcade::Frame& frame);
  void handleStatus(const arcade::Frame& frame, QuadrantState& node, uint32_t now_ms);
  void handleCalibrateAccepted(const arcade::Frame& frame, QuadrantState& node,
                               uint32_t now_ms);
  void handleInfo(const arcade::Frame& frame, QuadrantState& node);
  void handleSnapshot(const arcade::Frame& frame, QuadrantState& node);
  void handlePreflight(const arcade::Frame& frame, uint32_t now_ms);
  void handleBootloaderEntry(const arcade::Frame& frame, uint32_t now_ms);
  void handleRawScanRejected(const arcade::Frame& frame, uint8_t node_error_code,
                             uint32_t now_ms);
  void purgeQueuedFirmwareCommands();
  void handleTimeout(uint32_t now_ms);
  void schedule(uint32_t now_ms);
  void startQueued(uint32_t now_ms);
  bool queueNodeSync(uint8_t node);
  void parseRaw(uint8_t node, const arcade::Frame& frame);
  void finishRawIfReady();
  void updateCalibration(uint8_t node, uint8_t phase, uint8_t percent, uint32_t now_ms);
  void openRenderWindow(uint32_t now_ms);
  void sendBroadcast(arcade::MessageType type, const uint8_t* payload, uint8_t length);
  bool planSquareFanout(const uint8_t* squares, size_t count, bool all,
                        uint16_t (&masks)[arcade::kQuadrantCount],
                        bool (&targeted)[arcade::kQuadrantCount],
                        int8_t& last_node) const;
  // Formats one diagnostic-channel line. The paired Serial.printf() stays
  // separate at each site: it carries fields the log message deliberately omits.
  // The format attribute keeps -Wformat on the call sites (index 5/6 counts the
  // implicit `this`); without it the wrapper silently swallows argument mismatches.
  void logf(const char* level, const char* component, uint8_t node,
            const char* format, ...) const __attribute__((format(printf, 5, 6)));

  HardwareSerial* serial_ = nullptr;
  BusCallbacks callbacks_{};
  arcade::StreamDecoder decoder_;
  QuadrantState nodes_[arcade::kQuadrantCount]{};
  uint8_t orientation_[arcade::kQuadrantCount]{};
  QueuedCommand queue_[kQueueCapacity]{};
  uint8_t queue_head_ = 0;
  uint8_t queue_tail_ = 0;
  uint8_t queue_count_ = 0;
  bool pending_ = false;
  uint8_t pending_node_ = 0;
  uint8_t pending_sequence_ = 0;
  arcade::MessageType pending_type_ = arcade::MessageType::kPing;
  uint32_t deadline_ms_ = 0;
  char pending_correlation_[kMaximumCorrelationLength + 1]{};
  uint8_t sequence_ = 0;
  uint8_t poll_node_ = 0;
  uint8_t poll_count_[arcade::kQuadrantCount]{};
  uint32_t next_poll_ms_[arcade::kQuadrantCount]{};
  uint32_t next_render_ms_ = 0;
  uint32_t bus_quiet_until_ms_ = 0;
  uint32_t good_frames_ = 0;
  uint32_t bad_frames_ = 0;
  uint32_t timeout_count_ = 0;
  uint32_t snapshot_repairs_ = 0;
  bool raw_active_ = false;
  bool raw_scans_blocked_ = false;
  uint8_t raw_samples_ = 1;
  uint8_t raw_next_node_ = 0;
  uint8_t raw_target_mask_ = 0;
  uint8_t raw_done_mask_ = 0;
  uint8_t raw_response_mask_ = 0;
  uint32_t raw_scan_id_ = 0;
  char raw_correlation_[kMaximumCorrelationLength + 1]{};
  bool programming_handoff_ = false;
  uint32_t maintenance_token_ = 0;
  arcade::RuntimeMode runtime_mode_ = arcade::RuntimeMode::kNormal;
};
