# Managed footprint dependencies

This isolated project imports reusable resistor/capacitor parts and resolves
their geometry from the official KiCad 10.0.0 release commit. The resistor uses a
manifest-bound KiCad namespace; the capacitor uses an exact module URL. Both
share one repository download and one complete content inventory.

From the repository root, install the compiler and prepare the dependency:

```text
python -m pip install -e .
python -m copperscript lock examples/managed_footprints/board.copper
python -m copperscript audit-footprints examples/managed_footprints/board.copper --locked --offline --json
python -m copperscript export-kicad-pcb examples/managed_footprints/board.copper --locked --offline -o build/managed-footprints.kicad_pcb
```

The first command that resolves managed geometry downloads the complete pinned
footprint repository into this project's `.copper-cache`. The example's
`copper.lock` is created by `lock`; commit it in your own project. `--locked`
verifies the selected revision and inventory, and `--offline` requires the cache
to exist already. A lockfile alone does not contain the downloaded geometry.

No KiCad installation or `--footprint-root` is needed to audit or generate the
PCB. Native KiCad inspection remains a separate tool step. The layout is an
inspection draft; this example does not attempt manufacturing qualification.
See [footprint dependencies](../../docs/footprint-dependencies.md).
