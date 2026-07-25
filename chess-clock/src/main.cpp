// Including the required Arduino libraries
#include <MD_Parola.h>
#include <MD_MAX72xx.h>
#include <SPI.h>

#define HARDWARE_TYPE MD_MAX72XX::FC16_HW

// Defining size, and output pins
#define MAX_DEVICES 8
#define CS_PIN 10

// Two halves of the module chain, one per player's clock.
// If White/Black turn out swapped physically, swap these ranges.
#define ZONE_WHITE 0
#define ZONE_BLACK 1

// Button wiring: each button ends that player's turn (INPUT_PULLUP, active LOW)
#define BUTTON_WHITE_PIN 2
#define BUTTON_BLACK_PIN 3
#define DEBOUNCE_MS 50

// Create a new instance of the MD_Parola class with hardware SPI connection
MD_Parola myDisplay = MD_Parola(HARDWARE_TYPE, CS_PIN, MAX_DEVICES);

enum Player : uint8_t { WHITE = 0, BLACK = 1 };

// Chess clock state
unsigned long clockTimeLeft[2];     // ms remaining, indexed by Player
unsigned long clockLastShownSec[2]; // last rendered second, indexed by Player
Player activePlayer;
unsigned long activeLastTick;
bool clockRunning;

// Debounced button, tied to the player whose turn it ends
struct DebouncedButton {
  uint8_t pin;
  Player player;
  int stableState;
  int lastFlickerState;
  unsigned long lastChangeTime;
};

DebouncedButton buttons[2] = {
  { BUTTON_WHITE_PIN, WHITE, HIGH, HIGH, 0 },
  { BUTTON_BLACK_PIN, BLACK, HIGH, HIGH, 0 },
};

// Stopwatch variables
unsigned long stopwatchElapsed;
unsigned long stopwatchLastTick;
unsigned long stopwatchLastShownSec;

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

  Serial.println(text);

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

// Render one player's remaining time into their half of the display
void renderClockZone(Player p) {
  unsigned long sec = clockTimeLeft[p] / 1000;
  char buf[6];  // MM:SS\0
  snprintf(buf, sizeof(buf), "%02lu:%02lu", sec / 60, sec % 60);
  uint8_t zone = (p == WHITE) ? ZONE_WHITE : ZONE_BLACK;
  // pause = 0xFFFF so text doesn't immediately clear
  myDisplay.displayZoneText(zone, buf, PA_CENTER, 100, 0xFFFF, PA_PRINT, PA_NO_EFFECT);
}

// Set both clocks to startMs and put White on the move
void initChessClock(unsigned long startMs) {
  clockTimeLeft[WHITE] = startMs;
  clockTimeLeft[BLACK] = startMs;
  activePlayer = WHITE;
  activeLastTick = millis();
  clockRunning = true;

  renderClockZone(WHITE);
  renderClockZone(BLACK);
  clockLastShownSec[WHITE] = clockTimeLeft[WHITE] / 1000;
  clockLastShownSec[BLACK] = clockTimeLeft[BLACK] / 1000;
}

// Count down the active player's clock only; the other side stays frozen
void updateActiveClock() {
  if (!clockRunning) return;

  unsigned long now = millis();
  unsigned long elapsed = now - activeLastTick;
  activeLastTick = now;

  if (clockTimeLeft[activePlayer] > elapsed) {
    clockTimeLeft[activePlayer] -= elapsed;
  } else {
    clockTimeLeft[activePlayer] = 0;
    clockRunning = false; // flag fell
  }

  unsigned long sec = clockTimeLeft[activePlayer] / 1000;
  if (sec == clockLastShownSec[activePlayer]) return; // only re-render on change, avoid flicker
  clockLastShownSec[activePlayer] = sec;

  renderClockZone(activePlayer);
}

// End the active player's turn and hand the move to the other side
void switchTurn() {
  unsigned long now = millis();
  unsigned long elapsed = now - activeLastTick;
  if (clockTimeLeft[activePlayer] > elapsed) {
    clockTimeLeft[activePlayer] -= elapsed;
  } else {
    clockTimeLeft[activePlayer] = 0;
  }
  renderClockZone(activePlayer); // lock in the finished player's final time
  clockLastShownSec[activePlayer] = clockTimeLeft[activePlayer] / 1000;

  activePlayer = (activePlayer == WHITE) ? BLACK : WHITE;
  activeLastTick = now;
  clockLastShownSec[activePlayer] = ~0UL; // force immediate render for the newly active side
}

// Returns true exactly once per confirmed button press (active LOW)
bool wasPressed(DebouncedButton &btn) {
  int reading = digitalRead(btn.pin);

  if (reading != btn.lastFlickerState) {
    btn.lastChangeTime = millis();
    btn.lastFlickerState = reading;
  }

  if ((millis() - btn.lastChangeTime) > DEBOUNCE_MS && reading != btn.stableState) {
    btn.stableState = reading;
    return btn.stableState == LOW;
  }

  return false;
}

void loop() {
  for (uint8_t i = 0; i < 2; i++) {
    if (wasPressed(buttons[i]) && buttons[i].player == activePlayer && clockRunning) {
      switchTurn();
    }
  }

  updateActiveClock();
  pumpDisplay();
}

void setup() {
  // Initialize serial so the laptop can read what's being sent to the display
  Serial.begin(9600);

  pinMode(BUTTON_WHITE_PIN, INPUT_PULLUP);
  pinMode(BUTTON_BLACK_PIN, INPUT_PULLUP);

  // Split the module chain into two independently-animated zones
  myDisplay.begin(2);
  myDisplay.setZone(ZONE_WHITE, 0, 3); // first 4 modules
  myDisplay.setZone(ZONE_BLACK, 4, 7); // last 4 modules

  // Set the intensity (brightness) of the display (0-15)
  myDisplay.setIntensity(8);

  initChessClock(5UL * 60 * 1000); // 5 min per side
}
