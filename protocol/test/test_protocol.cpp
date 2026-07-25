#include "arcade_protocol/protocol.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

namespace {

// Verbatim copy of the buffered encoder that encodeFrame() replaced. The
// streaming version writes into the caller's buffer to keep the AVR's peak
// stack out of .bss, so it has to stay byte-identical to this reference.
size_t referenceCobsEncode(const uint8_t* input, size_t length, uint8_t* output,
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

size_t referenceEncodeFrame(const arcade::Frame& frame, uint8_t* output,
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
      referenceCobsEncode(decoded, crc_offset + kCrcSize, output, output_capacity - 1);
  if (encoded_length == 0 || encoded_length >= output_capacity) return 0;
  output[encoded_length] = 0;
  return encoded_length + 1;
}

void assertMatchesReference(const arcade::Frame& frame) {
  uint8_t streamed[arcade::kMaxEncodedFrame];
  uint8_t buffered[arcade::kMaxEncodedFrame];
  const size_t streamed_length = encodeFrame(frame, streamed, sizeof(streamed));
  const size_t buffered_length =
      referenceEncodeFrame(frame, buffered, sizeof(buffered));
  assert(streamed_length == buffered_length);
  assert(streamed_length > 0);
  assert(memcmp(streamed, buffered, streamed_length) == 0);

  // A truncated output must fail the same way at every capacity.
  for (size_t capacity = 0; capacity <= streamed_length; ++capacity) {
    assert(encodeFrame(frame, streamed, capacity) ==
           referenceEncodeFrame(frame, buffered, capacity));
  }
}

void assertEncoderParity() {
  arcade::Frame frame{};
  frame.flags = arcade::kResponse;
  frame.source = 3;
  frame.destination = arcade::kEspAddress;
  frame.type = arcade::MessageType::kStatus;
  frame.sequence = 0xa7;

  // Empty, single byte, an all-zero run, and every length at once so the CRC
  // bytes land on both sides of a zero boundary.
  frame.payload_length = 0;
  assertMatchesReference(frame);
  frame.payload_length = 1;
  frame.payload[0] = 0x00;
  assertMatchesReference(frame);
  frame.payload[0] = 0x42;
  assertMatchesReference(frame);

  for (size_t length = 0; length <= arcade::kMaxPayload; ++length) {
    frame.payload_length = static_cast<uint16_t>(length);
    for (uint8_t pattern = 0; pattern < 4; ++pattern) {
      for (size_t i = 0; i < length; ++i) {
        switch (pattern) {
          case 0: frame.payload[i] = 0; break;
          case 1: frame.payload[i] = 0xff; break;
          case 2: frame.payload[i] = (i % 3) ? static_cast<uint8_t>(i + 1) : 0; break;
          default: frame.payload[i] = static_cast<uint8_t>((i * 37) ^ 0x5a); break;
        }
      }
      frame.sequence = static_cast<uint8_t>(length * 4 + pattern);
      assertMatchesReference(frame);
    }
  }

  // The 254-byte-run branch is unreachable on the wire — kMaxDecodedFrame is
  // 122 — but the two encoders must still agree on the longest run that exists.
  frame.payload_length = arcade::kMaxPayload;
  for (size_t i = 0; i < arcade::kMaxPayload; ++i) frame.payload[i] = 0x01;
  assertMatchesReference(frame);
}

}  // namespace

int main() {
  assertEncoderParity();

  using namespace arcade;
  const uint8_t check[] = "123456789";
  assert(crc16Ccitt(check, 9) == 0x29b1);

  Frame sent{};
  sent.flags = kAckRequired;
  sent.source = kEspAddress;
  sent.destination = 2;
  sent.type = MessageType::kSetSquares;
  sent.sequence = 0x55;
  const uint8_t payload[] = {0x00, 0x01, 0, 0x7f, 0xff};
  sent.payload_length = sizeof(payload);
  memcpy(sent.payload, payload, sizeof(payload));

  uint8_t wire[kMaxEncodedFrame];
  const size_t wire_length = encodeFrame(sent, wire, sizeof(wire));
  assert(wire_length > 0 && wire[wire_length - 1] == 0);

  Frame received{};
  assert(decodeFrame(wire, wire_length - 1, received) == DecodeResult::kFrame);
  assert(received.flags == sent.flags);
  assert(received.source == sent.source);
  assert(received.destination == sent.destination);
  assert(received.type == sent.type);
  assert(received.sequence == sent.sequence);
  assert(received.payload_length == sizeof(payload));
  assert(memcmp(received.payload, payload, sizeof(payload)) == 0);

  StreamDecoder decoder;
  DecodeResult result = DecodeResult::kNone;
  for (size_t i = 0; i < wire_length; ++i) result = decoder.push(wire[i], received);
  assert(result == DecodeResult::kFrame);

  wire[2] ^= 0x40;
  const DecodeResult corrupt = decodeFrame(wire, wire_length - 1, received);
  assert(corrupt != DecodeResult::kFrame);

  Frame maximum{};
  maximum.source = kEspAddress;
  maximum.destination = 0;
  maximum.type = MessageType::kGetRawScan;
  maximum.sequence = 9;
  maximum.payload_length = kMaxPayload;
  for (size_t i = 0; i < kMaxPayload; ++i) maximum.payload[i] = static_cast<uint8_t>(i % 7);
  const size_t maximum_length = encodeFrame(maximum, wire, sizeof(wire));
  assert(maximum_length > 0);
  assert(decodeFrame(wire, maximum_length - 1, received) == DecodeResult::kFrame);
  assert(received.payload_length == kMaxPayload);
  assert(memcmp(received.payload, maximum.payload, kMaxPayload) == 0);

  StreamDecoder recovering;
  for (size_t i = 0; i < kMaxEncodedFrame + 4; ++i) recovering.push(0x55, received);
  assert(recovering.push(0, received) == DecodeResult::kOverflow);
  DecodeResult recovered = DecodeResult::kNone;
  for (size_t i = 0; i < maximum_length; ++i) recovered = recovering.push(wire[i], received);
  assert(recovered == DecodeResult::kFrame);
  puts("protocol tests passed");
}
