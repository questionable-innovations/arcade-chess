#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

#include "bringup_config.h"
#include "persistent.h"

namespace quadrant {

struct SensorEvent {
  uint8_t square;
  arcade::SensorState state;
  uint16_t raw;
  uint32_t at_ms;
};

class Sensors {
 public:
  explicit Sensors(Settings& settings) : settings_(settings) {}
  void begin();
  void tick(uint32_t now_us, uint32_t now_ms);
  bool popEvent(SensorEvent& event);
  void startCalibration();
  void cancelCalibration();
  bool calibrating() const { return calibration_active_; }
  uint8_t calibrationPercent() const;
  uint16_t calibrationSamples() const { return calibration_scans_; }
  bool calibrationJustFinished();
  bool calibrationSucceeded() const { return calibration_ok_; }
  bool startRawCapture(uint8_t samples);
  bool rawCaptureReady() const { return raw_capture_ready_; }
  bool rawCaptureBusy() const { return raw_capture_active_ || raw_capture_ready_; }
  uint8_t rawCaptureSamples() const { return raw_capture_count_; }
  uint16_t rawCaptureValue(uint8_t square) const { return raw_capture_average_[square]; }
  void consumeRawCapture() { raw_capture_ready_ = false; }
  uint16_t raw(uint8_t square) const { return raw_[square]; }
  uint8_t noise(uint8_t square) const { return settings_.noise[square]; }
  arcade::SensorState state(uint8_t square) const { return state_[square]; }
  uint16_t lastScanMs() const { return last_scan_ms_; }
  uint8_t eventDepth() const { return event_count_; }
  uint16_t overflowCount() const { return event_overflow_; }
  uint32_t scanCount() const { return scan_count_; }

 private:
  void setMux(uint8_t channel);
  void completeScan(uint32_t now_ms);
  void updateClassification(uint8_t square, uint32_t now_ms);
  void queueEvent(uint8_t square, uint32_t now_ms);

  Settings& settings_;
  uint16_t raw_[arcade::kSquaresPerQuadrant]{};
  uint16_t filtered_[arcade::kSquaresPerQuadrant]{};
  arcade::SensorState state_[arcade::kSquaresPerQuadrant]{};
  arcade::SensorState candidate_[arcade::kSquaresPerQuadrant]{};
  uint8_t candidate_count_[arcade::kSquaresPerQuadrant]{};
  SensorEvent events_[bringup::kEventQueueSize]{};
  uint8_t event_head_ = 0;
  uint8_t event_tail_ = 0;
  uint8_t event_count_ = 0;
  uint16_t event_overflow_ = 0;
  uint8_t channel_ = 0;
  uint8_t phase_ = 0;
  uint32_t deadline_us_ = 0;
  uint32_t scan_count_ = 0;
  uint16_t last_scan_ms_ = 0;
  bool calibration_active_ = false;
  bool calibration_finished_ = false;
  bool calibration_ok_ = false;
  uint16_t calibration_scans_ = 0;
  uint32_t calibration_sum_[arcade::kSquaresPerQuadrant]{};
  uint16_t calibration_min_[arcade::kSquaresPerQuadrant]{};
  uint16_t calibration_max_[arcade::kSquaresPerQuadrant]{};
  bool raw_capture_active_ = false;
  bool raw_capture_ready_ = false;
  uint8_t raw_capture_target_ = 0;
  uint8_t raw_capture_count_ = 0;
  uint32_t raw_capture_sum_[arcade::kSquaresPerQuadrant]{};
  uint16_t raw_capture_average_[arcade::kSquaresPerQuadrant]{};
};

}  // namespace quadrant
