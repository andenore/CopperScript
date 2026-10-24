# CLI design and review workflow

For standalone physical assets, `copper compile-macro SOURCE.copper -o ASSET.json`
lowers a `macro` definition with generated footprint digests. `--locked --offline`
verifies cached dependency inventory. Boards can import and bind named Copper
macros directly without generating JSON. This does not establish physical or
hardware qualification; see [definition syntax and contracts](macro-definitions.md).

Read only the section needed for the current task. Commands use the repository
examples; consumer projects must supply their own configuration and pins.

Python 3.11 or newer is required. From the repository root:

```console
python -m copperscript check examples/valid_board/board.copper
python -m copperscript check examples/invalid_board/board.copper
python -m copperscript power-check examples/valid_board/board.copper
```

`check` prints `file:line:column` for each finding it can locate in the root
source, appends a hint such as `(did you mean 'PB7'?)` when one is available and
ends with an `OK:` or `FAILED:` summary. `--json` emits a
`copperscript.check/v1` document; a finding's `fix` carries a source `range` and
`replacement` only when the replacement is machine-applicable (for example a
misspelled endpoint or declaration keyword). Compile errors use the same
document with exit code 2. `--pins warn|error|off` controls `UNMARKED_PIN`
(see [no-connect pins](language-reference.md#no-connect-pins)).

```console
python -m copperscript check examples/invalid_board/board.copper --json
python -m copperscript explain UNMARKED_PIN
python -m copperscript explain --list
```

`query` answers compact questions about the compiled flat design without
re-reading source. Names use hierarchical `INSTANCE/REF` paths; unknown names
fail with exit 2 and a close-match suggestion. `--json` emits deterministic,
compact `copperscript.query/v1` documents.

```console
python -m copperscript query BOARD.copper pins U1          # pin, number, net, status, type, notes
python -m copperscript query BOARD.copper net GND          # members, supply, decoupling capacitors
python -m copperscript query BOARD.copper component C1     # part, value, footprint, properties, pins
python -m copperscript query BOARD.copper unconnected      # open pins grouped by why they are open
python -m copperscript query BOARD.copper components       # one line per component
python -m copperscript query BOARD.copper nets             # one line per net
```

After a routing or layout run, `issues` lists grouped, located failures and `view`
renders one of them; see [routing and placement iteration](llm-workflow.md#routing-and-placement-iteration).
`issues` also reads `replay-unified`'s attempted `unrouted` nets and locates
them on its exported PCB. Other open nets in a subset replay's whole-board
summary are deliberately excluded from those attempted-failure issues. An empty
replay issue list does not establish complete-board routing or native acceptance.
For a bounded fine-grid diagnostic, `replay-unified --set pitch_nm=25000
--set 'grid_window_nm=[1000000,1000000,21000000,21000000]'` limits the raster
and search to an explicit rectangle in nanometres. Include every attempted
terminal. The full-board lattice phase, source outline, obstacles, fixed copper,
macro/array intent and fresh exact commit checks remain in force. The window
is a search bound, not a new board outline or permission to discard outside
copper, and a successful subset is not complete-board or native acceptance.
`route-board --stop-after STAGE`, `--checkpoint-dir` and `--reuse-placement`
shorten reruns; see [shorter reruns](llm-workflow.md#shorter-reruns).
The standard critical path also saves `critical-inputs.state.json` and, when
used, `critical-retry-inputs.state.json` in the checkpoint directory. These
capture the actual temporary reservations and can be viewed or passed to
`replay-critical`. A saved bypass-only copper prefix must pass fresh source,
occurrence, complete-association and geometry admission; it is never accepted
as arbitrary routed input. Value-only decoding restores original-edge identities
only for unique exact occurrences, preserving all values and context digests.

`route-board` uses the unified router by default: unified negotiation after the
established coupled differential/critical routing and decoupler preparation.
`--router legacy` selects the pre-refactor pipeline instead, with pass-based
ordinary routing, package-access search, power preparation, the late
zone-escape repair and scoped routing. Pair routing,
skew tuning and return-path checks remain available in this standard pipeline.
Clock fallback uses the selected unified engine with the original clock profile;
each clock is bounded to 60 seconds, 16 iterations and two commit rounds (or the
smaller caller bounds). Complete-group matching and exact acceptance still apply.
At equal explicit priority, coupled differential/CAN groups route before
single-ended critical groups. This reserves their joint exit before a flexible
single trace can obstruct it. An explicitly higher priority still takes
precedence; no extra search or rip-up allowance is created.

The unified router's own controls are the `--unified-*` flags and
`--no-route-smoothing` (which reaches either router); `--fanout` options seed
its pad escapes and repair rounds. It ignores the legacy ordinary-router
flags, which apply only with `--router legacy`: `--pitch-mm` (unless
`--local-demand-cost` is above 0), `--passes`, `--search-budget`, `--minimum-repair-pitch-mm`, `--heuristic-weight`,
`--soft-ripup`, `--constrained-pins-first`, `--progressive-guides`,
`--guide-distance-heuristic`, `--repair-budget-multiplier`,
`--search-reuse-entries`, `--guide-escape-mm` and `--no-escape-terminals`.
It also skips package-access search (`--package-*` flags and the
`--legacy-gates` stop), power preparation and area planning
(`--plan-power-areas`, `--power-area-net`, `--power-area-config`) and the late
zone-escape repair (`--zone-escape-trials`, `--zone-local-ripup-trials`,
`--zone-escape-movement-mm`, `--no-incremental-placement-repair`);
`--stop-after package-preflight|package-access` is refused. The CLI plane
post-stitch (`stitch_zone_pads`, then `repair_cut_off_plane_pads`) still runs
unless `--unified-direct`. When any of these flags is given with a non-default
value, `route-board` emits one `router_options` / `ignored` progress event and
lists them in the report's `work_limits.unified_ignored_flags` and
`work_limits.unified_skipped_stage_flags`; routing is unchanged.
`bench/ladder.json` and the shared `make/board.mk` `ROUTE_ARGS` pass legacy
flags, so their default runs report them as ignored; a router comparison adds
only `--router legacy`.

`replay-critical STATE --only NET --report RUN/route-report.json -o RUN/board.kicad_pcb`
isolates critical routing from a placement checkpoint or source-only saved input.
Selecting one differential member selects its whole existing group. All source
rules remain active; arbitrary routed input copper is rejected. `--pair-states`
bounds aggregate pair search (default 6000), and `--clock-router legacy|unified`
selects the clock fallback for comparison. Failed groups have located `C` issues;
pair reports include a bounded port inventory with headings and departure probes.
The transition inventory distinguishes native plated-layer contacts from generated
signal-via launches and exposes which terminal has no legal transition candidates.
`pair_attempts` reports at most 16 tried endpoint orientations, state counts and
closest approaches; it contains no resumable frontier or accepted copper.
For a pair with two shared endpoint packages and one separate nearby support
contact per member, local support ties remain pending while each surface or
signal/return-via escape is formed. `pending_contacts` clears only after complete
pair acceptance. The original breakout-distance, contact, via and geometry rules
remain active. Ambiguous or unsupported contact trees retain the established
preparation; no support is dropped to fit a two-terminal guide.
This diagnostic does not rerun placement, bypass preparation or native checks,
and never establishes full-board or current-source acceptance.

`route-board --unified-direct` sends ordinary and profiled
nets to unified negotiation from source-fixed copper. It skips the legacy
critical, bypass, preliminary escape and late plane-contact routing stages;
immutable macro copper and required via arrays still materialize. Legacy escape
seeding and native bridge repair are also disabled, even after a partial result.
Native refill/DRC still runs, and any needed repair must re-enter unified routing. Without `--unified-direct`, the current
unified flow retains its legacy bootstrap. `--unified-time-budget-s 1800`
bounds negotiation starting at the unified stage; placement/global planning
and final native verification are separate. `--unified-save-inputs STATE`
retains replay inputs. Direct mode reports final profile connectivity and
physical DRC findings, but fails differential/CAN acceptance explicitly because
unified coupling and return-path verification are not implemented. Independent
pair paths and a zero-open signal summary do not establish pair qualification.

```console
python -m copperscript issues build/BOARD/runs/STAMP
python -m copperscript view build/BOARD/runs/STAMP --issue P1 -o build/views/P1.png
python -m copperscript state show build/trials/candidate.state.json
```

Pin status is `connected`, `no_connect`, `part_do_not_connect`, `unmarked`,
`optional`, `required_unconnected`, `power_input_unconnected`, `unmodeled` or
`open`. Queries describe declared connectivity and intent only; they do not
run ERC, placement or routing.

The examples already have a committed lock. Populate the URL dependency cache,
then reproduce without network access:

```console
python -m copperscript check examples/full_vertical/board.copper --locked
python -m copperscript check examples/full_vertical/board.copper --locked --offline
python -m copperscript audit-footprints examples/full_vertical/board.copper --locked --offline
```

Compile a valid design to normalized JSON IR:

```console
python -m copperscript compile examples/valid_board/board.copper -o board.json
```

Source `physical_macro` declarations and their module-owned bindings survive
compilation as realization descriptors; geometry remains in the selected JSON
assets. `copper check` reports their physical checks as pending. Prepare managed
macro assets with `copper lock board.copper`; use the normal locked/offline
physical workflow afterwards. Source-bound macros are applied automatically
by placement, routing, PCB export and editor rebuilds. `--hard-macro SCENE.json`
remains available for legacy bindings; conflicting identities/ownership fail.
See [the source syntax](language-reference.md#physical-macro-bindings).
`copper lock board.copper --locked` prepares/verifies selected assets against an
existing lock without rewriting it; add `--offline` to require cached inputs.

Generate a self-contained KiCad 8 schematic:

```console
python -m copperscript export-kicad examples/valid_board/board.copper -o valid_board.kicad_sch
```

Generate a KiCad 8 PCB draft with resolved footprint geometry:

```console
python -m copperscript export-kicad-pcb examples/resolved_footprint_board/board.copper -o build/resolved/board.kicad_pcb
```

All PCB export commands automatically write a same-stem `.kicad_pro`, a
project-local `fp-lib-table`, and a `CopperScript.pretty/` footprint library
beside the PCB. No manual KiCad library registration is needed: keep that
generated directory together when sharing or moving it. These are generated
outputs, not user-maintained source libraries. Backend export uses resolved
assets and does not modify your global KiCad configuration. Input resolution
may fetch pinned URL dependencies before generation. See [generated KiCad
projects](kicad-project-export.md) for portability and safety details.

[Managed footprint dependencies](footprint-dependencies.md) resolve exact
GitHub/GitLab `.kicad_mod` asset URLs or KiCad `Library:Footprint` identifiers
bound through `footprint-library` in `copper.mod`. They share the pinned package
cache and `copper.lock`, so no manual footprint download or installation path
is needed. Try the [standalone example](../examples/managed_footprints/README.md).

Local `.kicad_mod` paths in imported parts/modules resolve from the declaring
source; board-local paths resolve from the board. Unbound KiCad identifiers can
still use explicit roots:

```console
python -m copperscript export-kicad-pcb board.copper --footprint-root path/to/kicad-footprints
```

Placement remains a deterministic inspection grid and routing is not generated,
so the output is not yet fabrication-ready. Generated proxy pads remain
available for backend development through explicit opt-in:

```console
python -m copperscript export-kicad-pcb examples/valid_board/board.copper --allow-proxy-footprints
```

Produce a deterministic legal placement candidate and a four-gate readiness
report:

```console
python -m copperscript plan-layout examples/valid_board/board.copper --allow-proxy-footprints --candidates 3 -o planned.kicad_pcb --report layout-report.json
```

The planner estimates global routing congestion but does not generate copper.
Its report therefore marks Route as not run and Verify as blocked. See the
[physical layout workflow](layout-workflow.md) for the research,
consolidated stages, algorithms, and limitations.
Placement now estimates facing escape corridors from connected SMD pad demand,
reserving space for via banks from both packages and a transit lane. Fixed
poses and rigid/proximity units remain authoritative; unresolved corridors
appear as `ESCAPE_CHANNEL_DEFICIT` warnings in the layout report. Optional
`--escape-margin-mm 0.5` and `--escape-transit-lanes 1` control this soft
estimate on `plan-layout`, `route-global` and `route-board`, not DRC clearance.
See the [escape-aware placement plan](placement-internals.md).
The physical planner also uses soft supply-domain distribution objectives:
`--power-domain-weight 0.25` is the default; `0` disables domain attraction.
Multi-supply parts participate in each rail group without a hard boundary or
midpoint rule. Declared plane nets have reduced generic wirelength attraction.
Package preflight reserves plane contacts by default with fanout enabled;
`--no-early-plane-stitch` opts out for comparisons. Regional power pours are
explicit `copper_zone` constraints, not automatically inferred planes. See the
[power-domain and routing plan](placement-internals.md) and
[language reference](language-reference.md).
The [detailed-routing research](detailed-routing-research.md) explains
the geometry checks, fabrication-rule profile, and current full-board limits.

Produce deterministic multilayer global-routing guides after transactional
placement feedback:

```console
python -m copperscript route-global examples/valid_board/board.copper --allow-proxy-footprints -o global-route.json
```

Global guides reserve corridors, layers, and proposed transitions for ordinary
signal nets; declared copper-zone nets are reported as deferred and consume no
global wire corridor. Guides are not tracks or fabrication data.

Attempt detailed routing and write a native DRC report plus an inspection-only
KiCad PCB draft:

```console
python -m copperscript route-board examples/valid_board/board.copper --allow-proxy-footprints --report route-report.json -o routed-draft.kicad_pcb
```

Detailed signal tracks prefer long straight runs and 45-degree bends. Nets with
declared copper zones (typically GND) are deferred from ordinary maze routing;
`route-board` instead attempts short pad escapes and plane vias. Their copper
fill remains provisional until KiCad refills the zones and verifies physical
connectivity. A net named GND without a declared zone receives no implicit
plane.

With `--router legacy`, if a zone pad remains inaccessible, the router first
tries a bounded local rip-up: it reserves a legal plane escape, identifies the ordinary nets that
block it, and reroutes only those nets. If that fails, escape feedback tries
alternative via exits and legal local placement changes from a clean placement.
Use `--zone-local-ripup-trials 0 --zone-escape-trials 0` to disable both, or
adjust `--zone-escape-movement-mm` for the placement step. These trials never
move components under existing copper. Soft plane-aware layer costs and
alternating inner-layer directions can be tuned with
`--layer-preference-cost` and `--direction-preference-cost` (zero disables
either preference); they do not override explicit layer restrictions or DRC.
Accepted ordinary copper is straightened unless `--no-route-smoothing` is
given. Opt-in `--layer-assignment-passes`, `--local-demand-cost` and
`--guide-escape-mm` move ordinary guide runs away from same-layer crossings and
full channels; see the [layer-balance review](routing-backlog.md).
A reserved package escape is one terminal of its pin, beside the pin's own
surface access and its launch via on every layer; escape copper the finished
route does not need is removed (`--no-escape-terminals` restores port-only
terminals). Opt-in `--package-destination-ports` turns boundary ports toward
each pin's nearest other terminal.
For intentional diagonal placement, add a
`constraint allowed_orientations(U1) { values = "0,45,90"; }`; unconstrained
parts retain 0/90/180/270-degree candidates.

The command exits nonzero when routing or DRC is incomplete. Even a successful
native check does not qualify proxy footprints or replace KiCad and CAM review.
Use `--progress` for flushed phase/group/trial events during long runs; the
shared Make routing workflow enables this automatically and saves them in
`routing.log`. Full ground-feedback trials can each rebuild the routing.
Progress is not completion or signoff evidence; see [routing progress](routing-progress.md).
With `--router legacy`, `--pitch-mm`, `--passes`, and `--search-budget` bound
detailed-routing work (the unified router uses `--unified-search-states` and
`--unified-time-budget-s`); exhausting the search budget is reported per net
rather than silently accepting an unfinished path.
The report's `critical_lane_review` section reviews the exported copper of every
critical net: track lengths, layers and vias, pair and `length_match` skew,
spacing to other copper inside and outside breakout regions, coupled length to
other pairs and bends. `route-board` and `python -m pcbir.critical_preflight`
(for its critical copper) print it as one `CRITICAL LANES:` line; see the
[language reference](language-reference.md#critical-routing-and-qualification).
With `--router legacy`, `route-board` plans package exits and compatible plane
contacts before area routing (the unified router uses `--fanout` only to seed
its pad escapes); `--no-fanout` explicitly selects the legacy diagnostic flow.
For difficult boards, `--soft-ripup` (legacy router) and `--maximum-ripup-blockers 4` try bounded transactional rerouting, and
`--detailed-feedback-trials 4` retries legal placement changes against exact
routing failures (legacy router). These experiments do not waive physical DRC.

Under the unified router, placement feedback is one congestion-driven loop:
after the first full pass, if nets stay open or critical nets fail, it builds a
per-tile congestion map (the router's own contention plus a RUDY demand
estimate, each normalised to its 95th percentile) and moves parts on failed nets
or hot tiles down the demand gradient. Only the two candidates that the estimate
and `route_global` rate best rerun the pipeline tail (critical routing,
plane stitch, ordinary routing, land closure, DRC), and a move is kept only if it
lowers the whole-board score (hard DRC, opens, critical failures, unrouted nets,
overflow, length, vias). `--congestion-feedback-trials N` bounds the reruns
(default 0, off); `--detailed-feedback-trials` and `--critical-feedback-trials`
map onto it, with a budget of `max(N, detailed + critical)`, and the report
lists them under `work_limits.unified_mapped_flags`. The report's
`placement_feedback` block lists every trial, the accepted moves and up to 16
hot tiles. `--router legacy` keeps the two old controllers unchanged.
(`--placement-feedback-trials` is the older, unrelated budget of global
placement trials per feedback iteration.)

Pass `--verify-plane-fill "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"` to
refill a disposable KiCad copy and include authoritative open-net/island
findings in the report; a zone outline or stitched via alone is not proof of
connectivity. See [routing research tasks](routing-backlog.md).
Use the [current backlog](../TODO.md) for unresolved solver work.

### Full-vertical routing from a fresh checkout (Windows / PowerShell)

This recomputes placement and routing from the declared board inputs. Use the
shared Make configuration for the selected source.
Package access, critical compatibility and fresh native verification are
separate gates; local escape witnesses alone do not prove board closure.
Exact component/internal-contact and coupled-winding evidence is required even
when copper DRC passes. See [routing invariants](filled-copper-feedback.md).

Install Git, [uv](https://docs.astral.sh/uv/getting-started/installation/), and
KiCad 10 with its footprint libraries first. The latest closure run used
KiCad 10.0.6 and Python 3.13.1. Only CopperScript needs checking out:

```powershell
# Use consistent package line endings in these new checkouts.
git clone -c core.autocrlf=false -c core.eol=lf https://github.com/andenore/CopperScript.git
Set-Location CopperScript

uv sync --python 3.13
# Fetch the exact locked URL dependency (subsequent runs can use --offline).
uv run --no-sync python -m copperscript check examples/full_vertical/board.copper --locked
```

`copper.mod` pins CopperLib's GitHub commit; `copper.lock` verifies its exact
part, footprint and physical-asset inventory in `.copper-cache/pkg/`. No manual
library checkout, footprint download or lock refresh is required. Canonical
package line endings are declared in `.gitattributes`. This reproduces routing
inputs/options, not byte-identical historical output. After fetching,
`--locked --offline` verifies every package byte without changing the lock.
If resolution fails, investigate changed bytes/revisions rather than relocking.
Initial cloning, `uv sync` and uncached URL dependencies need network access.
See uv's
[project workflow](https://docs.astral.sh/uv/guides/projects/) for environment setup.

After the setup above, run the complete workflow with:

```powershell
make EXAMPLE=full-vertical route
```

The shared Make workflow recomputes placement and routing from source with the six-layer
settings below, including the pinned placement scene, critical routing, fanout,
ordinary routing, ground feedback, native DRC and independent KiCad plane
verification and saves native filled copper. Each run writes a fresh `build/full-vertical/runs/<UTC-run-id>/` with
`board.kicad_pcb`, `board.kicad_pro`, `route-report.json`, `routing.log` and
`run.json` (command, timestamps, elapsed time and exit code). `build/` is ignored
by Git. Errors/interruption may leave only logs/partial artifacts; file existence
does not imply routing success. No Gerbers are generated.

The generic runner uses the current Python/uv environment and downloads the locked
dependencies if uncached; it never updates `copper.lock`. Set `RESOLVE_ARGS="--locked --offline"`
to prohibit fetching. It discovers `kicad-cli` on PATH or the standard Windows
KiCad 10 installation. Managed Git footprints are resolved from the lock; the
optional `KICAD_FOOTPRINTS` setting is used for native KiCad verification.
Override paths when needed, preview without routing using `BUILD_ARGS=--dry-run`, or
select a new/empty directory inside `build/`:

```powershell
make EXAMPLE=full-vertical route BUILD_ARGS=--dry-run
make EXAMPLE=full-vertical route `
  KICAD_CLI="C:/Program Files/KiCad/10.0/bin/kicad-cli.exe" `
  KICAD_FOOTPRINTS="C:/Program Files/KiCad/10.0/share/kicad/footprints" `
  RUN_DIR="build/my-routing-run"
```

The runner preserves the router's exit code: **1** means unmet routing/DRC
gates, **2** means setup/execution error, and **130** means interruption.
GNU Make itself returns nonzero (usually 2) when its recipe fails.
An existing nonempty output directory is rejected so old results cannot be
mistaken for a new run. Exit 0 still does not constitute manufacturing signoff.
Allow tens of minutes for a full-vertical run; the shared workflow is not a claim
that every example has passed complete routing.

Function profiling is enabled by default. Each run also saves `routing.prof`,
readable/JSON hotspot summaries and `phase-timings.json` under the same ignored
directory. Instrumentation adds overhead; use `PROFILE=none` for uninstrumented
speed comparisons (phase events are still saved). See
[profiling and optimization assessment](routing-performance.md) for details,
current evidence and the prioritized optimization work list.
The detailed [optimization todos](routing-optimization-todo.md) track implementation.
Package preflight can now negotiate alternate ordinary/critical/selected-ground
escape patterns before placement repair. The default is at most two proposals;
use `EXTRA_ROUTE_ARGS="--package-pattern-trials 0"` for a baseline comparison.
Every accepted pattern preserves prior exits and passes fresh owner/native
checks; this does not certify onward area routing. See
[package-access design and remaining work](package-access-first.md).
Local repair now supports bounded blocker-cone expansion; use
`EXTRA_ROUTE_ARGS="--zone-dependency-expansions 0"` to compare without that extension.
Small noncritical placement moves also try bounded incremental repair. Use
`EXTRA_ROUTE_ARGS=--no-incremental-placement-repair` to compare against full placement reroutes;
unsupported moves always retain that fallback.
Inspect a run without changing it using `uv run --no-sync python -m pcbir.routing_benchmark summarize "build/full-vertical/runs/<run-id>"`.
The [performance guide](routing-performance.md) explains guarded before/after comparisons.

Use the [build guide](make-builds.md) for effective command configuration.
Reports bind their input/output evidence; a successful historical run does not
approve changed inputs. `routing_complete=true` does not imply
`fabrication_ready=true`. Ground deferred by explicit-copper analysis needs
actual native-fill verification.
Vias avoid all pads by default, including same-net pads and annulus-edge contact.
The shared routing workflow does not enable blanket via-in-pad. Explicit
`--ground-via-in-pad` permits filled-and-capped GND vias for this six-layer
profile; that process must be explicitly qualified with the fabricator before
ordering a board.

For independent copper-layer plots and via/pad/bend metrics, see
[the layer-review workflow](routing-layer-review.md).

After a run passes, generate native Gerbers/drills from its saved filled board
using the [manufacturing-file workflow](manufacturing-files.md):

```powershell
uv run --no-sync python -m copperscript export-manufacturing `
  "build/full-vertical/runs/<run-id>/board.kicad_pcb" `
  --kicad-cli "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe" `
  --skip-independent-cam `
  -o "build/full-vertical/runs/<run-id>/manufacturing"
```

This refuses native opens/violations. It produces `gerbers-drill.zip` and
`manufacturing-package.zip`, not an independently qualified release. The
explicit CAM skip follows the requested prototype workflow. This fixture has
no reviewed assembly BOM/CPL; supplier availability and component/support-circuit
qualification still require review before ordering.

To repeat the final independent refill/DRC on a saved copy without modifying
the routed draft:

```powershell
Copy-Item "build/full-vertical/board.kicad_pcb" "build/full-vertical/verified.kicad_pcb"
Copy-Item "build/full-vertical/board.kicad_pro" "build/full-vertical/verified.kicad_pro"
& $kicadCli pcb drc --format json --severity-all --refill-zones --save-board `
  --output "build/full-vertical/kicad-drc.json" "build/full-vertical/verified.kicad_pcb"

$drc = Get-Content "build/full-vertical/kicad-drc.json" -Raw | ConvertFrom-Json
Write-Host "Unconnected items: $($drc.unconnected_items.Count)"
$drc.violations | Group-Object type | Select-Object Name, Count
```

Keep all reported findings visible; native results apply only to their bound inputs.

Validate and inspect a KiCad footprint before resolving it into a physical
design:

```console
python -m copperscript check-footprint path/to/package.kicad_mod --strict
```

For a development installation with the `copper` command:

```console
python -m pip install -e ".[test]"
copper check examples/valid_board/board.copper
python -m pytest
```

For a search that exhausts the unified router's per-path work bound, use
`--unified-search-states N` to choose a larger bounded search. The default is
400000 expanded states per path. `--unified-time-budget-s` remains a separate
wall-time limit. Both are computational settings; source and fabrication rules
are unchanged. The effective state limit appears in `work_limits` and replay
inputs. A failed bounded search is not proof that the connection is impossible.
