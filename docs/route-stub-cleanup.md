# Ordinary-route stub cleanup

## Finding

The detailed search does not draw every explored edge. Failed net attempts are
atomic and do not commit tentative tracks. Nevertheless, an accepted route can
contain a repeated physical node with a different heading state, or double
back along a selected pin-access lead-in. Turning that walk into an undirected
edge set and merging straight runs does not remove the resulting dead-end tail.

A regression forces a launch at (5, 6), a return to (4, 6), and an onward route
to the other terminal. Its pad-to-launch lead-in starts at (3, 6). The copper
between (4, 6) and (5, 6) is redundant. These coordinates belong exclusively to
test fixtures; no component-specific rules or example coordinates are used in
the router. An existing shared-grid access regression also retained an unused
third segment; its corrected route has two octilinear segments and intact
electrical connectivity.

Overlapping reversed segments can mask this shape from KiCad's dangling-track
check: each free endpoint appears to touch another track. The equivalent split,
unique copper exposes the dangling end. Therefore zero reported dangling tracks
alone is not proof that a route contains no redundant tails. This reproduces a
failure mechanism, not a claim that every visual branch in a saved board is a
stub.

## Implementation

- Erase closed physical-node excursions from each reconstructed walk before
  adding it to the routing tree. Layer identity is part of the node; ordinary
  layer transitions are not erased merely because their XY coordinates match.
- Normalize owned track centre-line overlaps and exact junctions into a graph,
  including out-and-back runs with different widths.
  Recursively prune degree-one branches without an electrical or immutable
  anchor, including multi-segment and diagonal tails.
- Run cleanup before straight-run merging and conservative corner chamfering.
  After selecting the detailed pass, allow explicitly owned, successful
  point-anchor fanout copper to participate in joint cleanup with new routes.
- Preserve original segment identities wherever their full copper survives.
  Report counts and lengths from the accepted geometry using the existing
  occurrence-based immutable-prefix subtraction convention.
- Drop fully removed grid legs from per-attempt congestion resources rather
  than leaving phantom usage at a retired tail. Surviving coarse legs and later
  corner refinement retain their conservative search-resource semantics.

Cleanup uses spatial broad phases and reuses the search's pad-clearance index.
It is postprocessing, not an additional operation on every expanded search
state. No shortcut, track relocation or copper/drill clearance relaxation is
introduced.

## Safety boundaries

Input copper is immutable without explicit occurrence ownership. Identical
locked occurrences survive even if another occurrence belongs to fanout.
Failed subset repairs and other nets retain their input copper. Accepted
`RoutingAccess` boundary prefixes remain immutable because their reusable
proofs require exact path identity and a specified physical launch layer.

Real terminal branches, pad-edge and mid-segment contacts, vias on their actual
spans, hard-macro/input contacts and ambiguous off-centre contacts are protected.
Conservative contact tests round odd-width radii up; uncertain contacts are kept,
not severed. The cleanup deliberately does not delete vias or rewrite critical
or paired routes, and does not use zone outlines as connectivity evidence.
Zone-net copper stays with the plane owner. Arbitrary redundant loops, dangling
via networks and obsolete locked reservations require separate ownership-aware
analysis rather than aggressive leaf deletion.

## Verification

Regression coverage includes repeated-node loops, reversed overlaps, recursive
tails, real T-junctions, different-layer identities, pad-edge contacts, immutable
input, occurrence ownership, plane deferral, reusable boundary proofs and report
counts. Installed KiCad independently checks the equivalent dangling fixture
before cleanup and accepts the cleaned fixture with zero findings. Normal native
connectivity, clearance and independent fill/signoff gates remain enabled.

Verification: 256 focused tests pass across detailed routing, route cleanup,
boundary access, fanout, subset repair, hard macros, critical routes, planes,
reporting and physical DRC. One pre-existing expensive all-layer-blocked search
case was deselected in this focused run; this is not a full-suite claim.

The full-vertical timing run started at revision `0919f0b` predates this fix. Its
result is not validation of the new cleanup and its saved artifacts are not
silently rewritten. A fresh full-board routing/layer review is still needed to
measure the impact on that example.
