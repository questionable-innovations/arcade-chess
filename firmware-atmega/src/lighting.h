#pragma once

#include <Arduino.h>
#include <FastLED.h>

#include "fastled_atmega328pb.h"
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
  // Per-pixel colour for one edge half-bar. `zone` 1 is bar A (PE0), 2 is bar B
  // (PE1); `colours` holds popcount(mask) RGB565 values in ascending *position*
  // order, and board_map::kEdgePixelForPosition folds in the LED9/LED8
  // transposition so a caller never has to know the chain order. A zero mask
  // hands the strip back to its default solid white.
  //
  // The colours land straight in the FastLED buffers the strips already own and
  // render() stops repainting a bar that has been written, so the whole feature
  // costs two bytes of SRAM. On a part with 500 bytes between `_end` and the
  // 480-byte stack guard, that is the difference between shipping this and not:
  // a shadow buffer would be 32 bytes for the bars alone, and per-square RGB a
  // further 34 on top. See docs/uart-api.md.
  static constexpr uint8_t kZoneSquares = 0;
  static constexpr uint8_t kZoneBarA = 1;
  static constexpr uint8_t kZoneBarB = 2;
  bool setPixels(uint8_t zone, uint16_t mask, const uint8_t* colours, uint8_t length);
  void clear(uint16_t mask);
  void identify(uint16_t duration_ms, uint32_t now_ms);
  void setBrightness(uint8_t brightness);
  void shutdownNow();
  void noteBusActivity(uint32_t now_ms) { last_bus_activity_ms_ = now_ms; }
  // Blank-EEPROM nodes are silent on the bus and so indistinguishable from an
  // absent one; blink instead of pretending to be a working quadrant.
  void markUnprovisioned() { unprovisioned_ = true; }
  uint16_t overrideMask() const { return override_mask_; }

 private:
  CRGB rgb565(uint16_t value) const;
  void setSquare(uint8_t square, const CRGB& colour);
  void render(uint32_t now_ms);
  void renderIdle(uint32_t now_ms);
  void renderUnprovisioned(uint32_t now_ms);
  bool busIdle(uint32_t now_ms) const;

  Settings& settings_;
  Sensors& sensors_;
  CRGB primary_[bringup::kSquareStripPixels]{};
  CRGB secondary_[bringup::kSquareStripPixels]{};
  CRGB edge_a_[bringup::kEdgeStripPixels]{};
  CRGB edge_b_[bringup::kEdgeStripPixels]{};
  uint16_t override_mask_ = 0;
  uint8_t override_red_ = 0;
  uint8_t override_green_ = 0;
  uint8_t override_blue_ = 0;
  // The two edge half-bars hold a written frame rather than the default white.
  bool bar_written_[2]{};
  uint32_t override_until_ms_ = 0;
  uint32_t identify_until_ms_ = 0;
  uint32_t next_frame_ms_ = 0;
  uint32_t last_bus_activity_ms_ = 0;
  bool render_requested_ = false;
  bool unprovisioned_ = false;
};

}  // namespace quadrant
