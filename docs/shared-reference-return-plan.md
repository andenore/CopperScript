# Explicit shared-reference USB transitions

## Decision and scope

The retained full-vertical board has no pad opens but one `via_dangling`
warning: a mandatory USB GND via contacts only In1.Cu. Both F.Cu and In2.Cu
are adjacent to that same declared GND plane. Adding unrelated surface copper
or disabling the warning is not a return-path repair.

[TI SLLA414A, section 3.5](https://www.ti.com/lit/an/slla414/slla414.pdf)
recommends preserving a common ground reference and stitching different ground
planes when the reference changes. [KiCad 10](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html#cleaning-up-tracks-and-vias)
identifies single-contact-layer vias as unnecessary connections. Applying the
common-reference case here is a topology inference, not a signal-integrity
measurement or proof of filled-plane continuity.

Keep `require_return_vias = true` strict by default. Add an explicit
`return_via_policy = "reference_change"` with a typed
`shared_reference_layer = "In1.Cu"`. Only two actual signal-contact layers
adjacent to the identical declared reference plane can use this exception.
That layer must have an unambiguous dedicated-plane intention for the return
net; transition points and the complete emitted pair must stay inside its
declared zone, avoiding declared plane voids/keepouts. Unknown, different,
missing or contradictory references never silently omit required vias.

The critical owner must independently derive contact layers from emitted
tracks, not from a through-via's barrel endpoints or a proposal's claim.
Expose shared-reference transition counts and retain external fill/impedance
assumptions. Bind both new rule fields into physical/ownership fingerprints.
Native refill/all-severity DRC and existing manufacturing gates remain intact.

## Implementation checklist

- [x] Add typed policy/reference fields, source lowering, validation and digests.
- [x] Apply the same conservative decision during paired candidate generation
  and owning validation; default always-required behavior remains unchanged.
- [x] Report shared-reference decisions and reject corrupted/missing contacts,
  voids, ambiguous planes and mismatched pair policies.
- [x] Explicitly annotate the four full-vertical USB member rules; do not edit
  old boards or claim an old report was produced by the new source.
- [x] Verify focused physical, paired-route, ownership and native KiCad tests.
- [ ] Commit the feature, then rerun the shared unprofiled full pipeline from
  source and pinned libraries in a new ignored build directory.
- [ ] Refill/check every layer; require zero native opens/violations and zero
  detailed resource overflow. Locate any remaining conflict rather than waive it.
- [ ] If native gates pass, generate manufacturing files using the explicit
  `--skip-independent-cam` workflow requested by the user. Do not claim supplier
  availability, assembly qualification, USB/RF performance or order approval.

## Limits

Zone intent is not refilled copper. This policy screens declared geometry;
native zero-open checks do not certify every point of a high-frequency return
path. No power-reference capacitor bridge, arbitrary plane inference,
three-dimensional paired maze, extra ground zone, via-in-pad permission,
clearance relaxation or ignored DRC finding is introduced.

## Feature verification — 2026-10-06

The final return/physical/paired/boundary/closure/package/plane batch passes
176 tests, including 17 new policy cases. A separate cleanup/checkpoint/style/
reporting batch passes 41 tests. The final full-vertical source-profile test
also passes; an earlier full-vertical/critical-feedback batch passed 45 tests
(overlapping batches must not be added). Python compilation and normal Git
whitespace checks pass. This is focused coverage, not a claim that the known
repository-wide CAM installation and CM4 fixture issues are fixed.

The installed KiCad 10.0.6 fixture independently refills the declared reference
plane and has zero opens and zero other violations: four matched signal vias,
two reported shared-reference transitions, and no single-plane-only return
via. Unknown references/default policies still create required vias; different
reference planes retain bridging GND vias. The owner rejects missing contact
layers, projected plane voids and track envelopes outside declared coverage.

The complete example run has not yet been repeated at this feature revision.
Old board/run evidence is unchanged. The next shared `make` run uses
`PROFILE=none`, locked/offline inputs and a new ignored output directory.
