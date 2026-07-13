#include <Arduino.h>
#include <Preferences.h>

#include "app_config.h"
#include "bus_manager.h"
#include "network_manager.h"

namespace {
Preferences preferences;
AppConfig config;
BusManager bus;
NetworkManager network;
char console_line[192]{};
uint8_t console_length = 0;

void sensorChanged(uint8_t square, arcade::SensorState state, uint16_t raw,
                   uint8_t node, uint8_t local) {
  network.publishSensor(square, state, raw, node, local);
}
void rawScanReady(bool complete, uint32_t scan_id) {
  Serial.printf("RAW64 scan=%u complete=%u\n", scan_id, complete);
  for (uint8_t node = 0; node < 4; ++node) {
    const QuadrantState& q = bus.node(node);
    Serial.printf("node %u:", node);
    for (uint8_t i = 0; i < 16; ++i) Serial.printf(" %u", q.raw[i]);
    Serial.println();
  }
  network.publishRawScan(complete, scan_id);
}
void commandComplete(const char* id, bool ok, const char* reason) {
  network.commandComplete(id, ok, reason);
}

void printHelp() {
  Serial.println(F(
    "Commands:\n"
    "  status | nodes | raw [samples] | calibrate <0-3|all>\n"
    "  identify <node> [ms] | brightness <node> <0-255>\n"
    "  config <node> <key> <value> | wifi <ssid> <password>\n"
    "  fw-preflight <node>\n"
    "  fw-enter <node> <token> <size> <crc32> <update-id> | fw-end <token>\n"
    "  device-id <id> | token <bearer-token> | snapshot | reboot\n"
    "Config keys: 1 enter, 2 exit, 3 debounce, 4 settle_us, 5 scan_ms,\n"
    "             6 brightness, 7 positive_rgb565, 8 negative_rgb565, 9 orientation"));
}

void executeConsole(char* line) {
  char* save = nullptr;
  const char* command = strtok_r(line, " ", &save);
  if (!command) return;
  if (!strcmp(command, "help")) printHelp();
  else if (!strcmp(command, "status")) {
    Serial.printf("uptime=%lu heap=%u ws=%u bus_good=%u bus_bad=%u timeouts=%u busy=%u\n",
      millis(), ESP.getFreeHeap(), network.connected(), bus.goodFrames(), bus.badFrames(),
      bus.timeoutCount(), bus.busy());
  } else if (!strcmp(command, "nodes")) {
    for (uint8_t i = 0; i < 4; ++i) {
      const QuadrantState& q = bus.node(i);
      Serial.printf("node=%u online=%u calibrated=%u last_seen=%u timeouts=%u raw_valid=%u\n",
                    i, q.online, q.calibrated, q.last_seen_ms, q.timeouts, q.raw_valid);
    }
  } else if (!strcmp(command, "raw")) {
    const char* value = strtok_r(nullptr, " ", &save);
    Serial.printf("raw request: %s\n", bus.requestRawScan(value ? atoi(value) : 1) ? "queued" : "busy");
  } else if (!strcmp(command, "calibrate")) {
    const char* value = strtok_r(nullptr, " ", &save);
    if (value && !strcmp(value, "all")) for (uint8_t i = 0; i < 4; ++i) bus.calibrate(i);
    else if (value) bus.calibrate(atoi(value));
  } else if (!strcmp(command, "identify")) {
    const char* node = strtok_r(nullptr, " ", &save); const char* duration = strtok_r(nullptr, " ", &save);
    if (node) bus.identify(atoi(node), duration ? atoi(duration) : 3000);
  } else if (!strcmp(command, "brightness")) {
    const char* node = strtok_r(nullptr, " ", &save); const char* value = strtok_r(nullptr, " ", &save);
    if (node && value) bus.setBrightness(atoi(node), atoi(value));
  } else if (!strcmp(command, "config")) {
    const char* node = strtok_r(nullptr, " ", &save); const char* key = strtok_r(nullptr, " ", &save);
    const char* value = strtok_r(nullptr, " ", &save);
    if (node && key && value) {
      const uint8_t n = atoi(node), k = atoi(key); const uint16_t v = atoi(value);
      bus.setConfig(n, k, v);
      if (n < 4 && k == 9 && v < 8) {
        config.orientation[n] = v; config.save(preferences); bus.setOrientation(n, v);
      }
    }
  } else if (!strcmp(command, "fw-preflight")) {
    const char* node = strtok_r(nullptr, " ", &save);
    if (node) Serial.printf("firmware preflight: %s\n",
                            bus.firmwarePreflight(strtoul(node, nullptr, 0)) ? "queued" : "rejected");
  } else if (!strcmp(command, "fw-enter")) {
    const char* node = strtok_r(nullptr, " ", &save);
    const char* token = strtok_r(nullptr, " ", &save);
    const char* size = strtok_r(nullptr, " ", &save);
    const char* crc = strtok_r(nullptr, " ", &save);
    const char* update = strtok_r(nullptr, " ", &save);
    if (node && token && size && crc && update) {
      const bool queued = bus.beginFirmwareHandoff(
          strtoul(node, nullptr, 0), strtoul(token, nullptr, 0),
          strtoul(update, nullptr, 0), strtoul(size, nullptr, 0),
          strtoul(crc, nullptr, 0));
      Serial.printf("firmware handoff: %s\n", queued ? "queued" : "rejected");
    }
  } else if (!strcmp(command, "fw-end")) {
    const char* token = strtok_r(nullptr, " ", &save);
    if (token) bus.endFirmwareMaintenance(strtoul(token, nullptr, 0));
  } else if (!strcmp(command, "wifi")) {
    const char* ssid = strtok_r(nullptr, " ", &save); const char* pass = strtok_r(nullptr, "", &save);
    if (ssid && pass) { config.wifi_ssid = ssid; config.wifi_password = pass; config.save(preferences);
      Serial.println(F("saved; reboot to connect")); }
  } else if (!strcmp(command, "device-id")) {
    const char* value = strtok_r(nullptr, " ", &save);
    if (value) { config.device_id = value; config.save(preferences); Serial.println(F("saved")); }
  } else if (!strcmp(command, "token")) {
    const char* value = strtok_r(nullptr, "", &save);
    if (value) { config.bearer_token = value; config.save(preferences); Serial.println(F("saved")); }
  } else if (!strcmp(command, "snapshot")) network.publishSnapshot();
  else if (!strcmp(command, "reboot")) ESP.restart();
  else Serial.println(F("unknown command; type help"));
}

void serviceConsole() {
  while (Serial.available()) {
    const char c = Serial.read();
    if (c == '\r') continue;
    if (c == '\n') {
      console_line[console_length] = 0; executeConsole(console_line); console_length = 0;
    } else if (console_length + 1 < sizeof(console_line)) console_line[console_length++] = c;
  }
}
}

void setup() {
  Serial.begin(115200);
  delay(100);
  Serial.println(F("\nArcade Chess ESP32 bring-up firmware 0.1.0"));
  config.load(preferences);
  for (uint8_t node = 0; node < 4; ++node) bus.setOrientation(node, config.orientation[node]);
  BusCallbacks callbacks;
  callbacks.sensorChanged = sensorChanged;
  callbacks.rawScanReady = rawScanReady;
  callbacks.commandComplete = commandComplete;
  bus.begin(Serial2, callbacks);
  network.begin(config, bus);
  printHelp();
}

void loop() {
  const uint32_t now = millis();
  serviceConsole();
  bus.tick(now);
  network.tick(now);
  delay(1);
}
