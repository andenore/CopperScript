# Global routing and package escape implementation

This is a staged implementation checklist, not a claim that the full board is
routed. The full-vertical board's current baseline is 51/58 detailed nets, with
one global via-resource overflow and seven pending fanouts. Each stage must
retain deterministic output and must not weaken exact native or KiCad DRC.

## 1. Make global resources physically meaningful

- [ ] Record the current board's tile-size sensitivity and the contributors to
  the (57.5, 42.5) mm via hotspot before changing routing behavior.
- [x] Remove courtyard-as-copper blockage; only copper/keepout geometry may
  obstruct tracks. Test a front-layer route beneath a large courtyard.
- [x] Estimate planar capacity per edge and layer from usable crossing width,
  actual pads/keepouts, and the net's width/clearance demand. Retain explicit
  safety margin rather than claiming exact detailed routability.
- [x] Estimate legal via sites per cell from the actual through-via size, drill,
  pad/hole clearance, and all-layer keepouts. Couple adjacent-layer transitions
  that consume the same physical through-via sites.
- [ ] Reserve or strongly penalize foreign-net use of declared plane layers;
  verify the resulting return-plane continuity after KiCad zone refill.
- [x] Distinguish no geometric route, over-capacity route, and abstract-model
  uncertainty in diagnostics; add focused tests and compare full-board results.

## 2. Represent multiple physical pad accesses

- [x] Enumerate deterministic candidate direct escapes and stub-plus-via
  options for each connected pad, with complete physical geometry, layer/span,
  cost, and rejection reason. Preserve fabrication-rule checks; never infer
  via-in-pad or blind/buried vias.
- [x] Use several candidates per pad in global routing instead of one nearest
  cell. Reserve only a selected access and report when no candidate is legal.
- [x] Make global guides expose the selected physical access and keep critical,
  fanout, and detailed routing compatible with it.
- [ ] Add focused QFP/QFN package tests and a physically realized nearest-
  access trap beyond the current synthetic candidate-selection fixture.
  - [x] Pass-15 fanout regression uses real radial candidate generation and
    rectangular via-only keepouts with two available sites. Low-slack ordering
    escapes both pins; nearest/reference-first allocation strands one. Broader
    QFP/QFN and joint escape coverage remains open.
- [ ] Benchmark candidate count, completion, runtime, and KiCad DRC against the
  baseline; do not call an abstractly connected net physically routed.

## 3. Solve package escapes together

- [ ] Build a conflict graph for candidates belonging to nearby pins and nets.
  - [x] Enumerate immutable legal radial domains, prioritize low-slack pins,
    recheck selected escapes incrementally and expose initial/consumed-domain
    diagnostics. This is a tested first increment, not joint matching/search.
  - [x] Expand zero-radial domains with bounded straight/45-degree two-leg exits,
    verify actual multi-segment anchor copper and propagate explicit cleanup
    ownership. Pass 16 improves 69 to 72 matched crowded-pin exits; MCU.62/.64
    alternatives are still consumed by selected neighbors.
  - [ ] Attribute local candidate conflicts and expand competing selected-pin
    domains, including pins with legal radial choices. Replay shows MCU_NRF_RX
    blocks every MODEM_EN exit and combines with MCU_MODEM_TX to block MCU_NRF_TX.
- [ ] Select a compatible set with deterministic bounded matching/search, then
  negotiated swaps for larger clusters; include onward-route cost.
- [ ] Keep escapes provisional until area routing succeeds and allow local
  replacement of an escape that blocks a later net.

## 4. Upgrade global-to-detailed feedback

- [ ] Re-route hotspot contributors with alternative topologies and layer
  transitions; use multi-terminal topology with width-aware resource demand.
- [ ] Feed exact pin-access failures, via blockers, guide deviations, and
  exhausted searches back into global costs and candidate selection.
- [ ] Prefer guide regions to compulsory single transition coordinates.

## 5. Prove board closure

- [ ] Fix repeated-number physical-pad connectivity accounting.
- [ ] Achieve 58/58 detailed nets, zero KiCad unconnected items after zone
  refill, and no unwaived native/KiCad DRC errors.
- [ ] Verify determinism, routing runtime, plane continuity, and manufacturing
  rules on the pinned four-layer JLCPCB profile before release artifacts.

Primary research: [OpenROAD/FastRoute](https://openroad.readthedocs.io/en/latest/main/src/grt/README.html),
[CUGR](https://cwpui.com/doc/c10.pdf),
[concurrent PCB escape/area routing](https://ieeexplore.ieee.org/document/9586143/),
[variable-rule matching escape routing](https://scholars.lib.ntu.edu.tw/entities/publication/84cb474e-5460-4f5a-a1b7-208929c42bea),
[negotiated PCB escape routing](https://scholars.hkbu.edu.hk/en/publications/a-negotiated-congestion-based-router-for-simultaneous-escape-rout/),
and [TritonRoute pin access](https://vlsicad.ucsd.edu/Publications/Conferences/363/c363.pdf).

## Step 1–2 implementation notes

The current increment replaces surface-courtyard obstruction with pad/keepout
crossing estimates, samples legal independent via sites, couples transitions
using a single through-via cell resource, and exposes multiple candidate
direct or through-via pin accesses. Direct access can use a bounded local
eight-direction search around foreign pads. Selected access geometry is
carried in global guides, and ordinary detailed routing remains free to choose
another legal physical realization. Candidate accesses are checked
**individually**, not as a mutually compatible set. A `region_only` candidate
has a legal pad exit but no proven copper path to its coarse guide center; it
is counted separately, and detailed routing must close it. Simultaneous
selection and replacement are explicitly step 3. A global route remains a guide, never a
DRC signoff or proof that a through-via at a guide cell center is legal.

On the pinned four-layer full-vertical board with a 5 mm global tile, two
negotiated iterations, and one placement trial, this increment produced guides
for **58/58 nets with zero modeled overflow**. Seven selected pad exits are
`region_only` and remain subject to detailed routing. A deliberately bounded
one-pass, 1 mm/2,000-state detailed smoke run routed 27/58 nets and introduced
no hard geometry findings; that run is not comparable to the earlier eight-pass
51/58 detailed baseline and is not signoff.
The like-for-like eight-pass rerun was stopped after more than eight minutes
without a report; its completion count is therefore unknown. Runtime
profiling and the complete detailed/KiCad comparison remain open acceptance
work for step 2.
