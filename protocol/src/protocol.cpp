#include "arcade_protocol/protocol.h"

#include <string.h>

namespace arcade {
namespace {

// Streaming COBS encoder. The AVR quadrants cannot afford a second full-frame
// scratch buffer on the stack, so bytes are encoded straight into the caller's
// output and the run's code byte is back-patched once the run closes.
class CobsWriter {
 public:
  CobsWriter(uint8_t* output, size_t capacity)
      : output_(output), capacity_(capacity), failed_(capacity == 0) {}

  void push(uint8_t byte) {
    if (failed_) return;
    if (byte == 0) {
      closeRun();
      return;
    }
    if (write_index_ >= capacity_) {
      failed_ = true;
      return;
    }
    output_[write_index_++] = byte;
    if (++code_ == 0xff) closeRun();
  }

  // Returns the encoded length excluding the trailing delimiter, or zero when
  // the output ran out of room at any point.
  size_t finish() {
    if (failed_ || code_index_ >= capacity_) return 0;
    output_[code_index_] = code_;
    return write_index_;
  }

 private:
  void closeRun() {
    if (code_index_ >= capacity_) {
      failed_ = true;
      return;
    }
    output_[code_index_] = code_;
    code_ = 1;
    code_index_ = write_index_++;
    if (write_index_ > capacity_) failed_ = true;
  }

  uint8_t* output_;
  size_t capacity_;
  size_t write_index_ = 1;
  size_t code_index_ = 0;
  uint8_t code_ = 1;
  bool failed_;
};

uint16_t crc16Byte(uint16_t crc, uint8_t byte) {
  crc ^= static_cast<uint16_t>(byte) << 8;
  for (uint8_t bit = 0; bit < 8; ++bit) {
    crc = (crc & 0x8000) ? static_cast<uint16_t>((crc << 1) ^ 0x1021)
                         : static_cast<uint16_t>(crc << 1);
  }
  return crc;
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
  for (size_t i = 0; i < length; ++i) crc = crc16Byte(crc, data[i]);
  return crc;
}

size_t encodeFrame(const Frame& frame, uint8_t* output, size_t output_capacity) {
  if (frame.payload_length > kMaxPayload || output_capacity < 2) return 0;
  const uint8_t header[kHeaderSize] = {
      kProtocolVersion,
      frame.flags,
      frame.source,
      frame.destination,
      static_cast<uint8_t>(frame.type),
      frame.sequence,
      static_cast<uint8_t>(frame.payload_length),
      static_cast<uint8_t>(frame.payload_length >> 8),
  };
  // One pass: header, payload and CRC are checksummed and COBS'd as they stream
  // past, so the undecoded frame is never materialised anywhere.
  CobsWriter writer(output, output_capacity - 1);
  uint16_t crc = 0xffff;
  for (size_t i = 0; i < kHeaderSize; ++i) {
    crc = crc16Byte(crc, header[i]);
    writer.push(header[i]);
  }
  for (uint16_t i = 0; i < frame.payload_length; ++i) {
    crc = crc16Byte(crc, frame.payload[i]);
    writer.push(frame.payload[i]);
  }
  writer.push(static_cast<uint8_t>(crc));
  writer.push(static_cast<uint8_t>(crc >> 8));
  const size_t encoded_length = writer.finish();
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
