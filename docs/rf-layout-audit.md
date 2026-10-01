# RF reference-layout audit and next implementation steps

This starts R8c after the verified joint USB routing increment. It is not an
RF-qualified layout or completed board. Electrical corrections deliberately
change the source after pass 5; historical copper measurements are not carried
forward as measurements of the corrected matching circuit.

## Source-backed electrical corrections

The shared antenna was generic `RF_ANTENNA` on a Johanson 2450AT18x100
footprint, with terminal 2 incorrectly called GND and connected to board ground.
[Johanson Doc# 36S0021A Revision 4.0](https://www.johansontechnology.com/docs/3827/Antenna-2450AT18A0100001E-Rev4.0.pdf),
page 2, identifies terminal 1 as feed and terminal 2 as a soldered NC mechanical
anchor. CopperLib now declares the explicit `JOHANSON_2450AT18A0100001E`,
records source identity/hash, and marks NC `do_not_connect` without inventing
an electrical profile. The example keeps land 2 but assigns it to no net.
Regression tests reject any net on that terminal. This is not permission to
omit solder, remove the land, or let a ground fill contact it.

The [Nordic QFAA reference archive v1.1](https://nsscprodmedia.blob.core.windows.net/prod/software-and-other-downloads/reference-layouts/nrf52832qfaxreflayoutv11.zip)
contains `nrf52832_qfaa_schematic.pdf`, revision 1.1, 30 November 2016,
sheet 1. C3 (0.8 pF) shunts ANT pin 30 to ground **before** series L1 (3.9 nH).
The example previously shunted the inductor output. It now correctly uses:

| Net | Members | Routing intent |
| --- | --- | --- |
| NRF_RF_RAW | U_NRF.ANT, C_BT_MATCH.1, L_BT_MATCH.1 | Critical chip-side tree; no 50-ohm claim |
| NRF_RF_ANT | L_BT_MATCH.2, ANT_BT.FEED | Point-to-point provisional 50-ohm feed |
| GND | C_BT_MATCH.2; not ANT_BT.NC | Shunt reference; actual return topology still unqualified |

Compact source hashes/locators are retained in CopperLib's
`data/full-vertical/rf-audit.json`; original documents remain ignored cache
assets. The schematic and relevant antenna drawing pages were visually checked,
not inferred from flattened PDF text. The matching correction does not complete
the Nordic crystal, DEC/VDD decoupling or manufacturer reference-ground circuit.

## Placement implementation sequence

### Corrected-netlist probe

Task artifacts `routing-review-pass6-rf-audit-2026-10-01` contain
`corrected-matched-preflight.json`, the partial PCB and a disposable KiCad
refill/DRC result. This probe compiles locked/offline inputs and replays pass 5's
51 legal placements, then recomputes global and critical routing. It is not a
fresh end-to-end placement optimization or complete ordinary-net route. The
intermediate NC-only placement run was stopped when the later matching
correction made its input obsolete; its resolved checkpoint is not an accepted
result.

Corrected raw matching tree: 66.14 mm; inductor-to-antenna feed: 15.82 mm.
Cellular/GNSS feeds remain 6.85/34.31 mm. The MCU pair connects (34.79/37.14 mm),
but the modem pair produces no accepted candidate against the new reservations
within 24 searches / 90,035 expanded states. No unsafe modem copper is locked.
Native checking finds 52 open nets plus full-route incompleteness, no hard
geometry errors. Independent KiCad finds 196 expected partial-board unconnected
items, including both modem-pair members, eight unchanged library findings and
no short/clearance/dangling violations. The six accepted critical nets have no
independent opens. These counts are not a completed-board regression benchmark.

Resolve/replay, global and critical timings were 0.29/31.58/235.96 seconds,
with tests running concurrently. Source/global/critical/native fingerprints
are in the report. All 333 CopperScript and 24 CopperLib tests pass; CopperLib's
compatibility reports remain deterministic and the example passes locked ERC.

This confirms why RF placement must precede another full board route: fixing
electrical intent exposes the old matching placement as unsuitable. Enlarging
search budgets or restoring the wrong capacitor/ground connection is not RF
layout qualification.

### Next steps

1. Complete the bounded Nordic support circuit from its reference schematic.
   Extract local member/pad transforms and matching-ground topology from the
   corresponding vendor PCB and placement data. Bind the template to specific
   part/package/footprint identities and provenance. A soft proximity group is
   not a substitute for that reference geometry.
2. Add rigid RF-cluster placement in the physical domain. Translation and
   explicitly allowed rotation move the entire cluster, including its local
   copper/reference keepouts. Pin bindings stay electrical; presentation and
   absolute board coordinates do not enter the semantic IR. Reject incompatible
   footprint templates and illegal courtyard/pad/keepout transforms.
3. Implement the Johanson corner-mounting/ground-clearance arrangement from
   page 3. Preserve the isolated solder anchor and a separate feed/matching
   region. Its evaluation-board dimensions and matching values are not a
   qualified six-layer tracker design. Layer scope and board-specific tuning
   remain explicit unresolved work, not guessed qualification metadata.
4. Place the GNSS receiver and connector as a short RF cluster, separating its
   sensitive front end from noisy digital/cellular/Bluetooth circuitry.
   [MAX-M10S integration manual UBX-20053088 R05](https://content.u-blox.com/sites/default/files/MAX-M10S_IntegrationManual_UBX-20053088.pdf),
   section 4.4, page 80, calls for short antenna connections, separation from
   interference sources and strong ground references. Its 5 mm separation
   guidance must not be misapplied as arbitrary spacing within a matching
   network. Check antenna isolation/interference and ground-plane current paths
   separately from short-wire optimization.
5. Evaluate cluster moves against placement legality, critical-route geometry,
   ordinary-net reachability and existing reservations. Commit only a validated
   improvement, then repeat the complete route, independent KiCad review and
   actual stackup/impedance/return-path qualification. Do not claim RF usability
   simply because an exact router connects the pads.

The audit corrects electrical intent first; RF placement/keepout implementation
and production qualification remain open on the routing working list.
