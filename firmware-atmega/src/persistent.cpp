#include "persistent.h"

#include <EEPROM.h>
#include <string.h>

#include "bringup_config.h"

namespace quadrant {
namespace {
constexpr uint32_t kIdentityMagic = 0x51434944UL;  // QCID
constexpr uint32_t kSettingsMagic = 0x51434346UL;  // QCCF
constexpr uint32_t kUpdateMagic = 0x51435550UL;    // QCUP

bool validUpdateMarker(const UpdateMarker& marker) {
  return marker.magic == kUpdateMagic && marker.version == 1 && marker.node_id < 4 &&
         static_cast<uint8_t>(marker.state) <= static_cast<uint8_t>(UpdateState::kValid) &&
         marker.crc == storageCrc(reinterpret_cast<const uint8_t*>(&marker),
                                  sizeof(marker) - sizeof(marker.crc));
}

bool generationNewer(uint8_t candidate, uint8_t current) {
  return static_cast<int8_t>(candidate - current) > 0;
}
}

uint16_t storageCrc(const uint8_t* bytes, size_t length) {
  uint16_t crc = 0xffff;
  for (size_t i = 0; i < length; ++i) {
    crc ^= static_cast<uint16_t>(bytes[i]) << 8;
    for (uint8_t bit = 0; bit < 8; ++bit) {
      crc = (crc & 0x8000) ? static_cast<uint16_t>((crc << 1) ^ 0x1021)
                           : static_cast<uint16_t>(crc << 1);
    }
  }
  return crc;
}

bool loadIdentity(Identity& identity) {
  EEPROM.get(kIdentityEepromAddress, identity);
  return identity.magic == kIdentityMagic && identity.version == 1 &&
         identity.node_id < 4 &&
         identity.crc == storageCrc(reinterpret_cast<const uint8_t*>(&identity),
                                    sizeof(identity) - sizeof(identity.crc));
}

void loadDefaultSettings(Settings& s) {
  memset(&s, 0, sizeof(s));
  s.magic = kSettingsMagic;
  s.version = 1;
  s.enter_threshold = bringup::kDefaultEnterThreshold;
  s.exit_threshold = bringup::kDefaultExitThreshold;
  s.debounce_scans = bringup::kDefaultDebounceScans;
  s.mux_settle_us = bringup::kDefaultMuxSettleUs;
  s.full_scan_ms = bringup::kDefaultFullScanMs;
  s.brightness = bringup::kDefaultBrightness;
  s.positive_rgb565 = bringup::kDefaultPositiveRgb565;
  s.negative_rgb565 = bringup::kDefaultNegativeRgb565;
  s.runtime_mode = arcade::RuntimeMode::kNormal;
  for (uint8_t i = 0; i < arcade::kSquaresPerQuadrant; ++i) {
    s.baseline[i] = bringup::kDefaultAdcMidpoint;
    s.noise[i] = 4;
  }
}

bool loadSettings(Settings& settings) {
  EEPROM.get(kSettingsEepromAddress, settings);
  const bool valid = settings.magic == kSettingsMagic && settings.version == 1 &&
      settings.enter_threshold >= bringup::kMinimumEnterThreshold &&
      settings.enter_threshold <= bringup::kMaximumEnterThreshold &&
      settings.exit_threshold < settings.enter_threshold &&
      settings.debounce_scans >= bringup::kMinimumDebounceScans &&
      settings.debounce_scans <= bringup::kMaximumDebounceScans &&
      static_cast<uint8_t>(settings.runtime_mode) <=
          static_cast<uint8_t>(arcade::RuntimeMode::kBringup) &&
      settings.crc == storageCrc(reinterpret_cast<const uint8_t*>(&settings),
                                 sizeof(settings) - sizeof(settings.crc));
  if (!valid) loadDefaultSettings(settings);
  return valid;
}

void saveSettings(Settings& settings) {
  settings.magic = kSettingsMagic;
  settings.version = 1;
  settings.crc = storageCrc(reinterpret_cast<const uint8_t*>(&settings),
                            sizeof(settings) - sizeof(settings.crc));
  EEPROM.put(kSettingsEepromAddress, settings);
}

bool loadUpdateMarker(UpdateMarker& marker) {
  UpdateMarker a{}, b{};
  EEPROM.get(kUpdateMarkerAEepromAddress, a);
  EEPROM.get(kUpdateMarkerBEepromAddress, b);
  const bool valid_a = validUpdateMarker(a);
  const bool valid_b = validUpdateMarker(b);
  if (!valid_a && !valid_b) {
    memset(&marker, 0, sizeof(marker));
    return false;
  }
  marker = valid_b && (!valid_a || generationNewer(b.generation, a.generation)) ? b : a;
  return true;
}

void saveUpdateMarker(UpdateMarker& marker) {
  UpdateMarker current{};
  const bool had_current = loadUpdateMarker(current);
  marker.magic = kUpdateMagic;
  marker.version = 1;
  marker.generation = had_current ? static_cast<uint8_t>(current.generation + 1) : 0;
  marker.crc = storageCrc(reinterpret_cast<const uint8_t*>(&marker),
                          sizeof(marker) - sizeof(marker.crc));
  const uint16_t destination = !had_current || current.generation & 1
      ? kUpdateMarkerAEepromAddress : kUpdateMarkerBEepromAddress;
  EEPROM.put(destination, marker);
}

}  // namespace quadrant
