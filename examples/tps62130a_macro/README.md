# TPS62130A macro layout probe

This single-regulator board uses the CopperLib electrical module and its current
hard-macro asset. It has no CopperVigo components or ordinary board routing.
The output is a KiCad board for inspecting the regulator's own copper and filled
planes. The default 180° view puts VIN on the left, the inductor and VOUT on the
right, and the broad GND return below, matching the orientation of TI datasheet
Figure 11-1. The committed `board.copper` imports the electrical module by URL
and selects its physical implementation with a `physical_macro` declaration.
The package exports `TPS62130A_MANAGED_ALWAYS_ON` and
`TPS62130A_MANAGED_ENABLED` as Copper definitions with exact footprint URLs.
They preserve the existing managed-footprint adaptations' copper and evidence;
the former local JSON files remain historical regression snapshots. The committed source and repository
manifest/lock are authoritative.

From the CopperScript repository root, after preparing its pinned dependencies:

```sh
copper lock examples/tps62130a_macro/board.copper --locked
copper plan-layout examples/tps62130a_macro/board.copper --locked --offline \
  --layers 6 --fab-profile jlcpcb-six-layer --candidates 1 \
  -o build/tps62130a-macro/board.kicad_pcb --report build/tps62130a-macro/placement.json
```

This places the rigid macro and writes the KiCad board with its locked copper.

The former Python trial script (with `--enabled`, `--rotation 90` and a KiCad 10
refill/DRC check that each owned GND zone has filled copper and the fixed VIN,
SW and VOUT polygons stay netted) was removed with the Python implementation.
Refill and check the exported board with KiCad separately.
No sibling library checkout or executable library-side generator is required.
This is a geometry probe,
not a production regulator qualification.

The enabled geometry fixture adds a synthetic one-land terminal outside the
rigid unit and an ordinary lead to its declared EN port. This terminates the otherwise dangling native track
without changing immutable macro copper; it does not model or qualify an EN
drive circuit. The always-on fixture needs no added terminal.
