#include "bus_manager.h"

#include <stdarg.h>
#include <string.h>

namespace {
constexpr uint8_t kBusRxPin = 17;
constexpr uint8_t kBusTxPin = 16;
constexpr uint32_t kBusBaud = arcade::kBusBaud;
// A raw capture takes samples × full_scan_ms on the node (full_scan_ms is
// capped at 200 ms), so the deadline scales with the request instead of
// stalling the whole bus for a worst-case constant.
constexpr uint32_t kRawResponseBaseMs = 400;
constexpr uint32_t kRawResponsePerSampleMs = 220;
// Online nodes re-poll status every kPollCycleLength polls so calibration and
// supply readings stay fresh; a calibrating node reports much more often.
constexpr uint8_t kPollCycleLength = 32;
constexpr uint8_t kPollCycleCalibrating = 6;
constexpr uint32_t kOfflineProbeBaseMs = 1000;
constexpr uint32_t kOfflineProbeMaximumMs = 10000;
constexpr uint8_t kOfflineTimeoutThreshold = 3;
constexpr uint32_t kTransientMissRetryMs = 50;
constexpr uint32_t kPersistentWriteTimeoutMs = 300;
constexpr uint32_t kRenderQuietMs = 4;
constexpr uint32_t kRenderIntervalMs = 1000 / arcade::kRenderFramesPerSecond;
// Sized for the longest line logf() currently formats, so no caller truncates.
constexpr size_t kMaximumLogMessage = 48;

template <size_t Capacity>
void copyCorrelation(char (&output)[Capacity], const char* input) {
  if (!input) { output[0] = 0; return; }
  strncpy(output, input, Capacity - 1);
  output[Capacity - 1] = 0;
}
}

void BusManager::logf(const char* level, const char* component, uint8_t node,
                      const char* format, ...) const {
  char message[kMaximumLogMessage];
  va_list arguments;
  va_start(arguments, format);
  vsnprintf(message, sizeof(message), format, arguments);
  va_end(arguments);
  log(level, component, message, node);
}

void BusManager::begin(HardwareSerial& serial, BusCallbacks callbacks) {
  serial_ = &serial;
  callbacks_ = callbacks;
  serial_->begin(kBusBaud, SERIAL_8N1, kBusRxPin, kBusTxPin);
  Serial.printf("[%10lu][I][BUS] UART2 %u baud RX=%u TX=%u\n", millis(),
                kBusBaud, kBusRxPin, kBusTxPin);
}

bool BusManager::enqueue(uint8_t node, arcade::MessageType type,
                         const uint8_t* payload, uint8_t length,
                         const char* correlation) {
  const bool valid_broadcast = node == arcade::kBroadcastAddress &&
      (type == arcade::MessageType::kMaintenanceBegin ||
       type == arcade::MessageType::kMaintenanceEnd ||
       type == arcade::MessageType::kFwPrepare ||
       type == arcade::MessageType::kFwEnterBootloader);
  if ((!valid_broadcast && node >= arcade::kQuadrantCount) ||
      length > sizeof(queue_[0].payload) || queue_count_ == kQueueCapacity) return false;
  QueuedCommand& q = queue_[queue_head_];
  q.node = node; q.type = type; q.length = length;
  if (length) memcpy(q.payload, payload, length);
  copyCorrelation(q.correlation, correlation);
  queue_head_ = static_cast<uint8_t>((queue_head_ + 1) % kQueueCapacity);
  ++queue_count_;
  return true;
}

bool BusManager::requestRawScan(uint8_t samples, const char* correlation) {
  if (raw_active_ || raw_scans_blocked_ || !onlineMask()) return false;
  raw_active_ = true;
  raw_samples_ = constrain(samples, 1, arcade::kMaximumRawCaptureScans);
  raw_next_node_ = 0;
  raw_done_mask_ = 0;
  raw_response_mask_ = 0;
  raw_target_mask_ = onlineMask();
  ++raw_scan_id_;
  copyCorrelation(raw_correlation_, correlation);
  for (auto& node : nodes_) node.raw_valid = false;
  return true;
}

bool BusManager::calibrate(uint8_t node, const char* correlation) {
  if (!isOnline(node)) return false;
  const uint8_t action = 1;
  return enqueue(node, arcade::MessageType::kCalibrate, &action, 1, correlation);
}

bool BusManager::identify(uint8_t node, uint16_t duration_ms, const char* correlation) {
  if (!isOnline(node)) return false;
  uint8_t payload[2]; arcade::putU16(payload, duration_ms);
  return enqueue(node, arcade::MessageType::kIdentify, payload, sizeof(payload), correlation);
}

bool BusManager::setBrightness(uint8_t node, uint8_t value, const char* correlation) {
  if (!isOnline(node)) return false;
  return enqueue(node, arcade::MessageType::kSetBrightness, &value, 1, correlation);
}

bool BusManager::setConfig(uint8_t node, uint8_t key, uint16_t value,
                           const char* correlation) {
  if (!isOnline(node)) return false;
  uint8_t payload[3] = {key, 0, 0}; arcade::putU16(payload + 1, value);
  return enqueue(node, arcade::MessageType::kConfigSet, payload, sizeof(payload), correlation);
}

// Resolves global squares onto per-node bit masks and marks which online nodes
// the fan-out covers. `all` is the empty-clear case: every online node is a
// target whatever its mask holds. The send loops iterate `targeted` rather than
// restating the predicate, so `last_node` cannot name a node they skip.
bool BusManager::planSquareFanout(const uint8_t* squares, size_t count, bool all,
                                  uint16_t (&masks)[arcade::kQuadrantCount],
                                  bool (&targeted)[arcade::kQuadrantCount],
                                  int8_t& last_node) const {
  for (size_t i = 0; i < count; ++i) {
    uint8_t node = 0, local = 0;
    if (locateGlobal(squares[i], node, local)) masks[node] |= 1U << local;
  }
  uint8_t targets = 0;
  last_node = -1;
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    targeted[node] = (all || masks[node]) && nodes_[node].online;
    if (targeted[node]) { last_node = node; ++targets; }
  }
  // All or nothing: a partial fan-out drops the correlation carried by the last
  // target and the client then waits forever for a terminal result.
  return targets && queue_count_ + targets <= kQueueCapacity;
}

bool BusManager::setGlobalSquares(const uint8_t* squares, size_t count, uint8_t red,
                                  uint8_t green, uint8_t blue, uint16_t duration_ms,
                                  const char* correlation) {
  uint16_t masks[arcade::kQuadrantCount]{};
  bool targeted[arcade::kQuadrantCount]{};
  int8_t last_node = -1;
  if (!planSquareFanout(squares, count, false, masks, targeted, last_node)) return false;
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    if (!targeted[node]) continue;
    uint8_t payload[7];
    arcade::putU16(payload, masks[node]);
    payload[2] = red; payload[3] = green; payload[4] = blue;
    arcade::putU16(payload + 5, duration_ms);
    if (!enqueue(node, arcade::MessageType::kSetSquares, payload, sizeof(payload),
                 node == last_node ? correlation : nullptr)) return false;
  }
  return true;
}

bool BusManager::setPixels(uint8_t node, uint8_t zone, uint16_t mask,
                           const uint16_t* colours_rgb565, uint8_t count,
                           const char* correlation) {
  if (!isOnline(node)) return false;
  const uint8_t length = static_cast<uint8_t>(3 + count * 2);
  if (length > kMaximumQueuedPayload) return false;
  uint8_t payload[kMaximumQueuedPayload];
  payload[0] = zone;
  arcade::putU16(payload + 1, mask);
  for (uint8_t i = 0; i < count; ++i) {
    arcade::putU16(payload + 3 + i * 2, colours_rgb565[i]);
  }
  return enqueue(node, arcade::MessageType::kSetPixels, payload, length, correlation);
}

bool BusManager::clearGlobalSquares(const uint8_t* squares, size_t count,
                                    const char* correlation) {
  const bool all = count == 0;
  uint16_t masks[arcade::kQuadrantCount]{};
  bool targeted[arcade::kQuadrantCount]{};
  int8_t last_node = -1;
  if (!planSquareFanout(squares, count, all, masks, targeted, last_node)) return false;
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    if (!targeted[node]) continue;
    uint8_t payload[2];
    arcade::putU16(payload, masks[node]);
    if (!enqueue(node, arcade::MessageType::kClearLighting, payload,
                 all ? 0 : sizeof(payload),
                 node == last_node ? correlation : nullptr)) return false;
  }
  return true;
}

void BusManager::tick(uint32_t now_ms) {
  // During a firmware handoff the STK500 programmer owns the bus UART; framed
  // reception would consume raw programmer bytes.
  if (programming_handoff_) return;
  receive(now_ms);
  for (uint8_t index = 0; index < arcade::kQuadrantCount; ++index) {
    QuadrantState& node = nodes_[index];
    if (node.calibration_watch &&
        static_cast<int32_t>(now_ms - node.calibration_deadline_ms) >= 0) {
      node.calibration_watch = false;
      if (callbacks_.calibrationResult) callbacks_.calibrationResult(index, false, "timeout");
    }
  }
  if (pending_ && static_cast<int32_t>(now_ms - deadline_ms_) >= 0) handleTimeout(now_ms);
  if (!pending_) schedule(now_ms);
}

void BusManager::receive(uint32_t now_ms) {
  while (serial_->available()) {
    arcade::Frame frame{};
    const arcade::DecodeResult result = decoder_.push(serial_->read(), frame);
    if (result == arcade::DecodeResult::kFrame) handleResponse(frame, now_ms);
    else if (result != arcade::DecodeResult::kNone && result != arcade::DecodeResult::kEmpty) {
      ++bad_frames_;
      Serial.printf("[%10u][W][BUS] decoder error=%u\n", now_ms, static_cast<unsigned>(result));
      logf("warn", "bus", arcade::kInvalidNodeAddress, "decoder error=%u",
           static_cast<unsigned>(result));
    }
  }
}

void BusManager::send(uint8_t node, arcade::MessageType type, const uint8_t* payload,
                      uint8_t length, const char* correlation, uint32_t now_ms) {
  arcade::Frame frame{};
  frame.flags = arcade::kAckRequired;
  frame.source = arcade::kEspAddress;
  frame.destination = node;
  frame.type = type;
  frame.sequence = ++sequence_;
  frame.payload_length = length;
  if (length) memcpy(frame.payload, payload, length);
  uint8_t wire[arcade::kMaxEncodedFrame];
  const size_t wire_length = arcade::encodeFrame(frame, wire, sizeof(wire));
  if (!wire_length) return;
  serial_->write(wire, wire_length);
  pending_ = true; pending_node_ = node; pending_sequence_ = frame.sequence;
  pending_type_ = type;
  // Anything that reaches saveSettings() blocks the node for ~3.3 ms per changed
  // EEPROM byte; the first CONFIG_SET after a node boots rewrites the whole
  // 72-byte record. A 50 ms deadline expires mid-write, the bus is re-armed in
  // the same tick and the late reply then collides with another node's.
  // Calibration completion saves inside the scan loop, so its STATUS poll needs
  // the same budget.
  const bool slow_eeprom = type == arcade::MessageType::kFwPrepare ||
                           type == arcade::MessageType::kFwEnterBootloader ||
                           type == arcade::MessageType::kConfigSet ||
                           type == arcade::MessageType::kSetBrightness ||
                           (type == arcade::MessageType::kStatus &&
                            node < arcade::kQuadrantCount &&
                            nodes_[node].calibration_watch);
  deadline_ms_ = now_ms + (type == arcade::MessageType::kGetRawScan
      ? kRawResponseBaseMs + raw_samples_ * kRawResponsePerSampleMs
      : slow_eeprom ? kPersistentWriteTimeoutMs : kResponseTimeoutMs);
  copyCorrelation(pending_correlation_, correlation);
  if (callbacks_.busTrace) {
    callbacks_.busTrace("tx", node, frame.sequence, type, "sent",
                        frame.payload, static_cast<uint8_t>(frame.payload_length), 0);
  }
}

void BusManager::handleTimeout(uint32_t now_ms) {
  pending_ = false; ++timeout_count_;
  if (callbacks_.busTrace) {
    callbacks_.busTrace("rx", pending_node_, pending_sequence_, pending_type_,
                        "timeout", nullptr, 0, 0);
  }
  QuadrantState& node = nodes_[pending_node_];
  const bool was_online = node.online;
  ++node.timeouts;
  if (node.consecutive_timeouts < 255) ++node.consecutive_timeouts;
  const bool confirmed_offline = !was_online ||
      node.consecutive_timeouts >= kOfflineTimeoutThreshold;
  node.online = !confirmed_offline;
  uint8_t offline_misses = 0;
  if (confirmed_offline) {
    offline_misses = was_online
        ? node.consecutive_timeouts - kOfflineTimeoutThreshold
        : node.consecutive_timeouts - 1;
  }
  const uint8_t shift = offline_misses > 3 ? 3 : offline_misses;
  const uint32_t retry_ms = confirmed_offline
      ? min(kOfflineProbeBaseMs << shift, kOfflineProbeMaximumMs)
      : kTransientMissRetryMs;
  next_poll_ms_[pending_node_] = now_ms + retry_ms;
  poll_count_[pending_node_] = 0;
  if ((was_online && confirmed_offline) || runtime_mode_ == arcade::RuntimeMode::kBringup) {
    Serial.printf("[%10u][W][BUS] node=%u %s timeout type=0x%02x retry_ms=%u\n",
                  now_ms, pending_node_, confirmed_offline ? "offline" : "miss",
                  static_cast<unsigned>(pending_type_), retry_ms);
    logf(confirmed_offline ? "error" : "warn", "bus", pending_node_,
         "%s on type=0x%02x retry_ms=%u",
         confirmed_offline ? "node offline" : "poll miss",
         static_cast<unsigned>(pending_type_), retry_ms);
  }
  if (was_online && confirmed_offline && callbacks_.nodePresenceChanged) {
    callbacks_.nodePresenceChanged(pending_node_, false);
  }
  if (pending_type_ == arcade::MessageType::kGetRawScan) {
    raw_done_mask_ |= 1U << pending_node_;
    finishRawIfReady();
  } else if (pending_correlation_[0] && callbacks_.commandComplete) {
    callbacks_.commandComplete(pending_correlation_, false, "timeout", pending_node_, 0);
  }
}

void BusManager::startQueued(uint32_t now_ms) {
  QueuedCommand q = queue_[queue_tail_];
  queue_tail_ = static_cast<uint8_t>((queue_tail_ + 1) % kQueueCapacity); --queue_count_;
  if (q.node == arcade::kBroadcastAddress) sendBroadcast(q.type, q.payload, q.length);
  else send(q.node, q.type, q.payload, q.length, q.correlation, now_ms);
}

bool BusManager::queueNodeSync(uint8_t node) {
  if (!isOnline(node) || queue_count_ > kQueueCapacity - 3) return false;
  uint8_t payload[3] = {arcade::configKey(arcade::ConfigKey::kOrientation), 0, 0};
  arcade::putU16(payload + 1, orientation_[node]);
  if (!enqueue(node, arcade::MessageType::kConfigSet, payload, sizeof(payload))) return false;
  payload[0] = arcade::configKey(arcade::ConfigKey::kRuntimeMode);
  arcade::putU16(payload + 1, static_cast<uint8_t>(runtime_mode_));
  return enqueue(node, arcade::MessageType::kConfigSet, payload, sizeof(payload)) &&
         enqueue(node, arcade::MessageType::kInfo, nullptr, 0);
}

void BusManager::sendBroadcast(arcade::MessageType type, const uint8_t* payload, uint8_t length) {
  arcade::Frame frame{};
  frame.source = arcade::kEspAddress;
  frame.destination = arcade::kBroadcastAddress;
  frame.type = type;
  frame.sequence = ++sequence_;
  frame.payload_length = length;
  if (length) memcpy(frame.payload, payload, length);
  uint8_t wire[arcade::kMaxEncodedFrame];
  const size_t wire_length = arcade::encodeFrame(frame, wire, sizeof(wire));
  if (wire_length) { serial_->write(wire, wire_length); serial_->flush(); }
}

void BusManager::openRenderWindow(uint32_t now_ms) {
  (void)now_ms;
  arcade::Frame frame{};
  frame.source = arcade::kEspAddress;
  frame.destination = arcade::kBroadcastAddress;
  frame.type = arcade::MessageType::kRenderWindow;
  frame.sequence = ++sequence_;
  uint8_t wire[arcade::kMaxEncodedFrame];
  const size_t length = arcade::encodeFrame(frame, wire, sizeof(wire));
  if (length) {
    serial_->write(wire, length);
    serial_->flush();
  }
  const uint32_t marker_sent_ms = millis();
  // Every AVR masks interrupts while shifting its four LED chains. The ESP
  // creates a shared quiet window so no response is lost during that interval.
  bus_quiet_until_ms_ = marker_sent_ms + kRenderQuietMs;
  next_render_ms_ = marker_sent_ms + kRenderIntervalMs;
}

void BusManager::schedule(uint32_t now_ms) {
  if (programming_handoff_) return;
  if (static_cast<int32_t>(now_ms - bus_quiet_until_ms_) < 0) return;
  if (static_cast<int32_t>(now_ms - next_render_ms_) >= 0) {
    openRenderWindow(now_ms);
    return;
  }
  if (queue_count_) { startQueued(now_ms); return; }
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    if (nodes_[node].needs_sync && queueNodeSync(node)) {
      nodes_[node].needs_sync = false;
      return;
    }
  }
  if (raw_active_) {
    for (uint8_t attempt = 0; attempt < arcade::kQuadrantCount; ++attempt) {
      const uint8_t node = raw_next_node_++ % arcade::kQuadrantCount;
      const uint8_t bit = 1U << node;
      if ((raw_target_mask_ & bit) && !(raw_done_mask_ & bit)) {
        const uint8_t sample = raw_samples_;
        send(node, arcade::MessageType::kGetRawScan, &sample, 1, nullptr, now_ms);
        return;
      }
    }
    finishRawIfReady();
    return;
  }
  for (uint8_t attempt = 0; attempt < arcade::kQuadrantCount; ++attempt) {
    const uint8_t node = poll_node_++ % arcade::kQuadrantCount;
    if (static_cast<int32_t>(now_ms - next_poll_ms_[node]) < 0) continue;
    if (!nodes_[node].online) {
      send(node, arcade::MessageType::kPing, nullptr, 0, nullptr, now_ms);
      return;
    }
    const uint8_t max_events = kMaximumEventsPerPoll;
    const uint8_t cycle = nodes_[node].calibration_watch
        ? kPollCycleCalibrating : kPollCycleLength;
    if (poll_count_[node] >= cycle) poll_count_[node] = 0;
    const uint8_t count = ++poll_count_[node];
    if (count == 1) send(node, arcade::MessageType::kStatus, nullptr, 0, nullptr, now_ms);
    else if (count == 2) send(node, arcade::MessageType::kGetSnapshot, nullptr, 0, nullptr, now_ms);
    else send(node, arcade::MessageType::kPollEvents, &max_events, 1, nullptr, now_ms);
    return;
  }
}
