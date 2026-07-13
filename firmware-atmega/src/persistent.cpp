#include "persistent.h"

#include <EEPROM.h>
#include <string.h>

#include "bringup_config.h"

namespace quadrant {
namespace {
constexpr uint32_t kIdentityMagic = 0x51434944UL;  // QCID
constexpr uint32_t kSettingsMagic = 0x51434346UL;  // QCCF
constexpr uint32_t kUpdateMagic = 0x51435550UL;    // QCUP

struct __attribute__((packed)) LegacySettingsV1 {
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
  uint16_t baseline[16];
  uint8_t noise[16];
  uint8_t calibrated;
  uint16_t crc;
};

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
  s.version = 2;
  s.enter_threshold = bringup::kDefaultEnterThreshold;
  s.exit_threshold = bringup::kDefaultExitThreshold;
  s.debounce_scans = bringup::kDefaultDebounceScans;
  s.mux_settle_us = bringup::kDefaultMuxSettleUs;
  s.full_scan_ms = bringup::kDefaultFullScanMs;
  s.brightness = bringup::kDefaultBrightness;
  s.positive_rgb565 = bringup::kDefaultPositiveRgb565;
  s.negative_rgb565 = bringup::kDefaultNegativeRgb565;
  s.runtime_mode = arcade::RuntimeMode::kNormal;
  for (uint8_t i = 0; i < 16; ++i) {
    s.baseline[i] = 512;
    s.noise[i] = 4;
  }
}

bool loadSettings(Settings& settings) {
  EEPROM.get(kSettingsEepromAddress, settings);
  const bool valid = settings.magic == kSettingsMagic && settings.version == 2 &&
      settings.enter_threshold >= 10 && settings.enter_threshold <= 400 &&
      settings.exit_threshold < settings.enter_threshold &&
      settings.debounce_scans >= 1 && settings.debounce_scans <= 20 &&
      static_cast<uint8_t>(settings.runtime_mode) <=
          static_cast<uint8_t>(arcade::RuntimeMode::kBringup) &&
      settings.crc == storageCrc(reinterpret_cast<const uint8_t*>(&settings),
                                 sizeof(settings) - sizeof(settings.crc));
  if (valid) return true;

  LegacySettingsV1 legacy{};
  EEPROM.get(kSettingsEepromAddress, legacy);
  const bool legacy_valid = legacy.magic == kSettingsMagic && legacy.version == 1 &&
      legacy.crc == storageCrc(reinterpret_cast<const uint8_t*>(&legacy),
                               sizeof(legacy) - sizeof(legacy.crc));
  loadDefaultSettings(settings);
  if (!legacy_valid) return false;
  settings.enter_threshold = legacy.enter_threshold;
  settings.exit_threshold = legacy.exit_threshold;
  settings.debounce_scans = legacy.debounce_scans;
  settings.mux_settle_us = legacy.mux_settle_us;
  settings.full_scan_ms = legacy.full_scan_ms;
  settings.brightness = legacy.brightness;
  settings.positive_rgb565 = legacy.positive_rgb565;
  settings.negative_rgb565 = legacy.negative_rgb565;
  settings.orientation = legacy.orientation;
  memcpy(settings.baseline, legacy.baseline, sizeof(settings.baseline));
  memcpy(settings.noise, legacy.noise, sizeof(settings.noise));
  settings.calibrated = legacy.calibrated;
  saveSettings(settings);
  return true;
}

void saveSettings(Settings& settings) {
  settings.magic = kSettingsMagic;
  settings.version = 2;
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
