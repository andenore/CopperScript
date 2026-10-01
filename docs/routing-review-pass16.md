# Pass 16: bounded two-leg package exits

This implements the escape-representation and ownership portion of R17 after
`1af13f2`. It is a matched partial-fanout experiment, not a replacement finished
board or a complete ordinary-routing rerun.

## Implementation

Ordinary crowded-pad fanout keeps its existing radial candidates. If that
initial domain is empty, it tries at most 256 off-ray endpoints on a half-step
lattice in deterministic near-to-far shells within the existing radius. The
default lattice is 0.25 mm and radius 3 mm. For each endpoint, try both
diagonal/straight orders; select and emit the same exactly checked path.
These new exits contain only straight/45-degree legs, not orthogonal corners
or oblique chords. Via span, drill spacing, outline, all-layer clearance and
the source-pad via exclusion remain unchanged. Final native hard-DRC acceptance
is transactional. Critical/zone nets retain their existing owners.

`FanoutOptions.two_leg_escapes` enables this fallback by default;
`maximum_two_leg_candidates` bounds endpoint enumeration. This is bounded
reachability, not an exhaustive package maze or optimal candidate assignment.
Only one legal path order per endpoint is retained. Pins with already legal
radial choices are not expanded in this increment.

The shared `pin_escape` owner provides exact access-path checking without a
fanout/detailed import cycle. Detailed routing verifies one/two existing legs
from the actual terminal center to an actual same-net, spanning via. Endpoints
must meet exactly on the terminal layer; every original segment is checked.
A missing leg, nanometre gap, wrong net/layer/span or oblique chord fails closed.
This verifier recognizes generated lead-ins, not every possible existing
pad-edge copper contact; general contact connectivity retains its own owner.

`FanoutResult.created_tracks` carries explicit ownership through initial,
placement-trial, neutral-fallback and zone-repair routing. Cleanup may remove
only owned occurrences on abandoned attempted nets, not coordinate-matching
pre-existing tracks. Reused input lead-ins/vias acquire no new ownership.
Absent ownership preserves input copper. Failed subset repairs retain their
input escapes even when ownership is supplied. Successful lead-ins remain;
only owned anchor vias unused on a second layer are pruned. Ownership is not
permission to move critical, ground or unrelated input copper.

CLI per-pin analysis adds `two_leg_candidate_count`. Candidate counts and
selected indices are observations, not onward net connectivity or signoff.

## Matched real-board result

Retain all 51 saved pass-13 poses, identical recomputed global guides and every
critical/GND track/via, including the late ground contact. Remove ordinary
copper only from a disposable experimental checkpoint, exactly as in pass 15.
Compare committed radial fanout with the new default:

| Measurement | Radial | Two-leg fallback |
| --- | ---: | ---: |
| Escaped crowded pins | 69 | 72 |
| Pending crowded pins | 7 | 4 |
| Added through-vias | 69 | 72 |

U_ACCEL.2, U_MCU.45 and U_MODEM.13 now escape. No previous escaped identity is
lost. All placements and prior critical/GND copper remain exactly unchanged.
Fresh KiCad 10.0.6 refill reports eight existing library findings, 72 dangling
fanout vias and 94 unconnected items, with no shorts, clearance or drill-spacing
violations. Those are **partial-stage** counts, not a full-board improvement
over the last routed board's 34 open items.

U_MCU.64 now has three legal immutable alternatives and U_MCU.62 has ten,
instead of zero radial candidates. Exact diagnostic replay attributes their
subsequent rejection to newly selected ordinary escapes, not immutable copper:

| Pending pin | Selected escape blockers |
| --- | --- |
| U_MCU.64 / MODEM_EN | MCU_NRF_RX, for all three alternatives |
| U_MCU.62 / MCU_NRF_TX | MCU_NRF_RX or MCU_MODEM_TX |
| U_MCU.61 / MCU_MODEM_RX | MCU_MODEM_TX |
| U_MCU.39 / USER_LED_2 | USER_LED_1 |

The trace relabels newly generated escapes as movable in a diagnostic-only
clearance index to identify them; actual routing never unlocks or removes them.
Neither another maze budget increase nor inner-layer preference can allocate
two conflicting package exits. R6a/joint package assignment remains open.

## Tests and next acceptance

Regressions exercise a real off-ray-only via window with one blocked path
order, bounded/deterministic enumeration, bad anchor contacts, reversed stored
segments, detailed consumption, owned cleanup, failed full/subset behavior,
existing-path reuse and independent installed-KiCad acceptance. Final full
suite: 475 passed, 459 upstream dependency warnings, in 185.84 seconds.

Next: build local candidate conflicts; expand the domains of competing selected
pins even when they have a legal radial choice; then use bounded compatible
assignment/owned replacement with fail-closed rollback. Ordering existing
domains alone cannot manufacture a missing compatible alternative. Preserve
critical/GND reservations, poses, fabrication rules and actual contacts. Once
the MCU exits survive allocation, rerun the whole pipeline and repeat filled-zone
verification and all-layer review. Full R17 acceptance remains pending that run.

Evidence in task outputs `routing-review-pass16-two-leg-escapes`: `compare.py`,
`comparison.json`, matched `global.json`, native and KiCad artifacts,
`pressure-drc.json`, and `blockers.py` / `allocation-blockers.json`.
Original full pass-13 artifacts are untouched. Three full-board ordinary nets
remain unclosed in the last verified complete routing attempt; production RF,
stackup, library and CAM qualification remain separate gates.
