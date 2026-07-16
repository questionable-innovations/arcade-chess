// Including the required Arduino libraries
#include <MD_Parola.h>
#include <MD_MAX72xx.h>
#include <SPI.h>

#define HARDWARE_TYPE MD_MAX72XX::FC16_HW

// Defining size, and output pins
#define MAX_DEVICES 4
#define CS_PIN 10

// Create a new instance of the MD_Parola class with hardware SPI connection
MD_Parola myDisplay = MD_Parola(HARDWARE_TYPE, CS_PIN, MAX_DEVICES);

void loop() {
  if (myDisplay.displayAnimate()) {
    myDisplay.displayReset();
  }
}

void renderText(
  const char* text,
  textPosition_t align = PA_CENTER,     // left/center/right alignment
  textEffect_t effect = PA_PRINT,       // PA_PRINT = static, PA_SCROLL_LEFT/RIGHT = scrolling
  uint16_t speed = 100,                 // lower is faster
  bool invert = false
) {
  myDisplay.setInvert(invert);
  myDisplay.displayClear();

  if (effect == PA_PRINT) {
    myDisplay.setTextAlignment(align);
    myDisplay.print(text);
  } else {
    myDisplay.displayScroll(text, align, effect, speed);
  }
}

// To implement - timer, stopwatch, and text render functions

void setup() {
  // Initialize the object
  myDisplay.begin();

  // Set the intensity (brightness) of the display (0-15)
  myDisplay.setIntensity(8);

  // Clear the display
  myDisplay.displayClear();

  renderText("Welcome to Arcade Chess!", PA_CENTER, PA_SCROLL_LEFT);
}
