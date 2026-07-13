#include "application.h"

#include <avr/wdt.h>

namespace quadrant {

Application::Application()
    : sensors_(settings_), lighting_(settings_, sensors_),
      firmware_update_(identity_, sensors_, lighting_),
      protocol_(identity_, settings_, sensors_, lighting_, firmware_update_),
      diagnostics_(identity_, settings_, sensors_) {}

void Application::setup() {
  if (!loadIdentity(identity_)) identity_.node_id = 0xff;
  loadSettings(settings_);
#ifdef ARCADE_STANDALONE_ASCII_DEBUG
  settings_.runtime_mode = arcade::RuntimeMode::kBringup;
  diagnostics_.begin();
#else
  protocol_.begin();
#endif
  sensors_.begin();
  lighting_.begin();
  firmware_update_.begin();
  wdt_enable(WDTO_2S);
}

void Application::loop() {
  const uint32_t now_ms = millis();
  sensors_.tick(micros(), now_ms);
  lighting_.tick(now_ms);
#ifdef ARCADE_STANDALONE_ASCII_DEBUG
  diagnostics_.tick(now_ms);
#else
  protocol_.tick();
  firmware_update_.tick(now_ms);
#endif
  wdt_reset();
}

}  // namespace quadrant
