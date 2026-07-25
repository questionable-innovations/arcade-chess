#include "lighting.h"

#include "bringup_config.h"

namespace quadrant {
namespace {

// Sweeps a fading comet head over one strip so every pixel and channel gets
// exercised in turn.
void attractComet(CRGB* pixels, uint8_t count, uint8_t head, uint8_t hue) {
  fill_solid(pixels, count, CRGB::Black);
  for (uint8_t tail = 0; tail <= bringup::kAttractCometTail; ++tail) {
    const uint8_t index = static_cast<uint8_t>((head + count - tail) % count);
    pixels[index] = CHSV(static_cast<uint8_t>(hue + index * 8), 255,
                         static_cast<uint8_t>(255U >> tail));
  }
}

}  // namespace

Lighting::Lighting(Settings& settings, Sensors& sensors)
    : settings_(settings), sensors_(sensors) {}

void Lighting::begin() {
  FastLED.addLeds<WS2812B, bringup::kLedPrimary, GRB>(primary_, bringup::kSquareStripPixels);
  FastLED.addLeds<WS2812B, bringup::kLedSecondary, GRB>(secondary_, bringup::kSquareStripPixels);
  FastLED.addLeds<WS2812B, bringup::kLedEdgeA, GRB>(edge_a_, bringup::kEdgeStripPixels);
  FastLED.addLeds<WS2812B, bringup::kLedEdgeB, GRB>(edge_b_, bringup::kEdgeStripPixels);
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
  const uint8_t first = static_cast<uint8_t>(square * bringup::kPixelsPerSquare);
  primary_[first] = value;
  primary_[first + 1] = value;
  secondary_[first] = value;
  secondary_[first + 1] = value;
}

void Lighting::render(uint32_t now_ms) {
  fill_solid(primary_, bringup::kSquareStripPixels, CRGB::Black);
  fill_solid(secondary_, bringup::kSquareStripPixels, CRGB::Black);
  fill_solid(edge_a_, bringup::kEdgeStripPixels, CRGB::White);
  fill_solid(edge_b_, bringup::kEdgeStripPixels, CRGB::White);
  const bool identifying = static_cast<int32_t>(identify_until_ms_ - now_ms) > 0;
  for (uint8_t square = 0; square < arcade::kSquaresPerQuadrant; ++square) {
    CRGB value = CRGB::Black;
    if (identifying) {
      if ((now_ms / bringup::kIdentifyBlinkMs) & 1U) {
        value = CRGB(bringup::kIdentifyRed, bringup::kIdentifyGreen,
                     bringup::kIdentifyBlue);
      }
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
    const CRGB identify_colour(bringup::kIdentifyRed, bringup::kIdentifyGreen,
                               bringup::kIdentifyBlue);
    fill_solid(edge_a_, bringup::kEdgeStripPixels, identify_colour);
    fill_solid(edge_b_, bringup::kEdgeStripPixels, identify_colour);
  }
  // These calls mask interrupts for roughly 2.4 ms total. They run only after an
  // ESP render-window broadcast, while the shared bus is intentionally idle.
  FastLED.show();
}

bool Lighting::attractActive(uint32_t now_ms) const {
  return static_cast<int32_t>(now_ms - last_bus_activity_ms_) >=
         static_cast<int32_t>(bringup::kBusIdleAttractMs);
}

void Lighting::renderAttract() {
  ++attract_step_;
  const uint8_t hue = static_cast<uint8_t>(attract_step_ * bringup::kAttractHueStep);
  // Both strip lengths divide 256, so the uint8_t counter wraps in step.
  const uint8_t square_head =
      static_cast<uint8_t>(attract_step_ % bringup::kSquareStripPixels);
  const uint8_t edge_head =
      static_cast<uint8_t>(attract_step_ % bringup::kEdgeStripPixels);
  // The complementary hue keeps the paired strips visually distinguishable.
  attractComet(primary_, bringup::kSquareStripPixels, square_head, hue);
  attractComet(secondary_, bringup::kSquareStripPixels, square_head,
               static_cast<uint8_t>(hue + 128));
  attractComet(edge_a_, bringup::kEdgeStripPixels, edge_head, hue);
  attractComet(edge_b_, bringup::kEdgeStripPixels, edge_head,
               static_cast<uint8_t>(hue + 128));
  // Overlay live sensor hits so squares stay testable without an ESP attached.
  for (uint8_t square = 0; square < arcade::kSquaresPerQuadrant; ++square) {
    if (sensors_.state(square) == arcade::SensorState::kPositive) {
      setSquare(square, rgb565(settings_.positive_rgb565));
    } else if (sensors_.state(square) == arcade::SensorState::kNegative) {
      setSquare(square, rgb565(settings_.negative_rgb565));
    }
  }
  // Safe to mask interrupts here: the bus has been silent for seconds, and a
  // frame arriving mid-show only costs the ESP one retry before attract stops.
  FastLED.show();
}

void Lighting::tick(uint32_t now_ms) {
  if (override_until_ms_ && static_cast<int32_t>(now_ms - override_until_ms_) >= 0) {
    override_mask_ = 0;
    override_until_ms_ = 0;
  }
  const bool attract = attractActive(now_ms);
  if (!attract && !render_requested_) return;
  if (static_cast<int32_t>(now_ms - next_frame_ms_) < 0) return;
  render_requested_ = false;
  next_frame_ms_ = now_ms + (1000U / bringup::kLedFramesPerSecond);
  if (attract) {
    renderAttract();
  } else {
    render(now_ms);
  }
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
