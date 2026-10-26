## Generic fixes for the CopperProbe open connections (2026-10-25)

Each cause in the section below got a generic change on the unified path;
nothing is board-specific and nothing relaxes a declared rule.

- **Pair search escalation.** A coupled pair that exhausts its bounded tier
  (`initial_pair_state_limit`, 6,000) without any candidate continues the
  same search, ports and gates under `critical_pair_state_escalation` times
  that bound (default 10; the legacy path never escalates). The USB_RX1 pair
  around J_USB needs about 20,000 states and connects on the real critical
  inputs at the default setting in 214 s. A pair that connects within its
  tier never escalates; the result records the bound it ended under.
- **Planar ordinary nets route in the critical stage.** A general net with
  one allowed layer and no via budget is promoted after the critical
  router's guide checks (`route_critical_nets(promote_planar=...)`, on with
  unified options) and routed before pairs under its own rules; the result
  board keeps the declared rules and the escape planner leaves these nets
  alone. CopperProbe's FB and the 21 analog front-end nets route this way.
- **Macro port halos for the planners.** Beyond each hard-macro port of a
  net that may change layer, a keepout one clearance past the port, two
  via pitches outward and one to each side binds the escape and bypass
  planners only (`_macro_port_halos`), so bypass copper no longer takes the
  via transition sites of the FX10 USB escapes.
- **Re-sited escapes.** Escape reservations that bypass planning released
  and revalidation could not restore are planned again around the bypass
  copper before critical routing (`_resite_escapes`; reservation ids stay
  index-stable). On CopperProbe 15 nets, K7 (ACQ_EN) and K9 (SWDIO) among
  them, get their exits back.
- **Witnesses verified in the critical context.** The bypass witnesses are
  admitted on the planning frame (halos, pre-release reservations); they are
  verified again in the critical router's keepout context so the copper
  binds as copper, not as all-net keepouts (which blocked USB_DM/DP's collar).
- **Rip-up for unbundled pairs.** A failed pair that no bundle covers tries
  the committed pairs sharing a terminal package as rip-up suspects under
  the bundle repair limit (TX2_IC's route took TX1_IC's collar space). The
  attempts are recorded on the failed group's result (`repairs` in the
  critical report, the `critical_group` progress event and the
  `CRITICAL-UNROUTED` issue), each with the ripped-up pair and how the failed
  group's search ended without it (states used, bound exhausted or not): a
  launch that is still blocked ends after a handful of states, a budget miss
  exhausts the bound.
- **Bypass copper takes a ball's site only as a swap.** A bypass link the
  escape reservations block used to release every reservation within 1.5 mm
  of its pads, retry all pending links at once and keep the whole release if
  any link improved: on CopperProbe 58 of 74 reservations went for 8 links,
  20 came back, 21 balls never got a site again (ADC_OVR, ADC_OE among
  them). Now each pending link is retried alone with that box lifted as the
  candidate set; a reservation its copper leaves legal stays bound; the balls
  whose reservation the copper does take are planned again beside it before
  the link is accepted, and a link that would leave a ball without a site
  stays pending, naming the ball (`bypass_reservation` `finished`: `swapped`,
  `rejected`, `pending_links`).

Bounded CopperProbe runs through the critical stage (fresh placement): the
critical stage now leaves only USB_TX1_IC open (two pairs before), with FB
routed and the bypass copper bound as copper. Full-board results follow.

## CopperProbe after the generic fixes (2026-10-25)

Full `route-board` runs with the fixes above (`bd2eba0`), `--fine-regions
arrays`, unified defaults. Run A routes the original board from the same
placement checkpoint as the `arrays` baseline below, so the two differ only
in the router. Run B routes the board with its three feasibility pins relaxed
(CopperProbe `6de0e82`: FB_XTAL, C_FX10_M8 and the TX1 coupling capacitors
become placement regions) from a fresh placement. Both ran at the same time
on one machine, so their times are inflated against the baseline's.

| Run | Open / connections | Signal nets open | Plane pads open | Vias | Track | Length/skew warnings | Commit rounds | Total |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `arrays` baseline (`ea0e23b`) | 11 / 237 | 10 | 3 (GND N4/N6, V33 FB_XTAL.1) | 335 | 2,587 mm | 4 | 2 (none adopted) | 6,720 s |
| A: fixes, original board | 6 / 237 | 5 | 2 (GND N4/N6) | 331 | 2,748 mm | 7 | 3 (one adopted) | 11,874 s |
| B: fixes, relaxed pins, fresh placement | 8 / 237 | 7 | 2 (GND N4/N6) | 321 | 2,677 mm | 4 | 4 (two adopted) | 14,789 s (2,615 s placing) |

Run B is not comparable net by net (fresh placement; single runs vary by
about +-4 nets), but it answers the two questions it was run for. The
relaxed pins did not connect USB_TX1_IC: its launch is free now (the first
pass exhausts 60,000 states instead of ending after 10) and still no route
fits beside TX2_IC, so the capacitors were part of the wall, not all of it.
And the escape-reservation release repeats exactly (58 of 74 released, 20
restored, 21 still pending after re-siting) with the same three FX10 balls
losing every launch at the global stage (C4/D3, L11/M10, L3/L5): the cause
is the planner, not the placement. Its USB_DM/DP skew and RX1 skew are
within budget on this placement (4 warnings against 7).

Run A connects the USB_RX1 pair (escalated search; 2.19 mm skew warning
against 0.127 mm), FB, FX_XTAL, ACQ_EN, SWDIO and the V33 pad of FB_XTAL. Of
the baseline's open nets only USB_TX1_IC, ADC_OVR and FX_V33 remain; XTALOUT
is new. The remaining opens have three causes:

- **USB_TX1_IC** fails in both critical passes after 10 states each: its
  launch is still blocked, and ripping up USB_TX2_IC did not free it (the
  attempts are now recorded on the result; this run predates that record).
  The capacitors under and beside the macro ports are the remaining wall,
  which run B tests.
- **ADC_OVR (E1), XTALOUT (M13): outer-ring balls with every exit taken.**
  Both are ring-0 balls that the kit exits straight out on the surface. E1's
  south exit is closed by ADC_D9's own surface escape, which leaves F1 next
  to it, loops west under E1 and runs north to a via site; M13's four
  interstitial sites hold FX_GPIO's 0.4 mm track, the D18 plane-stitch via,
  FX_XTAL's via and a GND stitch. Both nets had a reservation after the
  escape pass; ADC_OVR's was released for a bypass link and never restored
  or re-sited (so was ADC_OE's in run B, open there), XTALOUT's was re-sited
  but the site was gone by the detailed stage. The release is fixed below
  (swap); the ring-0 surface exits and the other stages' via sites are the
  escape plan's E9/E7 territory.
- **FX_V33** is rejected at the exact commit check (blocked by RESREF on
  In2.Cu); run B loses it with "no path to the remaining pad group".
- **GND N4/N6** stay inside the `FX10_USB_ESCAPE` macro fan without a via;
  the macro needs GND vias (consumer). The three `GLOBAL-UNROUTED` issues
  (FX_USB18 at L3, FX_VDDD at L11, GND at C4: "shared transition capacity
  rejected every candidate") are supply balls whose launch sites lie under
  their fixed bypass capacitors on B.Cu; they are fed by the bypass copper
  and planes, and the global stage reports them on the bare board at
  placement time, before any escape or bypass is planned.

The length/skew warnings grew from 4 to 7 because more nets are connected:
SENSE (9.87 mm against 4 mm), the DM/DP skew (0.94 mm against 0.5 mm), the
RX1 skew, the ADC bus skew (7.08 mm against 5 mm) and three ADC lines over
35 mm.

## CopperProbe: why the 11 open connections fail (2026-10-25)

Causes of the `--fine-regions arrays` run above (`ea0e23b`, fresh placement),
from `copper issues`/`copper view`, the native unconnected-item report, the
`critical-inputs`/`critical-retry-inputs` checkpoints and `replay-critical`.
Two pairs, six ordinary nets, two plane pads.

- **USB_RX1 pair (J_USB B10/B11 to the FX10 macro ports): pair search budget.**
  The connector's own pads let the pins depart only to the east and south-east,
  so the route must loop around the connector as RX2 did on B.Cu; the coarse
  guide's transition lands on an A-row pad and is rejected before search; the
  joint pair search spends its 6,000 states in 12 searches and ends 12 mm from
  the goal. On the real critical inputs, `replay-critical --pair-states 60000`
  connects it (via search, 145 s); 6,000 does not.
- **USB_TX1_IC pair (FX10 macro ports to C_USB_TX1M/P): terminal geometry and
  bypass copper, not budget.** From the bare placement it routes in 130 states;
  on the critical inputs it fails at 6,000 and 60,000 states and on the retry
  inputs too. West of the capacitors nothing is reserved, but the pair's launch
  lanes are laid between the two capacitors (1.5 mm apart, 0.15 mm from each pad
  edge against 0.29 mm clearance) and the search never tries the six departable
  ports 1.2 mm west of the pads (4-6 searches, 10 states). At the FX10 end the
  bypass planner put 10 GND and 10 FX_USB18 B.Cu segments and two GND vias under
  the macro ports, leaving no via transition, so neither the surface nor the via
  family has a candidate. TX2_IC, whose capacitors sit on B.Cu, succeeds through
  the via family at the capacitor end.
- **ACQ_EN (K7), SWDIO (K9): inner balls without a dogbone.** The ordinary escape
  plan sited every net (62 nets, 74 vias, 0 pending); bypass planning then
  released 58 of those reservations for 6 pending bypass links and only 22 were
  restored (18 nets unrestored, these two and ADC_OVR among them). By the time
  `plan_bga_fanouts` runs, the cell centres around K7/K9 hold FX_V33, GND and
  SWCLK vias and it reports 7 balls blocked (G5, K5, K7, K9, C7, E11, F11). The
  kit gives every inner ball its own via on a regular lattice; the E7 lattice
  does the same (58 sited, none blocked) but still measured worse on the whole
  board.
- **ADC_OVR (E1, outer ring): surface exit sealed.** Outer-ring balls escape on
  the surface (the kit hooks them outward through the gaps); E1's gap is taken by
  the ADC bus escapes (ADC_D2/D3/D5/D9/D11, routed first as a bus) and FX_P0/D18
  bypass copper on B.Cu, and its own escape reservation was among the unrestored.
- **FB: single-layer, zero-via net cut by a pair.** FB (F.Cu only, max_vias 0,
  8 mm) runs from PWR/U.5 to two resistors 3.5 mm below; the USB_RX2 pair, routed
  earlier, crosses that corridor on F.Cu while USB_DM/DP took B.Cu there. Nothing
  tells the critical router that a planar ordinary net needs the corridor.
- **FX_XTAL and V33 at FB_XTAL: fixed part enclosed.** FB_XTAL is fixed inside
  the USB_DM/DP corridor (placement warning W8); the pair wraps around it on F.Cu
  with GND bypass vias beside it, so pad 2 cannot reach M12 3.7 mm away and pad 1
  gets no via to the In3 V33 island (plane stitch `still_cut_off`).
- **GND N4/N6 and FX_V33 at C_FX10_J3: the macro exit fan.** The FX10_USB_ESCAPE
  macro carries five pairs out of columns M/N on F.Cu; the GND balls between the
  pairs (N4, N6; also M4-M10 get no lattice site) have no via and no surface
  exit, and the J3 rail capacitor at the fan's edge stays unconnected. The kit
  gives these GND balls vias; here the macro would have to.

Generic follow-ups, in order of connections recovered: pair state budget scaled
to the guide length (RX1); pair launches for two separate capacitors merged in
front of the pads, and bypass planning keeping the macro ports' transition
sites free (TX1_IC); the escape-reservation release restoring or re-siting the
released balls before fanout (K7, K9, E1); planar zero-via ordinary nets
routed before pairs (FB). Consumer-side: FB_XTAL's fixed position and GND vias
in the USB escape macro.

## CopperProbe: `--fine-regions arrays` against `dense` (2026-10-25)

Measured on the escape-planning branch at `ea0e23b` (main `842e643` merged),
both routes from one fresh placement checkpoint (the earlier In4 checkpoint
is refused by the merged checkpoint identity), unified router defaults,
congestion feedback on (it proposed no accepted move in either run).

| Mode | Lattice nodes | Setup | Repair rounds | Total | Open / connections | Signal nets open | Vias | Track | Length/skew warnings |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `arrays` (default) | 540,030 | 715 s | 1 (not adopted) | 6,720 s | 11 / 237 | 10 | 335 | 2,587 mm | 4 |
| `dense` | 977,130 | 1,016 s | 2 (one adopted) | 7,792 s | 11 / 237 | 8 | 339 | 2,590 mm | 5 |

`arrays` refines only the FX10 (k=2, 0.125 mm, 45,600 extra nodes). `dense`
also refines J_USB, PWR/U and U_CC (k=2), U_ADC (k=4, 0.0625 mm, 175,824
nodes) and U_DRV and U_NEG (k=5, 0.05 mm, 162,960 nodes): 81 % more nodes,
42 % longer setup and 16 % longer in all. Completion is identical. Both runs
lose the same critical pairs (USB_TX1 cohort clearance, USB_RX1 guide
transition at J_USB), FB, FX_XTAL, ACQ_EN and the plane nets GND/V33/D18 in
copper. `dense` connects ADC_OVR and SWDIO near the refined ADC but loses
FX_V33 (blocked by FX_VBUS at the exact commit check), and its ADC bus skew is
6.40 mm against 7.08 mm (limit 5 mm), with ADC_D1 and ADC_D10 instead of ADC_D8
over the 35 mm limit. Single runs vary by about +-4 nets, so none of these
differences is established. `dense` stays a measurement mode: on this board it
costs time for no completion gain.

## Physical via spans and guide costs through repair (2026-10-24)

Python and native search now choose a legality mask for each layer transition's
physical via span. Blind and buried vias ignore copper keepouts on layers their
barrels do not reach; through-via fallbacks still check the complete barrel.
Drill spacing remains layer independent, return vias must cover the proposed
barrel, and unsupported transitions are excluded before exact commit. Existing
source copper and final exact validation retain their authority.

Ordinary repair and both pour-recheck steps receive the original global guides,
so nonzero guide costs remain active after the initial negotiation. Regression
fixtures cover both search backends, occupied and unoccupied span layers, drill
spacing, return coverage, through-via fallback and guide handoff. The broader
router/index/CLI/checkpoint set passes 170 tests.

Both native kernels are rebuilt with Zig 0.13.0 using
`python tools/build_kernel.py --zig PATH_TO_ZIG`. The Windows kernel and Python
fallback are exercised locally; the Linux kernel is cross-built but not run on
this Windows host. The C source uses LF to keep its digest platform independent.

The matched standard ladder against clean `67f1c82` has no quality regressions:
round-led-ring retains 51/51 connections, 48 vias and 604.833 mm; nRF52 retains
35/35, 42 vias and 345.115 mm. Both have zero exact geometry errors and zero
estimated plane opens. Concurrent timings are not a speed comparison, and these
ladder runs do not include whole-board KiCad refill or hardware qualification.

## Rebind immutable prefix occurrences at bypass admission (2026-10-24)

A full-board integration exposed a source-array witness identity mismatch:
re-materializing the immutable prefix preserved exact geometry but created new
source-edge objects. The complete bypass reservation was rejected and fell back
to keepouts. The standard handoff now restores unique equal original occurrences
using the existing strict restore helper, then runs unchanged fresh ownership,
context, association and geometry admission. Missing or ambiguous occurrences
still fail closed; no context digest or copper value is changed.

A permanent regression reproduces the original witness_via_invalid failure and
passes after the fix. The 56-case affected set (including the added array-backed
standard pipeline case) passes across the completed runs. The full pipeline
keeps copper-mode reservations, retains all required arrays, preserves the
bypass through routing, and replays the checkpoint successfully. CopperBench
full435 was interrupted after its binding failure; it has no final/native result.
A new full run is required for the combined current-source result.

## Shared clock/data cohorts (2026-10-24)

Contiguous single-ended clock and critical jobs now negotiate together across
priorities. Coupled pair boundaries and independently retained verified owners
remain separate. The selected engine keeps each original profile and priority,
and the existing critical matching pass runs against all accepted copper.

On the same saved CopperBench early input, all 15 critical net names connect
with zero exact non-open errors and zero native opens/geometry errors on those
nets. Required source macro/array copper is retained. The two RMII clocks are
28.409533/28.409534 mm after normal tuning (1 nm skew; 2.54 mm source limit).
The partial export retains 499 unrelated open items and 17 dangling warnings;
this is not whole-board qualification. A separate untuned joint diagnostic
connected all eight clock/data nets but failed matching by 15.298 mm and was
rejected. No source length, via, clearance or matching requirement was relaxed.

The 60 critical/retained-copper regressions, including a fixture that requires
real clock-length tuning, pass, along with 15 additional bundle/lane cases.
All four standard unified examples retain zero native opens and geometry errors
and identical copper metrics to the preceding cohort selection. Ring and nRF
still fail overall on unchanged warnings; full-vertical and cm4 pass overall.
A matched two-million-expansion early-stage control is pending; the preceding
coarse diagnostic used 400000 expansions. These results are not a runtime claim.

## Reuse declared via arrays in cross-layer bypasses (2026-10-24)

The bypass path graph previously ignored filled/capped vias, including exact
required source arrays already present beneath exposed ground pads. The planner
could therefore miss a physical return through an existing array and reject a
complete supply/return transaction. It now recognizes only original occurrences
matching required array geometry, checks current copper/drill/land clearances
against the specific declared pad, and writes an explicit checked lead inside
that same land when needed. Unrelated pad groups and component internals remain
unsupported. New proposals still create ordinary through vias only.

The focused 48-test set covers existing path contracts, one- and nine-via arrays,
whole-net via budgets, exact occurrence preservation, idempotence, missing or
altered declarations/geometry and new foreign obstacles. CopperBench's original
PHY capacitor placement now obtains an explicit supply and return witness in a
bounded diagnostic (six added tracks, two ordinary vias), whereas the same
planner input previously exhausted its candidates. This is a scoped result;
whole-board routing, native evidence and engineering qualification are separate.

## Shared single-ended critical negotiation (2026-10-24)

When unified routing is selected, contiguous same-priority single-ended critical
jobs now share one bounded negotiation, preserving each original source profile.
Clocks, differential pairs and retained verified owners keep their existing job
boundaries. Each proposed net receives a fresh exact commit check; failed owners
add no copper. Changed input prefixes and added foreign owners are rejected.
No congestion-experiment commits or board-specific selectors are included.

The 153 related critical, retained-copper, pair, and unified regressions pass.
The four standard examples with explicit unified selection and native refill
complete with zero native opens and geometry errors. Ring and nRF still fail
the overall gate on native warnings; only full-vertical and cm4 pass overall.
These are connectivity results, not four clean passes. The initial omitted-router run
was interrupted and is not a matched comparison. On CopperBench's saved early
critical input, the coarse cohort connects four of six RMII nets versus three
in the earlier sequential full run; all MDI and clock connections remain. Fresh
native checking confirms RXD0/RXD1 each retain two open items, all other selected
nets connect, and source macro/array copper is retained. That diagnostic uses
400000 search expansions rather than the historical full run's 2000000, so no
matched runtime or pure algorithm attribution is claimed. A 0.125 mm /2000000
expansion diagnostic leaves three RMII nets open and is not selected. Whole-board
routing and engineering qualification remain separate.

## Isolated exact-congestion revalidation 429

This branch reapplies the four preserved congestion experiments to commit4ed3d66.
It is separate from main. Historical results below do not validate this new
combination. The via-budget-only trial423 regressed the matched consumer control;
the full combination now requires fresh tests and matched board validation.
No push, merge or board adoption is implied.

## Reproducible router review fixes (2026-10-24)

Unified plane contacts now materialize a required pad-to-grid lead before
claiming connectivity. Zero-via limits are enforced, and final whole-net length
and via checks include retained source copper after smoothing and bus routing.
Over-budget additions are rejected while source copper remains intact. Legacy
routing also counts retained vias against the whole-net limit. Quantity equality
and hashing now use normalized values independently of the display unit.

Unified negotiation checks its deadline before starting nets and searches and
periodically during Python search; cancellation preserves the previous incumbent.
The deadline remains soft: setup, final exact validation and an in-flight bounded
native search may finish. Unstarted nets report an explicit budget exhaustion.

The six original reproductions pass. Permanent regressions cover both search
backends, fixed-copper preservation, zero and positive budgets, already-connected
over-budget input, and expired negotiation. The plane-lead fixture also passes
KiCad refill. The broader routing/physical/compiler/ERC set passes 126 tests with
one optional skip.

Matched standard ladder runs against clean `ce42775` retain 51/51 connections on
round-led-ring and 35/35 on nRF52, with zero estimated plane opens and zero exact
geometry errors. Via counts and lengths are unchanged: 40 / 582.844 mm and
44 / 350.563 mm respectively. Concurrent runtime measurements are not a speed
comparison. These ladder runs do not include whole-board native verification;
the focused native refill is not a complete consumer or hardware qualification.

## Main integration without congestion experiments (2026-10-24)

This main selection merges the routing work through `053f4bb` and independently
cherry-picks the required-array placement fix `6fc36fd` as `f208c9f`. The unified
router implementation is unchanged from `053f4bb`. The congestion experiments
`30dd622`, `adb9a39`, `172407d` and `1210438` are excluded from both code and
ancestry; they remain on the experimental routing branch. Their lead/via-budget
regression files and congestion-specific validation claims are not included here.

On this exact selection, the related router, macro, placement-array, escape,
pair, return, decoupling and native-fill set405 passes 177 tests with five optional
skips. Repeating the skipped fixed-array/native refill case408 with KiCad on PATH
passes as well. Four optional Nordic archive/source-layout fixtures remain
unavailable in this checkout. These focused results are not a full-suite pass.

Fresh ladder406/407 reruns use this selected compiler. Initial completed checks
retain 51/51 signal connections on the legacy ring, 35/35 on nRF52, and native
zero opens/geometry errors on CopperLedRing. Other selected full-board checks
remain in progress at integration; results from the excluded congestion compiler
must not be substituted. The broader experimental-branch Python run394 is also
incomplete and failing; five failures independently reproduce on upstream main.
No all-tests, complete CopperBench, or hardware qualification claim is made.
## Exact tentative copper and through-via congestion (2026-10-24)

Unified negotiation now checks actual emitted tentative tracks and vias through
one fresh clearance index. Mutable occurrences identify both conflicting owners;
locked source copper never becomes a rip-up dependency. Emitted breakout widths
match exact commit. Track, lead and via history prices the actual conflicting
geometry instead of treating overlapping raster envelopes as proof of a clash.

Through-via proposals also price foreign mutable tracks on every barrel layer,
including layers the path does not visit. Both grid tracks and off-grid leads
contribute to that reservation. The largest candidate via and clearance provide
a conservative cost envelope; exact commit remains the acceptance gate. Required
arrays, source copper, return coverage, via budgets and native checks are retained.

Control376 exposes a real diagonal track/via clash between raster sample points.
Control380 exposes a via crossing an internal-layer track while searching only
front and back. Both Python and native kernels reproduce these cases. The latest
focused router/lead/via-budget set passes all 54 cases (381; pre-push403 repeats it).
The tested router file SHA-256 is
`7b54f4e00ac8321099a4ac7c7b69722d25ad5d6107dcba2164ebad5ecb04dfb0`.

All runs below use the standard pipeline with `--router unified`. Benchmark390
routes the four examples; independent KiCad refill checks are retained alongside
each output. Benchmark388 routes both maintained consumers with native verification.

| Board | Signal connections | Native opens | Exact geometry errors | Vias | Track length (mm) | Wall time (s) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Legacy round LED ring | 51/51 | 1 | 0 | 47 | 654.422 | 181.0 |
| nRF52 | 35/35 | 0 | 0 | 39 | 352.550 | 48.0 |
| Full vertical | 86/86 | 0 | 0 | 186 | 1605.004 | 219.3 |
| CM4 | 21/21 | 0 | 0 | 61 | 587.990 | 23.1 |
| CopperLedRing | 51/51 | 0 | 0 | 37 | 542.624 | 94.7 |
| CopperAssetTracker | 120/120 | 0 | 0 | 269 | 1995.230 | 1186.0 |

The legacy ring remains incomplete: one GND open after native refill. The matched
upstream-main control397 has two native opens and two signal opens. Example native
reports retain footprint-library warnings; the legacy ring also has isolated
copper and silkscreen warnings. Concurrent workloads make these timings unsuitable
as a speedup claim. No equivalent legacy-router benchmark pass is asserted.

All supported positive example check/compile targets391, all 22 physical-example
layout/editor smoke checks395 and all 20 Node editor/LSP tests389 pass. The local
Python suite394 is incomplete and has reported at least 13 failures. Five plane-
first assertions also fail on unchanged upstream main `5fce7028` (control402): four
expect a blocked intermediate result now found ready, and one expects the older
`plane_first` strategy name instead of `critical_and_plane_first`. Other failures
remain unclassified. Run384 had an invalid temporary-directory parent and is not
validation evidence. This is not an all-tests pass, whole-board CopperBench
closure, or electrical/assembly/CAM/hardware qualification.

## Exact off-grid lead/via interactions and bounded via-price families (2026-10-24)

The earlier half-diagonal lead sampling can mark legal copper as conflicted.
Control370 demonstrates four false positives across both kernels and mirrored
placements. Negotiation now retains the original raster centre sampling and
checks actual emitted lead fragments against tentative full-span vias through a
fresh clearance index. Only exact mutable lead occurrences receive feedback;
source copper stays locked. Lead-specific prices and via prices persist across
repair history. Declared breakout clipping uses the same operation as exact
commit. No source keepout, return requirement, via limit or acceptance is relaxed.

A connected via-bounded net first tries four bounded via-price multipliers with
ordinary congestion/history intact. Discounting is retained only as a final
proposal family. This lets hard-budget nets negotiate with one another instead
of repeatedly selecting the same cheapest path after erasing their soft costs.
The valid two-net corridor has a passing unpriced control; frozen6fc36fd fails
both priced-kernel cases367, while after366 connects both within two vias each
with zero exact geometry errors. Earlier359-365 used overlapping terminal bodies
or an unproven narrow corridor and are retained as invalid fixture experiments.

All19 focused cases371 pass, including real collisions, a nearby legal pair,
mirroring, stale history, occupied corridors and a real impassable wall. Related
router/return/source/macro/array checks372 pass with four optional skips. Actual
fresh PHY baseline358 and price-only368 remain incomplete; combined exact replay
373 and fresh native comparison are pending. Neither diagnostic establishes full
board, electrical, assembly or production qualification.

## Reserve required pad-via arrays during placement (2026-10-24)

Incremental placement now reserves every declared filled/capped array's annulus
and drill against accepted peer pads, footprint holes and other required arrays.
The checks are symmetric and use the actual candidate/held owner poses; unaccepted
source seeds and optional single-via permissions create no array obstacles.
Filled/capped array sites do not introduce a body-only courtyard exclusion.
Source nets, array sizes, spacing, multiplicity and final exact materialization
remain authoritative. Cached objects are geometry only, never acceptance.

The public array geometry helper accepts an optional partial map of accepted
owner poses. Default callers retain identical complete source geometry. The
placement checker uses ordinary copper/drill clearance floors and still requires
fresh full array materialization; this is not routing, assembly or CAM admission.

Control351 misses a 1 nm drill-clearance violation in both placement orders and
uses an unaccepted source owner pose. After352 passes the boundary/materialization
checks. Related354 passes with one optional native skip; expanded355 also proves
the generic planner moves a support off the array and preserves all nine sites.
Actual PHY search350 previously failed during array materialization; corrected
search353 finds a legal back-side INTSEL location while all other trial poses
remain fixed. Source regeneration and complete routing/native validation remain
required before adopting the two-component reference adaptation.
## Earlier off-grid sampling trial, superseded by exact interactions above

Half-pitch sampling can miss every representative grid node of a short off-grid
lead near a cell corner. Lead centre sampling and lead congestion pricing now
cover the half-diagonal distance. Conflict accounting identifies both mutable
owners of a contested resource; via history also prices a via whose clearance
region conflicts with the lead. Source raster masks and exact commit remain
unchanged. This is conservative negotiation accounting, not clearance relaxation.

Both kernels miss the exact via/lead collision in control339. Intermediate340
exposes only one owner. The complete correction passes both kernels, positive and
negative offsets, a nearby real collision and separated legal copper (eight
cases344). Related router, return, ownership and via-budget checks342 pass with
four optional skips. Actual18-net PHY replay343 and ladder comparison are pending;
no complete-board or speed result is claimed.

## Via-budget proposals can contest mutable congestion (2026-10-24)

After the existing higher-via-cost retry, a still over-budget proposal gets one
bounded retry with mutable congestion/history costs discounted. Source geometry,
layer restrictions, via/return masks, search-state limits and exact commit checks
remain active. The legal proposal keeps its normal congestion claims so other
mutable routes can negotiate. Global occupancy/history and per-net via pricing
are unchanged after the retry. Zero-cost buffers are local and reused by a router;
the Python and native kernels share the same policy without a native ABI change.

The two-kernel synthetic fixture fails before the correction despite an available
two-via corridor. Five after cases cover stale history, current occupancy pressure,
normal corridor claims and a real source wall that remains impassable. The related
router/return/macro set passes (four optional library/native fixtures skipped).
The first related run330 failed to create sandbox temporary directories; corrected
workspace-local temporary run332 passes without any gate changes.

Actual 18-net PHY replay331 reduces four unresolved nets in326 to two, with zero
exact geometry errors. D3V3 and TXD1 now connect. CRS_DV is blocked by PHY_ADDR;
RXD1 is blocked by RXD0. The test retains the source, immutable prefix, existing
return-via constraints and coarse grid. Its 159.7 s versus 186.5 s control is a
loaded diagnostic observation, not a complete-board speed or closure claim.

## Preserve macro boundary access during source-copper reuse (2026-10-24)

Union/find root labels do not distinguish private macro copper from public
access. Existing-copper anchors now yield to the group's declared macro ports;
plane-joined witnesses are also limited to the actual public port nodes. This
preserves ordinary pad-rooted exit reuse and real via barrel spans while stopping
an internal via from bypassing an immutable boundary stub. Required package-exit
witnesses remain authoritative. No macro copper is deleted or rewritten.

The regression fixture fails on both Python and native kernels before the fix,
then connects through the declared free-track port with exact ownership intact.
An independent KiCad10.0.6 refill reports zero opens or dangling tracks. Related
macro, source-access and router tests pass. Ladder323 exposed this issue as one
MODEM_EN dangling stub on the asset tracker despite 120/120 signal connectivity;
the full native consumer328 now passes with120/120 signals, zero native opens/
violations,265 vias and1993.925mm in1170.8s. Its ordinary prefix and immutable
port remain intact. This is consumer routing compatibility, not electrical or
manufacturing qualification.

## Preserve signal reservations through plane preparation (2026-10-24)

Early package-exit masks now survive critical routing, bypass work, BGA planning
and plane stitching. They are removed only when ordinary negotiation takes
ownership. Removing them before plane stitching allowed a later GND/supply via
to occupy a promised signal site. Final physical boards do not retain these
temporary masks; original declared keepouts remain authoritative.

A functional four-layer fixture demonstrates that an otherwise legal competing
GND-via site stays blocked during preparation and is released for negotiation.
The corrected fixture fails on frozen b7c1046 (control321), passes after the
change (319), and the 50-case related set passes (322). Earlier controls317/318
used a one-pad package, which has no neighbouring-land escape demand; those
failures were invalid evidence and are retained. Control320 accidentally imported
the active compiler through pytest configuration and is also not a valid control.
The matched control explicitly pins pytest's configuration and compiler path.

Ladder323 improves round LED from 50/51 to 51/51 signals with zero geometry errors,
40 vias and 582.844 mm (formerly 41 vias and 552.542 mm). It retains nRF52 35/35,
44 vias and 350.563 mm. The separate LED consumer passes fresh native with zero
opens/violations, 33 vias and 545.708 mm (formerly 35 vias and 545.611 mm).
The 30.302 mm extra round-LED copper and 0.097 mm extra consumer copper remain
tradeoffs; loaded timing is not an isolated speed comparison. Asset tracker
finishes with 120/120 signals and zero native opens, but one immutable MODEM_EN
port stub is dangling. This is a compatibility regression, not a native pass.

Actual restored critical replay316 connects both MDI pairs and all five clocks.
Support/plane screen319 retains the masks until negotiation and all physical
prefix copper. Subsequent coarse RMII replay324 connects all six nets. Fresh
KiCad10.0.6 refill has zero selected MDI/clock/RMII or GND opens and zero geometry
errors; 456 other opens and 12 dangling-track warnings remain on the partial
export. The largest existing ground-return distance is 2.134574 mm within the
source 2.2 mm bound, with no ground-via changes during RMII routing. The screen
excludes pending-support release retry and full ordinary routing: fresh complete
pipeline validation is still required.

## Revalidate collateral escape-reservation release (2026-10-24)

Bypass preparation may release provisional package-exit keepouts in a proximity
box to retry a pending capacitor. That box can include a complete signal exit
which does not conflict with the selected bypass copper. Removing its reservation
lets a later clock or pair route occupy the exit. The flow now checks every
original track and via of an affected net against the actual immutable/bypass
prefix, including its return-via rule. It restores only complete legal groups
whose proxy masks also clear that prefix. No copper or source pose is modified.

The new temporary mask context requires newly verified bypass paths and strict
source/prefix admission. Original ledgers are not rewritten to waive a context
change. Any failed witness or admission check rolls back the whole restoration.
A functional fixture shows a clock occupying the unprotected exit, then keeping
it clear after restoration. Tests also reject a legal via with a blocked stem
and exercise new-ledger validation and rollback. The 77-case related set passes.

On actual combined preparation, 37 collateral keepouts across 18 nets can be
restored, including RXD0 and CRS_DV. RXD1's original proposal conflicts with the
bypass prefix and stays withdrawn. Fresh exact bypass admission passes without
changing its 344 tracks or 108 vias, source poses, arrays or declared rules.
Critical replay316 is pending; this is not a full-board pass.

Ladder315 retains the prior copper/completion: nRF52 35/35, zero geometry errors,
44 vias and 350.563 mm in 31.1 s; round LED remains 50/51 with DEC3 open,
41 vias and 552.542 mm in 88.4 s; the separate LED consumer passes fresh native
with zero opens or violations, 35 vias and 545.611 mm in 73.5 s. Concurrent
runtimes do not establish a performance improvement or an unchanged-consumer
runtime guarantee.

## Explicit unified search-state budget (2026-10-23)

`route-board --unified-search-states N` now sets the existing maximum expanded
states per path search. The default remains 400000. The effective value is retained
in `work_limits` and saved replay options, and non-positive values are rejected.
This computational bound changes no width, clearance, layer or via requirements.
A bounded no-path result does not establish that no legal path exists.

Actual prepared consumer controls use identical inputs, 16 negotiation iterations,
three commit rounds and 90 s: TARGET_OE_N leaves four groups open at 400000 states
but connects at 2000000 states with zero exact geometry errors. Fresh KiCad10.0.6
refill has zero opens on that net and the retained MDI/clock prefix, zero geometry
errors, 499 unrelated open items and 12 dangling-track warnings. This remains a
partial diagnostic, without full-board or electrical qualification.

The 37-case CLI/router set passes. The CLI test compares an intentionally exhausted
one-state search with a 2000000-state search and checks actual connectivity,
reported work limits and typed replay settings. Ladder304 with the larger budget
retains 35/35 nRF52 signals, zero geometry errors, 44 vias and 350.563 mm in 57.2 s.
Concurrent timing is not a whole-board performance claim.

## Reuse existing copper independently of graph-root labels (2026-10-23)

Explicit connectivity can retain a pad identity as the root of a group already
joined to tracks and vias. The unified router previously omitted those copper
anchors whenever the root began with `pad:`. It now follows actual group
membership, retains all original copper, and exposes each via only on its real
barrel span. Required verified package accesses still replace these alternative
anchors, so this does not authorize bypassing a protected exit.

A front-layer barrier fixture has two existing pad-to-via exits and no remaining
via budget. It fails before the correction and connects on the back layer after
it, with zero exact geometry errors and unchanged source tracks/vias. Both search
kernels pass; a blind-via variant cannot invent a back-layer anchor. The 42-case
router/return set and 55-case native dependency/bypass ownership set pass.

Ladder298 retains the prior copper and completion: nRF52 35/35, zero geometry
errors, 44 vias and 350.563 mm; round LED 50/51 with DEC3 still open, zero geometry
errors, 41 vias and 552.542 mm. The separate LED consumer passes native with zero
opens and violations, 35 vias and 545.611 mm. nRF52 took 79.9 s under concurrent
jobs and the unchanged control300 took 77.7 s with identical copper. Loaded
timing is inconclusive; no runtime improvement is claimed.
Actual control-net replay299 remains partial: ADC_SDO connects but TARGET_OE_N
still cannot reach four groups. This generic correction is not presented as
closure of that board fault.

# Routing performance and optimization assessment

## Net-aware via halos outside pad annuli (2026-10-23)

The via raster now distinguishes a pad's foreign-net clearance halo from the
annulus exclusion that applies to every net. Exact `can_via` already makes this
distinction. Previously the raster marked the entire expanded pad halo foreign,
discarding legal same-net transitions between opposite-face lands. Foreign
copper and drill clearances retain their original envelopes. The inner pad
exclusion uses the actual annulus radius, including when the effective foreign
drill envelope is larger. No annulus may touch pad copper; exact commits remain
mandatory and no routing rule is reduced.

Both regression tests fail before the correction. One compares raster ownership
with exact via legality at an ordinary opposite-face gap, including foreign-net
queries, forbidden annulus contact and two hole-clearance values. The other
keeps a 2.2 mm route budget: the former detour is 2.571320 mm; the correction
connects the lands within the original budget with one ordinary via and zero
exact DRC errors. The 39-case router/CLI/coupled/bypass set passes.

Actual prepared consumer replay246 connects all eight ADC input networks on
the original coarse grid (control235 connects six). No source geometry, width,
clearance, layer, via-count or length rule changes. Fresh KiCad10.0.6 refill
has no selected-network opens and zero geometry errors;499other unconnected
items and12dangling-track warnings remain because ordinary routing is excluded.
This is not complete-board, noise, assembly or CAM acceptance.

Ladder250 retains35/35signals and zero geometry errors, using44vias and
350.563mm rather than40vias/346.483mm. Its72.9s flags the old22.9s timing
baseline. Unchanged compiler control251 also flags its old baseline at74.3s
under concurrent jobs; the timing comparison is inconclusive. The extra vias
and length are retained tradeoffs, not a claim of general routing-quality or
runtime improvement.

## Bounded fine-grid diagnostics (2026-10-23)

The optional `grid_window_nm` restricts a unified replay's raster/search nodes
while retaining the original lattice phase and complete physical board.
Attempted terminals outside the requested rectangle are refused; original
pads, keepouts, source copper, holes, outline and exact commit checks remain.
Geometry entering the window from outside is still an obstacle. Default runs
keep their previous grid and behavior.

A real two-lane fixture has a 1.05 mm corridor for two 0.2 mm traces with
0.2 mm clearances. The 0.25 mm grid cannot fit both; a bounded 0.025 mm grid
connects both with zero exact DRC errors and unchanged rules. Tests also cover
whole-source preservation, entering obstacles and invalid/excluded terminals.
The 35-case router/CLI/bypass set passes. nRF52 ladder244 retains35/35signals,
zero geometry errors,40vias and346.483mm in22.9s, with no regression.

In the actual prepared eight-channel consumer diagnostic, coarse control235
connects six channels; finer windows242/245 connect seven. This diagnostic
isolates a precision limitation but does not close the bank or establish native,
noise or complete-board acceptance. It also exposed overly conservative via
pad halos for a separate correction.

## Located unified replay failures (2026-10-23)

`copper issues` now reads the existing unified subset replay summary's attempted
failure map. Previously a replay with two exact-commit rejections displayed zero
issues because it had no full-pipeline `detailed` section. Unattempted open nets
remain excluded; grouping and pad-derived windows use the existing machinery.
The 19-case iteration-tools set passes. Ladder237 retains 35/35 signals, zero
exact geometry errors, 40 vias and 346.483 mm in 19.2 s; no regression against
control233, with no routing or verification acceptance change.

## Check the deadline before each repair alternative (2026-10-23)

The unified router now checks its existing deadline before each unseeded or
seeded repair attempt. Previously the outer round alone checked time: an
unseeded attempt could use the remaining budget and still launch a full escape
seeder afterward. It retains the best completed attempt before reporting the
skipped alternative. This does not interrupt a running search, geometry check
or seeder, and is not a hard wall-time guarantee.

A real-search regression expires a controlled clock as the unseeded repair
ends. Before the fix a third negotiation/seed starts; afterward exactly two
negotiations run, the seeder is never called, the connected fixed net stays
connected and the unresolved net remains explicit. The 32-case unified,
coupled-order and bypass-flow set passes. Ladder233 retains all 35 signals,
zero exact geometry errors, 40 vias and 346.483 mm; 45.9 s versus unchanged
control228's 71.6 s flags no regression, without a speedup claim.

## Native filled-polygon parity index (2026-10-23)

The native slit-bridge decoder indexes nonhorizontal edges by their exact
half-open Y intervals before evaluating polygonized faces. It retains original
oriented edges and the original even/odd crossing arithmetic. No geometry is
healed, rounded, buffered, or cached; representation rejection and linework
checks remain unchanged. The median lower endpoint gives the interval tree a
finite, balanced construction even when coordinates repeat.

`bench/native_fill.py` measures the public decoder, using `pcbir.workbench`
to load native PCB fills. On the identical 15,005-vertex synthetic walk,
controls223 took 7.57 s before and 1.65 s after. On the saved consumer native
37,210-vertex GND/In1.Cu region, controls224/223 took 6.83 s and 3.72 s.
Input hashes, area and normalized WKB hashes match exactly in each comparison.
These are isolated decode timings under concurrent workloads, not a claim
about complete routing or native feedback duration. A one-second sample of
the earlier full consumer verification was inside the former all-edge scan;
repeated complete verification still needs its own measurement.

The 76-case parity, qualification and filled-feedback test set passes. Boundary
tests compare active edge multisets and parity at vertices and neighboring
floating-point values in millimetre and nanometre scales; a 400-hole walk
retains its exact area and every void. Native connectivity, electrical/RF/PDN,
assembly and CAM qualification remain separate.

The matching unified nRF52 ladder227 retains all 35 signal connections, zero
exact geometry errors, 40 vias and 346.483 mm. Its 93.7 s versus control214's
19.7 s triggers the timing warning under concurrent full-board jobs; no whole
route speedup or clean timing gate is claimed. The unchanged active compiler
control228 also takes 71.6 s and flags the same earlier baseline; this timing
comparison remains inconclusive. Run225 omitted the unified
override and is an extra legacy control, not a comparable unified result.

## Coupled groups before equal-priority singles (2026-10-23)

Critical job ordering preserves explicit source priority and places coupled
groups before singles at an equal priority. The old alphabetical kind ordering
put clocks first; a legal single trace could wall off the same package's paired
exit. Coupled lanes require one contiguous joint channel and cannot use the
independent single fallback. The new tie-break is forward ordering, with no
additional portal, search, state, retry or rip-up allowance.

A real crossing fixture connects both a surface-only pair and an equal-priority
clock, using the clock's permitted layer transition. Giving the clock a higher
explicit priority preserves that choice and the blocked pair remains failed.
Both cases preserve all inputs and complete geometry checks. The 62-case
routing/flow set passes. The nRF52 ladder214 retains35/35connections, zero exact
geometry errors,40vias and346.483mm, with no regression.

In full consumer controls198/205, the clock's13-segment surface path runs
between paired package lands and termination supports; both pairs fail before
main search. Replay212 of the actual prepared input, including all344bypass
tracks,108vias and ordinary reservations, connects both pairs and all five
clocks with the new order. TX uses470states and RX3070, each under6000. The
two clock branches tune to23.106233mm each with two legal bumps. Fresh
KiCad10.0.6 refill has zero geometry errors and no selected-net opens;499
unrelated whole-board open items and12dangling-track warnings remain. This
is a prepared-critical subset, not a full ordinary-routing or SI result.

## Exact bypass reservations at the critical boundary (2026-10-23)

The standard flow can hold its complete planned bypass contribution as actual
copper through the existing validated critical-prefix interface. This preserves
net identity, real track/via shapes and holes, permits legal same-net joins, and
locks the complete accepted contribution through critical routing. The previous
all-net rectangular keepouts can forbid such joins and approximate round
geometry conservatively. An unsupported or ambiguous semantic ownership claim
retains that conservative protocol with an explicit reason; no proposed piece
is filtered out to manufacture a passing claim.

Exact source fields, prefix occurrence multiplicity, original admitted return
paths, every declared association and hard geometry are checked afresh. Changed
temporary reservation context can therefore refuse reuse too; that adaptation
is not silently authorized. The existing initial pair-state option now also
bounds the standard critical path, rather than only package-access preflight.
No maze search or retry allowance is added.

Checkpoint-directory builds save first and retry critical inputs. Bypass-only
replay restores each decoded edge to a unique equal original occurrence, then
runs all normal admission checks. Missing/duplicate occurrences, changed
values, context digests and stale source remain rejected. Restoration changes
no routing value and grants no acceptance authority. Workbench views show the
reserved proposal as diagnostic geometry.

Six new real-geometry/flow tests cover a ground join rejected by the old proxy,
foreign-net collisions, complete cross-face supply/return preservation,
serialized replay, ambiguous/stale ownership refusal and the aggregate pair
bound. A 64-case flow/admission/replay set passes. The nRF52 ladder206 retains
35/35 connections, zero exact geometry errors,40vias and346.483mm, with no
regression. Full-consumer205 admits all344planned tracks and108vias as copper;
clock-first ordering still obstructs its paired exits. Removing ordinary
reservations changes an original witness context and correctly retains the
conservative fallback rather than rebinding it without authority. Prepared
replay212, using the changed coupled-first order, needs no such release and
passes selected-native verification. No improved full-board result is claimed.

## Joint separate-support completion (2026-10-23)

Pairs with exactly two shared endpoint packages and one source-bounded separate
support contact per member keep those support contacts as obligations while
forming each escape. A proposed signal collar and its required ground return
are reserved before both local ties are completed against fresh exact geometry.
Changing the return site can therefore change the temporary support topology.
Only the ordinary complete-pair gates may adopt the resulting group; failure
rolls back all generated copper and retains every pending source contact.
Macro-owned, ambiguous, remote and unsupported contact trees keep their existing
preparation. Fixed copper and required via-array multiplicity remain immutable.

Eight distinct surface and eight transition proposals share the former
600-state local allowance per member: 300 states per family/member across all
local calls, at most 1200 total. Each support samples at most eight attachment
points on actual connected collar copper within its original breakout region.
An exact repeated local proposal can reuse geometric work across pitches only
with identical non-copper source fields, tracks/vias including occurrence
multiplicity, and exact caller prefixes. This transaction-local cache holds at
most 16 outcomes; it never certifies whole-pair acceptance or reuses a clearance
index. Every final candidate receives fresh complete validation.

Transition-pair ranking also checks each endpoint's one-pitch straight
continuation against its own complete local proposal. A clear continuation is
a preference inside the existing 128 combinations and search/state allowances,
not an admission gate: a shorter goal tail may reach another legal collar.
Compact attempted-endpoint telemetry reports at most 16 orientations, expanded
states, legal advances and closest approaches, excluding private frontiers and
routing identity.

On one unchanged 389-component consumer, serial preparation produced no PHY
transitions and failed TX. Joint completion produces 23 transitions, but its
first channel ranking still fails at 6000 and 60000 states. Endpoint approach
ranking then connects all six TX contacts in 4032 of 6000 states, with one signal
transition pair and its required ground return. Independent KiCad 10.0.6 refill
has zero geometry errors and no TX opens; unrelated board opens remain. RX also
retains all six contacts. Copper-total skew is about 0.5 mm and is not an
endpoint-delay or SI qualification. No capacitor or source-rule change is needed.

The nRF52 ladder controls188/194 retain 35/35 signal connections, zero exact
geometry errors, 40 vias and 346.483 mm, with no regression. Concurrent runtimes
19.3/18.9 seconds are observations, not a speedup claim. Full-board, native plane
and engineering qualification remain separate from these selected-net results.

## Terminal-transition rejection diagnostics (2026-10-23)

Critical replay now reports finite transition-stage counters and at most 16
rejection examples per terminal/layer/pitch, with two examples per stage and
eight exact blocker identities per example. Local access distinguishes an
exhausted frontier from its separate state limit. These observations do not
change candidate order, state allowances, copper ownership or acceptance.

One consumer's baseline has three short collars: one violates signal
via-in-pad policy and two collide with opposite-face bypass lands. Its local
fallback samples 155 sites, finds 12 legal via pairs and spends 32 local path
calls without a complete collar. A retained capacitor-move trial generates
six signal collars but rejects all 2472 sampled return sites, including sites
blocked by its own disposable termination branches. This identifies serial
support/escape/return preparation as a limitation; it is not a proof that the
physical placement has no valid route. Both diagnostics export byte-identical
PCBs and routing fingerprints to their corresponding pre-telemetry controls.

The 41-case pair/replay set, 51-case local/IPC/telemetry set and 58-case
transition/return set pass (overlapping sets). The nRF52 ladder case180 retains
35/35 signal connections, zero exact geometry errors, 40 vias and 346.483 mm;
38.3 seconds versus the prior 40.1-second control, with no ladder regression.
Native/plane and engineering gates remain open; no speedup claim is made.

The reference IPC reader also accepts an explicit empty-pad separator on a
plated via record. A named manufacturer via is classified as a via, not a
component with an empty pad. Units, drill dimensions and net identities remain
explicit; unsupported records still fail closed.

## Offset collars for staggered plated rows (2026-10-23)

Round plated terminals with a source breakout contract can now propose lateral
collars derived from neighboring land edges. At most two proposals per cardinal
heading are retained, with one opportunity on each lateral side. Lead-ins and
oriented tails use the existing exact source-width and whole-board clearance
checks; main search/state limits and complete-pair acceptance are unchanged.
The change is generic, rotation/mirror independent and has no board selectors.

On one consumer's unchanged revised placement, baseline161 failed both MDI
pairs after ten states each. Candidate160 connected RX within the same 6000-state
bound (3070 states, one complete candidate); TX still failed. Reported RX total
copper skew is zero, but includes termination branches and is not endpoint-delay
qualification. Independent KiCad refill of the RX diagnostic has zero geometry
errors and no RX opens; unrelated board opens remain. Transition diagnostics
then locate TX's missing generated launches at its surface-mounted terminal,
while its plated terminal offers twelve direct opposite-face contacts.

The 122-case routing set passed before the final lateral-slot diversification.
After that change, 58 targeted cases passed; 38 cases including new directional
and transition-inventory assertions also passed (sets overlap). The early nRF52
run157 retains all 35 signal connections, zero exact geometry errors, 40 vias,
346.483 mm and byte-identical PCB SHA256
`9f05741a655c1d478e1d53beeec7d1aaef80c5e75ea8e14ef6a539680b6014d3`.
Its concurrent runtime 16.1 to 67.5 seconds triggers the ladder timing warning;
this is retained without a speed claim. A first control launch167 accidentally
used the original editable install and failed CLI parsing; it is not a routing
comparison. Explicit-PYTHONPATH control168 and candidate169 repeat the same case.
Both retain byte-identical copper and all signal connections, with 75.4 and
40.1 seconds respectively and no ladder regression. External workloads differed;
this is not a whole-board speedup claim.
Native plane and engineering gates remain incomplete in this example subset.

## Direct unified pipeline subset (2026-10-23)

| Ladder case | Flow | Signal opens / connections | Exact DRC errors | Vias | Length mm | Time s |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| round-led-ring | retained legacy baseline | 2 / 51 | 0 | 33 | 523.802 | 178.2 |
| round-led-ring | direct unified | 0 / 51 | 0 | 39 | 591.697 | 77.2 |
| nrf52 | unified with bootstrap | 0 / 35 | 0 | 40 | 346.483 | 27.9 |
| nrf52 | direct unified, older export contacts | 0 / 35 | 0 | 37 | 360.121 | 21.8 |
| nrf52 | strict unified only | 0 / 35 | 0 | 35 | 359.121 | 11.7 |
| nrf52 | standard unified with exact neck emission | 0 / 35 | 0 | 40 | 346.483 | 74.5 |

Runs: `build/bench-main-baseline109`, `build/bench-unified-direct115`,
`build/bench-unified-nrf-baseline118`, `build/bench-unified-nrf-direct119`,
`build/bench-unified-only-nrf123`, `build/bench-unified-standard131`.
Native verification was not run; route status remains failed with deferred
plane/evidence gates. The nRF52 timings were concurrent with each other and a
consumer run. This is a measured subset, not full-ladder or release acceptance. The standard
neck-emission run reproduced the standard baseline completion, DRC, via count
and track length. The ladder flags its total wall time (27.9 to 74.5 s); loads
were not controlled. Instrumented unified-stage times were 7.69 and 7.92 s,
and export finished at 22.57 and 22.29 s respectively. The larger
process-level timing difference is retained without a cause or speed claim.


## Fine-grid keepout windows (2026-10-23)

A read-only stack sample of the consumer's standard unified pipeline caught its
critical-clock prepass in `detailed._blocked_nodes`: every fine-grid point
scanned all keepouts on every layer. The new broad phase visits each keepout's
inclusive axis window and retains the exact polygon and board-material
predicates. Source keepouts, layer membership, node coordinates and the result
set are unchanged; no geometry or routing budget is relaxed.

The existing hotspot runner now accepts `--only grid`. On its synthetic
six-layer fixture (120 small reservations, irregular axis coordinates), five
alternating samples gave median 4.280 s before and 0.311 s after (13.78x).
Every blocked-node output matched exactly. This measures grid construction,
not a full-board speedup or qualification. Working results:
`build/grid-hotspot132.json`.

The 64-test detailed/reuse/grid set passed 63 tests and initially rejected a
new invalid test fixture that named a layer outside its board stackup. The
corrected fixture and both existing grid-equivalence checks then passed.
All 32 critical/pipeline tests also passed. A warm same-checkout nRF52 ladder comparison used the exact original
`detailed.py` for the baseline and restored the committed candidate for the
second run. Both completed 35/35 signal connections with zero exact DRC errors,
40 vias and 346.483 mm; the emitted KiCad PCB bytes were identical
(SHA-256 `9f05741a655c1d478e1d53beeec7d1aaef80c5e75ea8e14ef6a539680b6014d3`).
Total times were 45.4 and 44.1 s and the ladder reports no regression.
The earlier cold-copy/concurrent attempt (108.6 s) and its warning remain in
`build/bench-grid133`; the warm baseline/candidate are
`build/bench-grid-baseline135` and `build/bench-grid-candidate136`.
Native verification was not run on these ladder cases. Full-board routing and
native verification remain separate pending checks.


## Isolated hotspot experiments (2026-10-12)

The fresh standard full-vertical profile in
`build/full-vertical/runs/20261012T054251927652Z` took 1,391.918 s with
`cProfile`. It identified four targets: redundant physical-board construction,
macro via-permission geometry, repeated maze-edge work and placement legality.
The pre-change sources used by the experiments match that profile's source
hashes, including its uncommitted routing work.

The repeatable fixture runner is
[`scripts/benchmark_routing_hotspots.py`](../scripts/benchmark_routing_hotspots.py).
Snapshot the working `pcbir` sources before an intervention, retaining the
`backends/kicad_pcb.py` subdirectory, then run:

```powershell
.venv/Scripts/python.exe -m scripts.benchmark_routing_hotspots `
  --baseline-dir build/performance-research-20261012/baseline `
  --output build/performance-research-20261012/isolated-results.json
```

`--only macro|construction|placement|maze` isolates a research area. The runner
loads selected baseline functions against the current unchanged helpers/types,
so each comparison isolates that intervention. It checks exact return values,
alternates method order, warms up once, and records five unprofiled samples,
medians, Python/platform information and source hashes. It also checks 64 maze
equivalence cases with different grids, congestion, guide-layer projection,
movable obstacles, forbidden vias and budget exhaustion. This is a developer
diagnostic, not a timing threshold or manufacturing qualification.

| Target | Methods tried | Selected method and correctness boundary |
| --- | --- | --- |
| Board construction | Whole-board replacement with linear validation; replacement with indexed validation; explicit rendering/track context | Pass the complete committed track prefix into chamfering and a UUID namespace into footprint rendering. No physical board is constructed just to change that context. Real board construction still validates its existing reference and macro invariants. |
| Macro permissions | Linear scan; linear scan with an LRU pad-shape cache; indexed land snapshot, cold and warm | An eight-entry memo retains exact immutable placement/footprint identities and indexes their SMD shapes. Every via still checks current process, binding, permission, exact contact, containment and multiplicity. |
| Maze | Existing resource/guide caches; congestion-only cache; symmetric edge-cost cache; edge costs plus integer keys | Cache only search-local symmetric costs and use grid-derived integer keys for best costs and edges. Arrival-direction bends and directional via guide transitions remain separate. Queue ordering, nodes, traceback and state budgets stay unchanged. |
| Placement | Rebuilt shapes; cached macro copper; cached local land/copper envelopes; those envelopes plus exact posed courtyards | Reject distant candidates using conservative aggregate bounds, then retain the original exact net/layer/copper/drill checks. Geometry caches retain immutable inputs, with 256-entry land/copper and 8,192-entry courtyard limits. Current rules and net ownership are read for every check. |

On this Windows/Python 3.13.1 run, representative median fixture batches were:

| Fixture batch | Baseline | Selected method |
| --- | ---: | ---: |
| 12 warm permission checks, 1,538 lands / two vias | 253.0 ms | 0.304 ms |
| One cold permission check | 20.75 ms | 12.60 ms |
| 30 surface paths with macro-bearing board | 640.0 ms | 0.650 ms |
| 30 library renders, 16-pad footprint | 657.3 ms | 7.386 ms |
| 240 macro-copper placement checks | 159.7 ms | 4.489 ms |
| 240 complete placement legality checks | 40.50 ms | 21.13 ms |
| Orthogonal maze with congestion | 177.7 ms | 143.9 ms |
| Octilinear maze with congestion | 624.2 ms | 413.4 ms |

All compared outputs matched. The large construction and warm-index ratios are
specific to deliberately dense, repeated-validation fixtures; they are not
whole-board speedup estimates. Congestion-only caching barely helped, so the
larger symmetric cost cache was selected. Retained JSON includes every method,
individual sample and exact-output digest. Bounded invalidation, rotated/odd-nm
geometry, permissions, drills, branches and renderer namespace regressions are
covered by the routing/cache/backend tests.
The cold-index case clears the land snapshot before each call; both methods
retain the existing coordinate-transform caches. It is not a cold-process run.

### Matched full-vertical profile

The optimized standard run is
`build/full-vertical/runs/20261012T071852682556Z`. Both runs used Python 3.13.1,
KiCad 10.0.6, the same locked/offline inputs, six-layer fabrication profile,
both immutable macros, `candidate-00`, 20,000 search states, two detailed passes
and the standard bounded pipeline. Router commands and all recorded input hashes
match. Source hashes did not change during either run.

| Observed cProfile run / phase | Before | After |
| --- | ---: | ---: |
| Full build elapsed | 1,391.918 s | 724.974 s |
| Placement/global planning | 312.686 s | 166.158 s |
| Initial package plane contacts | 129.998 s | 1.525 s |
| Detailed pass 1 | 308.930 s | 167.251 s |
| Detailed pass 2 | 316.699 s | 220.029 s |
| Ordinary area phase | 643.494 s | 398.753 s |
| Export phase | 5.761 s | 0.638 s |
| Independent KiCad phase | 33.543 s | 40.886 s |

The observed full profiled run used **47.9% less elapsed time (1.92x ratio)**.
This compares two instrumented runs, not repeated unprofiled whole-board runs.
Profiler overhead and external machine load affect the ratio. Function/phase
times below overlap and must not be summed.

| Hotspot, cumulative cProfile time | Before | After | Work/output check |
| --- | ---: | ---: | --- |
| Physical-board reference validation | 280.788 s | 11.67 s | Constructors reduced from 1,863 to 192; real constructors still validate. |
| Macro via permissions | 134.271 s | 0.18 s | 324 fresh permission calls; three cold land snapshots. |
| Surface paths | 176.955 s | 1.36 s | Same 1,356 calls. |
| Canonical footprint rendering | 82.437 s | 0.51 s | Same 476 renders. |
| Placement legality | 167.737 s | 62.75 s | Same 213,465 probes. |
| Placement versus macro copper | 64.970 s | 1.68 s | Same 58,653 checks. |
| Maze search | 603.149 s | 368.56 s | Same 393 searches and 1,170,524 neighbor-generation calls. |
| All plane stitching | 179.091 s | 2.86 s | Same two calls. |

Total profiled calls fell from 2.265 billion to 1.282 billion. The initial
critical tier still expanded 27 states; package exits remained 76 accepted / 0
pending. Both runs made 100 detailed net attempts, with the same two first-pass
failures and all 50 ordinary nets connected after the second pass. No additional
repair searches were needed.

The draft board SHA-256 and editor-intent SHA-256 match exactly. Saved native
boards differ only in **110 UUIDs for KiCad-generated empty Datasheet/Description
properties**; every other line, including routed and filled copper, is identical.
Both have 680 tracks, 223 vias, 1,981.243638 mm of track, and fresh native DRC with
zero violations / zero unconnected items. Native fill resolves the same three
deferred explicit-copper observations. The same eight engineering requirements
remain blocked; fabrication readiness is false.

Regression coverage: 2,673 passed / 18 skipped in the initial full-suite run,
plus 13 environment failures. All affected tests passed in a 107-test rerun
with localhost access and a temporary directory outside the repository. That
rerun also covers one regression added after initial collection, giving 2,687
distinct passing tests / 18 skipped / no unresolved failures. Localhost was
sandbox-blocked, and a repository-local temp directory changed the module root
seen by the import-escape test. Optional skipped capabilities remain unverified.

The optimized router process measured 110.6 MiB peak Windows working set and
457.1 MiB peak pagefile usage, including cProfile and excluding child KiCad
processes. A small exit hook captured these counters; its hash and scope are
retained in `peak-memory.json`. The baseline has no peak-memory measurement.

Retained research evidence is in `build/performance-research-20261012/`:
`isolated-results.json`, baseline source snapshots/hashes,
`full-profile-comparison.json`, `test-results.json` and the original test logs.
Each full run retains raw `routing.prof`, summaries, phase events, routing/native
reports and provenance. The remaining dominant consumer is maze search:
neighbor generation and grid/material queries account for much of its 368.6 s;
physical DRC is now 97.0 s and placement legality 62.8 s, with overlap.

Exact profiled sources are retained in
`build/performance-research-20261012/profiled-sources/`, verified against every
launch-time source hash. Subsequent shared-checkout edits to KiCad clearance-rule
export arrived after the profile finished and are preserved. These timing,
regression and native-verification results apply to the retained profiled
revision; they do not verify those later export changes. See
`post-profile-edits.json` for the recorded boundary.

## Branch comparison runs

`.github/workflows/routing-comparison.yml` runs on pushes to `claude/plan-*`
branches (and on manual dispatch). It routes full-vertical twice on matched
GitHub runners, unprofiled and with the release route job's settings: once at
the commit named by `baseline_ref` in `.github/routing-comparison.json` and
once at the pushed commit. Each run directory is kept as an artifact; the
compare job writes `python -m pcbir.routing_benchmark table` to the run
summary and keeps `summarize`/`compare` JSON. A plan step sets
`baseline_ref` to the commit it builds on.

Expanded states, outcomes and copper are deterministic and compare exactly.
Wall times on shared runners vary between jobs, so a timing claim needs the
same direction on repeated runs. Results are measurements, never signoff or
an automatic acceptance gate. The 2026-10-12 profile above used KiCad 10.0.6
on Windows; branch comparisons use KiCad 10.0.7 on Ubuntu and are not
comparable with it.

### Phase 1 baseline (2026-10-20)

Run 39459011927 routed full-vertical at 15f94b4 (KiCad 10.0.7 pin plus the
terminal-capacity fix) and at the search-telemetry commit on matched Ubuntu
runners. Global, critical and detailed fingerprints were identical, so the
telemetry does not change copper. Both runs: global success, package access
ready, 0 failed signal nets, 2 zone nets awaiting fill, 366 tracks, 46 vias,
1,572.318 mm of track, 0 KiCad unconnected items and 4 other independent
findings (2 silkscreen clipped by mask, 1 silkscreen clearance, 1 dangling
track).

| Area search work (telemetry run) | Value |
| --- | ---: |
| Executed `detailed_net` attempts | 113 |
| A* calls | 453 |
| Calls that found a path | 132 |
| Calls with no path | 245 |
| Calls out of state budget | 76 |
| States expanded | 1,412,850 |
| States expanded by calls that found nothing | 943,086 (67 %) |
| Search seconds | 135.7 |

Elapsed times were 370.3 s and 267.9 s for identical copper, so runner
variance alone exceeds 30 %. Phase 1 changes are judged on expanded states
and on routed outcome; timing claims need repeated matched runs.

## Package-access via search

The preferred plane-contact search stops considering farther via rings once
their minimum possible pad-to-via distance exceeds the best legal choice already
found. The existing 2 mm search cap and exact copper clearance checks remain.
This preserves candidate selection because a route cannot be shorter than its
pad-to-via displacement.

On the full-vertical `candidate-00` package-access preflight with both hard
macros, the same inputs and unprofiled process took 248.3 s with the previous
search and 121.7 s with this pruning. The three plane-stitch passes changed from
67.5/69.2/69.9 s to 21.4/21.3/33.8 s. Both results were ready with 113 plane
contacts, the same critical-route fingerprint, and the same native board and
report digests. These are single-run measurements, not a full-route timing or
manufacturing signoff claim. Ignored diagnostic reports are in
`build/package-access-timing-{unpruned,pruned}-20261009.json`.

## Profiling future runs

Package preflight now negotiates ordinary escape patterns after a bounded
initial pair-search tier (default 6,000 aggregate expanded states per pair/pass),
before committing to historical full-budget searches. If incomplete, it retries
the original pattern at full budget. `--package-initial-pair-states 0` provides
the comparison/disable mode; `--package-pattern-trials 0` also disables staging.
Pass these through the generic build wrapper after `--`. Reports include
`package_access.search_tiers` with all actual probe/revalidation work and the
selected tier. A limited failure is not evidence that no legal route exists.
The read-only run summary includes `critical_expanded_states` across completed
groups, including discarded pattern/probe work, plus tier/fallback details. Do
not add tier totals to the group totals again. The comparison helper allows the
staging knob as an explicit intervention, while preserving other budget,
quality, provenance and uninstrumented-timing checks.
See [the plan and acceptance criteria](routing-performance.md).

The complete workflow now enables standard-library `cProfile` function profiling
and streamed phase events by default; no additional dependency is required:

```powershell
make EXAMPLE=full-vertical route
```

Each fresh ignored `build/full-vertical/runs/<UTC-run-id>/` contains:

- `routing.prof`: complete Python call statistics, including callers/callees.
- `profile-summary.txt` and `profile-summary.json`: top 50 functions by self time
  and cumulative time, plus call counts.
- `phase-timings.json`: paired start/finish events, inclusive elapsed spans,
  trial decisions, malformed-event count and unfinished phases.
- `routing.log`: the original events and diagnostics.
- `run.json`: actual launch and routing commands, Python/platform/CPU information,
  Git commit and tracked-diff hash, input hashes, profile status and router exit code.

Direct `python -m copperscript route-board ...` remains unprofiled. To profile
another module invocation, preserving failure exit codes:

```powershell
New-Item -ItemType Directory -Force build/profile | Out-Null
uv run --no-sync python -m pcbir.profiling --output build/profile/routing.prof `
  --module copperscript -- route-board examples/valid_board/board.copper `
  --allow-proxy-footprints --report build/profile/route-report.json `
  -o build/profile/board.kicad_pcb
```

The proxy-footprint invocation above is only a small diagnostic example, not the
full-board or manufacturing workflow. Existing profile files can be inspected
with `uv run --no-sync python -m pstats <path-to-routing.prof>`.

Profiling is observational, not signoff evidence. The router's exit 1 remains
exit 1, even if profile collection succeeds. This wrapper intentionally does not
use `python -m cProfile`: its utility catches `SystemExit`. A forcibly terminated
process may leave no profile; the manifest reports `missing`, not success.
Normal exceptions and in-process interrupts attempt to save partial statistics;
they do not establish a completed routing result. Profiling/report failures are
recorded separately and cannot replace the router's exit status.

Function profiling adds overhead, potentially substantial in a function-call-heavy
maze search. Cumulative times include callees and must not be summed; phase spans
also overlap when trials contain whole pipelines. The profile covers the routing
Python process, not KiCad's internal CPU work; subprocess wait time and the
independent-KiCad phase are visible, but native KiCad internals are not attributed.
See the [Python profiling documentation](https://docs.python.org/3.13/library/profile.html).

For uninstrumented wall-time comparisons, keep progress events but disable
function instrumentation:

```powershell
make EXAMPLE=full-vertical route PROFILE=none
```

Inspect a run (including an unfinished run) without modifying its artifacts:

```powershell
uv run --no-sync python -m pcbir.routing_benchmark summarize "build/full-vertical/runs/<run-id>"
```

For the O1b comparison, first complete the function-profile run, then run these
uninstrumented variants sequentially on the same otherwise idle machine. Repeat
in alternating order before drawing a wall-time conclusion:

```powershell
make EXAMPLE=full-vertical route PROFILE=none EXTRA_ROUTE_ARGS=--no-incremental-placement-repair
make EXAMPLE=full-vertical route PROFILE=none
uv run --no-sync python -m pcbir.routing_benchmark compare "build/full-vertical/runs/<baseline-id>" "build/full-vertical/runs/<candidate-id>"
```

`routing_benchmark` checks named failed signal/critical nets, pending ground,
duplicate-land and fanout pads, native hard finding categories/affected nets,
required native-check coverage, overflow and independent KiCad findings. It
reports selected-route length/via/layer metrics rather than trading them away
implicitly. A comparison with new failures, missing final data, changed recorded
inputs/tools/settings or either run function-profiled produces **no timing ratio**.
Exit 0 means eligible for timing review, 1 means a quality regression, and 2 means
incomparable/incomplete data or an input error; none means manufacturing signoff.
The standalone `summarize` command returns 0 for valid observations, even if a run
is still active, and clearly marks its completion state.

The two explicit repair switches (`--no-incremental-placement-repair` and
`--zone-dependency-expansions`) are interventions under test and may differ. Other
settings, including search budgets, must match. Runtime provenance does not hash
all footprint files or capture machine load, so an eligible ratio still requires
external-library verification and repeated measurements. This is a review aid,
not automatic optimization acceptance. Structured progress provides completed
full/incremental trial and subset/probe counts, including rejected work; a legacy
plain log uses a clearly labelled final-report lower bound instead of inventing
zero work. General-area expanded maze states and peak memory are not yet recorded
by this tool; paired critical searches do report their expanded states.

### Placement and global progress

Normal `route-board --progress` forwards the existing placement seed stages
through `on_progress`, using zero-based seed indices and the configured
seed count. A bounded legalization failure is reported as a failed seed stage;
the final `placement_candidates` event reports the total, not another seed.
The existing refinement stage includes decoupling work and does not expose its
internal steps.

Global routing emits sparse graph/access setup and iteration start/finish
events, inside candidate, feedback-trial and final-certification boundaries.
Iterations report existing attempt overflow/unrouted counts and best-score
improvement; final route status describes global guides. These operational
events are not exact-copper closure or engineering qualification. They add no
heartbeat, inner-search events, solver work or timing code. Interrupted work
has no invented finish event. With no callback, results, ordering and budgets
are unchanged; existing raw placement/editor callbacks remain compatible.

### Per-net area progress

With `--progress`, detailed routing emits `detailed_pass` and `detailed_net`
start/finish events. A net starts **before grid construction**, so an unfinished
event identifies the active net even if grid preparation or a repair is slow.
Pass events include ordinary-net counts and provisional failed-net names. Net
events include pass/ordinal (where applicable), pad count, requested pitch and
search budget, layer/direction preferences, and final tentative connectivity,
track/via counts and diagnostics. These events do not change the IR, selected
copper, fingerprints, JSON reports or exports.

Repair stages are `failed_first`, `soft_merge`, `soft_ripup`, `evicted_net`
(with its repair owner), and `final_retry`; a normal pass uses `pass`.
`detailed_repair` and `detailed_neutral_fallback` events state whether their
candidate was selected. The same observer is forwarded through placement trials
and local zone dependency reroutes. There are no per-expanded-state callbacks.
The requested budget is not an aggregate net budget: branch searches, guide
expansions and bounded pitch refinements can perform several searches.

Every finished net event also carries a process-local `search_identity`.
Within one repair stage, a search repeated with identical inputs is answered
from a bounded memo (`search_reuse: "hit"`, with the original `reused_search_seconds`); a
`detailed_search_reuse` summary follows `detailed_repair`. The attempt is the
identical earlier result, so copper, reports and fingerprints do not change.
`--search-reuse-entries 0` disables it for comparison runs; see
[exact repair-search reuse](routing-performance.md).

Follow a running build in PowerShell:

```powershell
Get-Content "build/full-vertical/runs/<run-id>/routing.log" -Tail 20 -Wait
```

The read-only summary reports `detailed_net_attempts`,
`failed_detailed_attempts`, `detailed_attempt_stages` and
`reused_detailed_attempts`, including tentative and rejected work. Counts are
**not** unique accepted nets or DRC closure.
Older logs without completed per-net events have unknown (`null`) attempt
counts. Interrupted attempts remain in `work.unfinished`, without an invented
duration or success; a forcibly killed Windows process may also leave the
original manifest unfinalized. Check process liveness as well as its status.
Inclusive phase durations overlap and are not percentages or an ETA.

Compare repeated runs with identical source/lock, actual footprint files, options,
KiCad/Python versions and machine load. The recorded hashes do not fingerprint
every footprint or external tool; record CopperLib revision and KiCad version
alongside a benchmark. Compare connectivity, exact geometry/fingerprints, DRC,
pending escapes, wire length and via counts as well as time. Report wall time and
peak memory; distinguish cold and warm runs. Do not infer speedups by comparing
instrumented and uninstrumented times. An out-of-process sampler such as
[py-spy](https://github.com/benfred/py-spy) is a useful lower-overhead cross-check,
but is not a required dependency or enabled by the shared build workflow.

## Where parallelism is unsafe or unlikely to help first

Per-net threads are not a straightforward speedup: ordinary searches share mutable
usage, congestion history, rip-up decisions and committed copper. Independent
searches on the same snapshot can produce mutually conflicting routes, requiring
conflict detection, deterministic reconciliation and rerouting. Spatial partitions
likewise require boundary/halo ownership and cross-partition repair. Treat these as
algorithm changes, not a `ThreadPoolExecutor` wrapper.

The usual GIL-enabled CPython build does not run CPU-bound Python bytecode in
parallel through ordinary threads. Processes can bypass that limitation, but need
picklable inputs/results and an importable entry point; Windows spawn costs and
memory copies matter. See [Python's process-executor documentation](https://docs.python.org/3.13/library/concurrent.futures.html#processpoolexecutor).
Parallel rendering/logging or independent CAM checks may improve convenience, but
the historical few seconds of KiCad verification do not justify making them the
first optimization target. No parallel router or relaxed correctness gates have
been enabled by this profiling change.



## Exact search reuse and acceptance

Repair-stage memoization is bounded by `--search-reuse-entries` (default 128;
0 disables). Reuse requires identical board/obstacle geometry, keepouts, rules,
guides, anchors, congestion, options and ordered copper state. Any blocking
geometry change needs a fresh search, including previously failed searches;
budget exhaustion is not a reusable proof of impossibility on another board.
Separate board objects conservatively receive separate caches. Grid material
queries may reuse immutable board/axes data but keep fresh mutable query state.

Incremental placement repairs invalidate incident nets, moved-pad/keepout/contact
collisions and derived guide/access evidence. Unsupported ownership or excessive
dependency cones fall back to a full pipeline. No failed transaction publishes
tentative copper. Static geometry caches need complete snapshot/invalidation keys.

Compare connectivity, exact DRC, critical/package constraints, deterministic
results and final-copper quality alongside uninstrumented time and work counts.
Profiling alone must not change artifact bytes. Routing changes may select
different legal geometry; fewer states or a shorter runtime cannot excuse
newly failed routes. Open work is in [optimization backlog](routing-optimization-todo.md).


## Global capacity-graph static calculations

Each global graph owns an immutable snapshot of its axes, legal nodes,
resources/capacities, terminal alias edges, via sites and layer preferences.
Construction copies caller-owned maps. Terminal registration and via-capacity
refinement create replacement snapshots; all derived data starts empty on the
replacement. Private derived fields do not change the constructor, equality,
representation, route reports or fingerprints. The graph is internal and is
not a serialized public routing artifact.

Cartesian Points are constructed once per graph, with the original tuple-index
semantics. Lazy outgoing-step entries retain exact original neighbor order,
resource identity, direction and planar/via plus layer/direction preference
cost. Their key includes the node and all three static cost options. At most
`len(legal_nodes)` entries are retained across every cost variant; when full,
new variants compute normally. Point storage is bounded by the finite Cartesian
axes, and step storage by that entry cap and the graph's finite degree.

Every search freshly filters allowed layers and blocked resources and reads
current demand, usage, congestion history, present/history penalties, bends and
remaining via allowance. Heap keys, deterministic serials, comparisons, state
space, ordering and iteration/search limits are unchanged. No path, failure,
mutable congestion answer or acceptance decision is memoized.

The differential suite retains the original A* function and checks exact paths,
heap events, complete route JSON/fingerprints, changed congestion/cost options,
layer/via constraints, terminal aliases and refined/replaced graphs. Separate
synthetic uninstrumented timings can measure recomputation savings; they do
not establish a source-board speedup, electrical/routing closure or native
qualification. Cache container sizes are not process peak-memory measurements.


## Immutable pad and breakout-owner templates

Clearance indexes share only immutable original pad geometry and source-land
membership. An eight-entry template cache retains strong identity keys for all
placements, named footprints, nets, stack, board rules, routing rules and bin
size. A separate 512-entry cache retains transformed footprint geometry. New
source epochs rebuild template ownership, including effects of a moved other
land. The pure nearest-region predicate stops at the first exact zero squared
distance, preserving the original nearest-distance and first-tie result.

Each index owns fresh object, hole and bin containers. Current breakout query
caches, zones, keepouts, macro reservations, NPTHs, tracks, vias and mutable
additions remain local and are checked under current policies. Neither a
clearance answer nor an accepted routing witness is cached. Original order,
unconnected lands and plated drill geometry remain intact.

The constructor-only comparison used actual 159-component geometry with 615
copper pads, four copper-only snapshots per batch and five alternating pairs.
It compared the original constructor AND original breakout implementation with
the candidate, checking exact index state, pad owners and clearance queries.
The measured medians were 1.005159 s and 0.034161 s. This repeated-source fixture
measures construction reuse; concurrent load was recorded, and it is not a
whole-board speedup or a routing, native, RF, power or manufacturing result.


## Ordinary breakout capacity

Global search, target selection, negotiated resource use and final overflow
metrics use the same source-declared ordinary breakout width for an edge whose
entire centreline is inside one shared terminal-land breakout region. An edge
crossing that boundary retains its normal full-width demand. A narrower width
on another net, or separate regions containing only the two endpoints, does
not grant a reduced demand. Declared clearance remains unchanged; specialized
profiles keep their existing capacity policy. Distinct physical via-site
multiplicity is unaffected.

This avoids a false capacity overflow or needless detour when a source already
allows an ordinary neck-down beside a fine-pitch package. It does not emit a
trace, relax any source width, bypass detailed/native checks, or certify a
whole-board route. Only immutable source-region membership is cached within
the net's demand evaluator, with a 16,384-edge bound; mutable usage and history
are read on every search step. Nets without an effective width reduction keep
the existing fast path. Regressions include renamed fixtures, boundary-crossing
edges, declared clearance and unchanged physical-copper rules.


## Lazy breakout measurements and bypass baseline DRC

Copper-spacing checks first test the normal, unrelaxed spacing before
measuring a rational distance for a breakout-relaxation record. Pairs that
already meet normal spacing cannot produce such a record. Near pairs retain
the same exact predicates, measured values, ordering and report fields,
including half-nanometre width thresholds.

Cross-layer bypass proposals defer their immutable input's full DRC until a
complete supply/return witness passes exact geometry checks. An unsuccessful
search adopts no copper and needs no baseline comparison. Every potentially
accepted proposal still compares fresh full candidate DRC with fresh full
input DRC; no report cache or stale-board evidence is introduced. The
baseline is evaluated at most once inside that immutable transaction.

On an 892-track/79-via CopperProbe diagnostic snapshot, the complete DRC JSON
was byte-identical (77 findings and 347 breakout records). One paired timing
measured 5.36 s before and 3.35 s after the spacing optimization. This is a
snapshot measurement, not a whole-route speed guarantee. Existing running
jobs remain pinned to their original compiler.


## Compact fresh bypass launches

Fresh bypass launches now compare both existing bounded geometric sampling
tiers before spending the finite launch slots. Their score is new launch
Manhattan length plus remaining distance to the destination land, followed
by new launch length and deterministic coordinates. Existing contained
source tracks add no new length to that score. This is a local ranking
heuristic, not a shortest-path or electrical loop-quality proof.

The ranked list is interleaved with the original outward candidate inventory,
deduplicating exact choices before spending a launch slot. Compact choices keep
the first opportunity, while outward alternatives remain represented when
shorter launches cannot reach the destination surface. Neither ordering receives
an extra launch allowance.

Exact verified existing launches still consume their original slots first.
Each radial/two-leg sampling pass retains the site cap, and launch,
complete-proposal, shared local-path and inner-bridge allowances are unchanged.
The fine tier is considered even when coarse rays already fill the slots;
this may do more geometric sampling within the established maximum bounds.
All complete-path, source-prefix, whole-net budget and fresh DRC gates remain.


### Breakout membership across copper-only trial indexes

Breakout-region membership now shares a bounded memo across clearance indexes
with the same complete immutable source epoch: placement tuple, exact footprint
objects, net assignments, board rules, routing rules and stackup. The cache keeps
four source epochs with at most 8192 queries each. Strong identity keys prevent
identity reuse. A changed source field starts a new epoch; added/removed copper
still receives fresh spatial indexing and exact clearance checks. No board,
generated copper, fill, clearance result or routing acceptance is cached.

On one 414-track/69-via consumer control, eight repeated index builds and region
queries took 1.65 s before and 0.15 s after. The membership result digest and
complete physical DRC report were identical. This isolates repeated source
membership work, not complete-route performance or any engineering acceptance.


### Exact decoupling path-point lookup

The path graph narrows point candidates by inclusive X/Y bounds, selecting the
smaller sorted interval. The original integer collinearity test, distance order,
track/via occurrences and BFS state bound remain authoritative. The index is
local to one graph construction; it caches no witness or clearance decision.

`bench/decoupling_paths.py` loads saved inputs through `pcbir.workbench` and calls
the public verifier. Before259/after261 on eight opposite-surface associations
retain identical input and path hashes: 0.4969 s versus 0.2367 s. This is a
prepared-prefix measurement, not a whole-board runtime claim. A 55-case related
set passes, including randomized inclusive/reversed/diagonal lookup parity and
an 80-versus-6400 exact-candidate count. The initial test used an illegal zero-
length track and was corrected; track validation was not relaxed.

The nRF52 ladder263 retains 35/35 signals, zero exact geometry errors, 44 vias and
350.563 mm in 20.7 s. Its loaded baseline257 took70.9s with identical copper;
this comparison does not establish a whole-board speedup.


### Single-ended existing-return reuse

The source's existing return-via fields now constrain ordinary launch candidates,
maze proposals and final reservations, plus the unified Python/native via mask
and exact commit. The shared checker uses integer squared distance and actual
barrel spans. Independent DRC reports `DRC-RETURN-VIA` and coverage; it does not
prove native filled-ground continuity or SI. No return copper is invented and
all immutable copper is preserved. This is existing-via reuse, not a return-via
generator or a shared-reference waiver.

The regression fixture previously crossed without any permitted return in both
kernels; the new mask finds a source-covered transition or leaves it unresolved.
Exact-commit fault injection, distance boundaries, span mismatches, early launch
paths and a real two-face native refill fixture are covered. The related68-case
set passes. `replay-unified --save-result FILE` retains a typed result for further
workbench inspection and refuses to overwrite an existing result; it carries no
source/native admission authority.

Ladder281 retains35/35 signals, zero geometry errors,44 vias and350.563mm, with a
55.9s timing warning against an old20.7s baseline. Unchanged control283 takes35.9s;
final candidate285 takes30.7s with identical copper and no new warning. Concurrent
board verification makes these timings unsuitable as a whole-board speed claim.
