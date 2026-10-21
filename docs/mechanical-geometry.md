# Mechanical geometry contracts

Supported source forms and limits are defined in [mechanical language](mechanical-language.md)
and [advanced mechanical intent](advanced-mechanical-intent.md). This reference
defines the shared geometry invariants, rather than an implementation sequence.

## Ownership and representation

Electrical IR defines parts/nets without drawing coordinates. `Design.mechanical`
owns board material and physical intent; lowered physical IR is shared by
placement, routing, DRC and export. Geometry never creates electrical connections.

Use integer nanometres, stable feature IDs and explicit ownership. Cutouts and
NPTH holes/slots remove substrate. Screw-head/tool-access regions remove
placement space; they do not implicitly create copper keepouts. Plated holes
remain ordinary net-connected component pads. Reject degenerate/self-intersecting
boundaries, touching/nested cutouts and invalid hole intersections.

Minimum residual webs, drills, routed tools and process tolerances belong to
source-bound fabrication profiles. Do not invent screw or process dimensions.

## Shared legality

Exact material queries check every outer/cutout edge and drill void, including
concave crossings with legal endpoints and courtyards enclosing a void. Bounding
boxes/grids are broad phases. Every routing stage and hard macro uses these
queries; unsupported geometry fails closed.

Retain exact source curves for export. Conservative bounded query approximations
must not enlarge legal routing space or shrink obstacles. Circles retain an
authoritative circular boundary; the inscribed query ring has an explicit radial
chord bound (default 0.01 mm), exact edge certification and mismatch rejection.
Curved path queries currently require convex geometry and arcs <=180 degrees.

Copper-to-edge, copper-to-hole and drill-to-drill rules are distinct. Named
datums/edges, body-only overhang and height/tool-access intent never waive copper
containment. Reference DXF geometry receives no material or connectivity credit.

## Export and qualification

KiCad receives exact Edge.Cuts primitives and deterministic BOM-excluded NPTH
features; component holes retain their owner. Excellon PTH/NPTH and routed slots
must reconcile with intended identities, positions, dimensions and process.
Independent outline topology, web/tool tolerances, zone material coverage and
curved/slot CAM qualification remain separate release obligations. A KiCad DRC
pass, STEP view or unfilled zone outline cannot close them.

The IR-only probe is `uv run python -m examples.mechanical_probe.mechanical_example`.
It uses synthetic terminals to exercise geometry; it is not a functional circuit.
Real board status lives with each project, including
[CopperLedRing](https://github.com/andenore/CopperLedRing).
The [old LED-ring fixture](round-led-ring.md) is deprecated and retained for
compiler regression coverage.
