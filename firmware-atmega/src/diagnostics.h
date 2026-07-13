#pragma once

#include <Arduino.h>

#include "persistent.h"
#include "sensors.h"

namespace quadrant {

class Diagnostics {
 public:
  Diagnostics(const Identity& identity, const Settings& settings,
              const Sensors& sensors)
      : identity_(identity), settings_(settings), sensors_(sensors) {}

  void begin();
  void tick(uint32_t now_ms);

 private:
  const Identity& identity_;
  const Settings& settings_;
  const Sensors& sensors_;
  uint32_t next_dump_ms_ = 0;
};

}  // namespace quadrant
