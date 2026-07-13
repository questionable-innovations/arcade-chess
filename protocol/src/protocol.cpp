#include "arcade_protocol/protocol.h"

#include <string.h>

namespace arcade {
namespace {

size_t cobsEncode(const uint8_t* input, size_t length, uint8_t* output,
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

size_t cobsDecode(const uint8_t* input, size_t length, uint8_t* output,
                  size_t capacity) {
  size_t read_index = 0;
  size_t write_index = 0;
  while (read_index < length) {
    const uint8_t code = input[read_index++];
    if (code == 0 || read_index + static_cast<size_t>(code - 1) > length) return 0;
    for (uint8_t i = 1; i < code; ++i) {
      if (write_index >= capacity) return 0;
      output[write_index++] = input[read_index++];
    }
    if (code != 0xff && read_index < length) {
      if (write_index >= capacity) return 0;
      output[write_index++] = 0;
    }
  }
  return write_index;
}

}  // namespace

uint16_t crc16Ccitt(const uint8_t* data, size_t length) {
  uint16_t crc = 0xffff;
  for (size_t i = 0; i < length; ++i) {
    crc ^= static_cast<uint16_t>(data[i]) << 8;
    for (uint8_t bit = 0; bit < 8; ++bit) {
      crc = (crc & 0x8000) ? static_cast<uint16_t>((crc << 1) ^ 0x1021)
                           : static_cast<uint16_t>(crc << 1);
    }
  }
  return crc;
}

size_t encodeFrame(const Frame& frame, uint8_t* output, size_t output_capacity) {
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
  const size_t encoded_length = cobsEncode(
      decoded, crc_offset + kCrcSize, output, output_capacity - 1);
  if (encoded_length == 0 || encoded_length >= output_capacity) return 0;
  output[encoded_length] = 0;
  return encoded_length + 1;
}

DecodeResult decodeFrame(const uint8_t* encoded, size_t encoded_length, Frame& output) {
  if (!encoded_length) return DecodeResult::kEmpty;
  uint8_t decoded[kMaxDecodedFrame];
  const size_t length = cobsDecode(encoded, encoded_length, decoded, sizeof(decoded));
  if (!length) return DecodeResult::kBadCobs;
  if (length < kHeaderSize + kCrcSize) return DecodeResult::kBadLength;
  const uint16_t payload_length = getU16(decoded + 6);
  if (payload_length > kMaxPayload ||
      length != kHeaderSize + payload_length + kCrcSize) {
    return DecodeResult::kBadLength;
  }
  if (decoded[0] != kProtocolVersion) return DecodeResult::kBadVersion;
  const uint16_t expected_crc = getU16(decoded + kHeaderSize + payload_length);
  if (crc16Ccitt(decoded, kHeaderSize + payload_length) != expected_crc) {
    return DecodeResult::kBadCrc;
  }
  output.flags = decoded[1];
  output.source = decoded[2];
  output.destination = decoded[3];
  output.type = static_cast<MessageType>(decoded[4]);
  output.sequence = decoded[5];
  output.payload_length = payload_length;
  if (payload_length) memcpy(output.payload, decoded + kHeaderSize, payload_length);
  return DecodeResult::kFrame;
}

DecodeResult StreamDecoder::push(uint8_t byte, Frame& output) {
  if (byte != 0) {
    if (dropping_) return DecodeResult::kNone;
    if (length_ >= sizeof(encoded_)) {
      dropping_ = true;
      ++overflow_count_;
      return DecodeResult::kOverflow;
    }
    encoded_[length_++] = byte;
    return DecodeResult::kNone;
  }
  if (dropping_) {
    reset();
    return DecodeResult::kOverflow;
  }
  const DecodeResult result = decodeFrame(encoded_, length_, output);
  length_ = 0;
  return result;
}

void StreamDecoder::reset() {
  length_ = 0;
  dropping_ = false;
}

}  // namespace arcade
