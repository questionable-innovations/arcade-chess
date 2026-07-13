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
  const bool migrate_old_backend = websocket_host == "chess.qinnovate.nz";
  for (uint8_t i = 0; i < 4; ++i) {
    const String key = "orient" + String(i);
    orientation[i] = p.getUChar(key.c_str(), 0);
  }
  p.end();
  if (migrate_old_backend) {
    websocket_host = "chess-be.qinnovate.nz";
    p.begin("arcade-chess", false);
    p.putString("ws_host", websocket_host);
    p.end();
  }
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
  for (uint8_t i = 0; i < 4; ++i) {
    const String key = "orient" + String(i);
    p.putUChar(key.c_str(), orientation[i]);
  }
  p.end();
}
