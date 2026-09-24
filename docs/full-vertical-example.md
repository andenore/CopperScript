# Full-vertical acceptance design

`examples/full_vertical_board.copper` is the integration design used to grow
CopperScript from an electrical description into production Gerber and drill
files. It is intentionally more demanding than the small language examples:
it combines hierarchy, multiple programmable devices, several serial buses,
three RF paths, high-current modem power, debug connectors, and explicit power
states.

Reusable device, part, connector, and power-module definitions live in the
separate CopperLib package
`github.com/andenore/CopperLib/packages/full_vertical`. The local development
replacement in `copper.mod` points at the sibling `CopperLib` checkout; the
compiler repository does not keep duplicate library definitions.

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

Both SWD targets are bare-PCB Tag-Connect TC2050 footprints, wired for the
TC2050-ARM2010 adapter's SWD pinout. There is no fitted debug header or BOM
component at either target; production programming needs a compatible TC2050
cable/adapter. Pads 7-9 are unused, pin 6 (SWO) is optional, and pin 5 is
ground, not a debugger-supplied power input. The footprint's no-via/no-pour
and component-placement keepouts are preserved by the physical pipeline.

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
| Monitoring/control | GPIO | PC0 modem PWRKEY, PC1 modem STATUS, PC2 accelerometer INT, PC3 GNSS TIMEPULSE, PC8/PC9 USB-C current class, PC10 modem-buck enable |
| Debug | SWD | PA13 SWDIO, PA14 SWCLK, NRST |

## Power and RF intent

The intended input is 5 V at no more than 2 A through a GCT USB4135-GF-A
power-only USB-C receptacle. This is an *input target*, not a proven full-board
maximum. Quectel separately requires the 3.8 V modem rail to be capable of
2 A, or 7.6 W at the modem. At a hypothetical 90% modem-buck efficiency, that
alone uses 8.44 W / 1.69 A from a nominal 5 V source, leaving only 1.56 W /
0.31 A for the logic rail, USB_VBUS, protection losses, and margin. At 85%
efficiency it uses 8.94 W / 1.79 A. Input-voltage drop reduces the available
power further. These efficiencies are budgeting assumptions, not qualified
measurements or guarantees from the selected regulator. The modem rail now
uses a TPS62130ARGTR 3 A synchronous buck with a Coilcraft XAL4020-222MEC
2.2 uH inductor. Its 750 kOhm / 200 kOhm feedback divider targets 3.8 V from
TI's 0.8 V reference. A measured worst-case load budget, including regulator
efficiency and USB input losses, remains necessary before accepting the 2 A
input limit. The user's previously successful TLV76701DRV-family design is a
useful prototype reference, but TI rates that LDO for 1 A, so it cannot be
used here as evidence of a 2 A modem-supply capability.

USB Type-C current advertisement has default, 1.5 A, and 3 A classes, not a
native 2 A class. This board is a **sink**: it does not advertise 3 A itself.
It requires a source advertising the 3 A Type-C class before it enables the
modem, while its planned input draw remains capped at 2 A. The TUSB320LAI
handles the sink CC pull-downs and reports the source class on open-drain
OUT1/OUT2. Its GPIO-mode code is OUT1=low and OUT2=low for an attached
3 A source; unattached, default-current, and 1.5 A codes must all keep the
modem disabled. PC8/PC9 read these signals via 10 kOhm pull-ups to 3.3 V;
PC10 drives MODEM_EN, with a 100 kOhm hardware pull-down so the buck remains
off through reset. Firmware must implement the class check, deassert MODEM_EN
on detach or current-class downgrade, and enforce the 2 A input budget.
There is not yet firmware or a hardware-only current limiter, so this policy
is an integration requirement, not a validated protection mechanism.

The GCT receptacle is rated for 3 A collectively across its VBUS contacts.
There are **no separate 5.1 kOhm CC resistors**, because the TUSB320LAI has
dead-battery sink pull-downs. VBUS_DET is fed through 900 kOhm per TI's
datasheet. The board still needs input overvoltage/inrush protection. It must
not assume that every USB-C supply can deliver the modem's peak load.
`FullVerticalPowerTree` creates a 3.3 V logic rail and a switched 3.8 V modem
rail, with 22 uF local buck output capacitance and a separate 100 uF modem
bulk capacitor. The EG800G's 1.8 V `VDD_EXT` output powers the
low-voltage side of a fixed-direction UART translator. Its control input uses
an open-drain driver rather than exposing a 1.8 V modem pin directly to 3.3 V.

Power-source and device references: [USB Type-C Specification R2.0 §2.3.4](https://www.usb.org/sites/default/files/USB%20Type-C%20Spec%20R2.0%20-%20August%202019.pdf),
[Quectel EG800G QuecOpen Reference Design V1.1, VBAT design](https://developer.quectel.com/wp-content/uploads/2025/01/Quectel_EG800G_Series_QuecOpen_Reference_Design_V1.1.pdf), and
[TI TUSB320LAI datasheet](https://www.ti.com/lit/ds/symlink/tusb320lai.pdf),
[TI TPS62130A datasheet](https://www.ti.com/lit/ds/symlink/tps62130.pdf), and
[TI TLV767 datasheet](https://www.ti.com/lit/ds/symlink/tlv767.pdf).

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

- The EG800G-EU now has a 109-pad third-party JLCPCB/EasyEDA footprint in
  CopperLib and all physical pads are represented in the package definition.
  Quectel-identified ground pads are connected; other unverified functions are
  explicit unmodeled placeholders that ERC refuses to connect. The EasyEDA
  symbol conflicts with Quectel's QuecOpen reference on some multifunction
  pads, so complete electrical pin validation and mechanical/stencil review
  remain production blockers. The nRF52832-QFAA and STM32G0C1RET6 package maps are complete
  from the Nordic Product Specification v1.9, Table 1 and ST DS13564 Rev 5,
  Table 12 respectively; their support circuits remain incomplete.
- The installed KiCad 10 plus CopperLib footprint audit resolves all 27
  selected footprint types, including the TPS62130A buck and its inductor.
  This is package/pad-number coverage, not a board-level electrical or
  assembly qualification. In particular, capacitor dielectric, voltage rating,
  effective capacitance at DC bias, and individual MPNs need review.
- The orderable GCT SIM socket, Coilcraft USB choke, and U.FL RF connector now
  resolve to installed KiCad footprints. CopperScript imports the embedded
  copper keepouts in the SIM and U.FL footprints and carries them through
  placement, routing, DRC, geometry-bound signoff, and KiCad export.
- The MAX-M10S footprint is generated in CopperLib from u-blox's published
  18-land geometry and separate T-shaped stencil recommendation. The latter
  assumes the manual's 150-um stencil; fabricator review remains necessary.
- The 5 V USB-C entry has a 3 A source-class detection path and default-off
  modem enable, but firmware, input overvoltage/inrush/current protection,
  input power budgeting, and a load-transient/thermal qualification remain.
  The EG800G `USB_VBUS` connection while VBAT is off also needs a back-power
  review against Quectel's reference design.
- The 3.3 V logic regulator and UART level-shifter still need concrete
  orderable manufacturer selections and validated support components. The
  TPS62130A circuit needs measured 5 V-to-3.8 V efficiency, 2 A modem burst
  response, stability with all bulk capacitance, and RF-noise qualification.
- USB VBUS switching/current limiting, USB ESD, CAN protection, SIM ESD, input
  protection, programming-header conventions, crystals and complete vendor
  decoupling/reference circuits must be finalized.
- The provisional outline is 100 x 80 mm. A legal placement with all real
  footprints and four copper layers now exists. This is an inspection layout,
  not a mechanical design: connector alignment, mounting holes, enclosure,
  antenna clearance and assembly access still require review. The intended
  fabrication choice is [JLCPCB's nominal 1.6 mm four-layer
  `JLC04161H-7628` stackup](https://jlcpcb.com/impedance)
  (F.Cu / In1.Cu / In2.Cu / B.Cu). CopperScript
  records four copper layers and nominal thickness but does not yet encode
  that stackup's dielectric geometry or derive controlled-impedance widths.
  USB and RF traces therefore cannot pass signoff yet. The four-layer
  manufacturing profile includes all four copper Gerbers and refuses to
  omit an inner layer, but no fabrication release has been generated.
- On the provisional placement, five bounded congestion-negotiation iterations
  reached all 58 multi-terminal nets with zero grid overflow, certifying the
  placement for the global-routing model. Global-routing guides are not copper.
  The earlier cost-only detailed route on a 1 mm grid connected all 58 nets
  nominally but still had 153 shared routing resources. Exact physical DRC
  found 683 shorts, 368 clearance violations, and 13 open nets. The new
  geometry-checked router refuses those shorts. A fresh one-pass check of the
  current implementation on a 1 mm grid with the explicit four-layer JLCPCB
  prototype profile connected 32 nets and left 26 open. Native DRC found no
  shorts, clearance errors, or copper-keepout violations; it still reports
  the 26 opens and incomplete route. These are release
  blockers, not waiver candidates. Pin-access, placement/routing feedback,
  return-path review, clean physical and
  KiCad DRC, and independent CAM verification still gate any fabrication
  output. No production Gerbers should be exported from the current draft.
  Pad-centered access removed the observed pin-access failures in a guided
  20,000-state search, but still connected only 32 of 58 nets in one pass;
  24 nets hit the bounded search limit and two were unreachable in that route
  order. A grounded inner-plane/stitching strategy and iterative rip-up are
  needed before the physical route can close.
- `examples/full_vertical_provisional_4layer.kicad_pcb` is a placed,
  **unrouted** KiCad 10 preview for mechanical and component-location review.
  It is deliberately not a fabrication deliverable; its adjacent layout JSON
  records the placement gate result.
- The source now declares a provisional GND zone on `In1.Cu` with 0.5 mm
  outline inset. It is unfilled intent at source lowering; its actual copper
  connectivity, return path, and clearances are not signed off.
- KiCad 10 successfully parsed and refilled an inspection-only export of this
  zone. Correcting the PCB backend's rotated pad angles reduced KiCad's
  reported shorts from 20 to zero on the same placed draft. KiCad still
  reported 202 unconnected items and 351 other violations after the fix;
  these remain open, and this draft is not a release candidate.
- KiCad 10 parses the exported inspection schematic but reports 136 ERC
  violations on it. Many are caused by the current flattened automatic
  drawing; they are not implied to be 136 distinct board wiring defects.
  The schematic backend needs a separate review and clean KiCad ERC before
  that export can be used for signoff.

## Running the example

```text
python -m copperscript check examples/full_vertical_board.copper
python -m copperscript power-check examples/full_vertical_board.copper
python -m copperscript compile examples/full_vertical_board.copper -o board.json
python -m copperscript export-kicad examples/full_vertical_board.copper -o full_vertical_board.kicad_sch
python -m copperscript plan-layout examples/full_vertical_board.copper --locked --offline --layers 4 --fab-profile jlcpcb-four-layer --footprint-root path/to/kicad-footprints --footprint-root ../CopperLib/footprints --candidates 2 -o full_vertical_placed.kicad_pcb --report full_vertical_layout.json
python -m copperscript route-global examples/full_vertical_board.copper --locked --offline --layers 4 --fab-profile jlcpcb-four-layer --footprint-root path/to/kicad-footprints --footprint-root ../CopperLib/footprints --candidates 1 --feedback-iterations 1 --router-iterations 5 -o full_vertical.global-route.json
python -m copperscript route-board examples/full_vertical_board.copper --locked --offline --layers 4 --fab-profile jlcpcb-four-layer --footprint-root path/to/kicad-footprints --footprint-root ../CopperLib/footprints --candidates 1 --feedback-iterations 1 --router-iterations 5 --pitch-mm 1 --passes 1 --report full_vertical.route-report.json -o full_vertical.routed-draft.kicad_pcb
python -m copperscript audit-footprints examples/full_vertical_board.copper --locked --offline --footprint-root path/to/kicad-footprints --footprint-root ../CopperLib/footprints --json
```

The last command emits a deterministic JSON coverage list (`passed`,
`resolved`, `total`, and per-footprint errors) for the separate CopperLib
generation workflow. All 27 selected footprint types currently resolve with
KiCad 10 and the sibling CopperLib checkout; a new mismatch makes it fail.
