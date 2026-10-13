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
the former local JSON files remain historical regression snapshots. A `source.copper.txt` diagnostic
snapshot records the selected variant; the committed source and repository
manifest/lock remain authoritative.

From the CopperScript repository root, after preparing its pinned dependencies:

```sh
python -m copperscript lock examples/tps62130a_macro/board.copper --locked
python -m examples.tps62130a_macro.trial --offline
```

Use `--output-dir PATH`, repeatable `--footprint-root PATH`, `--offline`,
`--enabled`, or `--rotation 90` to vary the isolated probe. The script runs
KiCad 10 refill and DRC when `kicad-cli` is installed, and requires each owned
GND zone to have actual filled copper and the fixed VIN, SW and VOUT polygons
to remain netted.
No sibling library checkout or executable library-side generator is required.
This is a geometry probe,
not a production regulator qualification.

The enabled geometry fixture adds a synthetic one-land terminal outside the
rigid unit and an ordinary lead to its declared EN port. This terminates the otherwise dangling native track
without changing immutable macro copper; it does not model or qualify an EN
drive circuit. The always-on fixture needs no added terminal.
