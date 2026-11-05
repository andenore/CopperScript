# Native manufacturing-file generation

`export-manufacturing` consumes the **final routed KiCad board**, preserving
native copper fill, arcs, circles, cutouts and slots. It requires a matching
`.kicad_pro` so native DRC retains the design rules. It does not reconstruct the
board from electrical IR or bypass DRC.

```sh
copper export-manufacturing build/board.kicad_pcb \
  --kicad-cli kicad-cli --skip-independent-cam -o build/manufacturing
# Optional: include a reviewed BOM and matching JLCPCB placement file.
copper assembly bom board.copper --locked --offline --lock assembly.lock -o build/bom.csv
copper export-manufacturing build/board.kicad_pcb \
  --skip-independent-cam --bom build/bom.csv -o build/assembly-manufacturing
```

KiCad is the default for every step, and stays available. Two options replace
steps with CopperScript's own implementations:

- `--plotter native` writes the Gerbers, drills, IPC-D-356 netlist and
  positions with CopperScript's own plotter ([native plotting](native-plotting.md)).
  The files sit in the same places, except that positions are `positions.csv`
  instead of `positions-kicad.csv`.
- `--drc native` refills the zones with [`copper fill`](native-filling.md) and
  gates the export on [`copper drc`](native-drc.md) instead of `kicad-cli pcb
  drc`. Its report is `drc.json`. The plotted Gerbers and drills, from either
  plotter, are then read back, and their connectivity is checked against the
  board netlist.

With `--drc native --plotter native`, KiCad is not run and need not be
installed. The manifest records the choices: `drc`, `plotter`, `cam_readback`
and `drc_not_checked` (the KiCad checks the native DRC does not make).
`kicad_version` is `null` when KiCad was not used.

```sh
copper export-manufacturing build/board.kicad_pcb --skip-independent-cam \
  --drc native --plotter native -o build/manufacturing
```

The explicit acknowledgement is mandatory. This is **manufacturing-file
generation**, not a qualified release. The Python implementation's strict
release API and its qualification gates were removed with it. Firmware is not
required to manufacture an unprogrammed board.

The command stages a copy, refills its zones, runs native DRC (KiCad's at all
severities, or CopperScript's), and refuses violations, opens, incomplete reports, failed exports
or an existing output directory. Failed stages are removed; the input board
is never changed. It exports every declared copper layer, both masks,
silkscreens and pastes, Edge.Cuts, metric plated/nonplated drills and IPC-D-356.
Gerbers subtract mask openings from silkscreen. Absolute drill and plot origins
are consistent. `gerbers-drill.zip` is the fab upload: only artwork and drills,
flat at the archive root as in JLCPCB's KiCad guide (JLCPCB reported missing
drills when they sat in a `drill/` folder). Reports,
PCB/project, native positions, manifest and SHA-256 checksums accompany it.
`manufacturing-package.zip` packages all these files and their checksums; the
checksum list covers archive contents, not the outer package itself. It is the
archive, **not the fab upload**: it also holds the PCB, reports, footprints and the
nested `gerbers-drill.zip`, and JLCPCB reported its drills as missing. Upload
`gerbers-drill.zip` for the board, and `bom.csv` / `cpl.csv` for assembly; the
command prints these paths.
Inventory checks are not independent geometric CAM verification.
For repeat builds, `--replace` permits replacement of a recognized generated
directory only after all new exports pass; previous output is retained in a
`.manufacturing-previous-*` sibling, not deleted. Inputs must live outside the
output directory. Review/remove old ignored build snapshots when no longer needed.

With `--bom`, JLCPCB CPL columns are generated from native KiCad millimetre
positions, both sides, a shared unmirrored origin and native CCW rotations.
Reference sets must match; unpopulated references are filtered explicitly.
Centroids are KiCad footprint origins, not inferred body or pad centroids:
use centred footprints and check asymmetric packages. No guessed global
rotation correction is applied. Verify pin 1, LED/battery polarity and
supplier-specific orientation in the assembly preview before submitting.
See [JLC's native KiCad export guidance](https://jlcpcb.com/help/article/how-to-generate-the-bom-and-centroid-file-from-kicad)
and [CPL conventions](https://jlcpcb.com/help/article/pick-place-file-for-pcb-assembly).

The manifest states `independent_cam=skipped_by_request`,
`qualified_release=false`, and `supplier_availability=not_checked`.
Native DRC does not establish supplier stock, assembly capability, electrical
function, acceptable component selections or factory order approval. Select
fabrication parameters when ordering; exported files do not purchase anything.
