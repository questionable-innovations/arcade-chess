#include "lighting.h"

#include "board_map.h"
#include "bringup_config.h"

namespace quadrant {
namespace {
constexpr uint16_t kFrameIntervalMs = 1000U / bringup::kLedFramesPerSecond;

// One amber says "look at me" for both attention states: the addressed IDENTIFY
// and the unprovisioned blink.
CRGB identifyColour() {
  return CRGB(bringup::kIdentifyRed, bringup::kIdentifyGreen, bringup::kIdentifyBlue);
}
}

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
  const uint8_t first = board_map::firstPixelForSquare(square);
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
      if ((now_ms / bringup::kIdentifyBlinkMs) & 1U) value = identifyColour();
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
    fill_solid(edge_a_, bringup::kEdgeStripPixels, identifyColour());
    fill_solid(edge_b_, bringup::kEdgeStripPixels, identifyColour());
  }
  // These calls mask interrupts for roughly 2.4 ms total. They run only after an
  // ESP render-window broadcast, while the shared bus is intentionally idle.
  FastLED.show();
}

bool Lighting::busIdle(uint32_t now_ms) const {
  return static_cast<int32_t>(now_ms - last_bus_activity_ms_) >=
         static_cast<int32_t>(bringup::kBusIdleTimeoutMs);
}

void Lighting::renderIdle(uint32_t now_ms) {
  const uint8_t phase = static_cast<uint8_t>(
      (now_ms % bringup::kIdleBreathMs) * 256UL / bringup::kIdleBreathMs);
  const uint8_t value = map8(sin8(phase), bringup::kIdleMinimumValue,
                             bringup::kIdleMaximumValue);
  fill_solid(primary_, bringup::kSquareStripPixels,
             CHSV(bringup::kIdleHuePrimary, 255, value));
  fill_solid(secondary_, bringup::kSquareStripPixels,
             CHSV(bringup::kIdleHueSecondary, 255, value));
  fill_solid(edge_a_, bringup::kEdgeStripPixels,
             CHSV(bringup::kIdleHueEdgeA, 255, value));
  fill_solid(edge_b_, bringup::kEdgeStripPixels,
             CHSV(bringup::kIdleHueEdgeB, 255, value));
  // Settled pieces punch through the breath at full value, so squares stay
  // testable with no ESP attached.
  for (uint8_t square = 0; square < arcade::kSquaresPerQuadrant; ++square) {
    if (sensors_.state(square) == arcade::SensorState::kPositive) {
      setSquare(square, rgb565(settings_.positive_rgb565));
    } else if (sensors_.state(square) == arcade::SensorState::kNegative) {
      setSquare(square, rgb565(settings_.negative_rgb565));
    }
  }
  // Safe to mask interrupts here: the bus has been silent for seconds, and a
  // frame arriving mid-show only costs the ESP one retry before idle ends.
  FastLED.show();
}

void Lighting::renderUnprovisioned(uint32_t now_ms) {
  const bool on = (now_ms / bringup::kUnprovisionedBlinkMs) & 1U;
  const CRGB value = on ? identifyColour() : CRGB(CRGB::Black);
  fill_solid(primary_, bringup::kSquareStripPixels, value);
  fill_solid(secondary_, bringup::kSquareStripPixels, value);
  fill_solid(edge_a_, bringup::kEdgeStripPixels, value);
  fill_solid(edge_b_, bringup::kEdgeStripPixels, value);
  FastLED.show();
}

void Lighting::tick(uint32_t now_ms) {
  if (override_until_ms_ && static_cast<int32_t>(now_ms - override_until_ms_) >= 0) {
    override_mask_ = 0;
    override_until_ms_ = 0;
  }
  if (unprovisioned_) {
    // This node never transmits, so an unsynchronised shift-out costs nothing.
    if (static_cast<int32_t>(now_ms - next_frame_ms_) < 0) return;
    next_frame_ms_ = now_ms + kFrameIntervalMs;
    renderUnprovisioned(now_ms);
    return;
  }
  if (render_requested_) {
    // The marker is the ESP's 4 ms quiet window and is authoritative. Deferring
    // the interrupts-off shift-out to a local timer running at the same rate
    // would drop alternate windows and push the rest over live bus traffic.
    render_requested_ = false;
    next_frame_ms_ = now_ms + kFrameIntervalMs;
    render(now_ms);
    return;
  }
  if (!busIdle(now_ms)) return;
  if (static_cast<int32_t>(now_ms - next_frame_ms_) < 0) return;
  next_frame_ms_ = now_ms + kFrameIntervalMs;
  renderIdle(now_ms);
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
