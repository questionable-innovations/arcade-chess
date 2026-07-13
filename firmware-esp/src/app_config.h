#pragma once

#include <Arduino.h>
#include <Preferences.h>
#include <arcade_protocol/protocol.h>

struct AppConfig {
  String wifi_ssid;
  String wifi_password;
  String device_id = "arcade-chess-dev";
  String bearer_token;
  String websocket_host = "chess-be.qinnovate.nz";
  uint16_t websocket_port = 443;
  String websocket_path = "/board";
  uint8_t orientation[4]{};
  arcade::RuntimeMode runtime_mode = arcade::RuntimeMode::kNormal;

  void load(Preferences& preferences);
  void save(Preferences& preferences) const;
};
