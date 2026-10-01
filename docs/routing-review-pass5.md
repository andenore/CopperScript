# Pass 5: joint paired package escape and channel search

Electrical evidence superseded: this run used an incorrect USB choke winding
mapping. Its geometry measurements remain historical; USB wiring acceptance
does not. See [the source correction and fresh run](routing-review-pass11.md).

This is the R8b critical-stage rerun, not a complete board route. It retains
pass 4's source profiles, all 51 placements/rotations, footprints and six-layer
fabrication rules. The historical completed ordinary-routing board remains
untouched. Neither clearance nor electrical pin mapping was relaxed.

## Implementation

Non-aligned pairs now enumerate package exits jointly and search one oriented
spine with straight/45-degree successors. Member order is preserved through
offset-line-intersection corners, unequal pad pitches and fanout tapers.
Both lanes are tested together against foreign pads, keepouts, outline and
previously reserved critical copper. Actual package escapes are reservations;
an invented straight segment behind an exit must not block the first step.

The initial 1 mm search can miss the choke's narrow turning opportunity. The
critical stage retries at 0.5 and 0.25 mm, bounded to eight port combinations
and 30,000 expanded states per search at each pitch. Failed searches retain
their telemetry. Candidate acceptance still rechecks original length/skew/
uncoupled budgets and fresh native DRC atomically. A failed pair contributes
no copper; its members never fall back to independent ordinary-net routing.

Coupled length uses unioned projected parallel overlap, including unequal
mitered edges and split segments. A few nanometres of integer diagonal
quantization tolerance is measurement tolerance, not a clearance waiver or
electrical qualification. New paired layer transitions, non-octilinear pin rows
and multi-terminal pairs remain unsupported by this search. Explicitly allowed
45-degree component rotations are covered by a deterministic regression.

## Real-footprint result

Artifacts: task outputs `routing-review-pass5-r8b-2026-10-01`, specifically
`refined-preflight.json`, `refined-board.kicad_pcb`, its project, the disposable
refilled `verified-board.kicad_pcb`, `critical-kicad-drc.json` and
`verified-geometry.json`. The earlier diagnostic/short-port reports are not the
accepted result. Settings match pass 4: locked/offline, real KiCad/CopperLib
footprints, 100 x 80 mm, JLCPCB six-layer prototype profile, candidate-01,
one placement-feedback iteration, five global iterations, 5 mm global tiles.

| Pair | Lengths | Skew | Coupled length | Uncoupled lengths | Searches / states |
| --- | --- | --- | --- | --- | --- |
| USB MCU | 35.90 / 33.55 mm | 2.35 mm | 32.18 mm | 3.72 / 1.37 mm | 1 / 808 |
| USB modem | 35.35 / 36.61 mm | 1.26 mm | 32.54 mm | 2.81 / 4.07 mm | 21 / 22,986 |

Both pairs connect using joint search, with one emitted candidate each and no
vias. The four RF nets retain pass 4's paths: Nordic antenna tree 55.22 mm,
Nordic raw matching link 25.74 mm, cellular feed 6.85 mm, GNSS feed 34.31 mm.
Accepted copper totals 344 F.Cu track segments, zero vias, with all 471 pads.
Many segments are collinear search steps, not distinct bends. Segmentation is
not a wirelength improvement or regression by itself.

Native DRC finds only 50 ordinary/ground open nets and incomplete full routing;
there are no critical opens or hard geometry errors. Independent KiCad 10.0.6
refills a disposable copy and reports 195 expected partial-board unconnected
items, eight unchanged footprint-library findings (four issues/four mismatches),
zero shorts, clearance or dangling-track violations, and no unconnected items
on any of the eight critical nets. Ordinary nets have not been routed here;
195 cannot be compared as a completed-board regression against pass 3.

Phase timings: resolve 0.27 s, placement/global 140.04 s, critical 66.00 s.
Concurrent tests were running; these are operational observations, not a
controlled benchmark. Source and global/critical/native-board fingerprints
are retained in the report. No filled-zone result is an impedance certificate.

All 331 tests pass in the isolated Python 3.12 runtime, with 459 upstream
PyGerber warnings and installed-KiCad checks enabled. Six additions since pass
4 cover staggered paired terminals/pitches, split-edge coupling, bounded
failure/input preservation, permitted 45-degree package rotation, miter
clearance and the fictitious behind-port obstruction. Existing profile-budget,
atomic rejection, immutable reservations and paired repair-protection tests
continue to pass.

## Remaining work

R8c is next: enforce vendor RF reference-layout/matching clusters and antenna
keepouts, especially the long Nordic and GNSS paths. These routes are connected
but not qualified RF designs. USB widths/gaps, skew and uncoupled portions are
measured, not accepted signal-integrity limits; field-solver/stackup/return-path
evidence is still absent. The current F.Cu-only USB profile is provisional.

After that placement work, repeat the complete ordinary-routing workflow and
independent layer/connectivity review (R8d). Eight footprint-library findings,
fabrication and independent CAM signoff remain unresolved. This pass does not
produce production Gerbers or complete R8 overall. R6a/R6b, R5, R7 and full-run
operational checkpointing remain on the working list.
