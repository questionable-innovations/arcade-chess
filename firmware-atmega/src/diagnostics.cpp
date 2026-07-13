#include "diagnostics.h"

#include "bringup_config.h"

namespace quadrant {

void Diagnostics::begin() { Serial.begin(bringup::kBusBaud); }

void Diagnostics::tick(uint32_t now_ms) {
#ifdef ARCADE_STANDALONE_ASCII_DEBUG
  if (settings_.runtime_mode != arcade::RuntimeMode::kBringup ||
      static_cast<int32_t>(now_ms - next_dump_ms_) < 0) return;
  next_dump_ms_ = now_ms + 250;
  Serial.print(F("RAW,")); Serial.print(identity_.node_id); Serial.print(',');
  Serial.print(now_ms);
  for (uint8_t i = 0; i < 16; ++i) { Serial.print(','); Serial.print(sensors_.raw(i)); }
  Serial.println();
#else
  (void)now_ms;
#endif
}

}  // namespace quadrant
