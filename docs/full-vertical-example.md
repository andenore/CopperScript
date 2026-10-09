# Full-vertical acceptance design

`examples/full_vertical/board.copper` is the integration design used to grow
CopperScript from an electrical description into production Gerber and drill
files. It is intentionally more demanding than the small language examples:
it combines hierarchy, multiple programmable devices, several serial buses,
three RF paths, high-current modem power, debug connectors, and explicit power
states.

Reusable device and part definitions live in canonical CopperLib vendor and
generic packages. The project-only power tree and placeholder level-shifter
modules live under `examples/packages/vertical_support`; the compiler
repository does not duplicate the reusable part definitions.

The example is an engineering fixture, not yet a fabrication-ready reference
design. A clean ERC result means the currently modelled electrical rules are
satisfied; it does not certify RF layout, regulator stability, EMC, antenna
performance, USB or CAN signal integrity, or regulatory compliance.

The [2026-10-06 full rerun](routing-shared-reference-review.md) at `52adb05`
now completes all routing and native DRC gates: 49 ordinary nets, eight critical
nets, zero resource overflow, and zero KiCad violations/unconnected items.
Native Gerber/drill files are generated, with independent CAM skipped by request.
No assembly BOM or qualified-production claim is made. Earlier four-layer
experiments below are historical; the current shared Make preset uses six layers.

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
The default Make workflow binds both the TPS62130A buck and Nordic/Johanson
antenna hard macros. Their footprint-adapted assets live under
`examples/full_vertical/assets/`; their copper and member poses are immutable.
Placement/routing feedback cannot move a hard lock; incompatible constraints
must fail rather than silently relax it.

To generate an **unrouted** placement preview with installed footprints, after
the README setup:

```powershell
New-Item -ItemType Directory -Force build/constrained-placement | Out-Null
uv run --no-sync python -m copperscript plan-layout examples/full_vertical/board.copper `
  --locked --offline --layers 6 --fab-profile jlcpcb-six-layer `
  --hard-macro examples/full_vertical/buck-macro.json `
  --hard-macro examples/full_vertical/nrf-antenna-macro.json `
  --candidates 3 --placement-candidate candidate-00 `
  --report build/constrained-placement/layout-report.json `
  -o build/constrained-placement/placed.kicad_pcb
```

The README's complete `make EXAMPLE=full-vertical route` workflow consumes these
source constraints too. Previous routed boards and any run started before
these constraints were added are historical results, not routing validation
of this floorplan. A fresh full routing/signoff run is required.

The [project-local library exporter](kicad-project-export.md) now writes the
same-stem KiCad project, `fp-lib-table` and canonical `CopperScript.pretty`
footprints automatically. A fresh KiCad 10.0.6 check of this six-layer placed
preview reports zero non-connectivity violations (the previous eight library
warnings are gone) and 204 expected unconnected items. This is still unrouted;
neither that result nor library matching qualifies RF or fabrication readiness.

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

## Explicit critical profiles and cheap preflight

The source now declares separate symmetric USB pairs on the MCU and modem
sides of FL_USB. Their target is 90 ohms differential, consistent with
[Quectel EG800G QuecOpen Hardware Design V1.2, sections 4.1 and 5.3](https://developer.quectel.com/wp-content/uploads/2025/01/Quectel_EG800G%E7%B3%BB%E5%88%97_QuecOpen_%E7%A1%AC%E4%BB%B6%E8%AE%BE%E8%AE%A1%E6%89%8B%E5%86%8C_V1.2.pdf).
The provisional 0.18 mm width / 0.20 mm gap is **not** a qualified 90-ohm
geometry. Top-layer-only, zero-via routing is a conservative prototype choice,
not Quectel's recommendation for an inner shielded USB route. No device length
or skew limit has been invented. A production profile requires actual stackup,
field-solver and return-path evidence.

CELL_RF and GNSS_RF are explicit point-to-point 50-ohm targets. Quectel section
5.3 describes controlled RF feeds; the [MAX-M10S integration manual](https://content.u-blox.com/sites/default/files/MAX-M10S_IntegrationManual_UBX-20053088.pdf)
describes its matched 50-ohm RF input. Neither target qualifies a routed width.
NRF_RF_RAW remains generic critical geometry with **no** 50-ohm claim; its
chip-side matching net is protected as a critical tree. C_BT_MATCH shunts the
chip-side ANT node before L_BT_MATCH, as in the QFAA reference schematic.
NRF_RF_ANT is the point-to-point feed after that inductor. The Johanson
2450AT18A0100001E antenna's terminal 2 is an isolated, soldered NC anchor,
not ground. [Nordic's reference
circuitry and PCB guidelines](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/ref_circuitry.html)
require close adherence to the matching layout, including its ground topology
and inner-layer keepouts. This example has not qualified those requirements or
the antenna; generic connected RF traces are insufficient.

The [RF audit and next implementation steps](rf-layout-audit.md) record these
source-backed corrections. They change the acceptance netlist after pass 5;
the old RF routing measurements are historical, not measurements of the
corrected matching circuit. No complete board or RF signoff is claimed.

Before spending time on ordinary detailed routing, run the same initial
placement/global/critical stages in isolation (after the README setup):

```powershell
uv run --no-sync python -m pcbir.critical_preflight examples/full_vertical/board.copper `
  --locked --offline --layers 6 --fab-profile jlcpcb-six-layer `
  --hard-macro examples/full_vertical/buck-macro.json `
  --hard-macro examples/full_vertical/nrf-antenna-macro.json `
  --candidates 3 --placement-candidate candidate-00 `
  --feedback-iterations 1 --router-iterations 5 `
  --report outputs/critical-preflight.json -o outputs/critical-board.kicad_pcb
```

The report checkpoints resolved inputs and global results, then records critical
strategies, assumptions, rejection reasons and phase timings. Exit 1 means
global/critical acceptance failed; exit 0 only means that early gate passed,
**not** full-board signoff. Unrouted ordinary nets, unfilled planes and native
route-incomplete findings remain expected. The [pass-4 review](routing-review-pass4.md)
records the first real-footprint run: four connected RF nets, two rejected USB
pairs, no committed unsafe pair copper. The earlier completely connected
ordinary-routing draft remains a separate historical artifact.

Add `--package-access` to run the same ordinary fanout, critical routing,
plane-contact and boundary-access gate used by `route-board --fanout`, without
ordinary area routing. Its exit status also requires no pending package or
plane contacts, no failed critical nets and no hard physical findings. The
report retains pending pad identities and checkpoints interrupted searches.
Use `--stitch-surface-zones --plane-contact-radius-mm 5` when the full route
uses those options for opposite-side power pours. This mode exports the board
including its reserved contacts; it still requires native refill and complete
board routing. It cannot be combined with critical-only placement feedback.
Both modes accept repeatable `--hard-macro path/to/scene.json` arguments, bound
before placement and included in the input provenance.

The [pass-5 review](routing-review-pass5.md) adds joint paired package escape
and heading-aware channel search with bounded 1/0.5/0.25 mm refinement. Both
USB pairs connected with zero vias and unchanged placement in that run. Independent
KiCad finds no critical-net opens, shorts, clearance or dangling-track findings;
ordinary nets remain unrouted in this early artifact. The report includes
search counts, expanded states, candidate attempts and measured coupled/
uncoupled lengths. This is geometric progress, not qualified USB/RF performance
or a completed full-board rerun. Subsequent RF topology corrections require a
new rerun: at the old placement the modem pair is pending again, as recorded
in the RF audit. RF reference-layout clusters are next.

With the provisional source-backed RF scene, [pass 9](routing-review-pass9.md)
uses `--critical-feedback-trials 1` to rotate the choke algorithmically and rebuild
all critical groups. Independent KiCad verifies zero critical copper opens, but
the library's USB choke winding mapping was incorrect. Those boards must not be
fabricated: [pass 11](routing-review-pass11.md) corrects the source-backed model
and reruns from fresh placement rather than replaying that rotation. The USB
detours, remaining RF support/keepouts and complete ordinary rerun are still open;
this does not qualify impedance or production readiness.

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
- The locked Git providers plus CopperLib footprint audit resolves all 27
  selected footprint types, including the TPS62130A buck and its inductor.
  This is package/pad-number coverage, not a board-level electrical or
  assembly qualification. In particular, capacitor dielectric, voltage rating,
  effective capacitance at DC bias, and individual MPNs need review.
- The orderable GCT SIM socket, Coilcraft USB choke, and U.FL RF connector now
  resolve to pinned Git footprint assets. CopperScript imports the embedded
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
- Per-net pad grids and four deterministic passes now connect 42 of 58 nets on
  this provisional placement. Native DRC reports 16 opens and incomplete
  routing, with no shorts, clearance errors, or keepout violations. Isolated
  routing succeeds for 15 of those 16 open nets; only GND still exceeds the
  10,000-state budget without other ordinary traces. Alternate route ordering,
  placement, and a verified GND plane/stitching strategy remain necessary.
  This is still not a board ready for manufacture.
- Eight diversified route orders reach 43 of 58 nets without adding spacing
  violations. The remaining 15 opens still block board release.
- Selecting the distinct legal placement `candidate-01` reaches 49 of 58 nets
  after eight passes. Nine nets remain open, all at the 10,000-state search
  limit. The placement is promising but still not fabrication-ready.
- `examples/full_vertical/provisional_4layer.kicad_pcb` is a placed,
  **unrouted** KiCad 10 preview for mechanical and component-location review.
  It is deliberately not a fabrication deliverable; its adjacent layout JSON
  records the placement gate result.
- The source now declares a provisional GND zone on `In1.Cu` with 0.5 mm
  outline inset. It is unfilled intent at source lowering; its actual copper
  connectivity, return path, and clearances are not signed off.
- KiCad 10 successfully parsed and refilled an inspection-only export of this
  zone. Correcting the PCB backend's rotated pad angles reduced KiCad's
  reported shorts from 20 to zero on the same placed draft. KiCad still
  reported 202 unconnected items. A companion `.kicad_pro` now carries the
  explicit 0.09 mm clearance; on the same placement this removed 15 false
  default-rule clearance findings, leaving 336 other KiCad violations. Most
  are silkscreen findings. Preserving imported keepouts inside their owning
  footprints removed eight further false board-level keepout findings,
  leaving 328 violations: 320 silkscreen and eight footprint-library findings.
  All 320 silkscreen findings involved automatically placed reference text.
  Putting draft reference text on `F.Fab` eliminated those print collisions
  without removing assembly identity. KiCad now reports eight footprint-library
  findings and 202 unconnected items on this **unrouted** preview. A proper
  silkscreen label pass and library-identity audit are still required; this
  draft is not a release candidate.
- KiCad 10 parses the exported inspection schematic but reports 136 ERC
  violations on it. Many are caused by the current flattened automatic
  drawing; they are not implied to be 136 distinct board wiring defects.
  The schematic backend needs a separate review and clean KiCad ERC before
  that export can be used for signoff.

## Running the example

The current six-layer source uses `In1.Cu` and `In4.Cu` for GND reference,
four F.Cu GND fill regions around the buck's private copper, and a board-wide
`V3V3` distribution region on `In3.Cu`, each inset 0.5 mm from
the outline. The V3V3 net has 32 pads and is connected by native zone fill
instead of ordinary track-tree routing. Pads above their own distribution
region favor short local via escapes. On the same placement, this reduced
V3V3 F.Cu track length from 41.32 mm to 35.12 mm while increasing V3V3 vias
from 23 to 30. The 2026-10-09 trial in
`build/full-vertical/runs/20261009T065506239127Z` routed all 48 ordinary
nets; KiCad 10.0.6 reported zero unconnected items and zero DRC violations
after zone fill. This confirms geometric connectivity, not rail voltage drop
or thermal performance at the target load.

The inline differential-pair placement refinement moves FL_USB to
`(34, 15) mm, 270°` between U_MODEM and U_MCU. In the subsequent complete
route at `build/full-vertical/runs/20261009T081529667065Z`, the four USB
tracks total 54.82 mm versus 113.26 mm in the preceding route. All 48
ordinary nets routed, and native filled-board KiCad DRC found zero violations
and zero unconnected items. The modem-side USB pair now uses two vias per
member and In2.Cu; the signal geometry and return path still need electrical
review. See [the placement plan and results](critical-inline-placement-plan.md).

With both hard macros and the F.Cu GND fill, the verified `candidate-00`
route at `build/full-vertical/runs/20261009T092725319855Z` connects all 50
ordinary nets. Native filled-board KiCad DRC reports zero violations and
zero unconnected items. The four USB tracks total 60.80 mm; the default
automatic choice of `candidate-02` stretched them to 83.25 mm, so the Make
workflow explicitly selects the verified candidate. These are geometric
results, not electrical qualification of the modem supply or 2.4 GHz path.

USB profiles now permit matched terminal vias to `In2.Cu`, adjacent to the
declared `In1.Cu` GND plane, instead of requiring zero-via top-only routing.
The limit is two signal vias per member. Required GND return vias remain the
default; this example explicitly uses `return_via_policy = "reference_change"`
and `shared_reference_layer = "In1.Cu"` for both members of each USB pair.
Matched F.Cu/In2.Cu transitions sharing that declared reference need no redundant
single-plane-only return via. This is provisional geometric intent; common nominal width/gap does not establish
layer-specific 90-ohm impedance or filled-plane continuity. RF profiles remain
unchanged. See [paired layer transitions](paired-layer-transitions.md).

Use `make EXAMPLE=full-vertical route PROFILE=none` for the current complete
six-layer pipeline. The lower-level four-layer commands below document earlier
experiments, not the successful current run. See the
[complete run and native manufacturing export commands](routing-shared-reference-review.md).

```text
python -m copperscript check examples/full_vertical/board.copper
python -m copperscript power-check examples/full_vertical/board.copper
python -m copperscript compile examples/full_vertical/board.copper -o board.json
python -m copperscript export-kicad examples/full_vertical/board.copper -o full_vertical_board.kicad_sch
python -m copperscript plan-layout examples/full_vertical/board.copper --locked --offline --layers 4 --fab-profile jlcpcb-four-layer --candidates 2 -o full_vertical_placed.kicad_pcb --report full_vertical_layout.json
python -m copperscript route-global examples/full_vertical/board.copper --locked --offline --layers 4 --fab-profile jlcpcb-four-layer --candidates 1 --feedback-iterations 1 --router-iterations 5 -o full_vertical.global-route.json
python -m copperscript route-board examples/full_vertical/board.copper --locked --offline --layers 4 --fab-profile jlcpcb-four-layer --candidates 1 --feedback-iterations 1 --router-iterations 5 --pitch-mm 1 --passes 1 --report full_vertical.route-report.json -o full_vertical.routed-draft.kicad_pcb
python -m copperscript audit-footprints examples/full_vertical/board.copper --locked --offline --json
```

The last command emits a deterministic JSON coverage list (`passed`,
`resolved`, `total`, and per-footprint errors). Standard geometry comes from
the pinned KiCad Git repository; legacy connector, terminal-block, inductor
and transceiver geometry is carried by the pinned CopperLib packages.
