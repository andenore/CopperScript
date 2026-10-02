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

## Implemented first increment

With `route-board --fanout`, the order is now:

1. Legal placement and coarse global guides.
2. Joint ordinary crowded-pin fanout against the unrouted placement.
3. Profile-driven critical routing around those immutable local reservations.
   Pairs still use the coupled search; generic fanout never creates their stubs.
4. Explicitly requested early plane contacts, checked against both stages.
5. Package-access gate and optional bounded placement feedback.
6. Ordinary area routing, only if the gate passes; normal late plane contacts,
   native DRC, independent KiCad refill/connectivity and signoff follow.

The gate requires successful global planning, no pending requested access,
fresh critical-net connectivity, non-failed critical status and no hard native
finding. A blocked result preserves diagnostic copper, reports zero ordinary
search passes and cannot claim fabrication readiness. The ordinary stage and
late ground feedback cannot bypass it. Physically failed reservations produce
a failed design report; stale/unowned reservations are invalid API arguments.

`pcbir/package_access.py` owns the preflight and placement transaction.
`route_critical_nets(..., reserved_accesses=...)` accepts only a `FanoutResult`
whose source, created tracks and created vias match the unrouted board. The
locked-copper prefix includes these reservations; critical reports expose
`reserved_track_count`/`reserved_via_count`. Their ownership remains with fanout,
including downstream unused-via/abandoned-stub cleanup.

Placement trials rebuild global guides, fanout, critical copper and selected
plane contacts from an unrouted source. They try small translations with growing
radii and explicitly allowed rotations, including 45 degrees when declared.
Rigid placement units move together; fixed poses, orientations, proximity and
keepout constraints remain mandatory. A trial is accepted only if it strictly
reduces the identity of pending pads or failed critical nets, preserves every
previous ordinary exit, adds no pending contact/critical failure and has no hard
native finding. Changed eligibility cannot hide a previously required pin.
Rejected proposals leave the incumbent geometry unchanged.

Defaults are eight evaluated placement trials and a 0.5 mm initial movement.
Trial runtime can be substantial because critical routing is rebuilt. With
fanout enabled, `--critical-feedback-trials` adds budget to this unified
controller rather than running a second critical-first placement controller.

```powershell
uv run --no-sync python -m copperscript route-board examples/full_vertical_board.copper `
  --locked --offline --layers 6 --fab-profile jlcpcb-six-layer `
  --fanout --package-access-trials 8 --package-access-movement-mm 0.5 `
  --footprint-root "C:/Program Files/KiCad/10.0/share/kicad/footprints" `
  --footprint-root "../CopperLib/footprints" `
  --report build/access-route-report.json -o build/access-board.kicad_pcb
```

Create `build/` first. For the complete pinned workflow use
`uv run --no-sync python scripts/route_full_vertical.py`; its `--fanout` now
selects this stage ordering. `--package-access-trials 0` disables moves, **not**
the gate. Standalone fanout remains usable for partial diagnostics, and a flow
without `--fanout` retains its existing behavior. Report `package_access`
records readiness, pending identities, critical failures, hard findings,
placement trial outcomes and whether ordinary area routing started.

## Remaining implementation sequence

- [x] Preserve explicit ordinary access before critical long routes.
- [x] Add a fail-closed access gate and zero-pass diagnostic output.
- [x] Rebuild reservations transactionally during legal placement feedback.
- [x] Preserve original critical profiles, ownership and native geometry checks.
- [ ] Co-allocate critical paired/single-ended access domains with ordinary and
  dense power/GND exits; compare compatible package patterns instead of treating
  the first ordinary selection as immutable forever.
- [ ] Add local access collars/boundary ports and verify enough onward channel
  capacity on permitted signal layers. A through-via consumes physical space on
  every spanned layer; it is not automatically a usable exit beyond the package.
- [ ] Generate constrained multi-bend alternatives with extended exact anchor
  verification/ownership when simple patterns are insufficient.
- [ ] Derive directional placement margins from connected-pin bank demand,
  trace/clearance rules, via rows and nearby obstacles. Score margins cheaply,
  then gate finalists with exact access; do not inflate every courtyard or
  move decouplers independently. Existing high-pin spacing remains a heuristic.
- [ ] Rank complete patterns using onward congestion and escape order; permit
  bounded pattern replacement with complete critical/access revalidation.
- [ ] Repeat the full-board run, independent filled-zone/DRC checks and layer
  review before claiming closure or manufacturing readiness.

This increment still creates vias for eligible ordinary crowded SMD pins. It
does not yet offer a general via-free surface-port representation, include all
ground contacts by default, or prove access for every package type. An NC needs
no exit. Native/KiCad connectivity and the manufacturing gates remain required.
