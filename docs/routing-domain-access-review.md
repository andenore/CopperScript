# Domain-aware placement and early access: complete rerun

## Outcome

The unprofiled six-layer full-vertical run at `56c3c15` connects all **49 ordinary
nets**, all eight critical signal nets, and every GND pad after independent
KiCad 10.0.6 refill. There are zero native unconnected items, shorts, clearance
or drill-spacing violations, and zero via/pad overlaps. This resolves the prior
`V3V3` and `PWR/R_MODEM_EN_PD.2` connectivity blockers.

The build nevertheless exits **1** (`unmet_gates`), `routing_complete` remains
false and `fabrication_ready` remains false. One native `via_dangling` warning
and one reported detailed-resource overflow are outstanding. A connected board
is not a clean routing/manufacturing release. Neither outstanding finding is
waived and no Gerber/assembly/manufacturing files were generated in this run.

## Reproduction and evidence

The shared recipe was used from a clean tracked checkout, with the unchanged
source, pinned/offline CopperLib inputs, six-layer JLCPCB rules and the existing
placement templates. The build used CPython 3.13.1 and KiCad 10.0.6. The only
Python override selected the existing local virtual environment.

```powershell
make EXAMPLE=full-vertical route PROFILE=none `
  RESOLVE_ARGS="--locked --offline" `
  RUN_DIR="build/full-vertical-domain-aware-20261006"
```

Choose a new/empty `RUN_DIR` when repeating: the runner deliberately refuses to
overwrite old evidence. Set `PYTHON` to the desired interpreter, or use the
default `uv run --no-sync python` after the README setup.

Retained ignored outputs:

- `build/full-vertical-domain-aware-20261006/run.json`, `routing.log`,
  `phase-timings.json` and `route-report.json` describe the original run.
- `board.kicad_pcb` and its generated project/library are the original unfilled
  export. The original files and run status were not edited after the run.
- `verified.kicad_pcb` / `.kicad_pro` are independently saved/refilled copies;
  `kicad-drc.json` checks that exact saved copy with all severities enabled.
- `layers/` contains the six saved-copper SVGs and `layer-review.json` from the
  repository's read-only native KiCad audit.

The independent command used `--refill-zones --save-board --severity-all
--exit-code-violations`. It also exits nonzero on the remaining warning; it is
not treated as a passing native check. The existing generated-project check
policy was unchanged; enabling every severity does not enable ignored check
types. The raw board/export/fill digests remain recorded in the original report.

## Routing work

The joint preflight reserves 104 ordinary exits, including both level-shifter
power pins, and prospective contacts for all 80 requested GND pads. Its
`critical_first` alternative reconnects both USB pairs and all four RF groups
without dropping ordinary exits or introducing hard findings. The selected
placement is `candidate-00`, as in the previous shared run.

`V3V3` routes on the first ordinary attempt. The two initial passes leave ten
and thirteen ordinary failures respectively. Transactional merging/rip-up and
six final strict retries close the remaining nets without accepting a swap
which loses an incumbent net. The final higher-budget retries connect GNSS RX,
GNSS time-pulse, GNSS TX, both USB-C CC signals and the second current-status
signal. No late placement/ground feedback trial is needed.

The runner records **1,807.29 s wall time**, with function profiling disabled.
There are 202 completed detailed attempts, including 78 evicted-net attempts;
53 attempted searches fail, but none is a final ordinary open. Inclusive phase
timings overlap: ordinary routing is 1,549.677 s and its repair phase is
1,129.422 s. These times must not be added. The branch-checkpoint resume count
is zero in this run; do not credit the measured outcome to branch reuse.

The previous unprofiled run retained 48/49 ordinary nets and one GND contact
failure after about 9,045 s. This new run is an observed closure improvement,
not an isolated kernel-speed benchmark: early contact ownership, small-package
eligibility and placement planning all changed together.

## Per-layer audit

All tracks are straight or exactly 45 degrees. The native whole-annulus audit
finds zero via/pad contacts, including same-net contacts. Track lengths below
include GND surface contacts; the F.Cu signal-only length is 712.721 mm.

| Layer | Total track length (mm) | Sharp endpoint-turn candidates | Off-angle segments |
| --- | ---: | ---: | ---: |
| F.Cu | 904.052 | 38 | 0 |
| In1.Cu | 0 (filled GND plane) | 0 | 0 |
| In2.Cu | 1,404.676 | 82 | 0 |
| In3.Cu | 27.899 | 0 | 0 |
| In4.Cu | 54.113 | 0 | 0 |
| B.Cu | 116.683 | 1 | 0 |

Sharp-turn candidates are degree-two endpoint direction changes of 90 degrees
or more, not confirmed removable corners. They include owned package-access
copper, via contacts and obstacle-constrained paths. In2.Cu remains the dominant
signal layer; spare-looking inner layers are not proof of interchangeable
reference-plane capacity. Review onward port/layer choices and ownership-safe
corner cleanup rather than changing copper or relaxing clearances manually.

## Remaining closure findings

1. **USB return-via contact.** KiCad identifies the GND through-via at
   `(39.000, 17.545) mm`, beside the modem USB transition pair at approximately
   `(38.555, 18.345)` and `(39.445, 18.345) mm. It is connected on only one
   copper layer. The source explicitly requires return vias, so blindly
   removing it would weaken the intent. The nearest existing surface-ground
   contact is about 6.90 mm away; this is not a missing pad escape and is not
   solved merely by repeating the 5 mm pad-contact stage. Check shared-plane
   reference semantics and real surface/reference connectivity before choosing
   a generic repair. Do not add arbitrary copper solely to suppress a warning.
2. **Reported resource overflow.** Detailed metrics contain one conflict despite
   no native copper collision. At the run's revision, `detailed._route_net`
   collected resource keys before `chamfer_ordinary_corners` changed the emitted
   path. Its retired-tail filter also retained both original edge endpoints if
   any part of an edge survived. These could retain obsolete occupied vertices.
   The precise conflicting key was not serialized, so attribution of the
   full-run overflow remains a hypothesis, not a located board-level conflict.

## Subsequent resource-accounting correction

Resource accounting now follows stub pruning, collinear merging and corner
chamfering inside each successful tentative net attempt, before its copper and
occupancy are installed for the next net. This is not an end-of-board cleanup.
The ledger retains only surviving original nodes, fully covered original edges
and layer transitions supported by actual via spans. Fully retired tails do
not even query occupancy; split collinear coverage retains live edges, but gaps
and partly cut edges do not reserve a whole obsolete edge. Exact clearance
checks still cover the emitted diagonals and vias; resource hints do not replace
physical DRC.

A deterministic regression reproduces the bug: a chamfer removes a corner,
exact copper/drill checks allow another net's via at the former vertex, yet the
old maze ledger claims a shared node. The corrected ledger no longer does.
Additional cases cover live-node conflicts, partial tails, split and gapped
coverage, reversed diagonals, actual via spans, and the per-net commit order.
The existing retraced-access and physical-search-loop regressions also pass.

Current-source verification passed **187 distinct focused tests** without
profiling: 37 accounting/cleanup/checkpoint/closure tests (including ten new
accounting cases), 37 detailed-router tests, 51 style/package-access tests, and
62 reporting/guide/boundary tests. Python compilation and the normal repository
`git diff --check` also pass. This is focused regression coverage, not a claim
that the earlier complete-suite environment/fixture failures are resolved; see
the [validation record](power-domain-routing-plan.md#validation--2026-10-06).

The full-board run above predates this correction. Its recorded overflow and
failed status have **not** been rewritten, and the board has not yet been
fully rerouted with the correction. The USB return-via warning is separate and
is not resolved by changing the occupancy ledger.

The preflight DRC previously contained 104 dangling-via warnings: 103 ordinary
fanout vias and this one GND via. Ordinary routing/ownership cleanup resolves
the ordinary ones; the remaining GND warning was already present at preflight.

See the [next closure checklist](power-domain-routing-plan.md#next-closure-work).
Manufacturing export remains blocked until the corrected complete flow passes.
