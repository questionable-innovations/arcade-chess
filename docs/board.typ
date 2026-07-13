#set page(
  paper: "a4",
  margin: (x: 2cm, y: 2cm)
)
#set heading(numbering: "1.1")

#align(center)[
  #text(size: 18pt, weight: "bold")[Arcade Chess: Firmware-facing Board Map]
]

#v(1em)

= System Architecture Overview

The arcade chess board utilizes a dual-microcontroller architecture to separate high-level logic/connectivity from low-level sensor polling and LED multiplexing.
- *Primary Controller*: ESP32-WROOM-32E-N4.
- *Secondary Coprocessor*: ATMEGA328PB-AU.
- *Communication*: The two microcontrollers communicate via an intra-board UART bus.

= ESP32 (Main Controller) Specifications

The ESP32 acts as the primary brain of the system, likely handling game logic, state management, and external connectivity.

== Flashing and Boot Configuration
- *Auto-Flasher*: The circuit includes a CH340C USB-to-UART bridge. It utilizes DTR and RTS control signals connected via transistors to automate the boot mode and restart sequence. 
- *Programming Pins*: The CH340C RXD and TXD lines connect to the ESP32 via `ESP_TX` and `ESP_RX` nets. 
- *Manual Overrides*: Physical override test points/buttons are available for `ESP_BOOT` and `ESP_EN`.

== Main UART Bus (ESP32 to ATmega)
The ESP32 communicates with all quadrants over one shared two-wire UART bus. Net
names are relative to the quadrants and can be misleading in ESP firmware:
- *ESP transmit*: GPIO16 drives schematic `BusRX` and every ATmega PD0/RXD0 input.
- *ESP receive*: GPIO17 receives schematic `BusTX` from the diode-isolated shared
  quadrant return.
- *Return electrical behavior*: each ATmega PD1/TXD0 reaches `BusTX` through D8.
  R1 = 10 kohm pulls the ESP side to 3.3 V, preventing a 5 V idle-high level from
  reaching the ESP32.

Only the ESP initiates normal transactions. One addressed node responds and a
broadcast never receives a response. Bring-up begins at 38,400 baud pending scope
tests of the pull-up-limited rising edge.

= ATMega328PB (Coprocessor) Specifications

The ATMega handles real-time polling of the Hall effect sensors located in each square and manages the addressable LED data lines. 

== Core Setup
- *Clock Speed*: Driven by an external 16.000MHz crystal oscillator. 
- *Programming*: Accessible via a standard ISP Header (MISO, MOSI, SCK, RST). 
- *Intra-Board Comm*: Connects to the ESP32 using `ATMEGA_TX` and `ATMEGA_RX`.

== Hall Effect Sensor Array (Multiplexing)
Each of the 16 squares on the chessboard contains an SS49E linear Hall effect sensor. Because 16 sensors exceed the ATMega's analog inputs, the board utilizes two 74HC4051PW 8-channel analog multiplexers. 

*Multiplexer 1 (B1) - Lower 8 Channels*
- *Input Nets*: `SENSE1` through `SENSE8`.
- *Control Pins*:
  - `MuxLow1` $->$ ATMega Pin 12 (PB0).
  - `MuxLow2` $->$ ATMega Pin 13 (PB1).
  - `MuxLow3` $->$ ATMega Pin 14 (PB2).
- *Output*: `RawSenseA`. Passed through a TLC082CDR op-amp.

*Multiplexer 2 (B2) - Upper 8 Channels*
- *Input Nets*: `SENSE9` through `SENSE16`.
- *Control Pins*:
  - `MuxHigh1` $->$ ATmega Pin 2 (PD4).
  - `MuxHigh2` $->$ ATmega Pin 9 (PD5).
  - `MuxHigh3` $->$ ATmega Pin 10 (PD6).
- *Output*: `RawSenseB`. Passed through a TLC082CDR op-amp.

*Confirmed ADC inputs*
- `SenseA` $->$ PC0 / ADC0 / pin 23.
- `SenseB` $->$ PC1 / ADC1 / pin 24.

== Hall Effect Amplification & Polarity Detection
The raw output from the SS49E Hall effect sensors centers around 2.5V (quiescent state, no magnetic field). To maximize the resolution of the ATMega's ADC and accurately determine magnetic polarity, the `RawSense` signals are passed through TLC082CDR operational amplifiers. 

The circuit utilizes a level-shifting amplifier configuration referenced to a 2.5V bias (created by the 10k voltage dividers `R26`/`R27`). The firmware engineer should expect the amplified output voltage ($V_"out"$) to be governed by the following gain equation based on the chosen feedback resistor ($R_25$):

$ V_"out" = V_"in" ((2 R_25) / (10"k") + 1) - (5 R_25) / (10"k") $

This maps the narrow voltage swing of the sensor into a broader 1V to 4V range for the ADC:
- *Standard Gain (R25 = 13 kΩ)*: Amplifies a sensor range of 2.1 V - 2.9 V to a measured output of 1 V - 4 V.
- *High Gain (R25 = 33 kΩ)*: Amplifies a tighter sensor range of 2.3 V - 2.7 V to a measured output of 1 V - 4 V.

*Polarity Detection in Firmware*: Because the op-amp circuitry maintains the 2.5V center point, the firmware can detect chess piece polarity (e.g., distinguishing between Black and White pieces fitted with opposing magnets). An ADC reading trending towards 1V indicates the presence of one magnetic pole, while a reading trending towards 4V indicates the opposite pole. A reading near 2.5V means the square is empty.

= LED Arrays (Addressable)

The board uses COM-16347 addressable RGB LEDs (WS2812 protocol compatible). The firmware must manage data arrays for multiple distinct LED chains.

#v(1em)

#align(center)[
  #table(
    columns: (auto, auto, auto),
    inset: 10pt,
    align: horizon,
    [*Subsystem*], [*Configuration*], [*Routing Protocol*],
    [*Smart Squares*], 
    [4 LEDs per square.], 
    [Divided into two data paths per square: Primary (`DI_P` to `DO_P`) handling LED1 and LED2, and Secondary (`DI_S` to `DO_S`) handling LED3 and LED4.],
    
    [*Edge Lighting*], 
    [16-LED bars per board edge.], 
    [The schematic defines "half bars" of 8 LEDs, which pair together to form the full 16-LED edge bar.]
  )
]

#v(1em)

== Confirmed quadrant LED outputs

- *PE3 / `SDI_P`*: 32-pixel primary square chain, two pixels per square.
- *PE2 / `SDI_S`*: 32-pixel secondary square chain, two pixels per square.
- *PE0 / `LB1_DI`*: independent eight-pixel edge half-bar after the planned bodge.
- *PE1 / `LB2_DI`*: independent eight-pixel edge half-bar after the planned bodge.

Each quadrant therefore owns 80 pixels and requires approximately 2.4 ms of
WS2812 wire time for a full refresh. Firmware must coordinate UART traffic with any
driver that disables interrupts during transmission.

The edge half-bar schematic designator order is `LED5`, `LED6`, `LED7`, `LED9`,
`LED8`, `LED10`, `LED11`, `LED12`. Confirm the physical direction with a walking
pixel test.

= Expansion Connector

J1 is USB-C-shaped but is not a USB data interface. D+ carries the quadrant/module
to ESP return line through 120 ohms, D- carries the ESP-to-node line through 120
ohms, VBUS is the 5 V rail, and CC1/CC2 have sink pull-downs. Verify hot-plug inrush,
ESD behavior, grounding, and unpowered-node line loading before supporting modules.

= Known Bring-up Requirements

- Apply and continuity-check the CH340-to-ESP programming-UART crossing bodge.
- Apply and continuity-check the independent PE0/PE1 edge-bar bodge.
- Provision unique quadrant IDs 0-3 during initial ISP flashing.
- Verify ESP32 3.3 V high is accepted by every 5 V AVR RX input.
- Determine quadrant rotation, local-square mapping, LED orientation, and edge-bar
  direction from assembled hardware rather than schematic designators alone.

= Power Management
- *Input*: 5V supplied via a USB-C interface. 
- *Step-Down Regulation*: A TPS62A01DRLR buck converter steps the 5V logic down to 3.3V to safely power the ESP32. The LED arrays and ATMega logic remain on the 5V rail.
