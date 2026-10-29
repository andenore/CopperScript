# M4 critical-replay fixtures

Inputs of the `replay-critical*` comparison suites (see
`scripts/compare_corpus.json`). `copper replay-critical` replays the critical
routing stage from saved inputs; the Rust router cannot produce them yet, so
they are produced once by the Python CLI and committed. Both implementations
then read the same files.

## Checkpoints

`NAME.state.json` is the `critical-inputs.state.json` that

```sh
python -m copperscript route-board BOARD OPTIONS --routing-profile quick --checkpoint-dir DIR
```

writes, run from the repository root with `PYTHONHASHSEED=0 PYTHONUTF8=1`.
The rest of the routing run is not needed: `generate.py` stops it as soon as
the checkpoint is complete.

| Checkpoint | Board | Options |
|---|---|---|
| `routing` | `tests/rust-port/export-kicad-pcb/routing.copper` | `--allow-proxy-footprints --no-check --layers 6 --fab-profile jlcpcb-six-layer` |
| `si-dphy` | `tests/rust-port/si-check/dphy.copper` | `--locked --offline --allow-proxy-footprints --no-check --layers 6 --fab-profile jlcpcb-six-layer` |
| `si-four-layer` | `tests/rust-port/si-check/four_layer.copper` | `--locked --offline --allow-proxy-footprints --no-check --layers 4 --fab-profile jlcpcb-four-layer` |
| `si-layer-groups` | `tests/rust-port/si-check/layer_groups.copper` | as `si-dphy` |
| `si-no-stackup` | `tests/rust-port/si-check/no_stackup.copper` | as `si-dphy` |
| `si-reference-planes` | `tests/rust-port/si-check/reference_planes.copper` | as `si-dphy` |
| `si-single-ended` | `tests/rust-port/si-check/single_ended.copper` | as `si-dphy` |
| `si-two-layer` | `tests/rust-port/si-check/two_layer.copper` | `--locked --offline --allow-proxy-footprints --no-check --layers 2` |
| `nrf-antenna-macro` | `examples/nrf_antenna_macro/board.copper` | `--locked --offline --no-check --layers 6 --fab-profile jlcpcb-six-layer` |
| `full-vertical` | `examples/full_vertical/board.copper` | `--locked --no-check --layers 6 --fab-profile jlcpcb-six-layer` (resolves footprints online; proxies do not fit its macros) |

The trivial no-critical-net checkpoint of `examples/valid_board` (and its
placement checkpoint, which `replay-critical` also accepts) is the M5 report
fixture `tests/rust-port/reports/states/`.

## Derived inputs

`bad/` holds inputs for the error paths and `derived/` valid inputs for
result paths no checkpoint reaches. Each is a small edit of a real state,
re-digested so that it reaches the check it is about:

| File | Edit | Reaches |
|---|---|---|
| `bad/not-a-state.json` | another schema | not a state file |
| `bad/invalid-json.state.json`, `bad/empty.state.json`, `bad/not-utf8.state.json` | truncated, empty, not UTF-8 | the JSON and decode errors |
| `bad/digest-mismatch.state.json` | one digest digit | content digest mismatch |
| `bad/foreign-type.state.json` | an `os:system` value | the codec's type allow-list |
| `bad/plain-number.state.json`, `bad/no-global-route.state.json` | a number; the dict without `global_route` | missing board/global route |
| `bad/unknown-board-field.state.json` | `PhysicalBoard.bogus` | unknown init field |
| `bad/bad-guide-layer.state.json` | a guide segment on `X.Cu` | an enum `ValueError` |
| `bad/stale-macro-guides.state.json` | nRF guides with another placement fingerprint | stale hard-macro guides |
| `bad/reserved-accesses.state.json` | a `FanoutResult` without fields | Python's crash (Rust: not implemented) |
| `derived/asymmetric-pair.state.json` | `routing`'s `D_P` partner set to `GND` | the asymmetric pair result (`--only D_N`) |

Regenerate with

```sh
python tests/rust-port/critical/generate.py            # every checkpoint (minutes per board)
python tests/rust-port/critical/generate.py routing    # one checkpoint
python tests/rust-port/critical/generate.py --derived  # bad/ and derived/ from the committed checkpoints
```

Changed inputs invalidate the comparison: regenerate the derived inputs after
any checkpoint.

## Comparing

The report and stdout carry wall time (`seconds`, and
`local_preparation.elapsed_seconds` of each group), so the result suites
compare the exit status, stderr, the KiCad project files and the report
without those keys (the harness `json` option's `without` form), never
stdout. The error and usage suites print no wall time and compare stdout too.

## Synthetic inputs

`synthetic/NAME.state.json` are replay inputs of the small boards the
critical-routing unit tests build: the board, `route_global(board)` computed
as its test does, and no reserved accesses or plane-stitch options. They reach
the bundle, repair and tuning paths no checkpoint reaches. Regenerate with

```sh
python tests/rust-port/critical/synthetic.py [NAME ...]   # re-executes with PYTHONHASHSEED=0 PYTHONUTF8=1
```

Replay each with the `--pair-states` given (the test's bound; default 6000
otherwise). The paths were checked against the Python report and with call
counters; times are one Python replay.

| File | Source test/helper | `--pair-states` | Reaches | Python |
|---|---|---|---|---|
| `crossing-routed` | `test_planned_crossings._swapped` | | a routed planned crossing (paired swap F.Cu -> In2.Cu, return vias) | 1.3 s |
| `crossing-reversal` | `_reversed(six_layers=True)` | | two routed crossings on their own layers; crossing order (moving pairs first) | 1.7 s |
| `crossing-impossible-layer` | `_reversed(six_layers=False)` | | one routed and one `impossible` crossing (no layer left), then a rejected bundle repair | 1.0 s |
| `crossing-impossible-vias` | `_swapped(max_vias=1)` | | an `impossible` crossing (via budget): no search, no copper | 0.5 s |
| `crossing-failed` | `_swapped()` with In2.Cu closed between the rows | | a planned crossing whose pair finds no candidate (`failed`) | 21 s |
| `crossing-steps-aside` | `_crossing_beside(...)`, LANE_C moving | | crossing order with the crossed pair routed next to wide transition vias | 4.1 s |
| `crossing-shared-reference` | `_shared_reference(_swapped())` | | crossing transitions on a declared shared reference (no return vias) | 1.1 s |
| `crossing-breakout-inside` | `_swapped()`, 15 mm breakouts | | crossing transitions inside breakout regions (fallback) | 2.4 s |
| `crossing-tuned` | `test_a_length_match_group_with_a_crossing_pair_is_tuned` | | a crossing bundle tuned while routing (`_tune_bundle_pairs`) | 1.1 s |
| `nested-routed` | `test_nested_exits._corner_board()` | | two routed nested exits; `_tune_pair` intra-pair bumps | 0.6 s |
| `nested-breakout` | `_corner_board(TIGHT, clearance="0.3", breakout=True)` | 4000 | routed nested exits; `_tune_pair` bumps on pairs with breakout regions (99 tuned calls) | 1.5 s |
| `nested-fallback` | `_corner_board()` with a keep-out across the side columns | | one `fallback` and one routed nested exit, and an accepted bundle repair | 18 s |
| `bundle-repair` | `test_critical_bundles._repair_board(*REPAIRABLE)` | 4000 | an accepted bundle rip-up repair (`_bundle_repair`) | 14 s |
| `bundle-repair-squeezed` | `test_a_squeezed_pair_without_spacing_findings_rips_up_its_nearest_neighbour` | 20000 | a repair of the nearest neighbour (no spacing finding names a blocker) | 1.5 s |
| `loose-repair` | `test_loose_pair_repair._loose_board` | 2000 | a rejected loose (non-bundle) repair recorded on the group result | 3.7 s |
| `match-tuned` | `test_length_match_tuning._lanes_board()` | | final-pass tuning (`tuned`), pair bumps | 0.6 s |
| `match-failed` | `_lanes_board(TWO_LANES)` between walls | | final-pass tuning `failed` (insufficient room), copper restored | 0.5 s |
| `match-within-limit` | `test_groups_within_their_limit_are_untouched` | | one group `within_limit`, one `tuned` | 0.5 s |
| `match-incomplete` | `_lanes_board(TWO_LANES)` without LANE_B's rules | | an `incomplete` group | 0.5 s |
| `match-breakout-regions` | `test_tuned_copper_of_breakout_nets_follows_the_breakout_regions` | | tuning next to necked breakout copper | 0.6 s |
| `match-bumps-outside-breakout` | `test_tuning_bumps_stay_out_of_breakout_regions_where_there_is_room_elsewhere` | | bumps kept out of breakout regions | 0.5 s |
| `match-serpentine` | `test_serpentine_tuning._serpentine_lanes` between walls | | serpentine-style final tuning (R10) | 0.5 s |
| `bundle-tuning` | `test_bundle_tuning._bundle(HEMMED)` | 4000 | tuning while routing in a bundle (R12), two `routing` steps | 1.7 s |
| `group-tuning` | `test_group_tuning._group_board()` | | a tuning group bent together while routing (`_tune_group`, R14) | 0.5 s |

Pair terminal preparation (`pair_terminals`), single-ended guide vias and
package-access reservations. "cleared" guides are the tests' failed coarse
guides without accesses or copper; "unconnected" ones keep their accesses.
The `reserved-*` inputs also save `reserved_accesses`: the bypass-only
`FanoutResult` that route-board's critical stage receives
(`flow._bypass_copper_reservations` of `route_decouplers`).

| File | Source test/helper | `--pair-states` | Reaches |
|---|---|---|---|
| `terminals-crossover` | `test_local_pair_branches.fixture()` | | a matched local contact crossover (`_paired_local_contacts`, `matched_contacts`) |
| `terminals-crossover-rejected` | the same with `max_vias=0` | 1 | a `blocked` crossover: the preparation's reason replaces the terminal diagnostic |
| `terminals-crossover-support` | `test_crossover_with_support...`, ESD at x=23 | | a crossover joined with a nearby support (`_surface_pair_terminals` on the trial board) |
| `terminals-crossover-support-remote` | the same, ESD at x=18 | | an `unsupported` preparation |
| `terminals-support-joint` | `support_fixture()`, unconnected guides | 6000 | `plan_pair_supports` with surface support completion (`joint_support_contacts`) |
| `terminals-support-joint-tiny` | `support_fixture()` | 1 | pending support contacts kept on a failed pair |
| `terminals-support-remote`, `-ambiguous`, `-no-contract` | `test_support_contacts_require_one_source_declared_local_endpoint` | 6000 | no support plan; an `unsupported` preparation |
| `terminals-joint-returns` | `test_joint_supports_cross_layers_with_real_returns_or_roll_back(1.5 mm)` | 6000 | transition support completion with return vias |
| `terminals-joint-returns-far` | the same with 0.1 mm | 6000 | every return site rejected by support completion (Python 4 min) |
| `terminals-fallback`, `-duplicates` | `test_pair_terminal_fallback.fixture()`, cleared guides | | source terminals; same-package branch ties (`surface_contacts`) |
| `terminals-fallback-duplicates-tiny` | the duplicates board | 1 | ties under a one-state budget |
| `terminals-support-branches`, `-tiny`, `-remote` | `support_branch_fixture()`, cleared guides | 20000, 1, - | nearby support branches, joined variants (`joined_terminal_variants`); a remote third component |
| `terminals-hybrid-anchor`, `-renamed` | `test_pair_hybrid_contact_anchor.hybrid()` | | a plated joined contact as the main endpoint |
| `terminals-macro-ports`, `-coarse` | `test_partial_pair_macro_ports.bound_pair`, cleared / coarse guides | | the exact search from proved macro ports (`macro_terminal_guides_proved`) |
| `single-guide-via`, `-budget` | `test_critical_transition_without_special_technology_is_through_via` (and `max_vias=0`) | | a single-ended guide via (`_guide_via`) |
| `reserved-bypass` | `test_bypass_checkpoint_replays_only_after_unique_edge_restore_and_fresh_admission` | | a reservation decoded, restored and admitted (no critical net) |
| `reserved-crossface` | `test_formal_critical_pair_and_retained_clock_admit_only_complete_crossface_prefix` | 6000 | a pair and a clock routed over a cross-face bypass reservation |
| `reserved-crossface-forged` | the same with a forged extra via | 6000 | `invalid package-access bypass reservations` |
| `reserved-short` | the same with an ordinary reservation track across both pair lands | 6000 | a failed reservation preflight (`<package-reservations>`) |

No synthetic input reaches a tuned `tune_virtual_pair_spans`, the
clearance-aware retry `pair_terminals._matched_local_candidates`, or twin
bumps beside vias: their tests drive them with hand-made copper or patched
candidates, which a replay input cannot carry.

`python tests/rust-port/critical/synthetic.py --unified` writes unified-router
inputs under `tests/rust-port/unified/synthetic/` (`save_inputs_path`):

| File | Source test | Replay with | Reaches |
|---|---|---|---|
| `bus-skew` | `test_tuned_bus_copper_survives_route_smoothing` (saved with `buses_first=true`) | nothing, or `--set route_smoothing=true` | `_tune_bus_skew` lengthening A by 6 mm |
| `bus-skew-untunable` | the same board without `tuning_amplitude_limit` | nothing | the bus skew note on the board |
| `bus-bundles` | `test_bus_bundles_route_the_nets_between_the_bus_parts_with_the_bus` (saved with `maximum_iterations=5`) | `--set bus_bundles=true` | a bus phase of three nets (C joins the A/B group) |
| `return-vias` | `test_negotiation_uses_existing_nearby_returns_without_adding_or_dropping_them` (only `S`) | nothing | a single-ended net crossing a wall through vias beside existing return vias |
| `return-vias-missing` | `test_missing_returns_do_not_silently_allow_signal_transitions` | nothing | no via proposal without a return barrel |
| `technology-blind` | `test_routed_blind_vias_follow_the_declared_technology` | nothing | blind F.Cu-In1.Cu vias from a via catalog (`_via_transitions`, `_site_spans`) |
| `technology-through` | the same with `allowed_layers=(F.Cu, B.Cu)` | nothing | through vias selected from the catalog |
| `technology-none` | `test_a_layer_change_no_technology_serves_is_rejected_with_its_reason` | nothing | `no_via_technology for allowed layer transitions` |
| `technology-blind-span`, `-blocked` | `test_blind_vias_check_only_their_copper_span` | nothing | blind vias past a B.Cu via block; none past an In1.Cu one |
| `technology-catalog` | `_four_layer((THROUGH, BLIND, BACK_BLIND))` | nothing | a three-technology catalog (no layer change needed) |
