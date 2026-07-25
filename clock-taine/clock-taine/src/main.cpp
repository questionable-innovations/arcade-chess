#include <Arduino.h>

#define BUTTON_WHITE 2
#define BUTTON_BLACK 3
#define RELAY_WHITE 6
#define RELAY_BLACK 5
#define RELAY_SPARK 4
#define CLOCK_WHITE 10
#define CLOCK_BLACK 11

#define CLOCK_MAX_WHITE 256  // adjust these to the value that makes 5mA 
#define CLOCK_MAX_BLACK 256

void clockWrite(int clockPin, uint8_t value); // 0-255 value, 255 meaning clock done and 0 being 0
void relayWrite(int relayPin, bool state);

void setup() {
  pinMode(BUTTON_WHITE, INPUT_PULLUP);
  pinMode(BUTTON_BLACK, INPUT_PULLUP);
}

void loop() {
  if (digitalRead(BUTTON_WHITE) == LOW) {
    relayWrite(RELAY_WHITE, true);
  } else {
    relayWrite(RELAY_WHITE, false);
  }
  if (digitalRead(BUTTON_BLACK) == LOW) {
    relayWrite(RELAY_BLACK, true);
  } else {
    relayWrite(RELAY_BLACK, false);
  }
}

void relayWrite(int relayPin, bool state) {
  if (state) {
    // Turn relay on
    pinMode(relayPin, OUTPUT);
    digitalWrite(relayPin, LOW);
  } else {
    // Turn relay off
    pinMode(relayPin, INPUT);
  }
}

void clockWrite(int clockPin, uint8_t value) {
  if (clockPin == CLOCK_WHITE) {
    analogWrite(CLOCK_WHITE, (uint16_t) value * 256 / CLOCK_MAX_WHITE);
  } else if (clockPin == CLOCK_BLACK) {
    analogWrite(CLOCK_BLACK, (uint16_t) value * 256 / CLOCK_MAX_BLACK);
  }
}