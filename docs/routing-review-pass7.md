# Pass 7: source-backed Nordic matching placement scene

Electrical evidence superseded: the USB choke mapping used here was incorrect.
Do not reuse this PCB as valid USB wiring. See [pass 11](routing-review-pass11.md).

This increment integrates the R8c three-member matching template, not the complete
vendor reference circuit. The scene is explicit and data-only, pins source bytes
and resolved footprint digests, verifies physical pad/net roles, and permits
whole-macro 45-degree rotation. It never rewires the semantic design. C3 pad 1 is
GND and pad 2 is ANT, aligned with the visually checked vendor first-pad/top-copper
orientation. Internal courtyards may have zero additional gap but cannot overlap;
external spacing and copper/fabrication checks remain unchanged.

CopperLib emits portable UTF-8/LF packet bytes with pinned Git line endings so a
raw reference hash remains valid across Windows/Linux checkouts. The current
packet SHA-256 is `207ff21dda6881717e252cbb9e7f4dadfb0a51827068f7c001d770b700557cac`.
Only the machine-local footprint `source_path` is excluded from asset identity.
Source bytes, geometry, source library and remaining metadata stay hashed.

## Fresh real-footprint preflight

Task artifacts: `routing-review-pass7-rf-template-2026-10-01/preflight.json`,
`critical-board.kicad_pcb`/project and `kicad-check/drc.json`/refilled board.
Unlike passes 5/6, placement is recomputed with the explicit scene, not replayed.
Settings: locked/offline full-vertical source, actual KiCad/CopperLib footprints,
100 x 80 mm, JLCPCB six-layer prototype, candidate-01, one placement-feedback
iteration, five global iterations, 5 mm tiles. Reproduce the scene invocation in
[rigid-placement-clusters.md](rigid-placement-clusters.md).

| Group | Committed length | Result |
| --- | --- | --- |
| Nordic raw matching tree | 17.19 mm | Connected; coarse-guide detour remains |
| Nordic antenna feed | 23.03 mm | Connected; not antenna qualification |
| Cellular feed | 62.65 mm | Connected; worse than historical placement |
| GNSS feed | 33.50 mm | Connected; RF placement still open |
| MCU USB pair | 7.95 / 7.32 mm | Joint search connects without vias |
| Modem USB pair | None | 24 searches / 720,000 states / zero candidates |

All accepted critical copper is on F.Cu without vias. The rejected modem guide's
reported lengths are proposals, not committed tracks. No unsafe modem geometry
is locked, no clearance rule is relaxed, and members are not routed independently.
Global routing's 57 routed guides, one deferred GND net, zero overflow and 81
proposed vias are not evidence of detailed-copper completion.

Native DRC: 52 open nets plus incomplete routing, no hard geometry findings.
Independent KiCad 10.0.6 refill/DRC: 196 expected partial-board unconnected items,
including both modem pair members; no opens on the six accepted critical nets;
zero short, clearance or dangling findings. The eight unchanged library findings
remain visible (four issues, four mismatches). This is not an ordinary-net rerun
and cannot be compared as a completed-board regression with pass 3.

Observed phases: resolve 0.26 s, placement/global 126.00 s, critical 1185.40 s.
Tests ran concurrently; these are operational observations, not a benchmark.
The report's `complete: true` means preflight execution finished; critical status
is **failed**, native completeness is **incomplete**, fabrication readiness false.

Recorded identities:

- Global route: `04c9c98dd149df74841ce0ae667b9c1eee82f3616604346fef7da04fd6c48f6a`.
- Critical route: `616406966ae49d4a4d9749618dbfe6968b67c59d2f33de3fe63e32168ea8aa00`.
- Preflight bytes: `a15862d4442cb1c2b0c2b18b0bcb9da8d3ba6d3ce0e9afb70577687a68d82be8`.
- Original PCB bytes: `81de86fa179b58477e91023b860525ef9456f787fbabafce85d096b417f7b2a0`.
- KiCad report bytes: `ac30cfb3cbeeecbc7b6f0d6e24c94f845c47e43bc4479d5cdc70d699f492423c`.

This run predates LF normalization of the otherwise identical reference packet
(then SHA `a33f0545ba2bc7e543a2766ca0e7b9af04a02fcabaa5c3d78bd57e8666423ae5`).
The current scene/reference evidence identity therefore differs from the saved
run; matching coordinates/pad roles, placement and copper were not changed by
that normalization. Saved signoff tokens are historical, not reused for current
bytes. The following performance optimization was also added after the run.

## Verified performance change

A live sampling profile of the slow paired search showed repeated exact board-edge
distance checks. For actual axis-aligned rectangular outlines, track capsules and
via disks now use equivalent doubled-integer convex-erosion checks. Only immutable
outline classification is cached; other outlines retain the exact polygon path.
Odd-nanometre radii, tangency, translated/reversed rectangles, bowties and concave
outlines are covered. No clearance/search budget or acceptance gate changed.

A local 10,000-call rectangle check (best of three, one fixed segment) measured
0.4633 s original versus 0.00974 s fast path, approximately 47.6x for that predicate.
This is not whole-router acceleration and was not applied retroactively to the
1185 s result. Full-router progress/checkpoints and performance remain R9 work.

Verification: 369 CopperScript tests pass, with 459 upstream CAM-library warnings;
29 CopperLib tests pass and compatibility outputs remain deterministic. Real
matching-only fixtures route below 5 mm with no vias at both 0 and 45 degrees.
They do not include all support circuitry or prove RF electrical performance.

## Next bounded work

Apply R5 to critical matching trees: compare branch-safe short local alternatives
before accepting a legal but expensive coarse guide. Reevaluate modem paired
package access and placement, preserving critical reservations and exact rules.
Complete source-backed Nordic support/ground geometry, Johanson corner keepouts
and GNSS/cellular RF placement before another complete ordinary-net reroute.
Neither source identity nor connectivity substitutes for RF/return-path/stackup
qualification. R8c/R8d and manufacturing signoff remain open.
