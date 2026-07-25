#pragma once

#include <stdint.h>

enum MessageType : uint8_t {
  STATE = 0,                  // ESP Constantly sends this to modules
  SEGMENT_REQUEST = 1,        // ESP sends this to modules to request segment data
  SEGMENT_RESPONSE = 2,       // Module response to SEGMENT_REQUEST 
  START_ANIMATION = 3,        // ESP sends this to modules to start an animation
  REQUEST_CLOCK_BUTTONS = 4,  // ESP sends this to clock module to request turn data
  DISPLAY_CLOCK_MESSAGE = 5,  // ESP sends this to clock module to set the clock display message
};

// ================ FROM ESP ===============
struct State {
  MessageType type = STATE;

  // valid moves for currently selected piece
  // A1, B1, ..., G8, H8
  uint8_t valid_moves[8];

  // board eval 0-255
  uint8_t eval;

  // remaining time (for white) in seconds
  uint16_t white_time_remaining;

  // remaining time (for black) in seconds
  uint16_t black_time_remaining;

  /*
    ## State Object (1/2)
    state_a[0] - show valid moveset? (picked up piece?) 
    state_a[1] - enable show valid moveset
    state_a[2] - in_game?
    state_a[3] - turn?
    state_a[4] - mode[0]
    state_a[5] - mode[1]
    state_a[6] - mode[2]
    state_a[7] - mode[3]
  */
  uint8_t state_a;

  /*
    ## State Object (2/2)
    state_b[0] - relay[0]
    state_b[1] - relay[1]
    state_b[2] - relay[2]
    state_b[3] - relay[3]
    state_b[4] - show eval?
    state_b[5] -
    state_b[6] -
    state_b[7] -
  */
  uint8_t state_b;
};

struct RequestSegment {
  MessageType type = SEGMENT_REQUEST;

  // Module ID
  uint8_t module_id;
};

struct StartAnimation {
  MessageType type = START_ANIMATION;

  // Animation ID
  uint8_t id;

  // Optional start coordinate (0-63, A1, B1, ..., H8) 
  uint8_t start; 
};

struct RequestClockButtons {
  MessageType type = REQUEST_CLOCK_BUTTONS;
};

struct DisplayClockMessage {
  MessageType type = DISPLAY_CLOCK_MESSAGE;

  // Message Data (ASCII)
  uint8_t data[16];
};

// ================ FROM MODULES ===============
struct SegmentResponse {
  MessageType type = SEGMENT_RESPONSE;

  // white pieces
  uint16_t white_pieces;
  uint16_t black_pieces;
};
