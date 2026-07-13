#include "sensors.h"

#include <limits.h>
#include <string.h>

#include "bringup_config.h"

namespace quadrant {
namespace {
constexpr uint8_t kMuxLowPins[3] = {PIN_PB0, PIN_PB1, PIN_PB2};
constexpr uint8_t kMuxHighPins[3] = {PIN_PD4, PIN_PD5, PIN_PD6};
}

void Sensors::begin() {
  for (uint8_t bit = 0; bit < 3; ++bit) {
    pinMode(kMuxLowPins[bit], OUTPUT);
    pinMode(kMuxHighPins[bit], OUTPUT);
  }
  setMux(0);
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
    filtered_[i] = settings_.baseline[i];
    state_[i] = arcade::SensorState::kEmpty;
    candidate_[i] = arcade::SensorState::kEmpty;
  }
  deadline_us_ = micros() + settings_.mux_settle_us;
}

void Sensors::setMux(uint8_t channel) {
  for (uint8_t bit = 0; bit < 3; ++bit) {
    digitalWrite(kMuxLowPins[bit], (channel >> bit) & 1);
    digitalWrite(kMuxHighPins[bit], (channel >> bit) & 1);
  }
}

void Sensors::tick(uint32_t now_us, uint32_t now_ms) {
  if (static_cast<int32_t>(now_us - deadline_us_) < 0) return;
  if (phase_ == 0) {
    // Discard after switching the multiplexers so the sample-and-hold capacitor
    // and op-amp have a full conversion to settle.
    (void)analogRead(A0);
    (void)analogRead(A1);
    phase_ = 1;
    deadline_us_ = micros() + settings_.mux_settle_us;
    return;
  }

  const uint16_t low = analogRead(A0);
  const uint16_t high = analogRead(A1);
  const uint8_t low_index = channel_;
  const uint8_t high_index = channel_ + bringup::kMuxChannelCount;
  raw_[low_index] = low;
  raw_[high_index] = high;
  filtered_[low_index] = static_cast<uint16_t>(filtered_[low_index] +
      (static_cast<int16_t>(low) - static_cast<int16_t>(filtered_[low_index])) / 4);
  filtered_[high_index] = static_cast<uint16_t>(filtered_[high_index] +
      (static_cast<int16_t>(high) - static_cast<int16_t>(filtered_[high_index])) / 4);

  channel_ = static_cast<uint8_t>((channel_ + 1) % bringup::kMuxChannelCount);
  if (channel_ == 0) completeScan(now_ms);
  setMux(channel_);
  phase_ = 0;
  const uint32_t step_us =
      (static_cast<uint32_t>(settings_.full_scan_ms) * 1000UL) /
      bringup::kMuxChannelCount;
  deadline_us_ = micros() + (step_us > settings_.mux_settle_us
      ? step_us - settings_.mux_settle_us : settings_.mux_settle_us);
}

void Sensors::completeScan(uint32_t now_ms) {
  ++scan_count_;
  last_scan_ms_ = static_cast<uint16_t>(now_ms);
  if (raw_capture_active_) {
    for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) raw_capture_sum_[i] += raw_[i];
    if (++raw_capture_count_ >= raw_capture_target_) {
      for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
        raw_capture_average_[i] = static_cast<uint16_t>(raw_capture_sum_[i] / raw_capture_count_);
      }
      raw_capture_active_ = false;
      raw_capture_ready_ = true;
    }
  }
  if (calibration_active_) {
    for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
      calibration_sum_[i] += raw_[i];
      if (raw_[i] < calibration_min_[i]) calibration_min_[i] = raw_[i];
      if (raw_[i] > calibration_max_[i]) calibration_max_[i] = raw_[i];
    }
    if (++calibration_scans_ >= bringup::kCalibrationScans) {
      calibration_ok_ = true;
      for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
        const uint16_t range = calibration_max_[i] - calibration_min_[i];
        settings_.baseline[i] = static_cast<uint16_t>(
            calibration_sum_[i] / calibration_scans_);
        settings_.noise[i] = range > 255 ? 255 : static_cast<uint8_t>(range);
        if (settings_.baseline[i] < bringup::kMinimumCalibrationBaseline ||
            settings_.baseline[i] > bringup::kMaximumCalibrationBaseline ||
            range > bringup::kMaximumCalibrationNoise) calibration_ok_ = false;
      }
      settings_.calibrated = calibration_ok_ ? 1 : 0;
      if (calibration_ok_) saveSettings(settings_);
      calibration_active_ = false;
      calibration_finished_ = true;
    }
    return;
  }
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) updateClassification(i, now_ms);
}

void Sensors::updateClassification(uint8_t square, uint32_t now_ms) {
  const int16_t delta = static_cast<int16_t>(filtered_[square]) -
                        static_cast<int16_t>(settings_.baseline[square]);
  const uint16_t threshold = state_[square] == arcade::SensorState::kEmpty
      ? settings_.enter_threshold : settings_.exit_threshold;
  arcade::SensorState observed = arcade::SensorState::kEmpty;
  if (delta >= static_cast<int16_t>(threshold)) {
    observed = arcade::SensorState::kPositive;
  } else if (delta <= -static_cast<int16_t>(threshold)) {
    observed = arcade::SensorState::kNegative;
  } else if (state_[square] != arcade::SensorState::kEmpty &&
             static_cast<uint16_t>(abs(delta)) > settings_.exit_threshold) {
    observed = arcade::SensorState::kUncertain;
  }

  if (observed == state_[square]) {
    candidate_[square] = observed;
    candidate_count_[square] = 0;
    return;
  }
  if (observed != candidate_[square]) {
    candidate_[square] = observed;
    candidate_count_[square] = 1;
  } else if (candidate_count_[square] < 255) {
    ++candidate_count_[square];
  }
  if (candidate_count_[square] >= settings_.debounce_scans) {
    state_[square] = observed;
    candidate_count_[square] = 0;
    queueEvent(square, now_ms);
  }
}

void Sensors::queueEvent(uint8_t square, uint32_t now_ms) {
  if (event_count_ == bringup::kEventQueueSize) {
    event_tail_ = static_cast<uint8_t>((event_tail_ + 1) % bringup::kEventQueueSize);
    --event_count_;
    if (event_overflow_ != UINT16_MAX) ++event_overflow_;
  }
  events_[event_head_] = {square, state_[square], raw_[square], now_ms};
  event_head_ = static_cast<uint8_t>((event_head_ + 1) % bringup::kEventQueueSize);
  ++event_count_;
}

bool Sensors::popEvent(SensorEvent& event) {
  if (!event_count_) return false;
  event = events_[event_tail_];
  event_tail_ = static_cast<uint8_t>((event_tail_ + 1) % bringup::kEventQueueSize);
  --event_count_;
  return true;
}

void Sensors::startCalibration() {
  memset(calibration_sum_, 0, sizeof(calibration_sum_));
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
    calibration_min_[i] = UINT16_MAX;
    calibration_max_[i] = 0;
  }
  calibration_scans_ = 0;
  calibration_ok_ = false;
  calibration_finished_ = false;
  calibration_active_ = true;
}

void Sensors::cancelCalibration() {
  calibration_active_ = false;
  calibration_finished_ = true;
  calibration_ok_ = false;
}

uint8_t Sensors::calibrationPercent() const {
  return calibration_active_
      ? static_cast<uint8_t>((calibration_scans_ * 100U) / bringup::kCalibrationScans)
      : (calibration_ok_ ? 100 : 0);
}

bool Sensors::calibrationJustFinished() {
  const bool value = calibration_finished_;
  calibration_finished_ = false;
  return value;
}

bool Sensors::startRawCapture(uint8_t samples) {
  if (raw_capture_active_ || raw_capture_ready_) return false;
  raw_capture_target_ = constrain(samples, 1, arcade::kMaximumRawCaptureScans);
  raw_capture_count_ = 0;
  memset(raw_capture_sum_, 0, sizeof(raw_capture_sum_));
  raw_capture_active_ = true;
  return true;
}

}  // namespace quadrant
