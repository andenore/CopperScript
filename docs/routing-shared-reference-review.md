# Full-vertical routing closure — 2026-10-06

## Result and provenance

The complete shared Make pipeline at clean commit `52adb05` exits **0**.
All **49 ordinary nets** and **eight critical nets** connect. Detailed resource
overflow is **0**; native KiCad 10.0.6 refill/all-severity DRC reports **0
violations and 0 unconnected items**. The saved board has no native library
findings or dangling vias. All 80 requested GND pad contacts are established.

This run includes the emitted-copper resource correction at `fa08aaa` and the
explicit shared-reference policy at `52adb05`. It does not rewrite the earlier
[failed run](routing-domain-access-review.md). Removing one return via changes
the routing problem, so this is not an isolated algorithm-speed benchmark.

Ignored local evidence is in `build/full-vertical-shared-reference-20261006/`:
`run.json`, `routing.log`, `phase-timings.json`, `route-report.json`, saved
`board.kicad_pcb`/`.kicad_pro`, `kicad-drc.json`, and `layers/` with six SVG/PNG
plots and `layer-review.json`. The saved input PCB SHA-256 is
`afa543742853dd4afd8170020bd3149533679401a61e785945251ceffadc80d8`.

The run uses CPython 3.13.1, **PROFILE=none**, locked/offline dependencies,
the unchanged 100 x 80 mm outline, six-layer JLCPCB profile, placement templates,
clearances and default no-via/pad-overlap policy. No blanket via-in-pad, extra
power pour, relaxed fabrication rule or ignored DRC finding is introduced.

Reproduce the inputs/options after the README's checkout/dependency setup:

```powershell
make EXAMPLE=full-vertical route PROFILE=none `
  RESOLVE_ARGS="--locked --offline" `
  RUN_DIR="build/full-vertical-shared-reference-rerun"
```

Choose a new/empty `RUN_DIR`; outputs depend on the environment and are not
promised byte-identical. The recorded invocation used the repository's `.venv`
Python and installed KiCad 10.0.6 footprint root. `run.json` retains the exact
commands, clean source revision, input hashes and saved filled-board hash.

## Ground and critical ownership

Package preflight reserves 104 ordinary exits and all 80 requested GND contacts,
with no pending pad or hard finding. Critical negotiation accepts all four RF
groups and both USB pairs before ordinary area routing. No late ground/placement
feedback is needed.

The modem USB pair has two matched F.Cu/In2.Cu transitions, four signal vias,
two reported shared-reference decisions and no redundant GND return via.
The MCU USB pair stays on the surface. Both pairs explicitly declare In1.Cu as
their common reference; the default mandatory-return-via policy is unchanged.
Neither this declaration nor zero native opens qualifies 90-ohm impedance,
via stubs, high-frequency return continuity or USB/RF performance.

The explicit-copper IR DRC still records `DRC-OPEN-NET` for GND and
`DRC-ROUTE-INCOMPLETE` before fill: its polygon is **zone intent**, not copper.
GND is deferred, not waived. Independent native refill verifies the actual
plane; the shared runner then saves that filled copper and checks it again.
The final report has `routing_complete=true`, but `fabrication_ready=false`.
That distinction and the raw unfilled-stage report are retained.

## Layer review

The native whole-annulus audit finds **zero via/pad contacts**, including
same-net contact. All track segments are straight or exactly 45 degrees.
Sharp endpoint-turn counts are candidates, not proven removable bends; contacts,
branches, owned package escapes and constrained corridors need separate checks.

| Layer | Track length (mm) | Sharp-turn candidates | Off-angle segments | Peak 10 mm track-area estimate |
| --- | ---: | ---: | ---: | ---: |
| F.Cu | 880.331 | 35 | 0 | 10.45% |
| In1.Cu | 0 (filled GND plane) | 0 | 0 | Not a track-density measure |
| In2.Cu | 1,347.424 | 94 | 0 | 15.18% |
| In3.Cu | 52.013 | 2 | 0 | 2.56% |
| In4.Cu | 44.042 | 0 | 0 | 2.70% |
| B.Cu | 101.981 | 1 | 0 | 4.30% |

Visual inspection of all six saved-copper plots shows F.Cu dominated by package
access and local connections; In1.Cu is the filled GND plane. In2.Cu carries most
long runs and central detours; In3.Cu/In4.Cu/B.Cu are sparse. Density is only a
visualization heuristic. Sparse layers are not automatically equivalent
reference-plane capacity, and this review does not establish signal integrity.

## Unprofiled phase evidence

Runner wall time is **1,807.862 s** (about 30.1 minutes), essentially unchanged
from the preceding 1,807.29 s failed-gate run. Do not claim a speedup. Nested
ordinary routing and repair take 1,570.374 s and 1,157.908 s respectively; these
inclusive durations must not be added. Progress events are complete and valid.

There are 201 detailed attempts: 98 pass searches, 10 soft merges, 10 soft rip-up
proposals, 77 displaced-net searches and six final strict retries. All six final
retries connect. Fifty-three attempted searches fail, but none is a final open.
Branch-checkpoint resume count is **zero**, so it did not cause this closure.

Displaced-net searches consume 842.491 s. `V5` alone accounts for 20 attempts,
375.459 s and 19 failed trials; `USER_LED_2_A` has 11 attempts and seven failures.
Repeated transactional restore searches are a concrete next optimization target.
Failed trials continue to restore the accepted board, not publish tentative copper.

After the full export, the shared-build/manufacturing regression batch passes
**51 tests**, with one Linux-only default-path test skipped on Windows. GNU Make
integration uses the explicitly located installed executable. These focused
checks complement the feature's 176-test batch; they are not a claim that the
separate repository-wide CAM installation/CM4 fixture failures are resolved.

## Manufacturing files, not qualified production release

After the routing/native gates pass:

```powershell
uv run --no-sync python -m copperscript export-manufacturing `
  "build/full-vertical-shared-reference-rerun/board.kicad_pcb" `
  --kicad-cli "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe" `
  --skip-independent-cam `
  -o "build/full-vertical-shared-reference-rerun/manufacturing"
```

The recorded export succeeds, refills a staged copy and again has zero native
violations/opens. `manufacturing/gerbers-drill.zip` contains all six copper
layers, both masks/pastes/silkscreens, Edge.Cuts, and plated/nonplated drills:
15 artwork/drill files. `manufacturing-package.zip` has 55 entries including
the PCB/project, managed footprint library, DRC, IPC-D356, native positions,
manifest and checksums. All 54 checksum rows were independently rechecked.
The original saved board is unchanged.

As requested, independent geometric CAM is skipped. The manifest explicitly
records `qualified_release=false`, `supplier_availability=not_checked` and
`assembly_bom_included=false`. No reviewed BOM/JLCPCB CPL is invented. The
[electrical/support-circuit and fabrication qualifications](full-vertical-example.md#current-blockers-before-fabrication)
remain; these files are not an order approval or an assembled-board guarantee.

## Next improvements, outside this closure increment

- [x] Measure duplicate displaced-net states; bound any memoization to exact
  obstacles, rules, guides, anchors and search options. Never reuse a failure
  after the blocking geometry changes. A later rerun found 12 of 77 displaced
  searches (and four soft proposals) to be exact repeats; they are now reused
  by exact identity. Most `V5` failures are distinct states. See
  [exact repair-search reuse](repair-search-reuse.md).
- [ ] Improve ownership-safe corridor/layer choices and confirmed removable
  bends without losing the accepted clean board or its reference assumptions.
- [ ] Qualify concrete parts/support circuits, layer-specific impedance and
  assembly data before labeling this fixture a production-ready reference.
