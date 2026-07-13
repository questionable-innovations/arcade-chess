#include "app_config.h"

void AppConfig::load(Preferences& p) {
  p.begin("arcade-chess", true);
  wifi_ssid = p.getString("wifi_ssid", wifi_ssid);
  wifi_password = p.getString("wifi_pass", wifi_password);
  device_id = p.getString("device_id", device_id);
  bearer_token = p.getString("token", bearer_token);
  websocket_host = p.getString("ws_host", websocket_host);
  websocket_port = p.getUShort("ws_port", websocket_port);
  websocket_path = p.getString("ws_path", websocket_path);
  const uint8_t stored_mode = p.getUChar("mode", 0);
  runtime_mode = stored_mode <= static_cast<uint8_t>(arcade::RuntimeMode::kBringup)
      ? static_cast<arcade::RuntimeMode>(stored_mode) : arcade::RuntimeMode::kNormal;
  for (uint8_t i = 0; i < 4; ++i) {
    const String key = "orient" + String(i);
    orientation[i] = p.getUChar(key.c_str(), 0);
  }
  p.end();
}

void AppConfig::save(Preferences& p) const {
  p.begin("arcade-chess", false);
  p.putString("wifi_ssid", wifi_ssid);
  p.putString("wifi_pass", wifi_password);
  p.putString("device_id", device_id);
  p.putString("token", bearer_token);
  p.putString("ws_host", websocket_host);
  p.putUShort("ws_port", websocket_port);
  p.putString("ws_path", websocket_path);
  p.putUChar("mode", static_cast<uint8_t>(runtime_mode));
  for (uint8_t i = 0; i < 4; ++i) {
    const String key = "orient" + String(i);
    p.putUChar(key.c_str(), orientation[i]);
  }
  p.end();
}
