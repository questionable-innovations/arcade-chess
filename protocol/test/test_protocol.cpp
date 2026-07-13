#include "arcade_protocol/protocol.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

int main() {
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
