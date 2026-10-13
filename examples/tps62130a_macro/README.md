# TPS62130A macro layout probe

This single-regulator board uses the CopperLib electrical module and its current
hard-macro asset. It has no CopperVigo components or ordinary board routing.
The output is a KiCad board for inspecting the regulator's own copper and filled
planes. The default 180° view puts VIN on the left, the inductor and VOUT on the
right, and the broad GND return below, matching the orientation of TI datasheet
Figure 11-1. The committed `board.copper` imports the electrical module by URL
and selects its physical implementation with a `physical_macro` declaration.
The existing managed-footprint adaptation preserves the pinned library copper;
its always-on variant only rebinds the reviewed footprint identities. The enabled
variant reuses the full-vertical adaptation. A `source.copper.txt` diagnostic
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
