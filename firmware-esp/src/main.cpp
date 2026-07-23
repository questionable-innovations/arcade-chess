#include <Arduino.h>
#include <Preferences.h>

#include "app_config.h"
#include "avr_flasher.h"
#include "bus_manager.h"
#include "console.h"
#include "network_manager.h"

namespace {
constexpr uint32_t kDebugSerialBaud = 115200;
constexpr uint16_t kSerialStartupSettleMs = 100;

Preferences preferences;
AppConfig config;
BusManager bus;
NetworkManager network;
AvrFlasher flasher;
Console console;

void sensorChanged(uint8_t square, arcade::SensorState state, uint16_t raw,
                   uint8_t node, uint8_t local) {
  network.publishSensor(square, state, raw, node, local);
}

void rawScanReady(bool complete, uint32_t scan_id) {
  Serial.printf("RAW64 scan=%u complete=%u\n", scan_id, complete);
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    const QuadrantState& quadrant = bus.node(node);
    Serial.printf("node %u online=%u raw_valid=%u:", node, quadrant.online,
                  quadrant.raw_valid);
    if (quadrant.raw_valid) {
      for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
        Serial.printf(" %u", quadrant.raw[i]);
      }
    } else {
      Serial.print(" missing");
    }
    Serial.println();
  }
  network.publishRawScan(complete, scan_id);
}

void commandComplete(const char* id, bool ok, const char* reason) {
  network.commandComplete(id, ok, reason);
}

void nodePresenceChanged(uint8_t node, bool online) {
  Serial.printf("node presence: node=%u online=%u mask=0x%02x\n",
                node, online, bus.onlineMask());
  network.publishNodeStatus(node);
  network.publishSnapshot();
}

void nodeStatusChanged(uint8_t node) {
  network.publishNodeStatus(node);
}

void fwResponse(uint8_t node, arcade::MessageType type, bool ok,
                const uint8_t* payload, uint8_t length) {
  flasher.onFwResponse(node, type, ok, payload, length);
}

void busTrace(const char* direction, uint8_t node, uint8_t sequence,
              arcade::MessageType type, const char* result,
              const uint8_t* payload, uint8_t length) {
  network.publishBusTrace(direction, node, sequence, type, result, payload, length);
}

void calibrationProgress(uint8_t node, uint8_t percent) {
  network.publishCalibrationProgress(node, percent);
}

void calibrationResult(uint8_t node, bool ok, const char* reason) {
  Serial.printf("calibration result: node=%u ok=%u%s%s\n", node, ok,
                reason ? " reason=" : "", reason ? reason : "");
  network.publishCalibrationResult(node, ok, reason);
}
}

void setup() {
  Serial.begin(kDebugSerialBaud);
  delay(kSerialStartupSettleMs);
  Serial.println(F("\nArcade Chess ESP32 firmware 0.1.0"));
  config.load(preferences);
  bus.setRuntimeMode(config.runtime_mode);
  network.setRuntimeMode(config.runtime_mode);
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    bus.setOrientation(node, config.orientation[node]);
  }

  BusCallbacks callbacks;
  callbacks.sensorChanged = sensorChanged;
  callbacks.rawScanReady = rawScanReady;
  callbacks.commandComplete = commandComplete;
  callbacks.nodePresenceChanged = nodePresenceChanged;
  callbacks.nodeStatusChanged = nodeStatusChanged;
  callbacks.fwResponse = fwResponse;
  callbacks.busTrace = busTrace;
  callbacks.calibrationProgress = calibrationProgress;
  callbacks.calibrationResult = calibrationResult;
  bus.begin(Serial2, callbacks);
  flasher.begin(bus, Serial2);
  network.begin(config, bus);
  console.begin(preferences, config, bus, network, flasher);
  Serial.printf("runtime mode: %s\n",
                config.runtime_mode == arcade::RuntimeMode::kBringup ? "bringup" : "normal");
  console.printHelp();
}

void loop() {
  const uint32_t now = millis();
  console.tick();
  bus.tick(now);
  flasher.tick(now);
  network.tick(now);
  delay(1);
}
