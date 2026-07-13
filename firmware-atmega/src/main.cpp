#include <Arduino.h>
#include <avr/wdt.h>

#include "diagnostics.h"
#include "firmware_update.h"
#include "lighting.h"
#include "persistent.h"
#include "protocol_service.h"
#include "sensors.h"

namespace {
quadrant::Identity identity;
quadrant::Settings settings;
quadrant::Sensors sensors(settings);
quadrant::Lighting lighting(settings, sensors);
quadrant::FirmwareUpdate firmware_update(identity, sensors, lighting);
quadrant::ProtocolService protocol(identity, settings, sensors, lighting, firmware_update);
quadrant::Diagnostics diagnostics(identity, settings, sensors);
}

void setup() {
  if (!quadrant::loadIdentity(identity)) identity.node_id = 0xff;
  quadrant::loadSettings(settings);
#ifdef ARCADE_STANDALONE_ASCII_DEBUG
  settings.runtime_mode = arcade::RuntimeMode::kBringup;
  diagnostics.begin();
#else
  protocol.begin();
#endif
  sensors.begin();
  lighting.begin();
  firmware_update.begin();
  wdt_enable(WDTO_2S);
}

void loop() {
  const uint32_t now_ms = millis();
  sensors.tick(micros(), now_ms);
  lighting.tick(now_ms);
#ifdef ARCADE_STANDALONE_ASCII_DEBUG
  diagnostics.tick(now_ms);
#else
  protocol.tick();
  firmware_update.tick(now_ms);
#endif
  wdt_reset();
}
