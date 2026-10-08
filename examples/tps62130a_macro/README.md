# TPS62130A macro layout probe

This single-regulator board uses the CopperLib electrical module and its current
hard-macro asset. It has no CopperVigo components or ordinary board routing.
The output is a KiCad board for inspecting the regulator's own copper and filled
planes. The default 180° view puts VIN on the left, the inductor and VOUT on the
right, and the broad GND return below, matching the orientation of TI datasheet
Figure 11-1. The generated `board.copper` and local dependency manifest are
saved alongside the board for reproducibility.

From the CopperScript repository root, with a sibling CopperLib checkout and
KiCad footprints installed:

```sh
.venv/bin/python -m examples.tps62130a_macro.trial
```

Use `--copperlib PATH`, `--output-dir PATH`, `--footprint-root PATH`,
`--enabled`, or `--rotation 90` to vary the isolated probe. The script runs
KiCad 10 refill and DRC when `kicad-cli` is installed, and requires each owned
GND zone to have actual filled copper and the fixed VIN, SW and VOUT polygons
to remain netted.
This is a geometry probe,
not a production regulator qualification.
