#include "lighting.h"

#include "board_map.h"
#include "bringup_config.h"

namespace quadrant {
namespace {
constexpr uint16_t kFrameIntervalMs = 1000U / bringup::kLedFramesPerSecond;
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

bool Lighting::busIdle(uint32_t now_ms) const {
  return static_cast<int32_t>(now_ms - last_bus_activity_ms_) >=
         static_cast<int32_t>(bringup::kBusIdleTimeoutMs);
}

void Lighting::renderShimmer() {
  auto shimmer = [](CRGB* strip, uint8_t count) {
    for (uint8_t i = 0; i < count; ++i) {
      strip[i] = random8() < bringup::kShimmerOnChance ? CRGB::White : CRGB::Black;
    }
  };
  shimmer(primary_, bringup::kSquareStripPixels);
  shimmer(secondary_, bringup::kSquareStripPixels);
  shimmer(edge_a_, bringup::kEdgeStripPixels);
  shimmer(edge_b_, bringup::kEdgeStripPixels);
  // These calls mask interrupts for roughly 2.4 ms total. They run only after an
  // ESP render-window broadcast, while the shared bus is intentionally idle.
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
    renderShimmer();
    return;
  }
  if (render_requested_) {
    // The marker is the ESP's 4 ms quiet window and is authoritative. Deferring
    // the interrupts-off shift-out to a local timer running at the same rate
    // would drop alternate windows and push the rest over live bus traffic.
    render_requested_ = false;
    next_frame_ms_ = now_ms + kFrameIntervalMs;
    renderShimmer();
    return;
  }
  if (!busIdle(now_ms)) return;
  if (static_cast<int32_t>(now_ms - next_frame_ms_) < 0) return;
  next_frame_ms_ = now_ms + kFrameIntervalMs;
  renderShimmer();
}

void Lighting::setSquares(uint16_t mask, uint8_t red, uint8_t green, uint8_t blue,
                          uint16_t duration_ms, uint32_t now_ms) {
  override_mask_ = mask;
  override_red_ = red;
  override_green_ = green;
  override_blue_ = blue;
  override_until_ms_ = duration_ms ? now_ms + duration_ms : 0;
}

bool Lighting::setPixels(uint8_t zone, uint16_t mask, const uint8_t* colours,
                         uint8_t length) {
  // Squares are deliberately not a zone here: a per-square colour buffer is
  // 34 bytes of SRAM this part does not have (see lighting.h). Answering
  // "unsupported" rather than silently ignoring it is what lets the server
  // discover the capability per quadrant and drop to its one-colour tier.
  if (zone != kZoneBarA && zone != kZoneBarB) return false;
  // A half-bar is eight pixels, so a high mask byte can only be a caller
  // confusing zones with squares.
  if (mask & 0xff00U) return false;
  uint8_t count = 0;
  for (uint8_t bit = 0; bit < bringup::kEdgeStripPixels; ++bit) {
    if (mask & (1U << bit)) ++count;
  }
  if (length != static_cast<uint8_t>(count * 2)) return false;

  const uint8_t half = zone - kZoneBarA;
  CRGB* strip = half ? edge_b_ : edge_a_;
  if (!mask) {
    bar_written_[half] = false;
    return true;
  }
  // Fill first: a partial mask over an unwritten bar would otherwise inherit
  // whatever the last frame happened to leave in the buffer.
  if (!bar_written_[half]) fill_solid(strip, bringup::kEdgeStripPixels, CRGB::Black);
  bar_written_[half] = true;
  uint8_t offset = 0;
  for (uint8_t position = 0; position < bringup::kEdgeStripPixels; ++position) {
    if (!(mask & (1U << position))) continue;
    strip[board_map::kEdgePixelForPosition[position]] =
        rgb565(arcade::getU16(colours + offset));
    offset += 2;
  }
  return true;
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
  bar_written_[0] = false;
  bar_written_[1] = false;
  identify_until_ms_ = 0;
  FastLED.clear(true);
}

}  // namespace quadrant
