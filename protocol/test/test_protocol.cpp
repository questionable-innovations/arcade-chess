#include "arcade_protocol/protocol.h"

#include "reference_encoder.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

namespace {

using namespace arcade;

void assertMatchesReference(const Frame& frame) {
  uint8_t streamed[kMaxEncodedFrame];
  uint8_t buffered[kMaxEncodedFrame];
  const size_t streamed_length = encodeFrame(frame, streamed, sizeof(streamed));
  const size_t buffered_length =
      reference::encodeFrame(frame, buffered, sizeof(buffered));
  assert(streamed_length == buffered_length);
  assert(streamed_length > 0);
  assert(memcmp(streamed, buffered, streamed_length) == 0);

  // A truncated output must fail the same way at every capacity.
  for (size_t capacity = 0; capacity <= streamed_length; ++capacity) {
    assert(encodeFrame(frame, streamed, capacity) ==
           reference::encodeFrame(frame, buffered, capacity));
  }
}

void assertEncoderParity() {
  Frame frame{};
  frame.flags = kResponse;
  frame.source = 3;
  frame.destination = kEspAddress;
  frame.type = MessageType::kStatus;
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

  for (size_t length = 0; length <= kMaxPayload; ++length) {
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
  frame.payload_length = kMaxPayload;
  for (size_t i = 0; i < kMaxPayload; ++i) frame.payload[i] = 0x01;
  assertMatchesReference(frame);
}

void assertEncoderGuards() {
  Frame frame{};
  frame.type = MessageType::kPing;

  uint8_t wire[kMaxEncodedFrame] = {};
  assert(encodeFrame(frame, wire, 0) == 0);
  assert(encodeFrame(frame, wire, 1) == 0);

  // Over-long payloads are rejected before the payload is read. The buffer is
  // deliberately oversized so the ceiling, not the capacity, is what refuses it.
  uint8_t roomy[2 * kMaxEncodedFrame] = {};
  frame.payload_length = static_cast<uint16_t>(kMaxPayload + 1);
  assert(encodeFrame(frame, roomy, sizeof(roomy)) == 0);
}

// Round trips `sent` through the wire format and returns the encoded length
// including the trailing delimiter.
size_t assertRoundTrip(const Frame& sent, uint8_t* wire, size_t capacity) {
  const size_t wire_length = encodeFrame(sent, wire, capacity);
  assert(wire_length > 0 && wire[wire_length - 1] == 0);

  Frame received{};
  assert(decodeFrame(wire, wire_length - 1, received) == DecodeResult::kFrame);
  assert(received.flags == sent.flags);
  assert(received.source == sent.source);
  assert(received.destination == sent.destination);
  assert(received.type == sent.type);
  assert(received.sequence == sent.sequence);
  assert(received.payload_length == sent.payload_length);
  assert(memcmp(received.payload, sent.payload, sent.payload_length) == 0);
  return wire_length;
}

// COBS encodes a decoded frame body the real encoder would never produce, so
// the decoder's own validation can be reached, and returns the packet length.
size_t encodeBody(uint8_t version, uint16_t declared_payload_length,
                  uint16_t crc_xor, uint8_t* wire, size_t capacity) {
  uint8_t body[kHeaderSize + kCrcSize] = {};
  body[0] = version;
  body[4] = static_cast<uint8_t>(MessageType::kPing);
  putU16(body + 6, declared_payload_length);
  putU16(body + kHeaderSize,
         static_cast<uint16_t>(crc16Ccitt(body, kHeaderSize) ^ crc_xor));
  const size_t length = reference::cobsEncode(body, sizeof(body), wire, capacity);
  assert(length > 0);
  return length;
}

void assertDecoderRejections() {
  Frame received{};
  uint8_t wire[kMaxEncodedFrame] = {};

  assert(decodeFrame(wire, 0, received) == DecodeResult::kEmpty);

  // A zero code byte cannot occur inside a COBS packet.
  const uint8_t zero_code[] = {0x00, 0x01};
  assert(decodeFrame(zero_code, sizeof(zero_code), received) == DecodeResult::kBadCobs);

  // A run that claims more bytes than the packet carries.
  const uint8_t truncated_run[] = {0x05, 0x01};
  assert(decodeFrame(truncated_run, sizeof(truncated_run), received) ==
         DecodeResult::kBadCobs);

  // Valid COBS, but shorter than a bare header plus CRC.
  const uint8_t runt[] = {0x02, 0xaa};
  assert(decodeFrame(runt, sizeof(runt), received) == DecodeResult::kBadLength);

  size_t length = encodeBody(kProtocolVersion, 4, 0, wire, sizeof(wire));
  assert(decodeFrame(wire, length, received) == DecodeResult::kBadLength);

  length = encodeBody(kProtocolVersion, static_cast<uint16_t>(kMaxPayload + 1), 0,
                      wire, sizeof(wire));
  assert(decodeFrame(wire, length, received) == DecodeResult::kBadLength);

  // The version is checked before the CRC, so a mismatched build is reported as
  // such rather than blamed on line noise. Corrupting both proves the order.
  length = encodeBody(static_cast<uint8_t>(kProtocolVersion + 1), 0, 0x0001, wire,
                      sizeof(wire));
  assert(decodeFrame(wire, length, received) == DecodeResult::kBadVersion);

  length = encodeBody(kProtocolVersion, 0, 0x0001, wire, sizeof(wire));
  assert(decodeFrame(wire, length, received) == DecodeResult::kBadCrc);
}

void assertStreamDecoder(const uint8_t* wire, size_t wire_length) {
  Frame received{};

  StreamDecoder decoder;
  assert(decoder.overflowCount() == 0);

  // A delimiter with nothing in front of it is an idle line, not a bad frame.
  assert(decoder.push(0, received) == DecodeResult::kEmpty);

  // Two frames back to back must both surface, one delimiter at a time.
  for (uint8_t repeat = 0; repeat < 2; ++repeat) {
    DecodeResult result = DecodeResult::kNone;
    for (size_t i = 0; i < wire_length; ++i) result = decoder.push(wire[i], received);
    assert(result == DecodeResult::kFrame);
  }

  // Overrunning the buffer reports once, then stays quiet until the delimiter.
  for (size_t i = 0; i < kMaxEncodedFrame; ++i) {
    assert(decoder.push(0x55, received) == DecodeResult::kNone);
  }
  assert(decoder.push(0x55, received) == DecodeResult::kOverflow);
  assert(decoder.overflowCount() == 1);
  assert(decoder.push(0x55, received) == DecodeResult::kNone);
  assert(decoder.push(0, received) == DecodeResult::kOverflow);
  assert(decoder.overflowCount() == 1);

  // The next frame after an overflow decodes normally.
  DecodeResult recovered = DecodeResult::kNone;
  for (size_t i = 0; i < wire_length; ++i) recovered = decoder.push(wire[i], received);
  assert(recovered == DecodeResult::kFrame);

  // reset() drops a partial frame without waiting for the delimiter.
  StreamDecoder interrupted;
  for (size_t i = 0; i < wire_length - 1; ++i) interrupted.push(wire[i], received);
  interrupted.reset();
  DecodeResult after_reset = DecodeResult::kNone;
  for (size_t i = 0; i < wire_length; ++i) after_reset = interrupted.push(wire[i], received);
  assert(after_reset == DecodeResult::kFrame);
  assert(interrupted.overflowCount() == 0);
}

// The little-endian order of these helpers is part of the frozen wire contract.
void assertByteHelpers() {
  uint8_t buffer[4] = {};
  putU16(buffer, 0xbeef);
  assert(buffer[0] == 0xef && buffer[1] == 0xbe);
  assert(getU16(buffer) == 0xbeef);

  putU32(buffer, 0xdeadbeefu);
  assert(buffer[0] == 0xef && buffer[1] == 0xbe && buffer[2] == 0xad &&
         buffer[3] == 0xde);
  assert(getU32(buffer) == 0xdeadbeefu);
}

}  // namespace

int main() {
  assertEncoderParity();
  assertEncoderGuards();
  assertDecoderRejections();
  assertByteHelpers();

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
  const size_t wire_length = assertRoundTrip(sent, wire, sizeof(wire));
  assertStreamDecoder(wire, wire_length);

  Frame received{};
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
  const size_t maximum_length = assertRoundTrip(maximum, wire, sizeof(wire));
  assertStreamDecoder(wire, maximum_length);

  puts("protocol tests passed");
}
