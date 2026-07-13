#include "bus_manager.h"

namespace {
constexpr uint8_t kRotationMask = 0x03;
constexpr uint8_t kMirrorFlag = 0x04;
constexpr uint8_t kOrientationMask = kRotationMask | kMirrorFlag;
}

uint8_t BusManager::onlineMask() const {
  uint8_t mask = 0;
  for (uint8_t node = 0; node < arcade::kQuadrantCount; ++node) {
    if (nodes_[node].online) mask |= 1U << node;
  }
  return mask;
}

uint8_t BusManager::onlineCount() const {
  uint8_t count = 0;
  uint8_t mask = onlineMask();
  while (mask) { count += mask & 1U; mask >>= 1; }
  return count;
}

void BusManager::setOrientation(uint8_t node, uint8_t orientation) {
  if (node < arcade::kQuadrantCount) orientation_[node] = orientation & kOrientationMask;
}

uint8_t BusManager::globalSquare(uint8_t node, uint8_t local) const {
  uint8_t row = local / arcade::kQuadrantWidth;
  uint8_t col = local % arcade::kQuadrantWidth;
  const uint8_t orientation = orientation_[node] & kOrientationMask;
  if (orientation & kMirrorFlag) col = arcade::kQuadrantWidth - 1 - col;
  const uint8_t old_row = row;
  // Low two orientation bits encode clockwise quarter turns; bit two mirrors
  // the local X axis before rotation.
  switch (orientation & kRotationMask) {
    case 1: row = col; col = arcade::kQuadrantWidth - 1 - old_row; break;
    case 2:
      row = arcade::kQuadrantWidth - 1 - row;
      col = arcade::kQuadrantWidth - 1 - col;
      break;
    case 3: row = arcade::kQuadrantWidth - 1 - col; col = old_row; break;
    default: break;
  }
  const uint8_t base_row = (node / arcade::kQuadrantsPerBoardEdge) * arcade::kQuadrantWidth;
  const uint8_t base_col = (node % arcade::kQuadrantsPerBoardEdge) * arcade::kQuadrantWidth;
  return static_cast<uint8_t>((base_row + row) * arcade::kBoardWidth + base_col + col);
}

bool BusManager::locateGlobal(uint8_t global, uint8_t& node, uint8_t& local) const {
  if (global >= arcade::kBoardSquareCount) return false;
  for (uint8_t candidate_node = 0; candidate_node < arcade::kQuadrantCount;
       ++candidate_node) {
    for (uint8_t candidate_local = 0;
         candidate_local < arcade::kSquaresPerQuadrant; ++candidate_local) {
      if (globalSquare(candidate_node, candidate_local) == global) {
        node = candidate_node;
        local = candidate_local;
        return true;
      }
    }
  }
  return false;
}
