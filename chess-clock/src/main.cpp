// #include <Arduino.h>
// #include <MD_Parola.h>
// #include <MD_MAX72xx.h>
// #include <SPI.h>

// #define HARDWARE_TYPE MD_MAX72XX::FC16_HW
// #define MAX_DEVICES   4
// #define CS_PIN        10

// MD_Parola display = MD_Parola(HARDWARE_TYPE, CS_PIN, MAX_DEVICES);

// void renderText(const char* text) {
//   display.displayClear();
//   display.setTextAlignment(PA_CENTER);
//   display.print(text);
// }

// void setup() {
//   display.begin();
//   display.setIntensity(1);
//   display.displayClear();

//   renderText("88888");
// }

// void loop() {
//   // nothing yet
// }


// Including the required Arduino libraries
#include <MD_Parola.h>
#include <MD_MAX72xx.h>
#include <SPI.h>

// Uncomment according to your hardware type
#define HARDWARE_TYPE MD_MAX72XX::FC16_HW
//#define HARDWARE_TYPE MD_MAX72XX::GENERIC_HW

// Defining size, and output pins
#define MAX_DEVICES 4
#define CS_PIN 10

// Create a new instance of the MD_Parola class with hardware SPI connection
MD_Parola myDisplay = MD_Parola(HARDWARE_TYPE, CS_PIN, MAX_DEVICES);


void setup() {
  // Initialize the object
  myDisplay.begin();

  // Set the intensity (brightness) of the display (0-15)
  myDisplay.setIntensity(0);

  // Clear the display
  myDisplay.displayClear();

  myDisplay.displayScroll("Hello", PA_CENTER, PA_SCROLL_LEFT, 100);
}

void loop() {
  if (myDisplay.displayAnimate()) {
    myDisplay.displayReset();
  }
}