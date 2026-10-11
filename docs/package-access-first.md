# Package access before area routing

## Research and chosen approach

Dense package resources should be allocated before inter-package routes occupy
them. [Ozdal and Wong (2006)](https://scholars.hkbu.edu.hk/en/publications/algorithms-for-simultaneous-escape-routing-and-layer-assignment-o/)
separate escape routing from intermediate-area routing and optimize escape order
across packages. [Ali et al. (2021)](https://pmc.ncbi.nlm.nih.gov/articles/PMC8056246/)
use two optimization stages with boundary terminals passed between them.
The [2021 concurrent hierarchical PCB routing paper](https://ieeexplore.ieee.org/document/9586143/)
also includes escape refinement and respects differential/length constraints
throughout its flow. Its abstract is available; this implementation does not
claim to reproduce its solver or commercial-board results.
The [2024 large-pin-array SER paper](https://ieeexplore.ieee.org/document/10617655/)
uses a path-conflict graph, weighted independent-set layer ordering and channel
optimization. Its available abstract reinforces conflict-aware allocation, but
does not establish that a greedy via choice or a maximum-weight partial subset
is sufficient for our mandatory-pin and critical-interface constraints.

[OpenROAD/TritonRoute](https://openroad.readthedocs.io/en/latest/main/src/drt/README.html)
includes pin-access analysis before initial detailed routing. This is a useful
architectural precedent, not a PCB-specific rule or a drop-in solver for our IR.
[Altium's BGA fanout/escape workflow](https://www.altium.com/documentation/altium-designer/pcb/routing/interactive)
distinguishes reaching another layer from escaping beyond the device boundary.
A legal dogbone via alone therefore cannot certify onward routing.

[TI's AM62 escape guide](https://www.ti.com/lit/an/sprad13a/sprad13a.pdf)
connects placement, stackup, package escape and interface priority, and reserves
power/decoupling space before low-priority routes. Our recommendation combines
these principles: protect local access for all connected pins without allowing
ordinary GPIO fanout to silently invalidate USB/RF/clock requirements.

We retain the existing exact candidate conflicts and bounded CSP rather than
introducing a general ILP dependency. The literature's simplified pin-array
assumptions and reported completion rates are not guarantees for real pads,
through-vias, keepouts, coupled pairs or our finite search domains.

The assignment budget distinguishes cheap conservative bounding-box separation
from actual copper/drill compatibility checks. Defaults are 200,000 exact checks
and 2,000,000 total requests (including cached and broad-phase requests). Remote
packages therefore cannot exhaust the local exact-check budget before MRV search
starts; the separate total-query limit still bounds work. Disjoint pairs do not
fill the exact-pair cache. Reports expose `pair_checks`, `pair_queries` and
`broad_phase_accepts`. This changes accounting, not clearance predicates or
transactional whole-board acceptance.

## Implemented first increment

With `route-board` (package access enabled by default), the order is now:

1. Legal placement and coarse global guides.
2. Joint ordinary crowded-pin fanout against the unrouted placement.
3. Profile-driven critical routing around those local reservations.
   Pairs still use the coupled search; generic fanout never creates their stubs.
4. Source-declared inner-plane contacts by default, checked against both stages;
   explicit selectors remain available for diagnostics.
5. Allocate compatible provisional ordinary paths beyond package collars.
   Bounded cross-owner pattern negotiation and placement feedback repair failed
   access before the fail-closed gate.
6. Commit owned ordinary boundary paths and exact selected-layer anchors only
   after the gate passes. Ordinary area routing starts from those ports; normal late plane contacts,
   native DRC, independent KiCad refill/connectivity and signoff follow.

The gate requires successful global planning, no pending requested access,
fresh critical-net connectivity, non-failed critical status, compatible ordinary
boundary witnesses and no hard native finding. A blocked result preserves diagnostic copper, reports zero ordinary
search passes and cannot claim fabrication readiness. The ordinary stage and
late ground feedback cannot bypass it. Physically failed reservations produce
a failed design report; stale/unowned reservations are invalid API arguments.

`pcbir/package_access.py` owns the preflight and placement transaction.
`route_critical_nets(..., reserved_accesses=...)` accepts only a `FanoutResult`
whose source, created tracks and created vias match the unrouted board. The
locked-copper prefix includes these reservations; critical reports expose
`reserved_track_count`/`reserved_via_count`. Their ownership remains with fanout,
including downstream unused-via/abandoned-stub cleanup.

An alternate plane-first trial uses `plane_stitch_options` to generate fresh
contact copper inside the critical stage from source intent and validated
ordinary reservations. Reports separately expose `plane_reservation_track_count`
and `plane_reservation_via_count`; contacts never become ordinary-owned copper
or filled-connectivity evidence. Critical-only placement feedback and incremental
prefix reuse require a full package-preflight rebuild for this ownership order.

Placement trials rebuild global guides, fanout, critical copper and selected
plane contacts from an unrouted source. They try small translations with growing
radii and explicitly allowed rotations, including 45 degrees when declared.
Rigid placement units move together; fixed poses, orientations, proximity and
keepout constraints remain mandatory. A trial is accepted only if it strictly
reduces the identity of pending pads or failed critical nets, preserves every
previous ordinary exit, adds no pending contact/critical failure and has no hard
native finding. Changed eligibility cannot hide a previously required pin.
Rejected proposals leave the incumbent geometry unchanged.

### Cross-owner pattern negotiation

The first ordinary pattern is no longer permanently frozen. If it blocks a
critical interface or selected early plane contact, preflight tries alternate
owner orderings **before** moving components. Incomplete ordinary allocation
can also use the already-successful critical/selected plane owners to seed a
different ordinary pattern:

1. Probe the specialized critical owners against the clean, macro-materialized
   placement without ordinary stubs. Keep their original layer, coupling,
   length, via and return-path profiles.
   When they already passed, strip only the verified ordinary reservation prefix
   and reuse their exact copper as the proposal seed, with a fresh native check.
   This avoids a redundant pair search; changed proposals still receive full
   all-owner critical/plane revalidation before acceptance.
2. When selected plane contacts or ordinary allocation failed, propose ordinary escapes against both
   that critical copper and provisional plane contacts. A second bounded
   proposal can use critical copper alone. If the probe cannot establish contacts,
   or rebuilding critical copper around the new ordinary pattern loses contacts,
   use the second trial for `plane_first`: propose ordinary exits around early
   contacts, then freshly regenerate contacts before routing critical owners.
   The default two-trial bound remains unchanged, with no blanket power-first
   policy. For critical-only failures there is just one useful ordering.
3. Discard probe copper. Rebase only the proposed fanout-owned tracks/vias onto
   the original source, then rerun critical routing and selected plane contacts
   from scratch against those reservations.
4. Apply the same identity-preserving acceptance gate as placement feedback:
   retain every ordinary exit and required pin, introduce no new pending pad
   or failed critical net, strictly shrink a failure set, and pass fresh native
   geometry checks. Otherwise retain the entire incumbent pattern.

This is a deterministic bounded negotiation heuristic, not simultaneous
enumeration of every critical/ordinary/GND domain or a proof that no compatible
pattern exists. It does not split differential pairs into ordinary nets, relax
manufacturing/SI profiles, change placement, or modify hard-macro copper. Plane
contacts remain provisional until independently refilled and connectivity
checked. Unrequested ground pads still belong to the late plane stage.

`--package-pattern-trials` selects a budget of 0, 1 or 2 per search tier (default 2). Ready
preflights do not run probes. Failed clean-source critical probes and duplicate
geometry/owner-order combinations are not repeatedly searched. An unchanged
ordinary pattern can still be revalidated with a different owner order.
`package_access.pattern_trials`
records strategy, pending identities, critical failures, outcome and whether
the proposal received all-owner revalidation; progress events separate probe
and trial costs for profiling. These records describe the selected placement;
placement trials themselves remain separately recorded.

With paired nets, the first tier limits maze expansions to 6,000 states per
pair/pass, shared across surface/via families, pitches and ports. It samples
bounded candidate slices and negotiates patterns early. If not ready, retry the
original ordinary pattern with historical full search budgets; accept only an
identity-preserving, no-worse result. `--package-initial-pair-states 0` disables
staging, as does a zero pattern-trial budget. Reports include `search_tier` on
patterns and `package_access.search_tiers` with total work, including rejected
probes/trials. Limited failure is not evidence of impossibility. Exact/native
geometry, coupled pair profiles and all readiness gates remain unchanged.

### Feasibility-ranked placement feedback

When access remains blocked, the controller measures a bounded look-ahead of
legal whole-unit poses before choosing expensive full-routing trials. Each
disposable probe materializes the source macro/required-array owners, runs real
ordinary fanout, selected plane stitching and ordinary boundary allocation,
and checks hard native geometry. It records pending pin identities and lost
previously successful ordinary access. No probe copper is committed.

Candidates rank first by hard findings, lost access, newly pending pins and
remaining pending pins, then by the shared placement quality vector (including
escape-channel demand, congestion and weighted wire length). This can prefer a
slightly longer signal route when the shorter placement cannot reach a declared
plane contact. Equal ranks retain deterministic proposal order.

Generated critical routes are deliberately excluded from these local probes;
authored macro copper is retained. Local success is only a ranking hint. Every
selected pose receives fresh global routing and complete ordinary/critical/
plane/boundary preflight, with the existing identity-preserving acceptance gate.
A candidate breaking a critical length/via/layer/coupling requirement is rolled
back even when its local probe looks complete. Native filled connectivity and
RF/power/release approval remain separate gates.

`--package-placement-probes` is a total budget per feedback call (default 16),
shared across accepted moves. Probes do not consume or expand the full placement
trial budget. Once exhausted, unprobed moves retain the existing proposal order;
zero probes explicitly restores that order. Fixed or already-ready placements
and disabled placement feedback do not launch probes. This is bounded feedback
ranking, not a change to every analytical-placement inner loop or proof that the
best legal placement has been found.

Reports expose `placement_probes`, `placement_probe_scope`, the probe limit,
and `placement_probe_index` on full trials. Progress separates ranking/probe work
from full trials. `placement_stop_reason` distinguishes ready, disabled,
trial-budget exhaustion, no improvement and no legal unseen proposals. The last
status does not prove physical impossibility or identify every conflicting rule;
more detailed constraint explanations remain planned.

Defaults are eight evaluated full placement trials and a 0.5 mm initial movement.
Trial runtime can be substantial because critical routing is rebuilt. With
fanout enabled, `--critical-feedback-trials` adds budget to this unified
controller rather than running a second critical-first placement controller.

```powershell
uv run --no-sync python -m copperscript route-board examples/full_vertical/board.copper `
  --locked --offline --layers 6 --fab-profile jlcpcb-six-layer `
  --fanout --package-pattern-trials 2 `
  --package-access-trials 8 --package-placement-probes 16 --package-access-movement-mm 0.5 `
  --report build/access-route-report.json -o build/access-board.kicad_pcb
```

Create `build/` first. For the complete pinned workflow use
`make EXAMPLE=full-vertical route`; its `--fanout` now
selects this stage ordering. `--package-access-trials 0` disables moves, **not**
the gate or pattern negotiation. Locked Git providers supply footprints without
a sibling CopperLib checkout. Before an offline run, populate both source and
footprint caches with `copper audit-footprints examples/full_vertical/board.copper
--locked` (or the equivalent `uv run --no-sync python -m copperscript` command).
Standalone fanout remains usable for partial diagnostics. `--fanout` remains
accepted; `--no-fanout` explicitly selects the old no-package-preflight flow.
The lower-level Python API still accepts `fanout_options=None` for that flow.
Report `package_access`
records readiness, pending identities, critical failures, hard findings,
placement trial outcomes and whether ordinary area routing started.

Eligible ordinary crowded SMD pins normally receive dogbones; cleanup may remove
them when a verified surface path needs no transition. No-layer-change contracts
and unavailable dogbone sites now use directly validated via-free surface escapes.
Selected plane contacts participate in preflight, but it does not prove filled
continuity or access for every package type. An NC needs no exit. Native/KiCad
connectivity and the manufacturing gates remain required. See
[bounded filled-copper feedback](filled-copper-feedback.md) for native repair,
placement rejection explanations and the surface fallback.

Remaining specialized allocation and onward-pattern work is in the [routing backlog](routing-backlog.md).

Compatibility accounting separates conservative disjoint-bound acceptances from
exact overlapping-pair queries. Cached/broad-phase requests still count toward
a separate total-work bound; exhausting either allowance rolls back atomically.
No broad-phase speedup weakens copper, drill, layer or owner predicates.
