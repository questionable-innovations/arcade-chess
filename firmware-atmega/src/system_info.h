#pragma once

#include <Arduino.h>

namespace quadrant::system_info {

uint8_t resetCause();
uint16_t supplyMillivolts();
uint8_t highFuse();
bool residentBootloaderEnabled();

}  // namespace quadrant::system_info
