#pragma once

#include <Arduino.h>
#include <FastLED.h>

#include "persistent.h"
#include "sensors.h"

namespace quadrant {

class Lighting {
 public:
  Lighting(Settings& settings, Sensors& sensors);
  void begin();
  void tick(uint32_t now_ms);
  void requestRender() { render_requested_ = true; }
  void setSquares(uint16_t mask, uint8_t red, uint8_t green, uint8_t blue,
                  uint16_t duration_ms, uint32_t now_ms);
  void clear(uint16_t mask);
  void identify(uint16_t duration_ms, uint32_t now_ms);
  void setBrightness(uint8_t brightness);
  void shutdownNow();
 uint16_t overrideMask() const { return override_mask_; }

 private:
  CRGB rgb565(uint16_t value) const;
  void setSquare(uint8_t square, const CRGB& colour);
  void render(uint32_t now_ms);

  Settings& settings_;
  Sensors& sensors_;
  CRGB primary_[32]{};
  CRGB secondary_[32]{};
  CRGB edge_a_[8]{};
  CRGB edge_b_[8]{};
  uint16_t override_mask_ = 0;
  uint8_t override_red_ = 0;
  uint8_t override_green_ = 0;
  uint8_t override_blue_ = 0;
  uint32_t override_until_ms_ = 0;
  uint32_t identify_until_ms_ = 0;
  uint32_t next_frame_ms_ = 0;
  bool render_requested_ = false;
};

}  // namespace quadrant
