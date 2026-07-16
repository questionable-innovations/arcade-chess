#include "console.h"

#include <string.h>

void Console::begin(Preferences& preferences, AppConfig& config, BusManager& bus,
                    NetworkManager& network, AvrFlasher& flasher) {
  preferences_ = &preferences;
  config_ = &config;
  bus_ = &bus;
  network_ = &network;
  flasher_ = &flasher;
}

void Console::printHelp() const {
  Serial.println(F(
    "Commands:\n"
    "  status | nodes | mode [normal|bringup]\n"
    "  raw [samples] | calibrate <0-3|all> | snapshot\n"
    "  identify <node> [ms] | brightness <node> <0-255>\n"
    "  config <node> <key> <value> | wifi <ssid> <password>\n"
    "  fw-preflight <node>\n"
    "  fw-flash <node> (then stream Intel HEX; see tools/flash-quadrant.py)\n"
    "  fw-flash-all (one shared stream; lowest online node responds)\n"
    "  fw-abort | fw-enter <node> <token> <size> <crc32> <update-id> | fw-end <token>\n"
    "  device-id <id> | token <bearer-token> | reboot\n"
    "Config keys: 1 enter, 2 exit, 3 debounce, 4 settle_us, 5 scan_ms,\n"
    "             6 brightness, 7 positive_rgb565, 8 negative_rgb565,\n"
    "             9 orientation, 10 runtime_mode"));
}

void Console::setMode(const char* value) {
  if (!value) {
    Serial.printf("mode=%s\n", config_->runtime_mode == arcade::RuntimeMode::kBringup
        ? "bringup" : "normal");
    return;
  }
  arcade::RuntimeMode mode;
  if (!strcmp(value, "normal")) mode = arcade::RuntimeMode::kNormal;
  else if (!strcmp(value, "bringup")) mode = arcade::RuntimeMode::kBringup;
  else { Serial.println(F("mode must be normal or bringup")); return; }
  config_->runtime_mode = mode;
  config_->save(*preferences_);
  bus_->setRuntimeMode(mode);
  network_->setRuntimeMode(mode);
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) if (bus_->isOnline(node)) {
    bus_->setConfig(node, arcade::configKey(arcade::ConfigKey::kRuntimeMode),
                    static_cast<uint8_t>(mode));
  }
  Serial.printf("mode=%s saved; %u online quadrant update(s) queued\n",
                mode == arcade::RuntimeMode::kBringup ? "bringup" : "normal",
                bus_->onlineCount());
}

void Console::execute(char* line) {
  // During a hex upload every line belongs to the flasher except an abort.
  if (flasher_->receiving()) {
    if (!strcmp(line, "fw-abort")) flasher_->abort("user abort");
    else flasher_->consumeLine(line);
    return;
  }
  char* save = nullptr;
  const char* command = strtok_r(line, " ", &save);
  if (!command) return;
  if (!strcmp(command, "help")) printHelp();
  else if (!strcmp(command, "status")) {
    Serial.printf("uptime=%lu heap=%u ws=%u mode=%s bus_good=%u bus_bad=%u timeouts=%u busy=%u\n",
      millis(), ESP.getFreeHeap(), network_->connected(),
      config_->runtime_mode == arcade::RuntimeMode::kBringup ? "bringup" : "normal",
      bus_->goodFrames(), bus_->badFrames(), bus_->timeoutCount(), bus_->busy());
  } else if (!strcmp(command, "nodes")) {
    for (uint8_t i = 0; i < arcade::kQuadrantCount; ++i) {
      const QuadrantState& q = bus_->node(i);
      Serial.printf("node=%u online=%u calibrated=%u last_seen=%u timeouts=%u consecutive=%u sync=%u raw_valid=%u\n",
                    i, q.online, q.calibrated, q.last_seen_ms, q.timeouts,
                    q.consecutive_timeouts, q.needs_sync, q.raw_valid);
    }
  } else if (!strcmp(command, "mode")) setMode(strtok_r(nullptr, " ", &save));
  else if (!strcmp(command, "raw")) {
    const char* value = strtok_r(nullptr, " ", &save);
    Serial.printf("raw request: %s\n", bus_->requestRawScan(value ? atoi(value) : 1)
                  ? "queued" : "rejected (busy or no online quadrants)");
  } else if (!strcmp(command, "calibrate")) {
    const char* value = strtok_r(nullptr, " ", &save);
    if (value && !strcmp(value, "all")) {
      for (uint8_t i = 0; i < arcade::kQuadrantCount; ++i) {
        if (bus_->isOnline(i)) bus_->calibrate(i);
      }
    } else if (value) {
      bus_->calibrate(atoi(value));
    }
  } else if (!strcmp(command, "identify")) {
    const char* node = strtok_r(nullptr, " ", &save);
    const char* duration = strtok_r(nullptr, " ", &save);
    if (node) bus_->identify(atoi(node), duration ? atoi(duration) : 3000);
  } else if (!strcmp(command, "brightness")) {
    const char* node = strtok_r(nullptr, " ", &save);
    const char* value = strtok_r(nullptr, " ", &save);
    if (node && value) bus_->setBrightness(atoi(node), atoi(value));
  } else if (!strcmp(command, "config")) {
    const char* node = strtok_r(nullptr, " ", &save);
    const char* key = strtok_r(nullptr, " ", &save);
    const char* value = strtok_r(nullptr, " ", &save);
    if (node && key && value) {
      const uint8_t n = atoi(node), k = atoi(key); const uint16_t v = atoi(value);
      bus_->setConfig(n, k, v);
      if (n < arcade::kQuadrantCount &&
          k == arcade::configKey(arcade::ConfigKey::kOrientation) && v < 8) {
        config_->orientation[n] = v;
        config_->save(*preferences_);
        bus_->setOrientation(n, v);
      }
    }
  } else if (!strcmp(command, "fw-preflight")) {
    const char* node = strtok_r(nullptr, " ", &save);
    if (node) Serial.printf("firmware preflight: %s\n",
        bus_->firmwarePreflight(strtoul(node, nullptr, 0)) ? "queued" : "rejected");
  } else if (!strcmp(command, "fw-flash")) {
    const char* node = strtok_r(nullptr, " ", &save);
    if (!node) Serial.println(F("usage: fw-flash <node>"));
    else if (!flasher_->start(strtoul(node, nullptr, 0)))
      Serial.println(F("fw-flash rejected (busy, offline, or bad node)"));
  } else if (!strcmp(command, "fw-flash-all")) {
    if (!flasher_->startAll())
      Serial.println(F("fw-flash-all rejected (busy or no online nodes)"));
  } else if (!strcmp(command, "fw-abort")) {
    flasher_->abort("user abort");
  } else if (!strcmp(command, "fw-enter")) {
    const char* node = strtok_r(nullptr, " ", &save);
    const char* token = strtok_r(nullptr, " ", &save);
    const char* size = strtok_r(nullptr, " ", &save);
    const char* crc = strtok_r(nullptr, " ", &save);
    const char* update = strtok_r(nullptr, " ", &save);
    if (node && token && size && crc && update) {
      const bool queued = bus_->beginFirmwareHandoff(
          strtoul(node, nullptr, 0), strtoul(token, nullptr, 0),
          strtoul(update, nullptr, 0), strtoul(size, nullptr, 0),
          strtoul(crc, nullptr, 0));
      Serial.printf("firmware handoff: %s\n", queued ? "queued" : "rejected");
    }
  } else if (!strcmp(command, "fw-end")) {
    const char* token = strtok_r(nullptr, " ", &save);
    if (token) bus_->endFirmwareMaintenance(strtoul(token, nullptr, 0));
  } else if (!strcmp(command, "wifi")) {
    const char* ssid = strtok_r(nullptr, " ", &save);
    const char* pass = strtok_r(nullptr, "", &save);
    if (ssid && pass) {
      config_->wifi_ssid = ssid; config_->wifi_password = pass;
      config_->save(*preferences_); Serial.println(F("saved; reboot to connect"));
    }
  } else if (!strcmp(command, "device-id")) {
    const char* value = strtok_r(nullptr, " ", &save);
    if (value) { config_->device_id = value; config_->save(*preferences_); Serial.println(F("saved")); }
  } else if (!strcmp(command, "token")) {
    const char* value = strtok_r(nullptr, "", &save);
    if (value) { config_->bearer_token = value; config_->save(*preferences_); Serial.println(F("saved")); }
  } else if (!strcmp(command, "snapshot")) network_->publishSnapshot();
  else if (!strcmp(command, "reboot")) ESP.restart();
  else Serial.println(F("unknown command; type help"));
}

void Console::tick() {
  while (Serial.available()) {
    const char c = Serial.read();
    if (c == '\r') continue;
    if (c == '\n') {
      line_[length_] = 0;
      execute(line_);
      length_ = 0;
    } else if (length_ + 1 < sizeof(line_)) {
      line_[length_++] = c;
    }
  }
}
