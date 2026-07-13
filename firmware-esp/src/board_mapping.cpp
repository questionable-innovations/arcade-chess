#include "bus_manager.h"

uint8_t BusManager::onlineMask() const {
  uint8_t mask = 0;
  for (uint8_t node = 0; node < 4; ++node) if (nodes_[node].online) mask |= 1U << node;
  return mask;
}

uint8_t BusManager::onlineCount() const {
  uint8_t count = 0;
  uint8_t mask = onlineMask();
  while (mask) { count += mask & 1U; mask >>= 1; }
  return count;
}

void BusManager::setOrientation(uint8_t node, uint8_t orientation) {
  if (node < 4) orientation_[node] = orientation & 7;
}

uint8_t BusManager::globalSquare(uint8_t node, uint8_t local) const {
  uint8_t row = local / 4;
  uint8_t col = local % 4;
  const uint8_t orientation = orientation_[node] & 7;
  if (orientation & 4) col = 3 - col;
  const uint8_t old_row = row;
  switch (orientation & 3) {
    case 1: row = col; col = 3 - old_row; break;
    case 2: row = 3 - row; col = 3 - col; break;
    case 3: row = 3 - col; col = old_row; break;
    default: break;
  }
  const uint8_t base_row = (node / 2) * 4;
  const uint8_t base_col = (node % 2) * 4;
  return static_cast<uint8_t>((base_row + row) * 8 + base_col + col);
}

bool BusManager::locateGlobal(uint8_t global, uint8_t& node, uint8_t& local) const {
  if (global >= 64) return false;
  for (uint8_t candidate_node = 0; candidate_node < 4; ++candidate_node) {
    for (uint8_t candidate_local = 0; candidate_local < 16; ++candidate_local) {
      if (globalSquare(candidate_node, candidate_local) == global) {
        node = candidate_node;
        local = candidate_local;
        return true;
      }
    }
  }
  return false;
}
