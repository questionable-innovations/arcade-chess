#include <Arduino.h>
#include <Preferences.h>

#include "app_config.h"
#include "bus_manager.h"
#include "console.h"
#include "network_manager.h"

namespace {
Preferences preferences;
AppConfig config;
BusManager bus;
NetworkManager network;
Console console;

void sensorChanged(uint8_t square, arcade::SensorState state, uint16_t raw,
                   uint8_t node, uint8_t local) {
  network.publishSensor(square, state, raw, node, local);
}

void rawScanReady(bool complete, uint32_t scan_id) {
  Serial.printf("RAW64 scan=%u complete=%u\n", scan_id, complete);
  for (uint8_t node = 0; node < 4; ++node) {
    const QuadrantState& quadrant = bus.node(node);
    Serial.printf("node %u:", node);
    for (uint8_t i = 0; i < 16; ++i) Serial.printf(" %u", quadrant.raw[i]);
    Serial.println();
  }
  network.publishRawScan(complete, scan_id);
}

void commandComplete(const char* id, bool ok, const char* reason) {
  network.commandComplete(id, ok, reason);
}
}

void setup() {
  Serial.begin(115200);
  delay(100);
  Serial.println(F("\nArcade Chess ESP32 firmware 0.1.0"));
  config.load(preferences);
  bus.setRuntimeMode(config.runtime_mode);
  network.setRuntimeMode(config.runtime_mode);
  for (uint8_t node = 0; node < 4; ++node) bus.setOrientation(node, config.orientation[node]);

  BusCallbacks callbacks;
  callbacks.sensorChanged = sensorChanged;
  callbacks.rawScanReady = rawScanReady;
  callbacks.commandComplete = commandComplete;
  bus.begin(Serial2, callbacks);
  network.begin(config, bus);
  console.begin(preferences, config, bus, network);
  Serial.printf("runtime mode: %s\n",
                config.runtime_mode == arcade::RuntimeMode::kBringup ? "bringup" : "normal");
  console.printHelp();
}

void loop() {
  const uint32_t now = millis();
  console.tick();
  bus.tick(now);
  network.tick(now);
  delay(1);
}
