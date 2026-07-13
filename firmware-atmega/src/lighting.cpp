#include "lighting.h"

#include "bringup_config.h"

namespace quadrant {

Lighting::Lighting(Settings& settings, Sensors& sensors)
    : settings_(settings), sensors_(sensors),
      primary_(32, bringup::kLedPrimary, NEO_GRB + NEO_KHZ800),
      secondary_(32, bringup::kLedSecondary, NEO_GRB + NEO_KHZ800),
      edge_a_(8, bringup::kLedEdgeA, NEO_GRB + NEO_KHZ800),
      edge_b_(8, bringup::kLedEdgeB, NEO_GRB + NEO_KHZ800) {}

void Lighting::begin() {
  primary_.begin();
  secondary_.begin();
  edge_a_.begin();
  edge_b_.begin();
  setBrightness(settings_.brightness);
  primary_.show();
  secondary_.show();
  edge_a_.show();
  edge_b_.show();
}

uint32_t Lighting::rgb565(uint16_t c) const {
  const uint8_t red = static_cast<uint8_t>(((c >> 11) & 0x1f) * 255 / 31);
  const uint8_t green = static_cast<uint8_t>(((c >> 5) & 0x3f) * 255 / 63);
  const uint8_t blue = static_cast<uint8_t>((c & 0x1f) * 255 / 31);
  return Adafruit_NeoPixel::Color(red, green, blue);
}

void Lighting::setSquare(uint8_t square, uint32_t colour) {
  const uint8_t first = static_cast<uint8_t>(square * 2);
  primary_.setPixelColor(first, colour);
  primary_.setPixelColor(first + 1, colour);
  secondary_.setPixelColor(first, colour);
  secondary_.setPixelColor(first + 1, colour);
}

void Lighting::render(uint32_t now_ms) {
  primary_.clear();
  secondary_.clear();
  edge_a_.clear();
  edge_b_.clear();
  const bool identifying = static_cast<int32_t>(identify_until_ms_ - now_ms) > 0;
  for (uint8_t square = 0; square < 16; ++square) {
    uint32_t colour = 0;
    if (identifying) {
      colour = ((now_ms / 180U) & 1U) ? Adafruit_NeoPixel::Color(255, 72, 0) : 0;
    } else if (override_mask_ & (1U << square)) {
      colour = Adafruit_NeoPixel::Color(override_red_, override_green_, override_blue_);
    } else if (sensors_.state(square) == arcade::SensorState::kPositive) {
      colour = rgb565(settings_.positive_rgb565);
    } else if (sensors_.state(square) == arcade::SensorState::kNegative) {
      colour = rgb565(settings_.negative_rgb565);
    }
    setSquare(square, colour);
  }
  if (identifying) {
    const uint32_t orange = Adafruit_NeoPixel::Color(255, 72, 0);
    for (uint8_t i = 0; i < 8; ++i) {
      edge_a_.setPixelColor(i, orange);
      edge_b_.setPixelColor(i, orange);
    }
  }
  // These calls mask interrupts for roughly 2.4 ms total. They run only after an
  // ESP render-window broadcast, while the shared bus is intentionally idle.
  primary_.show();
  secondary_.show();
  edge_a_.show();
  edge_b_.show();
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
  primary_.setBrightness(brightness);
  secondary_.setBrightness(brightness);
  edge_a_.setBrightness(brightness);
  edge_b_.setBrightness(brightness);
}

void Lighting::shutdownNow() {
  override_mask_ = 0;
  identify_until_ms_ = 0;
  primary_.clear(); secondary_.clear(); edge_a_.clear(); edge_b_.clear();
  primary_.show(); secondary_.show(); edge_a_.show(); edge_b_.show();
}

}  // namespace quadrant
