#pragma once

#include <Arduino.h>
#include <arcade_protocol/protocol.h>

#include "bringup_config.h"

// Every square index the firmware exchanges with the ESP — events, snapshots,
// raw scans, SET_SQUARES masks — is geometric: row-major inside the quadrant,
// row 0 along the LED-bar edge, column 0 on the left. That is what
// BusManager::globalSquare() already assumes when it rotates a quadrant into
// board coordinates. The tables below are the only place the hardware's own
// order is allowed to leak in; they come from docs/TeamTwo_ArcadeChess.pdf
// sheet 2 together with the assembled placement.
namespace quadrant {
namespace board_map {

// Smart-square modules are placed in a boustrophedon, so SSM5 sits directly
// below SSM4 and the daisy chain never crosses back over the board:
//   row 0   SSM1   SSM2   SSM3   SSM4
//   row 1   SSM8   SSM7   SSM6   SSM5
//   row 2   SSM9   SSM10  SSM11  SSM12
//   row 3   SSM16  SSM15  SSM14  SSM13
// Indexed by geometric square, yielding the zero-based module number.
constexpr uint8_t kModuleForSquare[arcade::kSquaresPerQuadrant] = {
    0,  1,  2,  3,
    7,  6,  5,  4,
    8,  9,  10, 11,
    15, 14, 13, 12,
};

// SDI_P feeds SSM1 and every DO_P chains to the next module in designator
// order, two pixels per module (LED1 then LED2). SDI_S runs the same order over
// LED3 and LED4, so one offset addresses both strips.
constexpr uint8_t firstPixelForSquare(uint8_t square) {
  return static_cast<uint8_t>(kModuleForSquare[square] * bringup::kPixelsPerSquare);
}

// SENSEn belongs to SSMn, but mux B1 (SENSE1-8 into ADC0) is not wired in
// designator order: Y0..Y3 carry SENSE2, SENSE3, SENSE4, SENSE1. B2 (SENSE9-16
// into ADC1) is in order. These fold that wiring and the placement above into a
// single lookup, so a scan slot lands straight in the square it measures.
constexpr uint8_t kSquareForLowChannel[bringup::kMuxChannelCount] = {
    1, 2, 3, 0, 7, 6, 5, 4};
constexpr uint8_t kSquareForHighChannel[bringup::kMuxChannelCount] = {
    8, 9, 10, 11, 15, 14, 13, 12};

// The half bar chains LED5, LED6, LED7, LED9, LED8, LED10, LED11, LED12 while
// the parts sit in designator order along the bar, so pixels 3 and 4 are
// transposed with respect to physical position.
constexpr uint8_t kEdgePixelForPosition[bringup::kEdgeStripPixels] = {
    0, 1, 2, 4, 3, 5, 6, 7};

// A transposed digit above would mismap a square silently, so prove at compile
// time that each table is a permutation: n entries covering n distinct bits
// cannot contain a duplicate.
constexpr uint16_t coveredBits(const uint8_t* table, uint8_t count) {
  return count ? static_cast<uint16_t>((1U << table[count - 1]) |
                                       coveredBits(table, static_cast<uint8_t>(count - 1)))
               : 0;
}
static_assert(coveredBits(kModuleForSquare, arcade::kSquaresPerQuadrant) == 0xffff,
              "every square must map to exactly one smart-square module");
static_assert((coveredBits(kSquareForLowChannel, bringup::kMuxChannelCount) |
               coveredBits(kSquareForHighChannel, bringup::kMuxChannelCount)) == 0xffff,
              "the two mux scan orders must cover every square exactly once");
static_assert(coveredBits(kEdgePixelForPosition, bringup::kEdgeStripPixels) == 0xff,
              "every edge-bar position must map to exactly one pixel");

}  // namespace board_map
}  // namespace quadrant
