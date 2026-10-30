# Full-vertical integration example

`examples/full_vertical/board.copper` exercises hierarchical circuits, multiple
devices, buses, three RF paths, modem power and explicit power states. Reusable
definitions come from pinned library packages; project-only support composition
remains under `examples/packages/vertical_support`.

## Latest documented status

The six-layer two-macro `candidate-00` checkpoint in
`build/full-vertical/runs/20261009T092725319855Z/` connects all 50 ordinary nets;
native filled-board KiCad 10.0.6 DRC has zero violations and zero unconnected
items. This is recorded geometric evidence, not a new validation run or a
fabrication-ready reference. Revalidate after changed source, assets or tools.

The buck macro's conductor target is below 0.5 A per output rail, while the modem
requires 2 A peak. Its current/voltage-drop, thermal and burst adequacy is unproven.
Nordic matching/antenna performance, USB impedance/returns and independent CAM
remain unresolved. No approved assembly BOM or production release is claimed.

## Selected parts

| Function | Selection | Reason |
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

Both SWD ports use fitted Samtec FTSH-105-01-L-DV-007-K 1.27 mm, 2x5
surface-mount Cortex Debug headers. Position 7 is omitted for cable keying,
pin 9 is grounded as GNDDetect, pin 6 (SWO) is optional, and pin 8 is unused.
The headers are BOM items. Their CopperLib footprint uses nine SMD pads and
the connector land pattern must receive normal assembly review.

## Fixed mechanical floorplan

The source declares hard `fixed_placement` constraints for the requested
mechanical floorplan on the provisional **100 x 80 mm** outline. Coordinates
are in millimetres from the top-left board origin: x increases rightwards and
y downwards. They locate the **footprint origin**, not necessarily its body
centre (notably, the CAN connector origin is pin 1). All these parts are on
the front side; positive rotation follows KiCad's counterclockwise convention.

| Part | Origin (x, y), mm | Rotation | Intent |
| --- | --- | --- | --- |
| J_POWER | (12, 74) | 0° | USB-C at bottom left, opening toward bottom |
| J_CAN | (25, 72) | 0° | CAN beside USB-C at bottom left |
| U_MODEM | (20, 20) | 90° | Modem at top left, ANT_MAIN pad 35 facing north |
| J_CELL | (13.4, 6) | 90° | Cellular U.FL above modem, signal land toward modem |
| J_SIM | (10, 44) | 270° | SIM holder on left, insertion toward left |
| U_NRF | (84, 12) | 0° | Nordic RF macro anchor at top right |
| PWR/U_MODEM | (54, 48) | 0° | Enabled TPS62130A macro anchor; private top copper |
| U_GNSS | (86, 67) | 0° | GNSS at bottom right |

The Nordic hard macro places ANT_BT and its matching network along the
upper-right corner. These are legal prototype positions, not a claim
that connector bodies are flush with an enclosure or that RF keepouts are
qualified. Antenna ground clearance, feed geometry, enclosure access and
mechanical tolerances still need review. Changing the outline requires
reviewing these coordinates.

`J_GNSS` remains automatic but must be within 5 mm of the GNSS RF input.
Other components remain automatic and retain their existing proximity rules.
The board source selects both the TPS62130A buck and Nordic/Johanson antenna
hard macros through `physical_macro` declarations. The buck uses module-relative
bindings (`on PWR`); no additional Make/CLI macro arguments are needed.
Their footprint-adapted assets live under
`examples/full_vertical/assets/`; their copper and member poses are immutable.
Placement/routing feedback cannot move a hard lock; incompatible constraints
must fail rather than silently relax it.

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
input limit. TI rates TLV767-family LDOs for 1 A; they cannot establish a 2 A modem supply.

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

## Routing and reference intent

USB has explicit symmetric pairs on both sides of FL_USB, targeting 90 ohms;
0.18 mm width / 0.20 mm gap is provisional. Matched terminal transitions may use
In2.Cu with at most two signal vias per member. The explicit `reference_change`
policy declares shared In1.Cu GND; it does not establish actual return continuity
or layer-specific impedance. Review adjacent planes, discontinuities and launches.

CELL_RF/GNSS_RF target 50 ohms. The Nordic chip-side matching tree has no generic
50-ohm claim. Preserve source-backed shunt/series topology, prescribed VSS31
return, Johanson NC anchor, tee matching and antenna exclusions in the immutable
macro. Package/source evidence belongs to the owning library.

The source declares In1.Cu/In4.Cu GND references, F.Cu GND regions outside protected
macro geometry and In3.Cu V3V3 distribution. Only native filled evidence establishes
plane connectivity. Asset scenes bind footprint adaptations and original asset
identities; their copper cannot be redrawn by feedback.

## Remaining release obligations

- Complete EG800G electrical pin validation and third-party footprint/stencil
  review; unverified multifunction pads remain unmodeled and cannot be connected.
- Select exact logic-regulator/level-shifter and passive/crystal parts. Review
  support/ESD, USB VBUS switching/current limits, CAN/SIM and input protection.
- Validate effective capacitors, converter stability/efficiency, modem bursts,
  input budget, default-off/current-class firmware and back-power states.
- Review connector/enclosure/mounting access, RF keepouts and antenna adaptations
  on an exact stackup with field-solver and prototype evidence.
- Generate fresh source-bound native, assembly and manufacturing review files;
  independently qualify final CAM and obtain the appropriate human approvals.

## Run

From the repository root after dependency bootstrap:

```console
make EXAMPLE=full-vertical check
make EXAMPLE=full-vertical route
```

The shared six-layer preset supplies macros and candidate settings. Use
[Make builds](make-builds.md) for native paths, new run directories and exports.
For a diagnostic of the critical stage, use `copper replay-critical STATE` on a
placement checkpoint or saved inputs from a run with the same locked
inputs/layers/macros; success covers only that stage, not ordinary routing, fill
or release. (The former `critical_preflight` early gate with package access was
removed with the Python implementation.) Inspect the complete route
report and independent native result before using an output PCB.
