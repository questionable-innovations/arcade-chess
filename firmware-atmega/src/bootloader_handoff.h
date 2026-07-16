#pragma once

#include <Arduino.h>
#include <avr/interrupt.h>
#include <avr/io.h>

namespace quadrant {

// Shared with firmware-atmega/bootloader/urboot-arcade.patch.  Urboot consumes
// this five-byte record before its first call can reuse the top of the stack.
constexpr uintptr_t kBootloaderHandoffAddress = RAMEND - 5;
constexpr uint8_t kBootloaderHandoffMagic0 = 0x41;
constexpr uint8_t kBootloaderHandoffMagic1 = 0x43;
constexpr uint8_t kBootloaderHandoffMagic2 = 0x42;
constexpr uint8_t kBootloaderHandoffMagic3 = 0x4c;
constexpr uintptr_t kBootloaderStartAddress = 0x7E00;

// Transfer directly from the application into the protected hardware boot
// section.  This deliberately does not pretend that a watchdog reset is an
// external reset: MCUSR reset flags cannot be set by application software.
//
// All participants execute Urprotocol commands.  Only a participant entered
// with responder=true transmits replies on the shared return line.
[[noreturn]] inline void enterBootloader(bool responder) {
  cli();
  volatile uint8_t* const handoff =
      reinterpret_cast<volatile uint8_t*>(kBootloaderHandoffAddress);
  handoff[0] = kBootloaderHandoffMagic0;
  handoff[1] = kBootloaderHandoffMagic1;
  handoff[2] = kBootloaderHandoffMagic2;
  handoff[3] = kBootloaderHandoffMagic3;
  handoff[4] = responder ? 1 : 0;
  SP = RAMEND;
  asm volatile("jmp %0" :: "n"(kBootloaderStartAddress));
  __builtin_unreachable();
}

}  // namespace quadrant
