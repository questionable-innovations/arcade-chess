#pragma once

#include "arcade_protocol/protocol.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

// Verbatim copy of the buffered encoder that arcade::encodeFrame() replaced. The
// streaming version writes into the caller's buffer to keep the AVR's peak
// stack out of .bss, so it has to stay byte-identical to this reference.
namespace reference {

inline size_t cobsEncode(const uint8_t* input, size_t length, uint8_t* output,
                         size_t capacity) {
  if (capacity == 0) return 0;
  size_t read_index = 0;
  size_t write_index = 1;
  size_t code_index = 0;
  uint8_t code = 1;

  while (read_index < length) {
    if (input[read_index] == 0) {
      if (code_index >= capacity) return 0;
      output[code_index] = code;
      code = 1;
      code_index = write_index++;
      if (write_index > capacity) return 0;
      ++read_index;
    } else {
      if (write_index >= capacity) return 0;
      output[write_index++] = input[read_index++];
      if (++code == 0xff) {
        if (code_index >= capacity) return 0;
        output[code_index] = code;
        code = 1;
        code_index = write_index++;
        if (write_index > capacity) return 0;
      }
    }
  }
  if (code_index >= capacity) return 0;
  output[code_index] = code;
  return write_index;
}

inline size_t encodeFrame(const arcade::Frame& frame, uint8_t* output,
                          size_t output_capacity) {
  using namespace arcade;
  if (frame.payload_length > kMaxPayload || output_capacity < 2) return 0;
  uint8_t decoded[kMaxDecodedFrame];
  decoded[0] = kProtocolVersion;
  decoded[1] = frame.flags;
  decoded[2] = frame.source;
  decoded[3] = frame.destination;
  decoded[4] = static_cast<uint8_t>(frame.type);
  decoded[5] = frame.sequence;
  putU16(decoded + 6, frame.payload_length);
  if (frame.payload_length) {
    memcpy(decoded + kHeaderSize, frame.payload, frame.payload_length);
  }
  const size_t crc_offset = kHeaderSize + frame.payload_length;
  putU16(decoded + crc_offset, crc16Ccitt(decoded, crc_offset));
  const size_t encoded_length =
      cobsEncode(decoded, crc_offset + kCrcSize, output, output_capacity - 1);
  if (encoded_length == 0 || encoded_length >= output_capacity) return 0;
  output[encoded_length] = 0;
  return encoded_length + 1;
}

}  // namespace reference
