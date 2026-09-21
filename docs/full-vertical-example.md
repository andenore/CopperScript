# Full-vertical acceptance design

`examples/full_vertical_board.copper` is the integration design used to grow
CopperScript from an electrical description into production Gerber and drill
files. It is intentionally more demanding than the small language examples:
it combines hierarchy, multiple programmable devices, several serial buses,
three RF paths, high-current modem power, debug connectors, and explicit power
states.

The example is an engineering fixture, not yet a fabrication-ready reference
design. A clean ERC result means the currently modelled electrical rules are
satisfied; it does not certify RF layout, regulator stability, EMC, antenna
performance, USB or CAN signal integrity, or regulatory compliance.

## Selected parts

| Function | Initial selection | Reason |
| --- | --- | --- |
| Main MCU | STM32G0C1RET6, LQFP-64 | Native USB FS, FDCAN and enough independent UART/GPIO mappings |
| CAN | TCAN334G | 3.3 V CAN/CAN FD transceiver with standby/wake support |
| Cellular | Quectel EG800G-EU | LTE Cat 1 bis; UART and USB are both connected to the MCU |
| Bluetooth slave | nRF52832-QFAA | UART slave with a discrete 2.4 GHz antenna path and separate SWD |
| Motion | LIS2DW12 | Very-low-current accelerometer with I2C and interrupt output |
| GNSS | u-blox MAX-M10S-00B | Low-power GNSS module with UART and TIMEPULSE |

The source references stored in each part/device definition are optional
provenance. They document which manufacturer revision was used without making
source metadata a requirement of the language.

## Main MCU allocation

| Function | STM32 peripheral | Pins |
| --- | --- | --- |
| EG800G UART | USART1 | PB6 TX, PB7 RX, through a 3.3/1.8 V translator |
| MAX-M10S UART | USART2 | PA2 TX, PA3 RX |
| nRF52832 UART | USART3 | PB8 TX, PB9 RX |
| Debug UART | USART6 | PA4 TX, PA5 RX |
| Accelerometer | I2C2 | PA6 SDA, PA7 SCL |
| CAN | FDCAN1 | PC5 TX, PC4 RX |
| EG800G USB | USB FS | PA12 DP, PA11 DM, through a common-mode choke |
| User inputs/outputs | GPIO | PC13 button, PC6/PC7 LEDs |
| Monitoring/control | GPIO | PC0 modem PWRKEY, PC1 modem STATUS, PC2 accelerometer INT, PC3 GNSS TIMEPULSE |
| Debug | SWD | PA13 SWDIO, PA14 SWCLK, NRST |

## Power and RF intent

The board accepts 5 V. `FullVerticalPowerTree` creates a 3.3 V logic rail and
a 3.8 V modem rail. The modem rail is modelled as a 2 A-class source and has a
local 100 uF bulk capacitor. The EG800G's 1.8 V `VDD_EXT` output powers the
low-voltage side of a fixed-direction UART translator. Its control input uses
an open-drain driver rather than exposing a 1.8 V modem pin directly to 3.3 V.

The RF paths are represented electrically so they cannot disappear during
backend work:

- EG800G `ANT_MAIN` terminates at a U.FL connector.
- MAX-M10S `RF_IN` terminates at a separate U.FL connector.
- nRF52832 `ANT` passes through an explicit matching network to a 2.4 GHz chip
  antenna.

These networks still need vendor-reference-layout constraints, controlled
impedance, keepouts, ground-via fencing, and final matching values in the
physical design.

## Acceptance stages

1. **Electrical frontend (implemented):** parsing, package resolution,
   hierarchical power module, device mux selections, ERC, and steady-state
   power analysis.
2. **Schematic review:** generate a KiCad schematic, replace generic symbols
   where useful, and review all vendor reference circuits and unused pins.
3. **Production footprints:** complete the currently explicit electrical
   subsets, import manufacturer-verified `.kicad_mod` files, and require exact
   electrical-pad matching.
4. **Physical intent:** add outline, mounting holes, connector locations,
   placement regions, RF/antenna keepouts, differential-pair and impedance
   rules, high-current power constraints, and decoupling placement rules.
5. **Placement and routing:** produce a reviewed placement and routed physical
   IR. Run clearance, connectivity, return-path, and manufacturability checks.
6. **Manufacturing:** export KiCad PCB, Gerber X2, Excellon drill, BOM and
   pick-and-place files; verify them with an independent Gerber viewer and a
   fabrication-rule profile.

## Current blockers before fabrication

- The STM32, nRF52832 and EG800G definitions intentionally expose only the
  pins used by this design. Production footprints require complete package
  pin/bond definitions, including all power, ground, exposed and reserved pads.
- Regulator and level-shifter entries express architectural requirements but
  need concrete orderable manufacturer part numbers and validated support
  components.
- USB VBUS switching/current limiting, USB ESD, CAN protection, SIM ESD, input
  protection, programming-header conventions, crystals and complete vendor
  decoupling/reference circuits must be finalized.
- The present physicalizer creates an unrouted inspection draft. It is not a
  layout engine and must not produce fabrication outputs without routing and
  manufacturing validation stages.

## Running the example

```text
python -m copperscript check examples/full_vertical_board.copper
python -m copperscript power-check examples/full_vertical_board.copper
python -m copperscript compile examples/full_vertical_board.copper -o board.json
python -m copperscript export-kicad examples/full_vertical_board.copper -o full_vertical_board.kicad_sch
```
