#include "bus_manager.h"

#include <string.h>

namespace {
constexpr uint8_t kBusRxPin = 17;
constexpr uint8_t kBusTxPin = 16;
constexpr uint32_t kBusBaud = arcade::kBusBaud;
constexpr uint32_t kResponseTimeoutMs = 25;
constexpr uint32_t kOnlinePollIntervalMs = 10;
constexpr uint32_t kOfflineProbeBaseMs = 1000;
constexpr uint32_t kOfflineProbeMaximumMs = 10000;
constexpr uint8_t kOfflineTimeoutThreshold = 3;
constexpr uint32_t kRawResponseTimeoutMs = 7000;
constexpr uint32_t kPersistentWriteTimeoutMs = 300;
constexpr uint32_t kTransientMissRetryMs = 50;
constexpr uint32_t kRenderQuietMs = 4;
constexpr uint32_t kRenderIntervalMs = 1000 / arcade::kRenderFramesPerSecond;
constexpr uint8_t kMaximumEventsPerPoll = 8;
constexpr uint8_t kSnapshotRawOffset = arcade::kSquaresPerQuadrant;
constexpr uint8_t kSnapshotPayloadBytes =
    arcade::kSquaresPerQuadrant * (sizeof(uint8_t) + sizeof(uint16_t));
constexpr uint8_t kRawHeaderBytes = 3;
constexpr uint8_t kRawSquareBytes = 6;
constexpr uint8_t kRawPayloadBytes =
    kRawHeaderBytes + arcade::kSquaresPerQuadrant * kRawSquareBytes;
constexpr uint8_t kPreflightPayloadBytes = 18;
constexpr uint8_t kPreflightFuseOffset = 1;
constexpr uint8_t kPreflightBootloaderOffset = 2;
constexpr uint8_t kPreflightProtocolOffset = 3;
constexpr uint8_t kPreflightPageSizeOffset = 4;
constexpr uint8_t kPreflightFlashSizeOffset = 6;
constexpr uint8_t kPreflightApplicationLimitOffset = 10;
constexpr uint8_t kPreflightStateOffset = 14;
constexpr uint8_t kPreflightResetCauseOffset = 15;
constexpr uint8_t kPreflightSupplyMvOffset = 16;

template <size_t Capacity>
void copyCorrelation(char (&output)[Capacity], const char* input) {
  if (!input) { output[0] = 0; return; }
  strncpy(output, input, Capacity - 1);
  output[Capacity - 1] = 0;
}
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
       type == arcade::MessageType::kMaintenanceEnd);
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
  if (raw_active_ || !onlineMask()) return false;
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

bool BusManager::setGlobalSquares(const uint8_t* squares, size_t count, uint8_t red,
                                  uint8_t green, uint8_t blue, uint16_t duration_ms,
                                  const char* correlation) {
  uint16_t masks[4]{};
  for (size_t i = 0; i < count; ++i) {
    uint8_t node = 0, local = 0;
    if (locateGlobal(squares[i], node, local)) masks[node] |= 1U << local;
  }
  bool any = false;
  int8_t last_node = -1;
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    if (masks[node] && nodes_[node].online) last_node = node;
  }
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    if (!masks[node] || !nodes_[node].online) continue;
    uint8_t payload[7];
    arcade::putU16(payload, masks[node]);
    payload[2] = red; payload[3] = green; payload[4] = blue;
    arcade::putU16(payload + 5, duration_ms);
    any |= enqueue(node, arcade::MessageType::kSetSquares, payload, sizeof(payload),
                   node == last_node ? correlation : nullptr);
  }
  return any;
}

void BusManager::tick(uint32_t now_ms) {
  // During a firmware handoff the STK500 programmer owns the bus UART; framed
  // reception would consume raw programmer bytes.
  if (programming_handoff_) return;
  receive(now_ms);
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
  const bool slow_eeprom = type == arcade::MessageType::kFwPrepare ||
                           type == arcade::MessageType::kFwEnterBootloader;
  deadline_ms_ = now_ms + (type == arcade::MessageType::kGetRawScan
      ? kRawResponseTimeoutMs
      : slow_eeprom ? kPersistentWriteTimeoutMs : kResponseTimeoutMs);
  copyCorrelation(pending_correlation_, correlation);
}

void BusManager::handleResponse(const arcade::Frame& frame, uint32_t now_ms) {
  if (!pending_ || !(frame.flags & arcade::kResponse) || frame.source != pending_node_ ||
      frame.destination != arcade::kEspAddress || frame.sequence != pending_sequence_) {
    ++bad_frames_;
    Serial.printf("[%10u][W][BUS] unexpected src=%u seq=%u type=0x%02x\n", now_ms,
                  frame.source, frame.sequence, static_cast<unsigned>(frame.type));
    return;
  }
  pending_ = false; ++good_frames_;
  QuadrantState& node = nodes_[frame.source];
  const bool newly_online = !node.online;
  node.online = true;
  node.last_seen_ms = now_ms;
  node.consecutive_timeouts = 0;
  next_poll_ms_[frame.source] = now_ms + kOnlinePollIntervalMs;
  if (newly_online) {
    poll_count_[frame.source] = 0;
    Serial.printf("[%10u][I][BUS] node=%u online\n", now_ms, frame.source);
    node.needs_sync = !queueNodeSync(frame.source);
    if (callbacks_.nodePresenceChanged) callbacks_.nodePresenceChanged(frame.source, true);
  }
  bool ok = !(frame.flags & arcade::kError);
  const char* reason = ok ? nullptr : "node_error";

  if (ok && pending_type_ == arcade::MessageType::kPollEvents && frame.payload_length >= 1) {
      const uint8_t count = frame.payload[0] > kMaximumEventsPerPoll
          ? kMaximumEventsPerPoll : frame.payload[0];
    uint8_t offset = 1;
    for (uint8_t i = 0; i < count && offset + 8 <= frame.payload_length; ++i) {
      const uint8_t local = frame.payload[offset++];
      const auto state = static_cast<arcade::SensorState>(frame.payload[offset++]);
      const uint16_t raw = arcade::getU16(frame.payload + offset); offset += 2;
      offset += 4;
      if (local < arcade::kSquaresPerQuadrant) {
        node.state[local] = state; node.raw[local] = raw;
        if (runtime_mode_ == arcade::RuntimeMode::kBringup) {
          Serial.printf("[%10u][I][SENSOR] node=%u local=%u global=%u state=%u raw=%u\n",
                        now_ms, frame.source, local, globalSquare(frame.source, local),
                        static_cast<unsigned>(state), raw);
        }
        if (callbacks_.sensorChanged) callbacks_.sensorChanged(
            globalSquare(frame.source, local), state, raw, frame.source, local);
      }
    }
  } else if (ok && pending_type_ == arcade::MessageType::kGetRawScan &&
             frame.type == arcade::MessageType::kRawScan) {
    parseRaw(frame.source, frame);
    raw_done_mask_ |= 1U << frame.source;
    raw_response_mask_ |= 1U << frame.source;
    finishRawIfReady();
  } else if (ok && pending_type_ == arcade::MessageType::kStatus && frame.payload_length >= 7) {
    node.calibrated = frame.payload[5] != 0;
  } else if (ok && pending_type_ == arcade::MessageType::kGetSnapshot &&
             frame.payload_length >= kSnapshotPayloadBytes) {
    for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
      node.state[i] = static_cast<arcade::SensorState>(frame.payload[i]);
      node.raw[i] = arcade::getU16(frame.payload + kSnapshotRawOffset + i * sizeof(uint16_t));
    }
  } else if (ok && pending_type_ == arcade::MessageType::kFwPreflight &&
             frame.payload_length >= kPreflightPayloadBytes) {
    Serial.printf("[%10u][I][FW] node=%u hfuse=0x%02x boot=%u handoff_v=%u page=%u flash=%u app_limit=%u marker=%u reset=0x%02x avcc=%u\n",
                  now_ms, frame.source, frame.payload[kPreflightFuseOffset],
                  frame.payload[kPreflightBootloaderOffset],
                  frame.payload[kPreflightProtocolOffset],
                  arcade::getU16(frame.payload + kPreflightPageSizeOffset),
                  arcade::getU32(frame.payload + kPreflightFlashSizeOffset),
                  arcade::getU32(frame.payload + kPreflightApplicationLimitOffset),
                  frame.payload[kPreflightStateOffset],
                  frame.payload[kPreflightResetCauseOffset],
                  arcade::getU16(frame.payload + kPreflightSupplyMvOffset));
  } else if (ok && pending_type_ == arcade::MessageType::kFwEnterBootloader) {
    programming_handoff_ = true;
    Serial.printf("[%10u][I][FW] target=%u ACKed bootloader entry; framed polling stopped\n",
                  now_ms, frame.source);
  }
  if (!ok && pending_type_ == arcade::MessageType::kFwPrepare) {
    queue_count_ = 0;
    queue_tail_ = queue_head_;
  }
  if ((pending_type_ == arcade::MessageType::kFwHealth ||
       pending_type_ == arcade::MessageType::kFwConfirm ||
       pending_type_ == arcade::MessageType::kFwEnterBootloader ||
       pending_type_ == arcade::MessageType::kFwPrepare) &&
      callbacks_.fwResponse) {
    callbacks_.fwResponse(frame.source, pending_type_, ok, frame.payload,
                          static_cast<uint8_t>(frame.payload_length));
  }
  if (pending_correlation_[0] && callbacks_.commandComplete &&
      pending_type_ != arcade::MessageType::kGetRawScan) {
    callbacks_.commandComplete(pending_correlation_, ok, reason);
  }
}

void BusManager::parseRaw(uint8_t index, const arcade::Frame& frame) {
  if (frame.payload_length < kRawPayloadBytes) return;
  QuadrantState& node = nodes_[index];
  uint8_t offset = kRawHeaderBytes;
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
    node.raw[i] = arcade::getU16(frame.payload + offset); offset += 2;
    node.baseline[i] = arcade::getU16(frame.payload + offset); offset += 2;
    node.noise[i] = frame.payload[offset++];
    node.state[i] = static_cast<arcade::SensorState>(frame.payload[offset++]);
  }
  node.raw_valid = true;
}

void BusManager::handleTimeout(uint32_t now_ms) {
  pending_ = false; ++timeout_count_;
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
  }
  if (was_online && confirmed_offline && callbacks_.nodePresenceChanged) {
    callbacks_.nodePresenceChanged(pending_node_, false);
  }
  if (pending_type_ == arcade::MessageType::kGetRawScan) {
    raw_done_mask_ |= 1U << pending_node_;
    finishRawIfReady();
  } else if (pending_correlation_[0] && callbacks_.commandComplete) {
    callbacks_.commandComplete(pending_correlation_, false, "timeout");
  }
}

void BusManager::finishRawIfReady() {
  if ((raw_done_mask_ & raw_target_mask_) != raw_target_mask_) return;
  raw_active_ = false;
  uint8_t valid_mask = 0;
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    if (nodes_[node].raw_valid) valid_mask |= 1U << node;
  }
  const bool complete = (valid_mask & raw_target_mask_) == raw_target_mask_;
  Serial.printf("[%10lu][I][RAW] scan=%u target=0x%02x response=0x%02x complete=%u\n",
                millis(), raw_scan_id_, raw_target_mask_, rawResponseMask(), complete);
  if (callbacks_.rawScanReady) callbacks_.rawScanReady(complete, raw_scan_id_);
  if (raw_correlation_[0] && callbacks_.commandComplete) {
    callbacks_.commandComplete(raw_correlation_, complete, complete ? nullptr : "partial_scan");
  }
}

void BusManager::startQueued(uint32_t now_ms) {
  QueuedCommand q = queue_[queue_tail_];
  queue_tail_ = static_cast<uint8_t>((queue_tail_ + 1) % kQueueCapacity); --queue_count_;
  if (q.node == arcade::kBroadcastAddress) sendBroadcast(q.type, q.payload, q.length);
  else send(q.node, q.type, q.payload, q.length, q.correlation, now_ms);
}

bool BusManager::queueNodeSync(uint8_t node) {
  if (!isOnline(node) || queue_count_ > kQueueCapacity - 2) return false;
  uint8_t payload[3] = {arcade::configKey(arcade::ConfigKey::kOrientation), 0, 0};
  arcade::putU16(payload + 1, orientation_[node]);
  if (!enqueue(node, arcade::MessageType::kConfigSet, payload, sizeof(payload))) return false;
  payload[0] = arcade::configKey(arcade::ConfigKey::kRuntimeMode);
  arcade::putU16(payload + 1, static_cast<uint8_t>(runtime_mode_));
  return enqueue(node, arcade::MessageType::kConfigSet, payload, sizeof(payload));
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
    const uint8_t count = ++poll_count_[node];
    if (count == 1) send(node, arcade::MessageType::kStatus, nullptr, 0, nullptr, now_ms);
    else if (count == 2) send(node, arcade::MessageType::kGetSnapshot, nullptr, 0, nullptr, now_ms);
    else send(node, arcade::MessageType::kPollEvents, &max_events, 1, nullptr, now_ms);
    return;
  }
}
