# Route-board inputs

Boards only the `route-board-*` suites run (they are not globbed by the
export, plan-layout or si-check suites and their mutations):

| File | Reaches |
|---|---|
| `bga.copper` | a 7x7 1 mm ball grid: the unified router's ball-grid fanout and its `--unified-save-inputs` sidecar (`PATH.bga.json`); `--via-lattice` with one site |
| `bga_open.copper` | the same grid at 1.27 mm without the In1.Cu zone: a full via lattice (`--via-lattice`, with and without `--array-flow`) |
| `hole_clearance.copper` | `test_component_hole_clearance`: pads beside their own pegs under a scoped `hole_clearance` (DRC relaxation records, routing-stage requirement row) |
