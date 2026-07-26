#pragma once

#include <Arduino.h>
#include <Preferences.h>

#include "app_config.h"
#include "avr_flasher.h"
#include "bus_manager.h"
#include "network_manager.h"

class Console {
 public:
  void begin(Preferences& preferences, AppConfig& config, BusManager& bus,
             ArcadeNetwork& network, AvrFlasher& flasher);
  void tick();
  void printHelp() const;

 private:
  void execute(char* line);
  void setMode(const char* value);

  Preferences* preferences_ = nullptr;
  AppConfig* config_ = nullptr;
  BusManager* bus_ = nullptr;
  ArcadeNetwork* network_ = nullptr;
  AvrFlasher* flasher_ = nullptr;
  char line_[192]{};
  uint8_t length_ = 0;
};
