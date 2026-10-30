# Compact LLM design and verification workflow

Start with `copper agent-guide` and request `syntax`, `libraries`, `verification`
or `recovery` when needed. Search the consuming project's declared dependencies,
then retrieve selected contracts; see [library context](library-context.md).
Guidance and discovery metadata do not qualify a design.

## Source feedback

```console
copper check board.copper --json
copper explain UNKNOWN_PIN
copper query board.copper pins U1
copper query board.copper unconnected
```

`check --json` returns located findings, hints and machine-applicable fixes; apply
a `fix` only when its range still holds the original text. Declare deliberately
open pins with `no_connect` so `UNMARKED_PIN` separates omissions from decisions.
`query` verifies intent (pin-to-net, decoupling, open pins) from the compiled
design instead of re-reading source. See [CLI workflow](cli-workflow.md).

## Routing and placement iteration

```console
copper issues build/BOARD/runs/STAMP                  # grouped causes with ids and windows
copper view build/BOARD/runs/STAMP --issue R1         # one picture per relevant copper layer
copper view build/BOARD/runs/STAMP --issue N3 --filled
copper view build/BOARD/runs/STAMP --issue R1 --reference REF.kicad_pcb --align U1:U1
```

`issues` reads the run's report and board. Unrouted nets are grouped by reason
(`R`); native gaps (unconnected items) per net and place, other native
kinds (dangling copper, clearance) per type (`N`). Connected copper over a declared
length or skew budget is a `review` issue (`B`), not an open: decide by
placement or manual review. Layout reports list failed gates (`L`)
and non-passing findings (`F`). `--cluster-mm` changes grouping and ids, so
pass the same value to `view`. Lists are balanced across stages; use
`--stage`, `--net`, `--code` or `--limit 0`. `--json` emits
`copperscript.run-issues/v1`, including the SHA-256 of the run's board and
refilled board; CopperReview's **Load issues** draws those issues on the
matching board so review notes can cite their ids.

`view` draws pads, tracks, vias and copper areas in a millimetre window, one
panel per copper layer (`--layers auto` keeps layers with highlighted copper),
rings issue positions and labels parts and pads in small windows. It prints a
legend (pads, vias and tracks of the highlighted nets, marker positions) that
often answers the question without opening the image. `--reference` overlays a
second `.kicad_pcb` as magenta outlines, aligned on the pads of `--align
OURS:THEIRS` with mirroring tried automatically; the legend reports matched
pads, rotation and fit RMS. Targets may be a run directory, a report, a
`.kicad_pcb` or a `copperscript.state/v1` file holding a `PhysicalBoard`.
Native issues are drawn on the run's refilled board when present. When an
issue's markers spread wider than 40 mm, the densest group is framed and the
legend says so. Trapezoid and custom pads are drawn as their anchor rectangle.

### Shorter reruns

Placement and global routing can take a large part of a run, and an early
blocker (a package exit that cannot be reached) is known long before the
detailed stages finish. Stop at the stage you are fixing and keep placement
fixed while you change later stages:

```console
copper route-board BOARD --progress --stop-after placement --checkpoint-dir CK -o RUN/board.kicad_pcb --report RUN/route-report.json
copper route-board BOARD --progress --reuse-placement CK/placement.state.json ...
```

`--stop-after` accepts `placement`. A stopped run writes its report and
draft board, is marked `stopped_after`, skips filled-copper and
final native verification, and never passes; its open nets read "run stopped
after ...". `copper issues` shows the stop in its header.

`--checkpoint-dir DIR` saves `DIR/placement.state.json`, keyed on the physical
board, the placement/global/feedback options and the build identity of the
`copper` binary that wrote it (its version and a digest of its sources). A failed save is reported and the run continues.
`--reuse-placement` refuses a checkpoint whose board or options differ; a
checkpoint from a different `copper` build is allowed (that is the point of
holding placement fixed), and the report records it as `placement_reuse.code_changed`. Rerun placement
before signoff whenever placement code or inputs changed. A checkpoint
written by the retired Python CLI always counts as a different build.

The same directory also receives `critical-inputs.state.json` (and
`critical-retry-inputs.state.json` when the critical stage retries) with the
board and global route the critical stage starts from, and
`final-board.state.json` with the exported board. These diagnostic states serve
`copper state show`, `copper view`, `copper replay-critical` and
`pcbir.workbench` experiments. They are not accepted by `--reuse-placement`,
do not resume the routing pipeline and do not establish acceptance or signoff.
A failed save remains visible in the report's `checkpoints` field.

When `route-board --report RUN/route-report.json` fails with an error, it
writes `RUN/route-report.error.json`: error type, message, the components it
names and, with `--progress`, the stage that was running and those that
finished. `copper issues RUN` prefers it over an older report in the same
directory and lists it as `E1` with a window on those components; the next
successful run removes it.

Rejected geometry is also retained in a state file with a content digest. If native preview
export is unavailable (for example, a hard macro has not been materialized),
the primary error and state remain available, with the preview failure in
`diagnostic_errors`. Issues and views use the exact rejected state without
discarding immutable macro intent to make a preview.

For scripts, `pcbir.workbench` provides `load_run`, `run_issues`, `view`,
`scene_from_physical`, `save_state` and `load_state`. State files round-trip
`pcbir` dataclasses, enums, tuples, mappings, Decimals and Fractions exactly
(not plain classes or `init=False` fields), carry a content digest, and only
construct dataclasses and enums defined in `pcbir` modules. Views, issue groups and state files are reading aids and working
artifacts; they do not replace report gates, native verification or evidence.

## Report inspection

```console
copper summarize build/route-report.json
copper summarize build/route-report.json --json --issues 4
```

Saved `route-board` and `plan-layout` reports have same-stem `.summary.json`
sidecars. Summaries bind the full report's exact bytes and bound the issue list;
read the full report only for an identified issue or required omitted evidence.
`summarize` is read-only and exits 0 on successful reading even when the board
has failed gates; malformed/unsupported input exits 2. Older reports show missing
fields as `not_reported`. The issue cap defaults to eight and accepts 0–100.

## Requirement coverage

Routing reports retain their routing status and add `requirements_complete`,
`requirements` and `routing_quality`. Add `--require-verified-intent` to the
project's `route-board` invocation to require every hard declaration to pass,
as well as the existing routing gates. Layout reports block route-stage checks.
Check each requirement's status, provenance, measurements and explanation.

Supported checks include placement poses/orientations/alignment/distances,
footprint regions/keepouts, routing width, explicit length, via count, layers
and supported skew. Clearance coverage requires a current bound check at least
as strict as source intent. Original declarations cannot be weakened by a final
rule or display rounding. Unsupported complete pair-gap, return-policy, stub,
topology, tuning and impedance obligations remain blocked. External obligations
are hard blocked; assumptions retain `assumed`, and notes/groups are advisory.
Custom verifier names do not grant authority. Required-array geometry does not
establish fabrication-process qualification; hole findings are conservatively
attributed board-wide. No new waiver or external-evidence adapter is implied.

## Final-copper quality and acceptance

Metrics describe final tracks, vias, layers and angles. Surface bypass checks
use exact track contact, including interior junctions, excluding vias, polygons,
zone fills and component-internal joins. Connected-component length includes
branches; it is not a shortest bypass path or a universal electrical limit.
Reference coverage needs current export-bound native fill and an explicit net/
layer. Its conservative projected-envelope screen is not impedance qualification.

```console
copper benchmark-routing -o build/new-acceptance --kicad-cli /path/to/kicad-cli
```

Use a fresh output directory. Eight bounded two/four-layer cases include renamed/
rotated variants and intentionally blocked access. Authored synthetic lands are
test fixtures. Missing native tools make evidence incomplete; expected-open
cases remain open. Identities, digests, settings and expectations are recorded.
The suite is not a statistical success rate for arbitrary real boards.

Benchmark comparison reports final-copper tradeoffs. Increased measured length/
via costs block a neutral speedup claim; lost surface/reference/required-intent
evidence is a regression. Mixing measured and unmeasured reports blocks that
comparison. See [performance guidance](routing-performance.md).

These interfaces reduce the amount of material an agent must retrieve. Total
LLM token savings have not been measured. Remaining work is in
[the future plan](llm-workflow-improvements.md); engineering and prototype
qualification still require their own evidence.
