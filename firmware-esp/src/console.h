#pragma once

#include <Arduino.h>
#include <Preferences.h>

#include "app_config.h"
#include "bus_manager.h"
#include "network_manager.h"

class Console {
 public:
  void begin(Preferences& preferences, AppConfig& config, BusManager& bus,
             NetworkManager& network);
  void tick();
  void printHelp() const;

 private:
  void execute(char* line);
  void setMode(const char* value);

  Preferences* preferences_ = nullptr;
  AppConfig* config_ = nullptr;
  BusManager* bus_ = nullptr;
  NetworkManager* network_ = nullptr;
  char line_[192]{};
  uint8_t length_ = 0;
};
