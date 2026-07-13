#pragma once

#include "diagnostics.h"
#include "firmware_update.h"
#include "lighting.h"
#include "persistent.h"
#include "protocol_service.h"
#include "sensors.h"

namespace quadrant {

class Application {
 public:
  Application();
  void setup();
  void loop();

 private:
  Identity identity_{};
  Settings settings_{};
  Sensors sensors_;
  Lighting lighting_;
  FirmwareUpdate firmware_update_;
  ProtocolService protocol_;
  Diagnostics diagnostics_;
};

}  // namespace quadrant
