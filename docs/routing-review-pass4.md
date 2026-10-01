# Pass 4: explicit critical profiles and safe early acceptance

This is an R8 integration/preflight pass, **not** a complete board reroute.
The pass-3 ordinary-routing board and its zero KiCad opens remain untouched.
Input intent deliberately changes: eight explicit USB/RF routing profiles are
now in the acceptance source. No package/footprint bytes or fabrication limits
were changed. The geometry choices are provisional, not qualified impedances.

## Implementation

Critical groups commit atomically after fresh native geometry/connectivity
checks. Pair self-shorts, foreign pads, board edges, keepouts, drill spacing,
profile budgets and earlier reservations cannot become locked obstacles.
Rejected tracks/vias are absent from the returned board. Reports retain
candidate lengths for diagnosis but accepted object counts are zero.

Aligned pairs use a midpoint derived from both physical terminals, with
symmetric 45-degree tapers, replacing the unsafe one-member offset/stub case.
Single-ended critical routes may retry using the exact octilinear maze engine,
bounded to two passes / 20,000 states per search, against immutable earlier
critical copper. The original profile and native DRC are rechecked afterward.
Pairs never use this independent-net fallback.

Subset repair cannot reroute/prune paired copper; duplicate-land helpers leave
new critical bridges pending for their owning router. Tests cover actual
geometry, rather than accepting a synthetic return via placed between two
signal vias with overlapping annuli. Evidence-digest fixtures are synthetic
test inputs, not impedance qualification for this example.

The separate `python -m pcbir.critical_preflight` entry point runs the usual
initial placement/global/critical stages, checkpointing global results before
critical routing. It does not run ordinary copper, plane stitching, feedback or
manufacturing. A passing early gate never implies production readiness.

## Real-footprint result

Artifacts: task outputs `routing-review-pass4-r8-2026-10-01`, including
`critical-preflight.json`, the disposable `critical-board.kicad_pcb` and
`critical-kicad-drc.json`. Options: locked/offline source, installed KiCad and
CopperLib footprint roots, 100 x 80 mm, JLCPCB six-layer profile, candidate-01,
one placement-feedback iteration, five global iterations, 5 mm global tiles.
The ordinary-route options are irrelevant because that stage is not executed.

| Critical group | Result | Length | Vias | Strategy |
| --- | --- | --- | --- | --- |
| NRF_RF_ANT (tree including matching capacitor) | Connected | 55.22 mm | 0 | Exact single-net |
| NRF_RF_RAW | Connected | 25.74 mm | 0 | Exact single-net |
| CELL_RF | Connected | 6.85 mm | 0 | Global-guide candidate, validated |
| GNSS_RF | Connected | 34.31 mm | 0 | Exact single-net |
| USB MCU pair | Rejected | Candidate only | 0 committed | Coarse-guide pad/copper conflicts |
| USB modem pair | Rejected | Candidate only | 0 committed | Coarse-guide pad/copper conflicts |

Accepted copper: 32 tracks, all F.Cu, zero vias. Long Nordic/GNSS connections
are a placement/reference-layout problem, not RF-qualified routes. Choke/MCU
terminals have different midpoints and pitches; the current aligned-pair fast
path cannot resolve their package escapes. Coarse offset fanout crosses foreign
pads and the other member. Both whole pairs are rejected without copper.

Independent KiCad 10.0.6 refill of this disposable partial PCB finds eight
unchanged library violations (four issues / four mismatches), no short,
clearance or dangling-track violations, and 199 unconnected items from the
unrouted board. This is expected partial-stage evidence, not a regression
comparison against pass 3's complete ordinary board. The four accepted RF
nets have no KiCad unconnected findings. No errors were suppressed.

Measured phase times: load/resolve 0.26 s, placement/global 114.13 s, critical
19.82 s. Concurrent tests were running; these are operational timings, not a
controlled performance benchmark. Source hash and global/critical/native board
fingerprints are retained in the report. The exported partial artifact was
refilled by KiCad; no filled-zone result is treated as an impedance certificate.

All 325 tests pass in the isolated Python 3.12 runtime, with 459 upstream
PyGerber warnings. Twelve regressions were added since pass 3, including
aligned-pair shape/actual DRC, unsafe pair rejection, bounded tuning, paired
repair preservation, exact single-net repair against foreign pads and earlier
reservations, critical-land protection, source-profile lowering, and preflight
checkpoint/export behavior. Installed-KiCad optional tests remain enabled.

## Next acceptance work

R8 remains open. Next is joint paired package-access search with terminal-order
preservation, orientation-aware channels and exact two-member corner/fanout
checks, followed by bounded critical-cluster placement alternatives. Do not
convert failed pairs to ordinary nets or weaken clearance to make a rerun pass.
Then enforce the vendor RF matching/antenna layout and return-path requirements,
and repeat the complete six-layer route/independent review. R6a/R6b, R5, R7 and
the main full-route operational checkpoint work remain subsequent todo items.
