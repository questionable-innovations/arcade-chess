#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

namespace quadrant {

constexpr uint16_t kIdentityEepromAddress = 0;
constexpr uint16_t kSettingsEepromAddress = 16;
constexpr uint16_t kUpdateMarkerAEepromAddress = 128;
constexpr uint16_t kUpdateMarkerBEepromAddress = 160;

struct __attribute__((packed)) Identity {
  uint32_t magic;
  uint8_t version;
  uint8_t node_id;
  uint16_t crc;
};

struct __attribute__((packed)) Settings {
  uint32_t magic;
  uint8_t version;
  uint16_t enter_threshold;
  uint16_t exit_threshold;
  uint8_t debounce_scans;
  uint16_t mux_settle_us;
  uint16_t full_scan_ms;
  uint8_t brightness;
  uint16_t positive_rgb565;
  uint16_t negative_rgb565;
  uint8_t orientation;
  uint16_t baseline[arcade::kSquaresPerQuadrant];
  uint8_t noise[arcade::kSquaresPerQuadrant];
  arcade::RuntimeMode runtime_mode;
  uint8_t calibrated;
  uint16_t crc;
};

enum class UpdateState : uint8_t {
  kNone = 0,
  kRequested = 1,
  kProgramming = 2,
  kCandidate = 3,
  kValid = 4,
};

struct __attribute__((packed)) UpdateMarker {
  uint32_t magic;
  uint8_t version;
  uint8_t generation;
  UpdateState state;
  uint8_t node_id;
  uint32_t token;
  uint32_t update_id;
  uint32_t image_size;
  uint32_t image_crc32;
  uint16_t crc;
};

static_assert(sizeof(UpdateMarker) <= 32, "update marker must fit one EEPROM slot");
static_assert(kSettingsEepromAddress + sizeof(Settings) <= kUpdateMarkerAEepromAddress,
              "settings and update marker EEPROM regions overlap");

uint16_t storageCrc(const uint8_t* bytes, size_t length);
bool loadIdentity(Identity& identity);
bool loadSettings(Settings& settings);
void saveSettings(Settings& settings);
void loadDefaultSettings(Settings& settings);
bool loadUpdateMarker(UpdateMarker& marker);
void saveUpdateMarker(UpdateMarker& marker);

}  // namespace quadrant
