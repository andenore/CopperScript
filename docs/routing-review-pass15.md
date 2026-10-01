# Pass 15: escape-pressure ordering

This is an R6a/package-escape increment after `df75f6a`, not a completed-board
rerun. R16's matched replay identifies blocked MCU exits; increasing the maze
budget or shifting layer preference cannot repair a missing terminal exit.

## Implementation

Before reserving ordinary fanout, enumerate every pin's bounded radial
track/via domain against the same immutable board. Sort pins by legal candidate
count, then neighbor spacing/reference; retain deterministic outward/radius
candidate preference. Recheck each alternative against previously installed
escapes, with the original exact track, physical-span via, drill and board-edge
rules. Existing matching vias remain reusable; no via-in-pad technology is
inferred. The unchanged final native gate rolls back the entire proposal on a
new hard manufacturing violation. Input copper/poses are never moved.

`FanoutOptions.constrained_pins_first` defaults to true; false is an experimental
nearest-spacing/reference-order control, not a compatibility mode.
`route_fanout(..., only_nets=frozenset(...))` optionally scopes new escapes while
retaining every other net as an immutable obstacle. Unknown names fail; an
empty subset is a no-op. This does not grant ownership to remove existing
fanout or detailed copper.

`FanoutResult.pin_analysis` and route-board's `fanout.pin_access_analysis` record
initial legal candidate count, selected candidate index and a diagnostic:
zero immutable domain, candidates consumed by selected escapes, or whole
proposal rejection by native DRC. These are pin-access observations, not proof
of onward connectivity or manufacturing readiness.

Seven regressions cover a competing-site trap, real candidate generation with
two geometrically available via windows, determinism, protected critical/zone
nets, subset scope/input identity, locked/no-domain cases, native rollback and
report-only geometry identity. The real-window test does not mock clearance or
candidate generation. Existing fanout via-reuse, cleanup, drill-spacing,
detailed-routing and CLI tests remain exercised.
Final full suite: 459 passed, 459 upstream dependency warnings. After extending
the CLI integration assertion to exercise `--fanout` and its analysis JSON,
the updated CLI/pressure regression group passes all eight checks separately.

## Matched checkpoint result

Use all 51 saved pass-13 poses and identical global guides. Remove ordinary
signal copper only in a disposable experimental board; retain every critical
track and ground escape/via, including the late ground contact. Thus this is
not an exact replay of the original early-fanout stage, nor a repair accepted
onto the completed ordinary board. Compare the committed previous fanout with
the new default at the same 0.5 mm radial step and 3 mm radius.

| Measurement | Previous order | Low-slack order |
| --- | ---: | ---: |
| Escaped crowded pins | 68 | 69 |
| Pending crowded pins | 8 | 7 |
| Added through-vias | 68 | 69 |

PWR/U_MODEM.14 now escapes. No prior escaped identity is lost. All placements,
critical copper and ground copper are unchanged. Native introduces no hard
spacing/copper/keepout findings. Fresh KiCad 10.0.6 refill finds eight library
findings, 69 dangling fanout vias and 95 unconnected items: exactly the expected
**partial fanout** stage, not routed-board signoff. It reports no shorts,
clearance or drill-spacing violations. These counts must not be compared to
the full ordinary board's 34 opens as a routing regression or improvement.

The diagnostics show zero initial radial domains for U_MCU.62/.64, U_MCU.45,
U_ACCEL.2 and U_MODEM.13; U_MCU.61/.39 instead lose initially legal domains to
other selected escapes. The important UART/enable exit failures therefore
precede ordinary detail allocation and are not solved by ordering alone.

## Next

R17: expand the *escape representation* to bounded two-leg straight/45-degree
paths and correctly verify/own their multi-segment anchors. Analyze them against
immutable geometry, then consider bounded alternative allocation/owned escape
or placement feedback if needed. Do not relocate critical/GND copper implicitly,
infer via-in-pad, or treat a legal candidate as a routed net. Full rerouting and
independent filled-zone/layer review must follow before any closure claim.
R5/R6b/R7/R9 and production qualification remain open.

This decomposition follows the separate pin-access/assignment/search-and-repair
owners described in [OpenROAD's detailed-router documentation](https://openroad.readthedocs.io/en/latest/main/src/drt/README.html).
Our low-slack radial heuristic is a bounded PCB implementation choice, **not**
a claim to reproduce TritonRoute's pattern optimizer or best-known PCB routing.
Joint conflict-graph/matching and negotiated replacement remain tracked in the
[package-escape work list](global-routing-and-escape-todo.md).

Evidence: task outputs `routing-review-pass15-fanout-pressure/compare.py`,
`comparison.json`, matched `global.json`, old/new native reports and KiCad
artifacts, plus `pressure-drc.json`. The original fully routed pass-13 artifacts
remain untouched; this pass does not publish a replacement finished board.
