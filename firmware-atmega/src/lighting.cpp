#include "lighting.h"

#include "bringup_config.h"

namespace quadrant {

Lighting::Lighting(Settings& settings, Sensors& sensors)
    : settings_(settings), sensors_(sensors) {}

void Lighting::begin() {
  FastLED.addLeds<WS2812B, bringup::kLedPrimary, GRB>(primary_, 32);
  FastLED.addLeds<WS2812B, bringup::kLedSecondary, GRB>(secondary_, 32);
  FastLED.addLeds<WS2812B, bringup::kLedEdgeA, GRB>(edge_a_, 8);
  FastLED.addLeds<WS2812B, bringup::kLedEdgeB, GRB>(edge_b_, 8);
  FastLED.setBrightness(settings_.brightness);
  shutdownNow();
}

CRGB Lighting::rgb565(uint16_t c) const {
  const uint8_t red = static_cast<uint8_t>(((c >> 11) & 0x1f) * 255 / 31);
  const uint8_t green = static_cast<uint8_t>(((c >> 5) & 0x3f) * 255 / 63);
  const uint8_t blue = static_cast<uint8_t>((c & 0x1f) * 255 / 31);
  return CRGB(red, green, blue);
}

void Lighting::setSquare(uint8_t square, const CRGB& value) {
  const uint8_t first = static_cast<uint8_t>(square * 2);
  primary_[first] = value;
  primary_[first + 1] = value;
  secondary_[first] = value;
  secondary_[first + 1] = value;
}

void Lighting::render(uint32_t now_ms) {
  fill_solid(primary_, 32, CRGB::Black);
  fill_solid(secondary_, 32, CRGB::Black);
  fill_solid(edge_a_, 8, CRGB::Black);
  fill_solid(edge_b_, 8, CRGB::Black);
  const bool identifying = static_cast<int32_t>(identify_until_ms_ - now_ms) > 0;
  for (uint8_t square = 0; square < 16; ++square) {
    CRGB value = CRGB::Black;
    if (identifying) {
      if ((now_ms / 180U) & 1U) value = CRGB(255, 72, 0);
    } else if (override_mask_ & (1U << square)) {
      value = CRGB(override_red_, override_green_, override_blue_);
    } else if (sensors_.state(square) == arcade::SensorState::kPositive) {
      value = rgb565(settings_.positive_rgb565);
    } else if (sensors_.state(square) == arcade::SensorState::kNegative) {
      value = rgb565(settings_.negative_rgb565);
    }
    setSquare(square, value);
  }
  if (identifying) {
    fill_solid(edge_a_, 8, CRGB(255, 72, 0));
    fill_solid(edge_b_, 8, CRGB(255, 72, 0));
  }
  // These calls mask interrupts for roughly 2.4 ms total. They run only after an
  // ESP render-window broadcast, while the shared bus is intentionally idle.
  FastLED.show();
}

void Lighting::tick(uint32_t now_ms) {
  if (override_until_ms_ && static_cast<int32_t>(now_ms - override_until_ms_) >= 0) {
    override_mask_ = 0;
    override_until_ms_ = 0;
  }
  if (!render_requested_ || static_cast<int32_t>(now_ms - next_frame_ms_) < 0) return;
  render_requested_ = false;
  next_frame_ms_ = now_ms + (1000U / bringup::kLedFramesPerSecond);
  render(now_ms);
}

void Lighting::setSquares(uint16_t mask, uint8_t red, uint8_t green, uint8_t blue,
                          uint16_t duration_ms, uint32_t now_ms) {
  override_mask_ = mask;
  override_red_ = red;
  override_green_ = green;
  override_blue_ = blue;
  override_until_ms_ = duration_ms ? now_ms + duration_ms : 0;
}

void Lighting::clear(uint16_t mask) { override_mask_ &= static_cast<uint16_t>(~mask); }

void Lighting::identify(uint16_t duration_ms, uint32_t now_ms) {
  identify_until_ms_ = now_ms + duration_ms;
}

void Lighting::setBrightness(uint8_t brightness) {
  settings_.brightness = brightness;
  FastLED.setBrightness(brightness);
}

void Lighting::shutdownNow() {
  override_mask_ = 0;
  identify_until_ms_ = 0;
  FastLED.clear(true);
}

}  // namespace quadrant
