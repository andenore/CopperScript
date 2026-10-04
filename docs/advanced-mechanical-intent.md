# Advanced mechanical intent

Mechanical source and physical IR own coordinates; electrical IR never acquires
presentation/layout fields. No migration/version bump is needed before release.
Each feature must reach shared material/placement predicates, backend export and
tests before the editor exposes it as supported. Source ownership remains explicit.

## Named datums, edges and attachments (checklist 25)

`datum D { position = (xmm, ymm); }` names a stable point. Alternatively use
`relative_to = OTHER; offset = (dxmm, dymm);`. Resolve the dependency graph in
declaration-independent order; unknown targets, cycles and duplicate IDs fail.
Offsets between datums are board-frame vectors, not implicit rotating frames.

`edge E { start = (xmm, ymm); end = (xmm, ymm); }` names an existing straight
outer-boundary segment. Names do not depend on vertex indexes. Endpoint order
defines along-edge direction; inward normal comes from actual board winding.
An outline change that removes the named edge fails rather than silently retargeting.
Arc-edge attachment follows exact arc support, not a sampled-edge approximation.

`attach A { component = J1; target = D; ... }` targets a datum or named edge;
`position = (...)` is the explicit numeric alternative. `offset` at a datum is a
board-frame vector; at an edge it is `(along, inward)`. Along-distance must lie on
the named segment; inward may be signed. Diagonal edge coordinates quantize once
to nearest integer nanometres using high-precision arithmetic. No grid snap is
silently applied. Rotation (degrees) and side are explicit and validated against
source constraints. Default anchor is component origin. `anchor = pad` requires
`anchor_pad`; `anchor = mating_face` requires an audited local `anchor_point`.
Duplicate physical lands are not an unambiguous pad anchor.

Attachments are typed retained intent, resolved to source-owned fixed physical
pose constraints after footprint selection. Conflicting existing pose owners,
footprints, allowed orientations or multiple attachments on a component fail.
Reusable profiles namespace their datums/edges/dependencies and bind attachment
component roles explicitly. Imported owners remain read-only. Root attachments
are edited as attachments, not shadowed by competing fixed-placement constraints.

## Assembly envelope and access (checklist 26)

Explicit body overhang is independent of copper-edge clearance. It must be
component-owned and confined to an audited exterior allowance; no blanket copper,
internal cutout, mounting-hole or neighbour waiver. Pads, vias, tracks and fills
retain ordinary material/clearance requirements. Component/enclosure height is
typed; unknown height cannot satisfy a declared maximum. Assembly/tool-access
regions are side-specific and cannot be confused with signal-copper keepouts.

Implemented source forms:

```copper
overhang PLUG {
    component=J1; edge=TOP; start=10mm; end=20mm; distance=2mm;
    reason="Audited connector nose; pads remain inside board";
}
component_height J1_HEIGHT { component=J1; height=3mm; }
enclosure LID rectangle {
    origin=(0mm,0mm); width=40mm; height=30mm;
    side=front; maximum_height=4mm;
}
assembly_access INSERTION rectangle {
    component=J1; origin=(-3mm,-6mm); width=6mm; height=3mm;
    side=component; purpose="Plug insertion";
}
```

Overhang intervals follow the named edge's direction. The initial exact exterior
union supports axis-aligned edges only; diagonal/curved allowances fail explicitly.
Only that component's body/courtyard containment uses the exterior rectangle.
Unchanged outer-edge portions, internal cutouts, holes, screw heads, neighbours
and copper retain their ordinary checks. No expanded geometry reaches KiCad
`Edge.Cuts` or copper fills. Overhang owners' pads must satisfy original board
material and copper clearance during placement too.

Height declarations override a library footprint height for that instance only;
unknown height fails any intersecting same-side enclosure limit (and existing
height-limited placement keepouts). Zero enclosure height excludes all bodies.
Declared height exports as a hidden `CopperScriptHeightMM` footprint field, not
a native KiCad 3D model or a claim of native enclosure collision checking.

Access regions use component-local coordinates and `side=component` or
`side=opposite`, transformed with origin, rotation and rear reflection. They block
other components' bodies/courtyards on that side, not copper or their own owner.
Overlapping access areas are allowed (tools need not be used simultaneously).
Regions may be rectangle or polygon. Root policies are reviewed source edits;
profile component roles must be bound explicitly and remain read-only.

## Exact manufacturing geometry (checklist 27)

Line/arc/rounded outlines and non-plated routed slots retain exact manufacturing
primitives, stable IDs and closed topology. Curves may have a separately bounded,
conservative query approximation, never an outward enlargement of legal material.
Internal voids remain voids. KiCad exports exact lines/arcs and native oval/slot
drills where applicable. Validate malformed/disconnected/self-intersecting paths,
minimum slot dimensions, hole/slot proximity, copper clearance, native DRC and
Gerber/drill round trips before adding their UI controls. Unsupported curved
queries must fail closed, not silently substitute a bounding rectangle.

## Locked external reference overlays (checklist 28)

DXF/enclosure reference assets are locked by content checksum and imported through
a documented entity whitelist, explicit units, transform and coordinate frame.
References are read-only guides, not electrical/board-material connectivity.
Unsupported entities, ambiguity and broken loops fail; no automatic healing,
remote code execution, arbitrary browser file reads or runtime network fetches.
External geometry cannot replace source-owned manufacturing outlines implicitly.
Optional 3D inspection is later work, not a first-delivery completion requirement.

Implementation order: typed source/IR and validation → shared physicalization/
queries → native backend/export regression → reviewed source patches → shared UI
→ real example verification → checklist record and commit.
