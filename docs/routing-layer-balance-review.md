# Ordinary layer balance and route smoothing — 2026-10-06

## Question

The [clean full-vertical rerun](routing-shared-reference-review.md) left In2.Cu
with 1,347 mm of track and 94 sharp-turn candidates while In3.Cu, In4.Cu and
B.Cu carried 52, 44 and 102 mm. This increment asks why, and adds ownership-safe
mechanisms for layer choice and removable bends without weakening connectivity,
clearance, reference-plane reservation or explicit layer rules.

## Diagnosis

Four mechanisms reinforce one another; none is a single cost typo.

1. **Static layer ranks.** `signal_layer_preferences` ranks In2.Cu (next to the
   In1.Cu GND plane) 0, F.Cu 1 and In3.Cu/In4.Cu/B.Cu 2. Global search adds
   2 cost units/mm per rank to a 10 units/mm baseline; detailed search adds 4.
   A long run on In3.Cu therefore costs 40% (global) or 80% (detailed) more
   than on In2.Cu, while vias cost the same: every signal via is a through via.
2. **No negotiation pressure.** Full-vertical global routing succeeds in its
   first iteration with zero overflow (a 5 mm tile edge has about 17 physical
   lanes at 0.29 mm pitch, against at most five on the 1 mm detailed grid). Its
   ordinary guides used only F.Cu (365 mm) and In2.Cu (1,430 mm); In3.Cu,
   In4.Cu and B.Cu had none. Global search also has no notion of two guides
   crossing on one layer, which detailed copper can only resolve with a via
   pair or a detour.
3. **Layer-locked corridors.** Detailed corridor search admits only guide
   nodes on the guide's own layer, so copper follows guide layers. Fallback
   searches project the corridor onto every layer and then choose by rank, so
   they too return to In2.Cu. A pass-one replay on the saved pre-ordinary state
   found 113 corridor, 48 projected and 10 full-board successes; 103 searches
   exhausted their budget and consumed 124.5 s of 178 s of search time.
4. **In2.Cu terminals.** Package boundary access (CS-153) sorts witness layers
   by the same rank, so all 104 ordinary package exits of full-vertical (and all
   21 of nrf52) are fixed In2.Cu ports. A route that wants another layer needs
   an extra transition beside each port.

The remaining sharp turns are mostly short-leg geometry: of In2.Cu's 65
90-degree candidates, 46 have a leg under 0.5 mm and 17 under 1 mm, typically
pad-inserted grid jogs, same-net hairpins and port lead-ins; 23 more are
180-degree overlaps, mostly with reserved witness paths. Per-attempt chamfering
(`route_style`) needs both legs to be at least four track widths long, and it
cannot use space freed after rip-up and repair.

## Changes

Shipped default: route smoothing. Layer assignment, local demand and corridor
escape are implemented and tested but **opt-in**; the full-vertical
confirmation below shows why.

- **Crossing- and demand-aware ordinary layer assignment** (CS-161,
  `pcbir/layer_assignment.py`). After negotiation, each ordinary net's
  via-bounded planar run may be relabelled to another permitted signal layer
  when that lowers its cost: the unchanged rank/heading preference, plus one
  via pair (2 × global via cost) for each forced same-layer crossing and an
  optional local-demand cost per millimetre of a full edge, counted in detailed
  lanes. Tile paths, via positions, via counts, access pads and the global
  quality vector are unchanged, so placement scoring is unaffected. Planes,
  explicit `allowed_layers`, critical/paired/power guides, non-via pin accesses
  and inner-to-inner transitions are never moved; same-net runs never merge and
  no edge exceeds capacity. Opt-in: `--layer-assignment-passes 3` and
  `--local-demand-cost 20` (lanes counted at `--pitch-mm`); default off.
- **Corridor escape beside fixed-layer terminals** (`--guide-escape-mm 1`,
  opt-in, default off). Within that radius of a start or target node, the detailed
  corridor admits the projected corridor on any permitted layer, so a reserved
  In2.Cu port can transfer to its guide layer. Guide-deviation costs and exact
  track/via checks are unchanged.
- **Ownership-safe route smoothing** (CS-162, `pcbir/route_smoothing.py`).
  After owned-stub pruning, each owned chain between anchors (vias, branches,
  width changes, immutable copper, interior contacts) is greedily replaced, longest
  span first, by one straight or one 45-degree-plus-straight connection. A
  replacement must never be longer or sharper and must be shorter or less sharp
  in total (turns scored per 45 degrees beyond 45). Every new segment passes
  exact clearance, keepout and outline checks; every pad and via-layer contact
  of the retired copper must remain. A net whose explicit-copper islands or via
  contacts change keeps its original copper. Vias, critical, reserved boundary,
  immutable and zone-net copper are never inputs. Library default off; the
  board router enables it unless `--no-route-smoothing` is given.

Measured and **not** kept: ordering boundary ports by the guide layer and the
direction toward the guide's next tile. On nrf52 every port still selected
In2.Cu, the redirected edges added two vias and lengthened routes, so the change
was reverted. An in-search local-demand/crossing cost was also rejected: it
moved runs onto F.Cu and added vias instead of using spare inner layers.

## Method

Unprofiled `make EXAMPLE=… route` runs with `KICAD_FOOTPRINTS=/usr/share/kicad/footprints`
and `BUILD_ARGS="--profile none"`, each from a frozen copy of the sources so
concurrent edits could not leak into a run. Layer metrics follow
`scripts/review_routing_layers.py` definitions (degree-two endpoint turns of
90 degrees or more, 10 mm track-area density). On this Linux machine
`pcbnew.LoadBoard` from KiCad 10.0.6 segfaults even for KiCad's own templates,
so an equivalent s-expression reader computed the same track metrics; it
reproduces the published baseline table exactly. KiCad 10.0.6 refill and
all-severity DRC ran through `kicad-cli`. Single-mechanism variants replay only
the ordinary detailed stage from the base pipeline's saved pre-ordinary state;
the replayed base reproduces the baseline In2.Cu copper exactly.

nrf52 stops after routing with an unrelated export error ("filled-capped
via-in-pad requires the JLCPCB six-layer profile"), identical on the merged
base, so its saved board was checked with `kicad-cli pcb drc --refill-zones
--severity-all` on a scratch copy.

## nrf52 (iteration board)

Lengths in mm / sharp-turn candidates. "Vias" counts all saved vias (detailed
ordinary vias in parentheses). Times were measured with other jobs running and
vary by about ±10 s.

| Run | F.Cu | In2.Cu | In3.Cu | B.Cu | Total | Sharp | Vias | Time | KiCad DRC |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Merged base | 146.971 / 9 | 218.220 / 21 | 12.152 / 1 | 0 | 377.343 | 31 | 52 (18) | 127.9 s | 0 violations, 0 unconnected |
| Default (smoothing) | 146.115 / 5 | 216.155 / 18 | 11.075 / 0 | 0 | 373.345 | 23 | 52 (18) | 100.3 s | 0, 0 |
| Opt-in: all | 170.947 / 8 | 155.695 / 12 | 0 | 36.304 / 1 | 362.946 | 21 | 52 (18) | 85.9 s | 0, 0 |

In4.Cu is unused in all three. All 19 nets route in every run, and repeated
runs of each configuration (including the coordinator's separate base run)
produced identical saved geometry. The default changes no search, so its time
difference is machine variance. With all opt-in mechanisms, SWDCLK's 36.9 mm
In2.Cu run becomes 25.8 mm on B.Cu and XTAL_1 moves from In3.Cu to B.Cu; VBAT
and LED2 trade In2.Cu segments for F.Cu, so F.Cu grows by 24 mm while the via
count is unchanged.

## Full-vertical confirmation

Unprofiled runs on this machine. The baseline is the coordinator's merged-base
run (`build/fv-baseline-noprof` in the main checkout, run beside a profiled
baseline); the two new runs (`build/lb-src-fv1/build/fv-smooth` and
`fv-all` in this worktree) ran concurrently with each other. Lengths in mm /
sharp-turn candidates:

| Layer | Merged base | Default (smoothing) | Opt-in: all |
| --- | ---: | ---: | ---: |
| F.Cu | 880.331 / 35 | 874.018 / 22 | 812.103 / 23 |
| In1.Cu | 0 (GND plane) | 0 (GND plane) | 0 (GND plane) |
| In2.Cu | 1,347.424 / 94 | 1,324.990 / 76 | 1,004.243 / 79 |
| In3.Cu | 52.013 / 2 | 51.574 / 0 | 302.685 / 0 |
| In4.Cu | 44.042 / 0 | 44.042 / 0 | 88.280 / 0 |
| B.Cu | 101.981 / 1 | 98.576 / 1 | 243.864 / 0 |
| **Total** | **2,425.791 / 132** | **2,393.200 / 99** | **2,451.175 / 102** |

| Measure | Merged base | Default (smoothing) | Opt-in: all |
| --- | ---: | ---: | ---: |
| Saved vias (ordinary detailed) | 280 (115) | 280 (115) | 268 (103) |
| Peak 10 mm area, In2.Cu / F.Cu | 15.18% / 10.45% | 15.20% / 10.45% | 12.93% / 9.81% |
| Off-angle segments | 0 | 0 | 0 |
| Ordinary nets / final opens | 49 / 0 | 49 / 0 | 49 / 0 |
| Detailed attempts | 201 | 201 | 320 |
| Policy finally selected | preferred | preferred | neutral fallback |
| KiCad refill DRC | 0 violations, 0 unconnected | 0, 0 | 0, 0 |
| Runner wall time | 1,091.5 s | 886.6 s | 1,007.7 s |
| Critical results and critical-net copper | — | identical | identical |

The default searches exactly as the base (same 201 attempts and failure lists);
smoothing itself takes 0.3 s, so its wall-time difference is machine load,
not a speedup. It removes 33 sharp-turn candidates and 32.6 mm of track
without changing a via. Of In2.Cu's remaining 76 candidates, 54 sit on a
reserved boundary witness/port vertex (58 in the base); the others fell from 36
to 22, and F.Cu's non-via candidates from 25 to 13.

With all opt-in mechanisms, In2.Cu carries 25% less track and In3.Cu, In4.Cu
and B.Cu 635 mm instead of 198 mm, with 12 fewer vias, and the board still
closes cleanly. But the preferred-cost passes and repair left `USER_LED_2`
open; the existing neutral-cost fallback closed it and was selected, adding
about 200 s and 119 attempts. Replaying the saved pre-ordinary state with one
mechanism at a time made closure worse: crossing assignment with escape left
`MODEM_EN` open, assignment with local demand left `V3V3` open and escape alone
left `USER_LED_2` open, each even after the neutral fallback. Because every
ordinary terminal is an In2.Cu port, moving a run to another layer costs two
extra transitions next to crowded package collars. These mechanisms therefore
stay opt-in until terminal layers can follow the guides.

## Limitations and separate work items

- Spreading is bounded by the reserved In2.Cu package ports. The terminal-level
  causes need their own transactional design, outside this increment:
  offering a pad's direct surface access beside its boundary port and removing
  unused escape copper afterwards; choosing port layer/edge from the net's
  destination; collapsing escape-via, witness and router-via chains when a direct
  replacement validates; treating launch copper as part of the terminal (with
  same-net loop removal); and escaping only pins whose direct access is
  actually constrained.
- Remaining sharp turns are mostly constrained: corners at vias, at reserved
  port lead-ins and 180-degree overlaps with reserved witness paths. Smoothing
  does not delete or move vias and does not remove same-net loops.
- Global capacity still counts physical lanes at track pitch; the local-demand
  cost uses detailed lanes only to relabel layers, not to negotiate overflow.
- Spare-looking layers are not qualified reference planes. In3.Cu, In4.Cu and
  B.Cu have no adjacent dedicated plane in this stackup; nothing here qualifies
  impedance, crosstalk or return paths.
