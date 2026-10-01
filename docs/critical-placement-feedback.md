# Bounded critical placement feedback

`pcbir.critical_feedback.improve_critical_placement` runs after initial placement,
global routing and exact critical routing, before planes, package fanout and
ordinary detailed routing. It repairs failed critical groups by changing placement,
not by dragging tracks or editing a particular example's component coordinates.
Enable it explicitly with `--critical-feedback-trials N` on `route-board` or
`python -m pcbir.critical_preflight`. The default is zero: real-footprint paired
searches can take minutes per trial. A trial limit is not a connectivity waiver.

## Transaction and acceptance

The input placement must be unrouted and unfilled. The initial critical result
must match that placement, global placement/route fingerprints, its recomputed
critical fingerprint, and its locked copper. Inconsistent or stale inputs fail.

Candidate components are endpoints of failed critical nets, ordered by physical
pad count then reference. Small support parts are tried before large packages.
Permitted rotations are tried first (180-degree alternatives first), followed by
four translations of 0.5 mm. Explicit orientation rules, including 45 degrees,
are used without broadening them. Fixed positions/orientations, rigid-cluster
companions, full placement legality and represented keepouts remain mandatory.
A member move expands to its entire rigid unit; a fixed companion can veto it.

Every legal trial rebuilds the full global stage and **all** critical groups
from an empty copper snapshot. Reservations are never reused beneath moved
footprints. Global routing must succeed. Fresh native DRC must find no hard
error other than expected ordinary-net opens/incomplete routing, and the set of
failed critical net identities must become a strict subset of the incumbent set.
Result omissions, failed batch records, diagnostics and actual native opens count
as failures. Replacing one critical failure with another is rejected. Rejected
trials retain the original placement and copper; accepted trials retarget the
remaining failures until the bound or termination condition is reached.

The full pipeline then starts plane/fanout/detailed routing from the selected
critical result and its new global guides. It never applies a placement move to
existing ordinary tracks. Later detailed-placement feedback can supersede this
stage; `critical_feedback` is historical evidence, not the final board identity.

## Evidence and limits

Reports retain each proposed pose, all changed component references, outcome,
remaining failed critical nets, route fingerprints and elapsed seconds. Timing
and callback telemetry do not affect route geometry/fingerprints. Preflight is
still partial and always reports `fabrication_ready: false`.
It checkpoints the baseline and proposed pose before each global/critical trial,
so an interrupted search remains incomplete and cannot conceal the trial stage.

This is a bounded deterministic local controller, not a globally optimal placer,
paired layer-transition solver or exhaustive feasibility proof. It preserves
original width, spacing, length, via and layer restrictions. Independent KiCad
checking, complete ordinary-net rerouting, RF support/ground and antenna rules,
actual stackup/impedance qualification, library consistency and manufacturing
checks remain separate gates. Synthetic tests exercise acceptance, strict net
identity preservation, fresh geometry rejection, rigid/fixed and 45-degree rules,
determinism, stale input rejection, CLI and downstream pipeline integration.
