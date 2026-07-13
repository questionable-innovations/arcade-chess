#include <Arduino.h>

#include "application.h"

namespace {
quadrant::Application application;
}

void setup() { application.setup(); }
void loop() { application.loop(); }
