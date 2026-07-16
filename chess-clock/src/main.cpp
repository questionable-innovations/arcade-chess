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

// Timer variables
unsigned long timeLeft;
unsigned long lastTick;
unsigned long lastShownSec;

// Stopwatch variables
unsigned long stopwatchElapsed;
unsigned long stopwatchLastTick;
unsigned long stopwatchLastShownSec;

// Start timer with a specified duration (in ms)
void startTimer(unsigned long startMs) {
  timeLeft = startMs;
  lastTick = millis();
  lastShownSec = ~0UL; // force first render
}

void startStopwatch() {
  stopwatchElapsed = 0;
  stopwatchLastTick = millis();
  stopwatchLastShownSec = ~0UL; // force first render
}

void renderText(
  const char* text,
  textPosition_t align = PA_CENTER,     // left/center/right alignment
  textEffect_t effect = PA_PRINT,       // PA_PRINT = static, PA_SCROLL_LEFT/RIGHT = scrolling
  uint16_t speed = 100,                 // lower is faster
  bool invert = false
) {
  myDisplay.setInvert(invert);

  if (effect == PA_PRINT) {
    // pause = 0xFFFF so text doesn't immediately clear
    myDisplay.displayText(text, align, speed, 0xFFFF, PA_PRINT, PA_NO_EFFECT);
  } else {
    myDisplay.displayScroll(text, align, effect, speed);
  }
}

void updateStopwatch() {
  unsigned long now = millis();
  stopwatchElapsed += now - stopwatchLastTick;
  stopwatchLastTick = now;

  unsigned long sec = stopwatchElapsed / 1000;
  if (sec == stopwatchLastShownSec) return; // only re-render on change, avoid flicker
  stopwatchLastShownSec = sec;

  char buf[6];  // MM:SS\0
  snprintf(buf, sizeof(buf), "%02lu:%02lu", sec / 60, sec % 60);
  renderText(buf, PA_CENTER, PA_PRINT);
}

// Pump the display to update animations
void pumpDisplay() {
  if (myDisplay.displayAnimate()) {
    myDisplay.displayReset();
  }
}

// Hold for a specified duration (in ms)
void holdFor(unsigned long ms) {
  unsigned long start = millis();
  while (millis() - start < ms) {
    pumpDisplay();
  }
}

void runStopwatchFor(unsigned long ms) {
  startStopwatch();
  unsigned long start = millis();
  while (millis() - start < ms) {
    updateStopwatch();
    pumpDisplay();
  }
}

void updateTimer() {
  unsigned long now = millis();
  unsigned long elapsed = now - lastTick;
  lastTick = now;

  if (timeLeft > elapsed) {
    timeLeft -= elapsed;
  } else {
    timeLeft = 0;
  }

  unsigned long sec = timeLeft / 1000;
  if (sec == lastShownSec) return;
  lastShownSec = sec;

  char buf[6];  // MM:SS\0
  snprintf(buf, sizeof(buf), "%02lu:%02lu", sec / 60, sec % 60);
  renderText(buf, PA_CENTER, PA_PRINT);
}

void loop() {
  updateTimer();
  pumpDisplay();
}

void setup() {
  // Initialize the object
  myDisplay.begin();

  // Set the intensity (brightness) of the display (0-15)
  myDisplay.setIntensity(8);

  // TESTS BELOW
  runStopwatchFor(5000);

  renderText("Welcome to Arcade Chess!", PA_CENTER, PA_SCROLL_LEFT, 50);
  holdFor(6000);
  renderText("HELLO");
  holdFor(2000);

  renderText("LEFT", PA_LEFT);
  holdFor(2000);

  renderText("RIGHT", PA_RIGHT);
  holdFor(2000);

  renderText("05:00", PA_CENTER, PA_PRINT, 100, true);
  holdFor(2000);

  renderText("SCROLL >>", PA_LEFT, PA_SCROLL_LEFT, 50);
  holdFor(6000);
  startTimer(5UL * 60 * 1000); // 5 min
}
