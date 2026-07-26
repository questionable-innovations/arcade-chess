#include "bus_manager.h"

#include <string.h>

namespace {
constexpr uint32_t kOnlinePollIntervalMs = 10;
constexpr uint32_t kCalibrationWatchMs = 30000;
// Pre-extension firmware exposes only the persisted calibrated flag, which
// startCalibration() never clears — so an already-calibrated node reads 1 for
// the whole run and the flag alone can't signal completion. Wait out the run
// (128 scans x default 16 ms plus margin) before trusting the flag.
constexpr uint32_t kLegacyCalibrationSettleMs = 8000;
constexpr uint8_t kSnapshotRawOffset = arcade::kSquaresPerQuadrant;
constexpr uint8_t kSnapshotPayloadBytes =
    arcade::kSquaresPerQuadrant * (sizeof(uint8_t) + sizeof(uint16_t));
constexpr uint8_t kRawHeaderBytes = 3;
constexpr uint8_t kRawSquareBytes = 6;
constexpr uint8_t kRawPayloadBytes =
    kRawHeaderBytes + arcade::kSquaresPerQuadrant * kRawSquareBytes;
// Minimum stays 18 so a node still running pre-refusal-byte firmware — exactly
// the node you are trying to update — still parses.
constexpr uint8_t kPreflightPayloadBytes = 18;
// Field offsets into that one packed reply struct.
constexpr uint8_t kPreflightBroadcastRefusalOffset = 18;
constexpr uint8_t kPreflightFuseOffset = 1;
constexpr uint8_t kPreflightBootloaderOffset = 2;
constexpr uint8_t kPreflightProtocolOffset = 3;
constexpr uint8_t kPreflightPageSizeOffset = 4;
constexpr uint8_t kPreflightFlashSizeOffset = 6;
constexpr uint8_t kPreflightApplicationLimitOffset = 10;
constexpr uint8_t kPreflightStateOffset = 14;
constexpr uint8_t kPreflightResetCauseOffset = 15;
constexpr uint8_t kPreflightSupplyMvOffset = 16;
}

void BusManager::handleResponse(const arcade::Frame& frame, uint32_t now_ms) {
  if (!pending_ || !(frame.flags & arcade::kResponse) || frame.source != pending_node_ ||
      frame.destination != arcade::kEspAddress || frame.sequence != pending_sequence_) {
    ++bad_frames_;
    Serial.printf("[%10u][W][BUS] unexpected src=%u seq=%u type=0x%02x\n", now_ms,
                  frame.source, frame.sequence, static_cast<unsigned>(frame.type));
    logf("warn", "bus", frame.source, "unexpected reply seq=%u type=0x%02x",
         frame.sequence, static_cast<unsigned>(frame.type));
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
  // An error payload is [request type, node error code]. The reason string stays
  // in the stable set and the code (docs/uart-api.md) rides alongside it — it is
  // the only way to tell "calibrating" from "busy" or a lost maintenance lease.
  const uint8_t node_error_code =
      !ok && frame.payload_length >= 2 ? frame.payload[1] : 0;
  const char* reason = ok ? nullptr : "node_error";
  if (callbacks_.busTrace) {
    callbacks_.busTrace("rx", frame.source, frame.sequence, frame.type,
                        ok ? "ok" : "error", frame.payload,
                        static_cast<uint8_t>(frame.payload_length), node_error_code);
  }

  // Order matters: the trailing kGetRawScan arm is the catch-all that retires a
  // raw slot whenever the accepted-reply arm above it did not match.
  if (ok && pending_type_ == arcade::MessageType::kPollEvents && frame.payload_length >= 1) {
    handlePollEvents(frame, node, now_ms);
  } else if (ok && pending_type_ == arcade::MessageType::kGetRawScan &&
             frame.type == arcade::MessageType::kRawScan) {
    handleRawScan(frame);
  } else if (ok && pending_type_ == arcade::MessageType::kStatus && frame.payload_length >= 7) {
    handleStatus(frame, node, now_ms);
  } else if (ok && pending_type_ == arcade::MessageType::kCalibrate &&
             frame.type == arcade::MessageType::kCalibrationResult) {
    handleCalibrateAccepted(frame, node, now_ms);
  } else if (ok && pending_type_ == arcade::MessageType::kInfo &&
             frame.payload_length >= 3) {
    handleInfo(frame, node);
  } else if (ok && pending_type_ == arcade::MessageType::kGetSnapshot &&
             frame.payload_length >= kSnapshotPayloadBytes) {
    handleSnapshot(frame, node);
  } else if (ok && pending_type_ == arcade::MessageType::kFwPreflight &&
             frame.payload_length >= kPreflightPayloadBytes) {
    handlePreflight(frame, now_ms);
  } else if (ok && pending_type_ == arcade::MessageType::kFwEnterBootloader) {
    handleBootloaderEntry(frame, now_ms);
  } else if (pending_type_ == arcade::MessageType::kGetRawScan) {
    handleRawScanRejected(frame, node_error_code, now_ms);
  }
  if (!ok && pending_type_ == arcade::MessageType::kFwPrepare) purgeQueuedFirmwareCommands();
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
    callbacks_.commandComplete(pending_correlation_, ok, reason, frame.source,
                               node_error_code);
  }
}

void BusManager::handlePollEvents(const arcade::Frame& frame, QuadrantState& node,
                                  uint32_t now_ms) {
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
}

void BusManager::handleRawScan(const arcade::Frame& frame) {
  parseRaw(frame.source, frame);
  raw_done_mask_ |= 1U << frame.source;
  raw_response_mask_ |= 1U << frame.source;
  finishRawIfReady();
}

void BusManager::handleStatus(const arcade::Frame& frame, QuadrantState& node,
                              uint32_t now_ms) {
  const uint32_t uptime_ms = arcade::getU32(frame.payload);
  const bool rebooted = node.last_uptime_ms && uptime_ms < node.last_uptime_ms;
  bool status_changed = false;
  // An uptime step backwards means the node rebooted mid-watch; its restored
  // EEPROM flag (and phase code 2) must not masquerade as a fresh completion.
  if (node.calibration_watch && rebooted) {
    node.calibration_watch = false;
    if (callbacks_.calibrationResult) {
      callbacks_.calibrationResult(frame.source, false, "node_reset");
    }
  }
  node.last_uptime_ms = uptime_ms;
  node.reset_cause = frame.payload[4];
  const bool calibrated = frame.payload[5] != 0;
  if (node.calibrated != calibrated) {
    node.calibrated = calibrated;
    status_changed = true;
  }
  // The extended block is a separate guard: pre-extension firmware stops at
  // byte 6 and would otherwise publish whatever follows in the frame buffer.
  if (frame.payload_length >= 17) {
    node.status_extended = true;
    node.event_depth = frame.payload[6];
    node.last_scan_ms = arcade::getU16(frame.payload + 7);
    const uint16_t rx_good = arcade::getU16(frame.payload + 9);
    const uint16_t rx_bad = arcade::getU16(frame.payload + 11);
    const uint16_t event_overflow = arcade::getU16(frame.payload + 13);
    node.supply_mv = arcade::getU16(frame.payload + 15);
    // Only growth is news: a reboot resets these saturating counters, and
    // "differs" would then report the drop back to zero as a fresh fault.
    if (rx_bad > node.rx_bad || event_overflow > node.event_overflow) {
      status_changed = true;
    }
    node.rx_good = rx_good;
    node.rx_bad = rx_bad;
    node.event_overflow = event_overflow;
  }
  if (rebooted) {
    // Outside a calibration watch this used to go unnoticed: the node kept
    // online, kept its stale cached squares, and ran on defaults for the rest
    // of the session because orientation and runtime mode were never re-sent.
    if (node.reboots != UINT16_MAX) ++node.reboots;
    poll_count_[frame.source] = 0;
    node.needs_sync = !queueNodeSync(frame.source);
    status_changed = true;
    Serial.printf("[%10u][W][BUS] node=%u rebooted uptime_ms=%u reboots=%u\n",
                  now_ms, frame.source, uptime_ms, node.reboots);
    logf("warn", "bus", frame.source, "node rebooted (reboots=%u); resyncing",
         node.reboots);
  }
  if (status_changed && callbacks_.nodeStatusChanged) {
    callbacks_.nodeStatusChanged(frame.source);
  }
  if (frame.payload_length >= 19) {
    updateCalibration(frame.source, frame.payload[17], frame.payload[18], now_ms);
  } else if (node.calibration_watch &&
             static_cast<int32_t>(now_ms - (node.calibration_started_ms +
                                            kLegacyCalibrationSettleMs)) >= 0) {
    node.calibration_watch = false;
    if (callbacks_.calibrationResult) {
      callbacks_.calibrationResult(frame.source, calibrated,
                                   calibrated ? nullptr : "not_calibrated");
    }
  }
}

// Calibration accepted; completion arrives via the status poll.
void BusManager::handleCalibrateAccepted(const arcade::Frame& frame, QuadrantState& node,
                                         uint32_t now_ms) {
  node.calibration_watch = true;
  node.calibration_started_ms = now_ms;
  node.calibration_deadline_ms = now_ms + kCalibrationWatchMs;
  node.cal_percent = 0;
  poll_count_[frame.source] = 0;
  if (callbacks_.calibrationProgress) callbacks_.calibrationProgress(frame.source, 0);
}

void BusManager::handleInfo(const arcade::Frame& frame, QuadrantState& node) {
  const bool changed = !node.fw_known ||
      memcmp(node.fw_version, frame.payload, sizeof(node.fw_version)) != 0;
  memcpy(node.fw_version, frame.payload, sizeof(node.fw_version));
  node.fw_known = true;
  if (changed && callbacks_.nodeStatusChanged) callbacks_.nodeStatusChanged(frame.source);
}

void BusManager::handleSnapshot(const arcade::Frame& frame, QuadrantState& node) {
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
    const auto state = static_cast<arcade::SensorState>(frame.payload[i]);
    const uint16_t raw =
        arcade::getU16(frame.payload + kSnapshotRawOffset + i * sizeof(uint16_t));
    // A silent overwrite left the ESP right and every tier above it wrong
    // forever; a non-zero repair count is the proof that events are being lost.
    const bool repaired = state != node.state[i];
    node.state[i] = state;
    node.raw[i] = raw;
    if (repaired) {
      ++snapshot_repairs_;
      if (callbacks_.sensorChanged) callbacks_.sensorChanged(
          globalSquare(frame.source, i), state, raw, frame.source, i);
    }
  }
}

void BusManager::handlePreflight(const arcade::Frame& frame, uint32_t now_ms) {
  Serial.printf("[%10u][I][FW] node=%u hfuse=0x%02x boot=%u handoff_v=%u page=%u flash=%u app_limit=%u marker=%u reset=0x%02x avcc=%u last_broadcast_refusal=%u\n",
                now_ms, frame.source, frame.payload[kPreflightFuseOffset],
                frame.payload[kPreflightBootloaderOffset],
                frame.payload[kPreflightProtocolOffset],
                arcade::getU16(frame.payload + kPreflightPageSizeOffset),
                arcade::getU32(frame.payload + kPreflightFlashSizeOffset),
                arcade::getU32(frame.payload + kPreflightApplicationLimitOffset),
                frame.payload[kPreflightStateOffset],
                frame.payload[kPreflightResetCauseOffset],
                arcade::getU16(frame.payload + kPreflightSupplyMvOffset),
                frame.payload_length > kPreflightBroadcastRefusalOffset
                    ? frame.payload[kPreflightBroadcastRefusalOffset] : 0);
}

void BusManager::handleBootloaderEntry(const arcade::Frame& frame, uint32_t now_ms) {
  programming_handoff_ = true;
  Serial.printf("[%10u][I][FW] target=%u ACKed bootloader entry; framed polling stopped\n",
                now_ms, frame.source);
}

// Error (busy/unsupported) or unexpected-type reply: the node's slot must
// still complete, or the scheduler retries this node forever and starves
// every other poll on the bus.
void BusManager::handleRawScanRejected(const arcade::Frame& frame,
                                       uint8_t node_error_code, uint32_t now_ms) {
  Serial.printf("[%10u][W][RAW] node=%u rejected raw scan type=0x%02x code=%u\n",
                now_ms, frame.source, static_cast<unsigned>(frame.type),
                frame.payload_length >= 2 ? frame.payload[1] : 0);
  logf("warn", "bus", frame.source, "raw scan rejected code=%u", node_error_code);
  raw_done_mask_ |= 1U << frame.source;
  finishRawIfReady();
}

// Drop only queued firmware-handoff commands; unrelated commands (and their
// correlations) must still run to completion.
void BusManager::purgeQueuedFirmwareCommands() {
  const uint8_t count = queue_count_;
  uint8_t read = queue_tail_;
  uint8_t write = queue_tail_;
  queue_count_ = 0;
  for (uint8_t i = 0; i < count; ++i) {
    const QueuedCommand entry = queue_[read];
    read = static_cast<uint8_t>((read + 1) % kQueueCapacity);
    if (entry.type == arcade::MessageType::kFwPrepare ||
        entry.type == arcade::MessageType::kFwEnterBootloader) continue;
    queue_[write] = entry;
    write = static_cast<uint8_t>((write + 1) % kQueueCapacity);
    ++queue_count_;
  }
  queue_head_ = write;
}

void BusManager::parseRaw(uint8_t index, const arcade::Frame& frame) {
  if (frame.payload_length < kRawPayloadBytes) return;
  QuadrantState& node = nodes_[index];
  node.measured_avcc_mv = arcade::getU16(frame.payload + 1);
  uint8_t offset = kRawHeaderBytes;
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
    node.raw[i] = arcade::getU16(frame.payload + offset); offset += 2;
    node.baseline[i] = arcade::getU16(frame.payload + offset); offset += 2;
    node.noise[i] = frame.payload[offset++];
    node.state[i] = static_cast<arcade::SensorState>(frame.payload[offset++]);
  }
  node.raw_valid = true;
}

// Maps the node's calibration phase codes (0 never, 1 sampling, 2 ok, 3 failed)
// onto progress/result callbacks. Results fire only while a watch is armed so a
// reboot-time "2" from EEPROM never replays as a fresh completion.
void BusManager::updateCalibration(uint8_t index, uint8_t phase, uint8_t percent,
                                   uint32_t now_ms) {
  QuadrantState& node = nodes_[index];
  const uint8_t previous_phase = node.cal_phase;
  const uint8_t previous_percent = node.cal_percent;
  node.cal_phase = phase;
  node.cal_percent = percent;
  if (phase == 1) {
    // Adopt a run this ESP did not arm — a lost kCalibrate ACK, a restart inside
    // the ~2 s run, or another client's request. Publishing progress without a
    // watch would tell the UI "calibrating" and then discard the terminal phase
    // below, leaving it latched with its per-node calibrate button disabled.
    const bool adopted = !node.calibration_watch;
    if (adopted) {
      node.calibration_watch = true;
      node.calibration_started_ms = now_ms;
      node.calibration_deadline_ms = now_ms + kCalibrationWatchMs;
      poll_count_[index] = 0;
    }
    if ((adopted || percent != previous_percent) && callbacks_.calibrationProgress) {
      callbacks_.calibrationProgress(index, percent);
    }
    return;
  }
  if (!node.calibration_watch) return;
  if (phase == 2 || phase == 3) {
    node.calibration_watch = false;
    if (callbacks_.calibrationResult) {
      callbacks_.calibrationResult(index, phase == 2,
                                   phase == 2 ? nullptr : "noise_or_baseline_out_of_range");
    }
  } else if (previous_phase == 1) {
    // Sampling stopped without a result code — treat as cancelled.
    node.calibration_watch = false;
    if (callbacks_.calibrationResult) callbacks_.calibrationResult(index, false, "cancelled");
  }
}

void BusManager::finishRawIfReady() {
  // A late reply for a node whose slot was already retired by handleTimeout()
  // reaches here after the sweep closed; without this the callback fires twice.
  if (!raw_active_) return;
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
    callbacks_.commandComplete(raw_correlation_, complete,
                               complete ? nullptr : "partial_scan",
                               arcade::kInvalidNodeAddress, 0);
  }
}
