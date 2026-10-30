# Route-board inputs

Boards only the `route-board-*` suites run (they are not globbed by the
export, plan-layout or si-check suites and their mutations):

| File | Reaches |
|---|---|
| `bga.copper` | a 7x7 1 mm ball grid: the unified router's ball-grid fanout and its `--unified-save-inputs` sidecar (`PATH.bga.json`); `--via-lattice` with one site |
| `bga_open.copper` | the same grid at 1.27 mm without the In1.Cu zone: a full via lattice (`--via-lattice`, with and without `--array-flow`) |
| `bypass_release.copper` | a back-side bypass capacitor held as copper while a surface-only pair fails past ordinary escape keepouts: the retry re-proves the held copper on the released board (`rebind_bypass_keepout_removal`) |
| `cache_miss/board.copper` | a locked footprint module absent from the package cache: under `--offline` the error record's `FootprintResolutionError` (PKG011) |
| `hole_clearance.copper` | `test_component_hole_clearance`: pads beside their own pegs under a scoped `hole_clearance` (DRC relaxation records, routing-stage requirement row) |
