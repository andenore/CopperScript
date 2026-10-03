# Mechanical geometry specification and implementation plan

Status: accepted design; implementation tracked below. No released-schema migration.

## Scope and ownership

Electrical IR continues to describe parts, nets and electrical intent without
schematic coordinates. A separate mechanical/physical description defines real
board material, holes, keepouts and placement datums. The physical IR is the
canonical lowered geometry for placement, routing, DRC, KiCad and manufacturing.
Geometry is not a drawing-only backend annotation or a fake electrical net.

Initial scope: one connected board with a simple concave or convex polygonal
outer boundary, named strictly interior polygonal cutouts and round NPTH
mechanical holes. Straight/circular-arc boundaries, rounded shapes and slots
follow after the first complete slice. Plated mounting holes remain component
pads with ordinary net assignments; this feature does not introduce a second
connectivity definition. Panelization, castellations, blind cavities, countersinks
and CAD import are deferred and must not be silently approximated or accepted.

## Canonical representation

- Integer nanometres and stable feature IDs, independent of KiCad IDs.
- `BoardOutline` retains an outer boundary and gains named `BoardCutout` loops.
  `BoardGeometry` is the future aggregate vocabulary, not a parallel shape source.
- `MechanicalHole`: stable ID, centre, finished round drill diameter and optional
  screw-head clearance radius. It has no net or BOM entry. Initial holes are NPTH
  through the full board. Positive dimensions are mandatory; odd-nanometre radii
  must be treated conservatively rather than rounded down in legality checks.
- Cutouts remove substrate. Holes remove substrate and must appear in drill
  output. Screw-head keepouts remove placement space, not substrate/copper unless
  the user explicitly requires copper exclusion. No automatic screw dimensions.
- Component-footprint NPTH holes and board-level holes share clearance and CAM
  inspection logic but retain their respective ownership; no duplicate export.
- Repeated closing vertices are normalized. Zero-length edges, repeated interior
  vertices, zero area, self-intersection, touching/intersecting/nested cutouts,
  cutouts outside the board and intersecting holes fail validation.
- Minimum residual material web, drill size/spacing, routed-slot tool size and
  process tolerances are fabrication-profile rules, not arbitrary parser limits.

## Shared legality

One shared geometry query layer determines whether a point, swept track/via or
component courtyard is inside real material with its required margin. It must
check every outer/cutout edge and drill void, including concave crossings whose
endpoints appear legal and a courtyard enclosing an entire cutout or hole.
Bounding boxes and routing grids are broad phases only. Physical geometry and
independent export verification remain acceptance gates.

Reuse existing integer/Fraction predicates for polygon topology and distances.
Keep the rectangle fast path for unchanged boards. Cache immutable geometry
where useful; do not add a numerical/CAD dependency before the predicates require
one. Arc support must preserve exact source arcs for export and use explicit,
bounded conservative approximation where a consumer needs polygons; rounding
must never enlarge legal routing space or shrink an obstacle.

All placement, global/detailed/critical routing, escapes, plane contacts and DRC
must consume these queries. Hard macros cannot introduce a mechanical bypass.
Unsupported geometry must fail closed while a consumer is being integrated.
Copper-to-edge, copper-to-hole and drill-to-drill requirements are distinct.

## Language and export

Add a dedicated mechanical block in `.copper` after the model/query foundation
is verified. Provide polygon/rectangle constructors, named cutouts, holes and
datums; subsequently add circles, rounded rectangles and line/arc paths. Freeze
exact syntax with grammar/diagnostic tests rather than treating draft examples
as supported syntax. Placement may reference named datums/edges; no implicit
"bottom-left corner" convention for arbitrary boundaries. Imported geometry
will later use explicit units/transforms and locked data assets, without hidden
gap healing or executing external content.

KiCad exports outer/cutout loops on `Edge.Cuts`; native drilled mechanical holes
lower to generated, BOM-excluded NPTH footprint objects. Plated component holes
retain their existing pad export. Circle/arc output must preserve actual curves,
not a coarse routing polygon. Excellon PTH/NPTH and routed operations must be
classified and reconciled against expected feature identity, positions, dimensions,
plating and process. Gerber outline topology must match physical IR, not just
have a plausible bounding box or number of files. STEP/3D is useful inspection,
not the authoritative mechanical input or manufacturing proof.

References: [KiCad 10 outlines and cutouts](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html#_board_outlines_edge_cuts),
[KiCad mechanical pads/rules](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html#_working_with_pads),
[JLCPCB manufacturing capabilities](https://jlcpcb.com/capabilities/pcb-capabilities/).
Fabrication values must be recorded with profile/source revision and applicable
stackup/process; published limits can change.

## Implementation sequence / acceptance checklist

1. [x] Polygon/cutout/hole IR, topology validation and shared exact material
   queries. Regression cases: concave notch, enclosed void, tangent clearance,
   malformed boundary, outside/nested cutouts, odd dimensions and overlapping
   holes. Include holes/cutouts in physical identity and signoff digests.
2. [ ] Shared legality integrated into placement, global/detailed/critical
   routing, fanout/plane escapes and native DRC. Preserve rectangle behaviour;
   routes crossing a notch/hole/cutout must be rejected, including soft rip-up.
3. [x] KiCad polygon/cutout and round NPTH export with deterministic assets,
   native outline/clearance DRC and round-hole Excellon reconciliation tests.
4. [x] Mechanical language block and compiler lowering: circle/rectangle/polygon,
   cutouts, NPTH holes, physical rules, source diagnostics and CLI precedence.
   See [mechanical language](mechanical-language.md). Named placement datums and
   datum-relative placement references remain deferred, not implemented syntax.
5. [ ] Nonrectangular copper-zone generation/refill and manufacturing outline
   reconciliation. Do not claim closure from unfilled polygons or KiCad alone.
6. [ ] Complete routed L-shaped example with cutout/mounting holes, malformed
   examples, per-layer/mechanical renderings and CI smoke coverage.
7. [ ] Native arcs/circles/rounded corners, conservative query geometry, slots
   and their CAM/tooling checks; second curved-board example.

Each checked item requires tests. Commit major completed increments. No unrelated
example-board placement or routing changes and no fabrication-ready claim from
this feature alone.

### Initial increment: implemented and bounded

`pcbir/mechanical.py` supplies shared exact polygon/material predicates.
Placement, detailed/global routing, shared critical/fanout/plane copper
acceptance and physical DRC reject cutouts and round holes. Item 2 remains open
for complete filled-zone coverage: any normalized fill on a board with the new
voids currently produces a required unsupported check, never a passing material
claim. Placement-region/keepout outlines reject nested cutouts rather than
silently ignore them. The existing edge clearance policy is
`DesignRules.minimum_clearance_nm`, now explicitly exported to KiCad; separate
profile edge/web/tool limits are still pending. `minimum_non_plated_drill_nm`
is a distinct optional provenance-bound fabrication capability; a board-owned
hole without that capability leaves the fabrication process gate incomplete.

The optional head radius is CopperScript placement intent on both sides. It
does not create copper exclusions and is not yet a native KiCad placement rule;
the exporter warns when present, so manual movement requires renewed legality.

Run the physical-IR probe from the repository root:

```console
uv run python -m examples.mechanical_example
```

Ignored `build/mechanical-example/` contains a routed L-shaped KiCad project,
one interior window, two round NPTHs, global/detailed/physical-DRC reports,
summary timings and `routing.prof`. It is a deliberately small geometry probe
with synthetic one-pad terminals, **not a real circuit or .copper frontend
example**. The regression tests additionally use KiCad's native DRC and its
separate PTH/NPTH Excellon export. This does not qualify Gerber outline topology,
substrate-web/tool tolerances, zone clipping or production manufacturing.
The manufacturing-release entry point rejects these new void features until
independent outline/tooling qualification is implemented; ordinary inspection
KiCad export remains available. A passed copper DRC alone cannot bypass that gate.

### Round-outline increment

`BoardOutline.circle()` now retains an authoritative `CircularBoardBoundary`,
exports a native KiCad circle and uses exact disk containment for shared material
queries. The separately stored inscribed ring has an explicit radial chord bound
(default 0.01 mm) and exact edge certification; it never becomes the exported
outline. Circle/ring mismatches and excessive unsupported precision fail rather
than silently change source intent. Curved placement regions/keepouts remain
unsupported. Required filled-zone material coverage and manufacturing release
also fail closed for circular boundaries until their independent qualification
is implemented. Item 7 remains open for general arcs/rounded paths/slots/CAM.

The [twelve-LED / MCU / CR2032 example](round-led-ring.md) demonstrates this
bounded support with fixed radial placement and explicit inspection status.
