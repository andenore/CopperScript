# D-PHY and differential-pair routing work list

This plan adds MIPI D-PHY (and general differential-pair) routing support.
It is based on a review of published MIPI layout guidance (TI SPRACP4A,
Renesas R01AN5871, Efinix UG-PCB-MIPI, Toradex layout guide) against what
CopperScript can express and route, using a four-lane D-PHY board
(QFN-to-0.4 mm-pitch-connector lanes, 2.5 Gbit/s) as the test case. Each item
names its syntax, IR, checks and tests, so items can be built in parallel
without colliding.

Shared rules for every item:
- **Fail closed.** Unknown or invalid properties are errors with source locations.
- **Screening is labelled.** Impedance and return-path results are screening
  evidence (`EvidenceGrade.SCREENING`), never sign-off.
- **No invented limits.** Defaults never add a constraint the source did not
  declare.
- **Determinism.** Identical inputs give byte-identical reports and geometry.
- **Tests.** Every item adds tests. Existing tests keep passing; the known
  environment-only failures (KiCad overlay probe, stale caches) are unchanged.
- **Docs.** `docs/language-reference.md` documents every new property.

## W0 — Already fixed during the review

- [x] `critical._route_pair`: a short pair whose terminals share one global
  tile produced an empty offset guide and crashed in `_pair_pin_stubs`. The
  coarse candidate is now rejected and the exact search still runs.
  Test: `test_pair_guide_without_segments_is_rejected_and_exact_search_still_runs`.
- [x] `critical._validate_candidate`: candidates are rejected only for hard
  findings they introduce. Pre-existing footprint findings are no longer
  attributed to them; open nets are never subtracted.
  Test: `test_pre_existing_footprint_finding_does_not_reject_an_unrelated_pair`.
- [x] `critical_preflight` physicalised only the electrical IR (`load_board`),
  so the preflight dropped the whole `mechanical` block: the outline (it used
  the 100 mm default), holes, rules, edges, overhangs and stack-up. Its lane
  table then reported "no stack-up declared". It now loads the full design
  like the CLI, and passes `--locked`/`--offline` to the footprint resolver.
  Test: `test_preflight_physicalizes_the_mechanical_block`.
- [x] `route_critical_nets` with package-access reservations ran a full-board DRC
  and failed the reservations (`<package-reservations>`) on *any* hard finding.
  A footprint's own land-to-locating-hole finding therefore blocked every
  critical pair and the ordinary area router in `route-board` (0 pair searches,
  0 ordinary nets routed). Only findings the reservations introduce now count.
  Test: `test_pre_existing_footprint_finding_does_not_fail_package_reservations`.

## W1 — Diagnostics and robustness

- [x] **D1 Rejection reasons.** For each critical group, record the
  first-failing gate of every rejected exact candidate. Gates: DRC code, skew,
  uncoupled length, via budget, return via, plane reservation and
  connectivity. Report them as counted summaries, for example
  `rejections: {"DRC-CLEARANCE": 4, "skew": 2}`, plus up to three example
  messages, in `CriticalNetResult` and the progress events.
  Done: `CriticalNetResult.rejections` / `rejection_examples`, the
  "none accepted (first-failing gates: …)" diagnostic, both reports and
  `critical_group` progress events. Tests: `tests/test_critical_diagnostics.py`.
- [x] **D2 Tracebacks.** `critical_preflight --debug` and `route-board --debug`
  print the full traceback of unexpected exceptions. The default stays a
  one-line error.
  Tests: `test_preflight_debug_prints_the_full_traceback`,
  `test_route_board_debug_prints_the_full_traceback`.
- [x] **D3 Coarse-guide transitions.** The coarse pair candidate must not place
  transition vias where pad clearance or the escape keep-out rules them out.
  For a pair whose profile allows no vias or a single layer, it proposes none.
  Done in `critical._coarse_transition_conflict`; the exact searches still run.
  Tests: `test_coarse_guide_*` in `tests/test_critical_diagnostics.py`.
- [x] **D4 Lane table.** Report each critical net's routed length, via count
  and layers in the route and preflight reports, plus estimated delay when a
  stack-up is declared (L1).
  Done: `critical_lane_table`, the `lanes` list of the critical report (in
  both reports) and preflight console lines. Tests: `test_lane_table_*`.
- [x] **D5 Skew compensation near the mismatch.** `_tune_pair` compensates on
  the shorter member's segment nearest the terminal or bend where the
  mismatch arises. It uses bounded serpentine bumps no taller than
  `tuning_amplitude_limit`, spaced at least 3 × width apart, instead of one
  bump on the longest segment anywhere.
  Tests: `test_tuning_*` in `tests/test_critical_routing.py` and
  `test_drc_rejections_are_recorded_even_when_a_later_candidate_is_accepted`.
- [x] **D6 Routed critical-lane review.** Review the routed copper of every
  critical net in the route and preflight reports, so lanes can be checked
  against layout guidance without ad-hoc scripts: per net the track length,
  layers and signal vias; per pair the skew and per `length_match` group the
  spread against `max_skew`; the least edge-to-edge spacing to other copper,
  inside and outside the net's breakout regions, against critical and other
  signal nets, with location and neighbour; the length closer than
  2 × `pair_gap` to another critical pair; and the sharpest bend and the bends
  over 45°.
  Done (CS-168): `pcbir/critical_review.py` (`critical_lane_review`,
  `lane_review_line`), on the exported board in `route-board` and on the
  critical board in the preflight (`critical_lane_review` in both reports, one
  `CRITICAL LANES:` console line). It reuses `critical_lane_table`,
  `verify_match_groups`, `BreakoutRegions` and `RoutingClearanceIndex`
  (`nearby_copper`, a 1 mm search radius). Zone nets are ignored. Report-only:
  routing and boards are unchanged. Tests: `tests/test_critical_lane_review.py`.

## W2 — Language and IR

- [x] **L1 Stack-up declaration.** Inside `mechanical`, ordered top to bottom:

  ```copper
  stackup {
      copper F.Cu { thickness = 0.035mm; }
      dielectric P1 { thickness = 0.1mm; er = 4.1; loss_tangent = 0.02; material = "3313"; }
      copper In1.Cu { thickness = 0.0175mm; }
      ...
  }
  ```

  Lowers to `Stackup.physical_layers`, whose fields already exist. Validation:
  - copper layers alternate with dielectrics;
  - copper names and count match the selected `--layers`;
  - thicknesses and `er` are positive;
  - the total thickness becomes `Stackup.thickness_nm`.

  KiCad export writes the board stack-up when it is declared.
  Done: `mechanical_stackup.py` (optional dielectric `type = core|prepreg` for
  the KiCad export). Tests: `tests/test_stackup_declaration.py`.
- [x] **L2 Impedance screening.** Add edge-coupled differential screening to
  the existing single-ended Hammerstad microstrip: IPC-2141A coupling for
  microstrip, and the symmetric/offset stripline estimates for inner layers.
  New routing properties:
  - `target_single_ended_ohms` (integer)
  - `impedance_tolerance_percent` (default 10)

  For every rule with a target on every allowed layer, compute the
  differential and single-ended estimates from width, gap and the declared
  stack-up. Report a warning `SI-IMPEDANCE` when an estimate is outside the
  tolerance, and `SI-NO-STACKUP` when a target exists without a stack-up.
  Done: `engineering.edge_coupled_microstrip_impedance`,
  `stripline_impedance`, `edge_coupled_stripline_impedance` and
  `signal_integrity.screen_impedance`. The JLCPCB anchor (0.14/0.26 mm over
  0.10 mm, εr 4.1) screens to 99.7 Ω differential, 51.9 Ω single-ended.
  Tests: `tests/test_signal_integrity.py`.
- [x] **L3 Reference-plane adjacency.** Warning `SI-NO-REFERENCE-PLANE` when a
  differential, clock or RF rule allows a layer with no `copper_zone` on an
  adjacent copper layer. After a native fill, run
  `engineering.return_path_continuity` for every critical net, reporting the
  fraction and a list of uncovered segments.
  Done: `signal_integrity.reference_plane_warnings` and
  `signal_integrity.return_path_report` (to be wired into the route report).
- [x] **L4 Length matching.** New constraint kind:

  ```copper
  constraint length_match(CSI_SRC_CKP, CSI_SRC_CKN, CSI_SRC_DA0P, ...) {
      id = "csi-src-lanes"; max_skew = 1.5mm;
  }
  ```

  It lowers to an IR `NetMatchGroup(id, nets, max_skew_nm)`. Every net must
  exist, a net can belong to only one group, and at least two nets are
  required. The route report gives each member's length and the group skew.
  A group over the limit is a hard verification failure
  (`DRC-LENGTH-MATCH`). Tuning toward the group comes later (R3).
  Done: `NetMatchGroup`, `PhysicalBoard.match_groups`,
  `signal_integrity.verify_match_groups` (lengths, skew, shortfall) and the
  DRC check. The route report itself is not wired yet. Note: critical
  candidate validation runs the full DRC, so a candidate that completes an
  over-skew group now fails with `DRC-LENGTH-MATCH`; R3 should decide whether
  `critical._validate_candidate` defers this code.
  Tests: `tests/test_dphy_routing_intent.py`.
- [x] **L5 Layer groups.** Routing property `layer_group = "name"`.
  - **Pre-route:** a warning when members of a group allow different layer sets.
  - **Post-route:** a report of each member's layers, and a warning when
    members' main runs end up on different layers.
  Done: `signal_integrity.layer_group_warnings` (`SI-LAYER-GROUP`) and
  `layer_group_report` (`SI-LAYER-GROUP-SPLIT`).
- [x] **L6 Breakout properties.** Routing properties `breakout_length`,
  `breakout_width`, `breakout_gap` and `breakout_clearance`, all optional.
  Within `breakout_length` of a terminal pad, the breakout values replace
  `width`, `pair_gap` and `clearance`. Outside it, `clearance` applies to
  every foreign object. IR fields on `NetRoutingRule`, validated (breakout
  values may only relax, never tighten). Router and DRC use them in R1.
  Done: IR fields `breakout_*_nm`; compile-time (`CMP110`) and board-level
  validation against the effective width/clearance and fabrication minimums.
- [x] **L7 `copper si-check` command.** Runs L2, L3 and L5 before routing, plus
  a summary of match groups. Same resolution flags as `check`.
  Done: `copper si-check BOARD [--locked --offline --layers N --fab-profile P
  --footprint-root R --allow-proxy-footprints --json]`; exits 0 with warnings.

- [x] **L8 Mode defaults for bonds and pads.** A `mode_group` default only
  applies to group conditions today. Bond and pad conditions (`PAD@MODE=CHOICE`)
  without an explicit `modes` selection leave pins `UNMODELED_PIN`. That blocked
  modelling the MAX96792A/MAX96793 C-PHY alternate function in CopperLib.
  Apply defaults in `erc.active_bonded_pads`, `pin_profile`, physicalize, the
  KiCad backends, pin resolution and power analysis.
  Done: every mode condition is evaluated through `pcbir/modes.py`
  (`effective_modes` = explicit `modes` over each group's `default`;
  `active_bonded_pads`, `active_device_pads`, `pins_bonded_to_pad`). Used by
  ERC (`_Context.modes`/`active_bonded_pads`/`pin_profile`, mux and route
  options, signal groups), `pin_resolution.resolve_package_pin`,
  `power._bonded_pads`/domain states, `physicalize._physical_pin_number`,
  `backends.kicad._physical_pin_name` and the compiler's `configure`
  lowering. Explicit selections still override; a group with neither default
  nor selection matches no condition (`MODE_NOT_SELECTED`).
  Tests: `tests/test_mode_defaults.py`.
- [x] **L9 Absolute-maximum limits.** Pins can only carry operating
  `voltage_min`/`voltage_max`. Add `absolute_min`/`absolute_max` (ERC error)
  alongside the operating range (ERC warning), and allow limits relative to
  another pin's supply (MAX96792A: `VTERM + 0.1 V`).
  Done: pin/pad properties `absolute_min`/`absolute_max` (a voltage,
  `"PAD[+|-]OFFSET"`, or a capped `"VTERM+0.1V, 1.36V"` where the tighter
  bound wins) lower to `ElectricalProfile.absolute_voltage`
  (`rating="absolute"`, bounds `Voltage` or `RelativeVoltage`); compile errors
  `CMP120` (malformed), `CMP121` (inconsistent with the operating range),
  `CMP122` (unknown reference). ERC `_check_pin_voltage`: outside absolute is
  `SUPPLY_VOLTAGE_ABSOLUTE_LOW/HIGH` (error), outside operating but inside a
  declared absolute bound is `SUPPLY_VOLTAGE_LOW/HIGH` as a warning, and
  operating-only parts keep the error. Relative limits resolve per component
  from the reference pad's declared supply (`_Context.reference_voltage`);
  failure is `VOLTAGE_LIMIT_UNRESOLVED`. Serialized as
  `profile.absolute_voltage` only when declared (existing electrical digests
  are unchanged). Tests: `tests/test_absolute_voltage_limits.py`.

- [ ] **L10 Operating-range severity on power pins.** Once a pin declares an
  absolute bound, an operating-range violation inside that bound is only a
  warning (L9). That suits signal pins, but a supply pin outside its operating
  range is a design error. Keep it an error for `domains = "power"` pins (or
  add a per-pin severity), so supply pins can also carry absolute ratings.
- [ ] **L11 Multi-band operating ranges.** Some supply pins accept two
  disjoint ranges (e.g. 0.95–1.05 V or 1.14–1.26 V; 1.7–1.9 V or 3.0–3.6 V).
  One `voltage_min`/`voltage_max` envelope accepts values between the bands.
  Allow several bands per pin, optionally selected by a mode.

## W3 — Router and placement

- [x] **R1 Region-aware breakout.** The pair search, critical validation and
  physical DRC apply breakout width, gap and clearance only within
  `breakout_length` of the pad. The DRC records the region that justified a
  smaller clearance.
  Done: `pcbir/breakout.py` (`BreakoutRegions`). A terminal land's region is
  its copper swept by `breakout_length` in plan view; copper is inside when
  its whole centreline is (both track ends, a via centre). `split` and
  `split_tracks` cut octilinear tracks at the boundary and give the inside
  pieces the breakout width. When a pair declares breakout properties, its
  members are spaced by `pair_gap` (`breakout_gap` inside a region), not
  `clearance`. Uses: `RoutingClearanceIndex` (every query), the pair search
  (`pair_search._tracks`/`_legal`, `pair_refine`), `critical._route_pair`
  (cut before and after tuning), `_route_single` and `_improve_pair_spine`.
  DRC: `_check_track_rules`, `_check_copper_spacing` and
  `_check_zone_fill_spacing` through `_BreakoutSpacing`. Relaxed checks go to
  `PhysicalDrcReport.breakout_relaxations` (`DrcBreakoutRelaxation`: check,
  objects, normal and breakout values, measurement, and the justifying net,
  land and breakout length), plus a `breakout_regions` coverage entry when a
  rule declares breakout properties; reports without them are unchanged.
  `critical._validate_candidate` is unchanged; it runs the region-aware DRC.
  Tests: `tests/test_breakout_regions.py`. Ordinary nets neck down too
  (CS-164, `BreakoutRegions.ordinary_pieces`/`neck_down`,
  `tests/test_ordinary_neck_down.py`).
- [x] **R2 Mismatched-pitch taper.** Generalise `_aligned_pair_paths` to
  terminals whose pitch differs from the declared pair pitch (for example a
  0.5 mm QFN to a 0.4 mm connector). It produces symmetric 45° tapers whose uncoupled
  length counts against `maximum_uncoupled_length`.
  Done in `critical._aligned_pair_paths`: both ends must share one midpoint
  and member order on one layer; the land pitch is free at either or both
  ends and each end gets its own 45° ramp. The equal-pitch case is unchanged.
  Tests: `tests/test_pair_taper.py`.
- [x] **R3 Length-match tuning.** After all critical groups are accepted, tune
  the shortest members of each `length_match` group toward the longest, using
  D5's bounded serpentines and the same atomic validation.
  Done: `critical._tune_match_groups` / `_tune_match_group` tune each group
  over its `max_skew`, unit by unit (a pair is one unit: both members get the
  same bumps at the pair spacing, so its own skew is kept; a single-ended net
  is tuned alone). A unit first matches the group's longest member, then falls
  back to the least length that reaches `max_skew`; every step passes the
  profile gates (`_retuned_result`) and `_validate_candidate`, and a group's
  copper changes only when it ends within the limit. Placement rule in
  `pcbir/critical_tuning.py` (`UnitTuner`): slots on straight runs, most free
  room first (swept area against other copper, lands, keep-outs and the edge,
  via `RoutingClearanceIndex.can_area`), then farthest from the terminals; at
  most `MATCH_TUNING_BUMP_LIMIT` = 16 bumps per unit, no taller than
  `tuning_amplitude_limit`. `_validate_candidate` defers `DRC-LENGTH-MATCH`
  during routing; a failed group fails the stage and final DRC still reports
  it. Report: `CriticalRoutingResult.match_tuning` (`match_tuning` in the
  critical report) and one preflight line per group. With R1, tuned copper of
  breakout nets is re-cut at the region boundaries (`split_tracks`), so a bump
  never carries the breakout width outside a region.
  Tests: `tests/test_length_match_tuning.py`.
- [x] **R4 Corridor reservation.** Routing property `reserve_corridor = true`.
  When both terminal components of a critical pair are fixed, placement keeps
  other components out of the corridor between their terminal lands, sized
  pair width + gap + clearance on each side. It behaves like hand-drawn
  placement keepouts, and the report lists the derived corridors.
  Implemented by `placement.reserved_corridors`: the corridor is the convex
  hull of both nets' terminal lands, expanded by the margin. Movable terminals
  skip the corridor with a `CORRIDOR_NOT_RESERVED` warning. A fixed
  non-terminal component inside the corridor is an error. `plan-layout
  --report` lists `reserved_corridors` and `skipped_corridors`.
  Tests: `tests/test_corridor_reservation.py`.
- [x] **R6 Bundle-aware critical ordering.** Critical groups are routed one
  at a time, and accepted pairs are immutable. On the test board, later lanes failed on
  clearance against lanes routed earlier (`tracks 148 and 162 violate copper
  spacing`). Route a bundle's pairs in physical order (outermost first,
  following terminal order). On a clearance failure against a committed group
  in the same bundle, try a bounded rip-up of that group and reroute both, with
  the same atomic validation.
  Done: `pcbir/critical_bundles.py` plans bundles (same two components, kind
  and priority) and their outermost-first order; `critical.route_critical_nets`
  routes them in the bundle's slots and runs `_bundle_repair` (at most
  `BUNDLE_REPAIR_LIMIT` = 4 attempts per bundle, `bundle_repair_limit`
  argument). The critical report lists `bundles` with order and repairs.
  A pair that fails with no candidate at all (no spacing finding names a
  blocker) tries its two physically nearest routed bundle pairs instead
  (`_bundle_neighbours`): a middle pair squeezed by a pair-to-pair clearance
  larger than the package pitch allows routes first, its neighbour after.
  Tests: `tests/test_critical_bundles.py`.
- [x] **R5 Planned crossings.** When the pair order at the two ends of a
  bundle is inverted, as on a symmetric CSI pinout, plan one paired layer swap
  with return vias for the crossing pair before the surface search. Design
  note first; implementation bounded by the existing paired-via machinery.
  Done (CS-165): `critical_bundles.plan_bundles(plan_crossings=True)` plans
  them (`_plan_crossings`, `_crossing_ranks`, `_surface_pairs`,
  `_crossing_layers`) into `CriticalBundle.crossings` (`PlannedCrossing`,
  `CrossingTransition`) and puts moving pairs first in the bundle order.
  `critical._route_pair_group` routes a planned crossing with
  `pair_vias.paired_via_candidates(layers=…, site_rank=…)` only
  (`_crossing_site_rank`), strategy `planned_crossing`; `_bundle_repair`
  keeps the plan; `_crossing_outcomes`/`crossing_outcome` record the routed
  transitions. `route_critical_nets(plan_crossings=False)` restores the
  previous behaviour. Report: `crossings` in the bundle record (only when
  present) and `crossing_line` in the preflight. `transition_spacing` now
  spaces the vias of a breakout pair by its pair gap. On the test board,
  with the same 2000-state budget, the crossing pair found 0 candidates
  without R5 and routes in 267 states with it. Limitation: with return vias
  required, `_return_via` places the via on the pair's centre line ahead of
  the transition, so the inner run may detour back past it (a declared
  shared reference avoids this); pairs that would need more crossing layers
  than allowed are reported impossible; via barrels are not counted in
  lengths. Tests: `tests/test_planned_crossings.py`.

  **R5 design.**
  - *Detection.* Per R6 bundle, each pair's land centre at a component is
    ranked by angle around that component's courtyard centre, in one
    rotational sense for both components, starting from the direction that
    points away from the other component. Read this way, pairs routed side
    by side without crossing have reversed ranks at the two ends (facing rows,
    side-by-side rows and package corners alike), so the second component's
    ranks are reversed. Two pairs must cross when their ranks are in opposite
    order at the two ends. A pair centre on the courtyard centre, two pair
    centres at one angle, coincident component centres or components on
    different board sides plan nothing (the bundle routes as before).
  - *Minimum set.* The pairs that stay on the surface have no crossing among
    them. The set keeps as many pairs that cannot swap layers as possible,
    then as many pairs as possible, so the fewest pairs move (a longest
    increasing subsequence of the ranks when every pair can swap). Ties keep
    the inner pairs (later in the R6 order), so transitions sit at the
    bundle's edge where vias have room, then the lower ranks at the first
    component.
  - *Layer.* An allowed layer of both members (`routing_layers`, so other
    nets' dedicated planes are excluded) other than the terminal surface, with
    a legal via span from the surface; with a `layer_group`, a layer every
    member of that group allows. Layers with a `copper_zone` on an adjacent
    copper layer (L3) are used when any qualifies (otherwise all candidate
    layers, and the record lists no reference plane; L3 already warns), in
    `signal_layer_preferences` rank and stack-up order. Moving pairs take
    layers in bundle order: never the layer of a moving pair they cross
    (both would cross there), and the layer of a moving pair of their own
    `layer_group` when that is legal. A moving pair with no layer left (for
    example the second outer pair of a full three-pair reversal on a board
    with one crossing layer) is reported impossible.
  - *Transitions.* The existing paired-via machinery builds them
    (`pair_vias._transitions`): a surface collar from the terminal pair, a
    matched via pair at `transition_spacing`, return vias within
    `maximum_return_via_distance` unless a declared shared reference covers
    both contact layers (CS-160), then the planned layer to the other end's
    transition, so each member gets exactly two vias. For a planned crossing
    a transition is dropped when a via centre (signal or return) lies in any
    breakout region (R1), and transitions with every via inside the pair's
    reserved corridor (R4) are tried first. The search runs on the planned
    layer only and never uses the coarse guide or its via proposals. Members
    of a pair with breakout properties are spaced by the pair gap (R1), so
    `transition_spacing` uses the pair gap when it exceeds the clearance.
  - *Ordering, repair, tuning and budget.* A bundle with crossings routes its
    moving pairs first (in R6 order), then the surface pairs in R6 order, so
    surface pairs route around the transition sites instead of over them.
    Bundle repair re-routes a crossing pair with its plan. Every candidate
    still passes the `_route_pair` profile gates, the plane reservation and
    `_validate_candidate`. Length-match tuning (R3) tunes a routed crossing
    pair like any pair; via barrels are not counted in lengths. A member whose
    `max_vias` is below 2 cannot swap: the crossing is reported impossible and
    the pair fails with that reason, without a surface or coarse fallback.
  - *Report.* A bundle with crossings gains `crossings`: each moving pair, the
    pairs it crosses, surface and planned layer, the layer's reference
    planes, the status (`routed`, `failed` or `impossible`) and reason, and per
    transition its component, each member's via centre and either the return
    vias or the shared reference layer. The preflight prints one line per
    crossing. Bundles without crossings, and their reports, are unchanged.

- [x] **R7 Nested exits at package corners.** When a bundle leaves a package
  across a corner (some pairs on one edge, the rest on the next), each
  side-edge pair runs past the column of its far-end terminal before turning
  and then comes back (an S-bend). On a 0.5 mm QFN this added about 1–2 mm to
  the outermost pair: package-escape planning reserves each pair's exit region
  on its own, so the outer pair clears the inner pair's whole exit region.
  Plan nested exits instead: each outer pair turns as soon as it clears the
  inner pair's actual exit copper. Shorter corner wraps reduce the structural
  bundle skew that R3 would otherwise have to tune out.
  Done (CS-171): the overshoot came from the joint pair search, whose fixed
  port lengths and 1 mm lattice step make a side pair run straight on before
  it may turn. `critical_bundles._plan_nested_exits` plans a `NestedExit` per
  side-edge surface pair of a corner wrap (`_pair_edge`): innermost first,
  each on its far column or just outside the inner pair's band, and
  `_nest_order` gives each nest its bundle slots innermost first.
  `critical._route_nested_exit` tries `pair_search.nested_exit_candidates`
  before every other candidate: shortest legal port, one 45° diagonal onto the
  run (longest legal first, so an outer pair hugs the inner pair's copper),
  the run, and a 45° jog back before the far port when the run lies beyond
  the column. A run that does not clear, or whose D5 bumps do not, steps out
  by `NESTED_TURN_STEP_NM` (at most `NESTED_TURN_RANGE_NM`); at most
  `NESTED_VALIDATION_LIMIT` candidates take the unchanged gates, otherwise the
  pair routes as before. `route_critical_nets(nested_exits=False)` restores
  the previous behaviour. Report: `nested_exits` in the bundle record (only
  when present) and one preflight line per nested exit. On a synthetic QFN
  corner the outer pair drops from 14.46 to 12.28 mm and the inner from 11.38
  to 9.79 mm; with a 0.3 mm clearance and a tight connector, the inner pair
  failed before and now routes. On the test board's sink bundle, D2 drops from
  13.91/14.03 to 12.40/12.53 mm and D3 from 9.66/9.79 to 9.23/9.36 mm (CK, D0
  and D1 unchanged), the bundle spread from 6.626 to 5.124 mm; all ten pairs
  connect and package access stays ready. Limitation: a bundle with every
  pair on the side edge (no facing pair) is not a corner wrap and routes as
  before; a far end that also needs a turn is not planned. Tests:
  `tests/test_nested_exits.py`.

- [ ] **R8 Bundle corridors for corner wraps.** `reserve_corridor` keeps
  components out of the band between each pair's own terminals. When a
  bundle leaves a package across a corner, the outer pairs route outside
  their own band, around the inner pairs. On a QFN-32 a passive and its
  stitch via placed in the outer pair's wrap left that pair with no
  candidate. Reserve one corridor for the whole bundle, sized by the
  nesting width of the inner pairs, instead of per-pair bands.

- [x] **R9 45° tuning corners.** R3 bumps and D5 skew bumps are rectangles,
  so every bump adds four 90° corners. D-PHY and other high-speed guides ask
  for no 90° corners on these lanes. Chamfer each corner at 45° and keep the
  perpendicular legs.
  - With chamfer leg c, a bump of height h adds 2h − 4(2 − √2)c, close to
    today's 2h. A 45°-sided trapezoid would add only 2(√2 − 1)h ≈ 0.83h.
  - Both members of a pair get the offset chamfers of a coupled 45° bend, so
    the pair spacing stays constant.
  - Each bump still has two left and two right turns, so both members add
    the same length.
  - Room checks and the swept area use the chamfered outline.
  - **Tests:** exact added length per bump; pair skew unchanged; no bend
    sharper than 45°; native DRC clean.

  Done (CS-167): `critical_tuning.bump_chamfers`, `bump_path` and `bump_gain`
  build both tuners' bumps. Rule: for height h and width w the member inside a
  turn gets leg c = min(w, ⌊h/4⌋, ⌊(h − d)/2⌋) and its partner c + d, with
  d = ⌊(2 − √2) × spacing⌋ (0 for a single net). Flooring d keeps parallel
  pieces never closer than the spacing (at most 2 nm farther); a bump too low
  for c ≥ 1 has one 45° ramp per side. The bulge-side member is inside the
  turn at both run corners, its partner at both top corners, so both gain
  exactly `bump_gain` in the rounded-per-piece length measure. R3:
  `UnitTuner.tune` counts each slot's capacity and levels heights by that gain
  (`_levelled`: one nanometre of height changes it by −2, 0 or +2, so the
  excess is removed exactly), and `_swept` is the convex outline of the
  chamfered bump (the rectangle no longer covered the run chamfers). D5:
  `_tune_pair` takes c from the member's narrowest width and uses the fewest
  bumps at the least common height that reach the excess. On boards with
  breakout regions both tuners aim 32 nm inside `max_skew`
  (`_BREAKOUT_TUNING_MARGIN_NM`): re-cutting a 45° piece at a region boundary
  may round a nanometre differently. Measured: for pairs, the outside offset d
  costs about 0.69 × spacing per bump and member, so a 0.2/0.2 mm pair at
  1 mm height gains about 63% of 2h (single nets lose only 2.34 × c) and
  needs more or taller bumps; a pair bump adds up to about 3.3 spacings of
  uncoupled length per member. Tests: `tests/test_length_match_tuning.py`
  (`test_pair_bumps_have_coupled_45_degree_corners_and_add_exact_lengths`)
  and the `test_tuning_*` D5 tests in `tests/test_critical_routing.py`.

- [x] **R10 S-shaped (two-sided) serpentines.** Today every tuning bump
  leaves its run on one side, returns to the same line, and needs 3 × width
  of straight track before the next bump. An S-shaped serpentine alternates
  sides instead: the trace crosses the original line between legs and snakes
  about it.
  - **Density:** at the same amplitude per side, each leg adds the full
    swing and no straight return is needed. That fits about 1.5× the length
    into a run. For a 0.14/0.26 mm pair at 0.35 mm amplitude:
    - one-sided bumps add 0.70 mm per 1.64 mm of run;
    - S-legs at a 0.52 mm leg gap add 0.70 mm per 1.06 mm.
  - **Limit:** the serpentine needs room on both sides of the run. In a
    packed bundle each side is shared with a neighbouring pair, so it helps
    most on outer pairs, fanned-out bundles and single-ended nets.
  - *Language.* Routing property `tuning_style`:
    - `"bumps"` is the default and today's geometry;
    - `"serpentine"` is the S-shape;
    - both members of a pair must agree.
    - `tuning_amplitude_limit` stays the maximum excursion on each side of
      the original line.
    - New optional `tuning_spacing` sets the minimum edge gap between
      adjacent legs. It defaults to the larger of 3 × width and the net's
      `clearance`, so a pair keeps its pair-to-pair spacing between its own
      legs.
  - *Geometry.*
    - On a straight run (axis-aligned first; 45° runs later), legs run
      perpendicular to the run at a pitch of leg width + `tuning_spacing`.
    - The legs join alternately at +a₁ and −a₂, each within the room on
      that side and the amplitude limit.
    - Each full leg adds a₁ + a₂, and the first and last half-legs add a₁
      or a₂. Corners use R9's chamfers.
    - A pair's members follow each other at the pair spacing. The turns
      alternate left and right, so both members add the same length and the
      pair's skew is unchanged, exactly in integer nanometres.
  - *Room and placement.*
    - Room is measured per side, as in R3: the swept area against foreign
      copper, lands, keep-outs, holes and the board edge, through
      `RoutingClearanceIndex.can_area`.
    - A run takes a serpentine only when both sides have room; otherwise it
      falls back to one-sided bumps.
    - The tuner uses the fewest legs that provide the length, with levelled
      amplitudes, under a leg bound like `MATCH_TUNING_BUMP_LIMIT`.
    - Every result passes the same atomic validation.
  - *Scope.* R3 group tuning only. D5's intra-pair compensation keeps
    one-sided bumps that bulge away from the partner, because an S-shape on
    one member would swing into its partner.
  - *Self-coupling.* Adjacent legs couple, so a serpentine's delay is
    slightly shorter than its length suggests. Reports keep the geometric
    length. The default leg gap keeps the error small; qualifying it with a
    field solver is out of scope.
  - *Report.* Each unit's `match_tuning` entry gains the style, the leg
    count and the amplitudes. The preflight line names the style.
  - **Tests:**
    - exact added length per leg;
    - pair skew unchanged;
    - fallback to bumps when one side is blocked;
    - clearance to neighbours under native DRC;
    - determinism;
    - boards without `tuning_style` route byte-identically.

  Done (CS-169): `TuningStyle` and `NetRoutingRule.tuning_style` /
  `tuning_spacing_nm` (lowered, `CMP110`-checked, pair members must agree,
  digest-bound only when not the default). `critical_tuning.UnitTuner(style=
  SERPENTINE)` surveys `_windows`: tops on a grid of pitch lane width + leg
  gap, each top's room measured like a bump over its two legs
  (`_room` with the least top and largest foot chamfer), windows of two or
  more tops with room on alternating sides. Refinements: legs on one line
  share one chamfer leg c, R9's rule on every leg (`_window_chamfer`), so each
  leg costs both members exactly `_leg_loss(c)` whatever the heights; tops
  are levelled with any remaining bumps by R3's `_levelled`, and c is lowered
  until it suits every leg and makes the length exact. Tops lower than d + 2
  nm cannot have coupled corners, so small additions (on a 0.14/0.26 mm pair
  below about 0.53 mm) fall back to bumps. Serpentine units also join collinear
  pieces (`_straight_lines`): breakout-region cuts had split the test board's
  source lanes into runs under 2 mm, too short for any bump slot; bump-style
  units keep per-piece runs, so they tune byte-identically. One serpentine per
  line, most capacity first, the last trimmed (`_trimmed`); other lines keep
  bumps; at most `MATCH_TUNING_LEG_LIMIT` = 32 legs; a unit whose serpentines
  fall short gets R3's bumps alone. Report: `UnitTuning` per step and
  `MatchTuningUnit` (`units`, with style, legs and amplitudes, and `legs` in
  the group entry, only when a step used a serpentine or ran while routing);
  the preflight line then lists each step. Tests:
  `tests/test_serpentine_tuning.py`.

- [x] **R11 Via-in-pad arrays.** IC layout guides ask for an array of vias in
  an exposed pad, for heat and a low-inductance ground return. On a QFN whose
  exposed pad is its only ground, the single last-resort 0.30/0.20 mm via is
  far too little. `rows`, `columns` and an optional `pitch` on `via_in_pad`
  require a centred array of those vias, placed as fixed copper before any
  routing.
  Done (CS-166): `pad_via_arrays` computes the grid (`array_pitch_nm`,
  `via_in_pad_array_vias`), checks its fit when the board is built
  (`validate_array_fit`) and commits it with the owner prefix
  (`materialize_via_in_pad_arrays`, called by `materialize_hard_macros`;
  `macro_source` strips it again). Plane stitching takes the array as the
  pad's contact; fanout and detailed routing refuse a board whose array is
  missing; incremental placement repair falls back to the full pipeline when
  an array's component moves. Report: `via_in_pad_arrays` in the route-board
  and critical-preflight reports (only when present). Limitation: one land
  per pad, the fallback via size only and a square pitch; a site that fails at
  a candidate placement stops the run instead of rejecting that placement.
  Tests: `tests/test_pad_via_arrays.py`.

- [x] **R12 Tune bundle pairs as they route.** R3 tunes a `length_match`
  group only after every critical group is accepted. By then each bundle
  pair sits at the clearance limit next to its neighbours, so a pair hemmed
  in on both sides has no room for even one bump. On a test bundle, a 7 mm
  lane short by 0.12 mm could not be tuned, and three adjacent straight pairs
  short by about 5.8 mm each had one usable slot between them.
  - Right after a bundle pair of a `length_match` group is accepted, tune it
    toward the longest accepted member of its group, to within `max_skew`,
    before the next bundle pair routes. Use the same tuner (R3, R9, and R10
    where declared) and the same atomic validation.
  - Its bumps or serpentine then claim room, and later pairs route around
    them. Outermost-first order often routes the longest pairs (those
    wrapping a package corner) first.
  - If a later pair turns out longer, the final R3 pass tops up the earlier
    pairs as today.
  - Groups already within `max_skew`, and bundles without a `length_match`
    group, route exactly as before.
  - **Tests:** a hemmed-in bundle that R3 cannot tune and R12 tunes within
    `max_skew` with native DRC clean; identity without match groups;
    determinism.

  Done (CS-170): `critical._tune_bundle_pairs` runs after each accepted
  bundle pair of a `length_match` group and tunes every accepted pair of the
  bundle that is short of the longest accepted member (`_tune_unit`, shared
  with R3). Refinements: (1) not only the pair just accepted: pairs routed
  before the longest are topped up as soon as it is accepted, while their
  inner neighbours are still unrouted. On the test board the source bundle
  routes DA2 and DA1 first and the longest lane DA3 third, so the plan as
  written would have left them to the final pass. (2) No prediction of the
  longest member: guide lengths were 8.0–13.5 mm against routed 7.2–14.0 mm
  there, land-to-land distances miss corner wraps by several millimetres,
  and a pair tuned past the true longest cannot be shortened. (3) No room is
  reserved for a pair's own later serpentine or for its neighbours: tuning
  claims room as copper. A trial that kept the unrouted pairs' land hulls
  clear left R12 unable to tune any hemmed pair, while tuning into a later
  pair's way once left that pair with no candidate. So when a later bundle
  pair finds no candidate, the bundle's tuning from routing is removed and
  the pair searched once more before any rip-up repair. A pair that fails is
  retried only after re-routing or when the target grows (`failed_targets`).
  The final pass tops up the rest. Report: `match_tuning` `units` with
  `stage` `routing`, lengths before tuning as routed; preflight lines name
  each step. On the test board (both groups at 0.127 mm, CSI rules
  `tuning_style = "serpentine"`, amplitudes 0.3/0.35 mm), the source group
  goes from 0.249 to 0.083 mm (DA2 and DA1 +0.249 mm, CK +0.166 mm, one bump
  each, tuned while routing), all ten pairs connect, the lane review keeps
  0.521 mm outside the breakouts and no bend over 45°. The sink group fails:
  it needs 3.2 mm (D3) to 5.1 mm (CK, D0, D1) per pair, but its open straight
  runs are 1.4–4.9 mm and neighbouring pairs at the connector's 1.2 mm pitch
  leave 0.14 mm on each side, below the 0.234 mm a 0.14/0.26 mm pair's
  coupled corners need, so no serpentine fits and bumps reach 0.34–1.0 mm per
  pair. Tests: `tests/test_bundle_tuning.py`.

- [x] **R13 Surface-only package escapes.** Package access gives each
  fine-pitch pin of an ordinary net a via dogbone (`pcbir/fanout.py`) or a
  boundary witness port. A net restricted to one outer layer
  (`allowed_layers` with one layer, or `max_vias = 0`) gets no dogbone
  choices, because `_legal_choices` and `_maze_choices` need two allowed
  layers. Its pins stay pending, and package access fails.
  - Give such pins a checked surface escape on their own layer to the package
    collar, as boundary witness ports do, and never a via.
  - Nets with two or more allowed layers keep today's choices.
  - **Tests:** a QFN pin on a one-layer net is escaped without a via, and the
    board is ready; nets with more layers are unchanged.

  Done (CS-172): `fanout._surface_layer` selects the pins whose net has one
  routing layer, or `max_vias = 0`, and whose pad layer is allowed. Their
  choices come from `boundary_access.surface_escapes`: from the pad centre on
  the pad layer to beyond the collar, through the witness ports and both 45°
  path orders, then the fine ports and the octilinear maze. They join the joint
  assignment and the native DRC gate with the dogbones. The selected path is
  owned fanout copper and `FanoutResult.surface_accesses` (a `RoutingAccess`
  launched from the pad), merged into `routing_accesses`; `accesses` stay via
  anchors. A `max_vias = 0` net with several layers got via dogbones before;
  it now escapes on the surface too. With the crystal constraint on its four
  crystal nets, the test board's preflight is ready with no pending pads (four
  were pending): each crystal pin has a 1.32 mm straight F.Cu escape. In its
  full route both X1 nets route on F.Cu without vias (15.0 and 13.6 mm), but
  both X2 nets stay unrouted: a crystal ground land and its plane-contact via
  sit between each X1 pin and its crystal land, so X1 loops around the
  crystal's X2 land. That needs a crystal placement or orientation change, not
  a different escape. Tests: `tests/test_surface_package_escape.py`.

## Order

1. In parallel: W1 (D1–D5), W2 (L1–L7), R4, and the CopperLib work below.
2. Then: R1 (needs L6), R2, R3 (needs L4 and D5).
3. Then R5 and R6 (both done; a pad-ordered pinout still avoids crossings).
4. R7 when corner-wrap skew matters. R9, then R10 (which uses R9's corners),
   when tuned lanes must avoid 90° corners or more length must fit a short run.
   R12 with R10, when bundle pairs must match tightly. R13 when a pin-field net
   must stay on one layer.
5. Finally the test board adopts each feature and its critical preflight is re-run.

## CopperLib (separate repository)

- [x] Device models for MAX96792A and MAX96793 with `differential_pair` groups
  for every D-PHY lane and clock. Part and pin names stay unchanged. C-PHY is
  modelled with L8: a `PHY` mode group (MAX96793) or `PHY_A`/`PHY_B` (MAX96792A),
  D-PHY by default, with trio bonds per the data-sheet pin tables. Lane 2/3 pins
  have no documented C-PHY function and are unmodelled in C-PHY mode; trios have
  no group type yet.
- [x] D-PHY pin voltage limits: MAX96793 1.35 V absolute maximum; MAX96792A
  VTERM + 0.1 V, 1.36 V at most, as L9 `absolute_min`/`absolute_max` (-0.3 V
  lower bound on both). Supply pins deliberately keep operating ranges only: an
  absolute bound would turn an operating-range violation on a supply pin (e.g. a
  1.2 V core pin on a 1.8 V rail) from an ERC error into a warning (see L10).
- [x] Four-lane CSI-2 cable interface modules `CSI2_DPHY_X4_SOURCE_PORT` and
  `CSI2_DPHY_X4_SINK_PORT` in `interfaces/mipi/csi2-dphy-x4-cabline-ca`
  (40-position I-PEX 20525-040E-02 pinout in the GMSL parts' D-PHY pad order,
  so no pair crosses; source and sink share positions and a straight cable
  loops back).
