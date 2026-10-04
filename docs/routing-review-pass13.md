# Routing review pass 13: corrected-library full routing and layers

Source: `f85e36b`; CopperLib: `5bcbaa4`. This run follows the critical-only
[pass 12](routing-review-pass12.md). Historical pass-3 zero-open USB copper
used an incorrect choke winding map and is not an electrical acceptance or
route-quality baseline for this run.

## Scope and method

Recompute from the locked/offline `.copper` source and actual KiCad/CopperLib
footprints, rather than load old copper. Use the provisional 100 x 80 mm,
six-layer JLCPCB profile, pinned Nordic placement template, candidate-01,
one placement/global feedback iteration, five global iterations and no
critical-placement trials. Ordinary routing uses 1 mm pitch, two passes,
20,000-state base searches, 10x failed-net repair, progressive guides,
constrained-pins-first, fanout and transactional soft rip-up. Ground helpers
use the previously authorized filled/capped via-in-pad process, a 5 mm contact
radius, six local escape trials and four full escape-feedback trials.

The normal `route-board` CLI is invoked through the existing timing-only
`outputs/rerun_r12_with_phase_log.py` wrapper. No route candidate, electrical
constraint, fabrication limit or acceptance predicate is changed by the wrapper.
KiCad 10.0.6 independently refills/checks disposable PCB/project copies.

Layer measurements use geometry read from the saved board with installed
KiCad Python. Native KiCad SVG plots supply the displayed pad/copper geometry;
the six-layer mosaic and crops are not rectangular pad approximations.
The source policy is In1.Cu dedicated GND, In2.Cu horizontal preference,
In3.Cu vertical preference and In4.Cu horizontal preference. These are soft
preferences, not direction restrictions or proof of reference-plane continuity.

Five synthetic checks exercise measurement clipping, split-invariant tile
length/density, two-nanometre heading tolerance, critical/ordinary/ground
partitioning and physical via spans. Tile length and summed width/clearance
corridors are density proxies, **not verified channel-capacity overflow**.
MST ratios are geometric baselines, not proof of routed connectivity, electrical
limits or optimal wirelength. Endpoint degree-two turn counts do not describe
interior T-junction topology. Connectivity is reviewed separately using KiCad.

## Code findings to evaluate with the layer evidence

1. R6b: a failed ordinary net can trigger a whole-board neutral layer/direction
   rerun (`pcbir/detailed.py`, `route_detailed`), selecting it when failure/overflow
   counts improve. The returned report does not record requested versus effective
   costs or whether this fallback won. Source settings alone cannot explain the
   committed layer balance. Add effective-policy/proposal telemetry, then
   localize the fallback with full transaction acceptance.
2. R5/R6a: `_search` returns the first feasible narrow/widened guide search.
   Projected eligible-layer corridors are tried only after earlier searches
   fail. A connected expensive detour does not cause comparison against spare
   signal layers. Compare bounded whole-net alternatives, preserving committed
   critical reservations and connectivity rather than forcing equal usage.
3. R16: `_search_once` charges `50 * COST_UNIT` for each out-of-guide edge.
   Unlike planar length/layer/wrong-way costs, this does not scale with physical
   length. Splitting a one-millimetre out-of-guide path into ten edges changes
   this term from 50 to 500 event units. Pad-inserted coordinate splits can
   distort guide-versus-layer alternatives. Separate true boundary-entry events
   from length-normalized deviation cost; test coordinate/pitch invariance.
4. R7: multi-terminal paths deliberately skip two-terminal compaction because
   shortcuts can erase later branch attachment nodes. Orthogonal-first and
   budget fallback can therefore retain right-angle branches. Cleanup must
   protect actual pad/via/junction contacts and validate the complete tree,
   not prune segments solely by bend count.

These are observations of current code, not claims that every mechanism was
activated by this run. No routing algorithm changes are made during the measured
rerun. A report-only follow-up now adds [per-net policy telemetry](routing-search-policy.md),
including mixed-policy subset repairs. The running process loaded `f85e36b`
before this addition; its report is explicitly treated as lacking policy evidence.
No requested-versus-effective value is reconstructed from layer balance.

## Full rerun and independent outcome

The run completes with 46 of 49 ordinary signal nets connected. All eight
critical nets retain their exact pass-12 copper (83 F.Cu segments, no vias);
all 51 component poses and the global/critical routing fingerprints match
pass 12. The source and dependency lock bytes are unchanged. No old copper or
signoff evidence is imported into this fresh run.

KiCad reports **34 unconnected items on three signal nets**, zero GND/critical
opens, no short/clearance/dangling findings and eight unchanged library findings
(four issues/four mismatches). One filled GND polygon is present. Native checking
still defers GND because it cannot infer independent fill; the overall plane
verification remains false because other nets are open and library findings
remain. Ground connectivity is an independent observation, not a waived gate.
The CLI exits 1 and `fabrication_ready` remains false.

| Open net | KiCad items | Search diagnostic |
| --- | ---: | --- |
| MCU_NRF_TX | 1 | Cannot reach U_NRF.10; MCU pad 62 to Nordic pad 10 is absent. |
| MODEM_EN | 2 | Octilinear and fallback budgets exhausted for U_MCU.64; three-terminal enable net remains absent. |
| V3V3 | 31 | 200,000-state repair search exhausted for R_RESET_MCU.1. The failed whole tree is not committed; these are not 31 distinct net failures. |

One accepted local ground repair escapes U_MODEM_KEY.2 and reroutes MCU_RESET,
MCU_SWDCLK and USB_C_CURRENT_1, preserving the same three failed signal identities.
All surface-helper pending ground pads are resolved, including J_SIM.SH and
U_CC.3/.10/.11. No placement/full escape-feedback rerun is needed. The saved
original PCB/project is independently refilled again on disposable `latest`
copies; their raw DRC confirms the CLI observation.

## Layer and shape review

All six native plots and central crops were visually inspected. The current
committed signal length is 1,380.45 mm, including the 169.63 mm protected critical
copper. **Do not credit this as a wirelength reduction:** three signal nets,
including the distributed V3V3 tree, are missing. Earlier fully connected copper
is electrically superseded and used different critical reservations.

| Layer | Signal / ordinary length | Observed shape and bottleneck |
| --- | ---: | --- |
| F.Cu | 526.47 / 356.84 mm | Dense MCU/modem access. Hottest 5 mm tile x35–40/y30–35 has 23.72 mm from five nets; x35–40/y40–45 has nine nets. All protected critical copper is here by explicit rule. |
| In1.Cu | 0 / 0 | Dedicated filled GND plane. No foreign signal tracks; independent ground opens are zero. Through-via antipads remain visible. |
| In2.Cu | 429.81 / 429.81 mm | Largest ordinary allocation; long 45-degree fan-in around MCU/GNSS. Hottest x35–40/y40–45 has 21.93 mm from four nets. 64.3% of orthogonal length is horizontal. |
| In3.Cu | 264.28 / 264.28 mm | Long horizontal modem UART/SIM/supply corridor around y20–25. Only 15.75% of orthogonal length is vertical despite the source preference; this does not prove which fallback won. |
| In4.Cu | 122.30 / 122.30 mm | Spare space, but MCU_RESET retains a right-angle staircase. Hottest tile is only 7.04 mm/two nets; 77.5% of orthogonal length is horizontal. |
| B.Cu | 37.59 / 37.59 mm | Very sparse; mainly USB_C_CURRENT_2 and NRF_RESET. All through-via lands are still obstacles even where traces are sparse. |

There are 153 physical through-vias: 55 GND and 98 signal. All signal segments
are axis/45-degree within two-nanometre integer tolerance; no oblique segment
remains. Degree-two shared endpoints include 236 45-degree, 57 90-degree and
eight 135-degree turns. GNSS_RX has one collinear overlapping seam: its
F.Cu body from (49,53.2) to (52.25,53.2) overlaps the fanout from the actual
U_GNSS.3 pad (50.75,53.2) to (52.25,53.2). This is 1.5 mm of redundant overlap,
not a critical-pair reversal or a KiCad dangling/open finding. Preserve the real
pad/contact graph before trimming owned fanout/maze seams. R7 tracks this work.
MCU_RESET is the one committed net reporting orthogonal search mode; cleanup
still needs to protect its multi-terminal tree.

USER_LED_1_A remains a feasible but expensive top-layer detour: 9.99 mm against
a 2.02 mm pad-MST baseline (4.95x). MCU USB retains 28.30/27.04 mm paths at the
unchanged placement. These justify bounded alternative comparison (R5), not
relaxing profiles or independently pruning a pair. PWR/MODEM_SW and feedback
also have multilayer detours; route connectivity alone does not qualify the
power-stage layout. Qualified power/RF/return-path work remains separate.

## Next implementation order

1. Reproduce the three failed-net terminal/tree cases on the saved immutable
   baseline; add route-stage/access/guide telemetry (R9). Coarse global routing
   has zero overflow but detailed access still fails, so zero global overflow
   is not a local routability certificate.
2. Fix physical-length guide-deviation costs (R16), retaining explicit planes,
   original access/profile limits and full transaction checks. Compare the
   same failed identities and eligible-layer alternatives before larger budgets.
3. Localize the now-reported neutral fallback (R6b) and add clearance-aware
   local/failure pressure (R6a). Spare layers are candidates, not a mandate for
   equal lengths, and their reference paths still need qualification.
4. Compare bounded feasible-route alternatives (R5) and contact/branch-safe
   access-seam/tree cleanup (R7), including the LED detour, reset staircase,
   acute seams and redundant GNSS fanout overlap. Preserve critical pairs and
   connected unaffected copper atomically.
5. Rerun and repeat independent filled-zone/layer review; resolve signal opens
   before route-quality claims. RF support/antenna clusters, library qualification,
   actual stackup/return paths and manufacturing/CAM remain production gates.

The report-only policy follow-up passes all 439 CopperScript tests (seven new
policy cases, 459 upstream CAM warnings), plus the five measurement checks.
The strengthened post-suite policy-only rerun also passes all seven cases,
including a fresh route with the reporter disabled that produces identical
board/metrics/fingerprint. No search or acceptance cost changes are included.

Operational timing through CLI KiCad verification is 1,603.50 s (26.7 minutes):
placement/global 121.07 s, critical 87.96 s, fanout 3.72 s, initial detailed
1,247.98 s, land closure/native 9.19 s, zone feedback 117.23 s and independent
verification 3.64 s. The tests ran concurrently; these are not speed benchmarks.
A read-only stack snapshot during zone repair shows normal native exact-spacing
checking. Full per-net/search-stage progress remains R9 work.

Recorded identities:

- Unchanged global: `c5668ea3f5bb102e68f3becf6cb6eace033ba502d8b6a5af617c2577dd7cb86f`.
- Unchanged critical: `d4abd396efd4f0c21a20733d1ff2dca2d06a8236ad02f9facb66cd18a7f89e0f`.
- Detailed route: `e26c8ff9e7f440a9f9ed30dca09755f83ffd408e11a62fab79e487247ec8a304`.
- Report bytes: `8629e63d0c6cbe92f7fcbc34a879ff4679016286881eb6ced996b495fa64a06b`.
- Original PCB: `bb66c7e3f6e1f3bf87174c979f60041700db1c8be439aec7228c074c99a54e41`.
- Independent DRC: `9a7f3c93b93b512083affb40c0f31b47549e5c9c66b1fa13d92eae2fe6d3d021`.

Reproduce with the README's complete `route-board` command, also passing
`--placement-templates examples/full_vertical/placement_templates.json` and
`--critical-feedback-trials 0`. Keep the corrected library and locked bytes.
Future runs include policy telemetry, but its addition does not change geometry.

Artifacts are outside the repository under:
`C:/Users/anden/Documents/Codex/2026-09-17/referenced-chatgpt-conversation-this-is-an/outputs/routing-review-pass13-layers-2026-10-01/`.
They include timing checkpoints, original and disposable filled boards, raw
independent DRC, extracted geometry, analysis JSON, native per-layer SVG/PNG,
the six-layer mosaic, central crops and independently derived missing-contact
overlay (`missing-connections.png`, dashed lines are KiCad airwires, not proposed
copper). `provenance.json` binds the saved files and verifies unchanged critical
copper/poses/inputs. The original user-owned draft files
are untouched. RF support/antenna layout, qualified stackup/return paths,
eight footprint-library findings and manufacturing/CAM signoff remain separate
gates even if routing closes.
