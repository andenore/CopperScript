# CopperScript

CopperScript is a semantic, strongly typed language for describing PCB
connectivity, electrical intent, and design constraints.

The v0.1 compiler parses `.copper` source into a typed intermediate
representation and runs electrical-rules checks (ERC). It deliberately does not
claim fabrication readiness from routing guidance. Its prototype physical
workflow can place components and attempt geometry-checked detailed routing,
but a routed board still requires independent physical and KiCad DRC signoff.

The [mechanical-geometry specification and checklist](docs/mechanical-geometry.md)
adds polygonal outlines, cutouts and round mounting holes to the physical IR.
Try the routed L-shaped inspection probe (including performance profiling):

```console
uv run python -m examples.mechanical_probe.mechanical_example
```

Output: ignored `build/mechanical-example/`. The frontend supports a separate
[`mechanical` section](docs/mechanical-language.md) for circle/rectangle/polygon,
rounded and closed line/arc outlines, cutouts, NPTH holes/slots and physical rules.
Curved-query limits are explicit; complete manufacturing qualification remains
separate from routing and inspection export. Board-specific
builders live under `examples/`, never in the generic `pcbir` package. The
[examples index](examples/README.md) keeps each standalone example and its
assets in one directory.

A local [mechanical/floorplan editor](docs/mechanical-editor.md) displays real
footprint pads/courtyards, mechanical geometry and locked DXF/enclosure guides.
It supports rough auto-placement, ratsnest/copper filters, checked dragging,
temporary poses and reviewed persistent source locks/geometry edits with undo.
Named datums/edges, attachments, body-only overhangs, height/access policies,
atomic closed-path editing and an optional VS Code host share generic compiler
code. Temporary placement remains unsaved unless explicitly locked into source;
source changes always require diff review and Save. The
[verified implementation checklist](docs/mechanical-editor-plan.md) records tests
and limitations, including convex curved paths and reference-only DXF overlays.

```powershell
uv run copper edit-mechanical examples/mechanical_editor_demo/board.copper --footprint-root "C:/Program Files/KiCad/10.0/share/kicad/footprints"
```

The tool opens a loopback-only browser UI; Ctrl+C stops the service.
Use `--no-browser` to open its printed URL yourself. Footprints may resolve from
explicit roots or pinned Git library assets; proxy inspection is opt-in only.
Scroll the wheel to zoom under the pointer; hold the right mouse button to pan.
Ordinary left-drags apply after checking (with Undo). Numeric poses and rough
auto-placement remain explicit previews; apply or discard them before more edits.

Try `make EXAMPLE=mechanical-curves route` for rounded geometry/slot routing or
`make EXAMPLE=mechanical-reference edit` for a checksum-pinned enclosure guide.
Source edits stale previous routed/fill outputs; rebuild before using them.

[Importable mechanical profiles](docs/mechanical-profiles.md) split reusable
outlines, holes, keepouts and anchored connector placement into separate `.copper`
files. They create no electrical connections. Try the fictional carrier demo
(uses the existing pinned CopperLib connector, no manual library checkout):

```powershell
uv run copper check examples/mechanical_profile_project/board.copper --locked
uv run copper edit-mechanical examples/mechanical_profile_project/board.copper --locked
```

For a real host pattern, see the [CM4 baseboard example](examples/cm4_baseboard/README.md).
It binds two real Hirose 100-pin sockets to a CopperLib mounting profile with
four holes, separates project-owned mechanics from connections, and reserves
module/antenna clearance. It includes a minimal Makefile, editor/placement
commands and a profiled routing workflow that saves ground fills and verifies
zero opens/violations with native KiCad. This four-layer eMMC carrier is a
**routed prototype**, not an order-ready board; no manual CopperLib checkout
is required.

```powershell
make EXAMPLE=cm4 route
```

All examples use the [same Make targets and generic build runner](docs/make-builds.md),
not per-board routing scripts. `make compile-examples` compiles all complete
registered examples; `make EXAMPLE=cm4 compile` builds just one. GNU Make is
required in addition to Python/uv (use `mingw32-make` on Windows if that is its name).

A [50 mm circular LED-ring example](docs/round-led-ring.md) uses an offset nRF52832
as a non-radio controller, twelve GPIO-controlled LEDs and a rear CR2032 holder.
It exports a true KiCad circle and fixed radial placement, with profiling and an
optional routing run:

Its circle, rear ground pour and all fixed placements are declared directly in
`examples/round_led_ring/board.copper`. No Python builder is needed to export it:

```powershell
uv run copper export-kicad-pcb examples/round_led_ring/board.copper --locked `
  -o build/round-led-ring/round_led_ring.kicad_pcb
```

The optional example wrapper adds placement previews, profiling and native checks:

```powershell
uv run python -m examples.round_led_ring.round_led_example `
  --offline
```

Output: ignored `build/round-led-ring/`; default is a placed, unrouted inspection
board. See the linked guide for `--route` and independent ground-fill checks.

## GitHub inspection builds

The [board-routing workflow](.github/workflows/board-routing.yml) attempts the
complete full-vertical routing flow on relevant `main` pushes and manual runs.
Every pushed tag also routes the nRF52 hard-macro example and publishes an
experimental prerelease with source distributions, KiCad projects/local
footprints, six layer SVG previews, reports, logs, profiling and checksums.
CopperLib is fetched by pinned URL into the managed cache; no separate checkout
is required. Failed or timed-out attempts remain inspection drafts, never
manufacturing releases. See [CI setup and downloads](docs/github-board-builds.md).

## Quick start

An opt-in [physical hard-macro trial](docs/physical-hard-macros.md) preserves
Nordic RF matching copper and demonstrates a separate Johanson antenna tee,
corner fill exclusions and whole-assembly rotations. CopperLib downloads through
the pinned URL dependency automatically:

```powershell
uv run python -m examples.nrf_antenna_macro.hard_macro_trial `
  --offline
```

Output: ignored `build/nrf-hard-macro/`. This is an RF-only geometry probe,
not an operational radio or full-vertical routing integration.

A [small powered nRF52/CR2032 example](docs/nrf52-coin-cell.md) adds a 10-pin
SWD connector, two LEDs, two buttons and the radio support circuit:

```powershell
uv run python -m copperscript check examples/nrf52_coin_cell/board.copper --locked
uv run python -m examples.nrf52_coin_cell.nrf52_example --offline
```

Output: `build/nrf52-coin-cell/`. The RF macro is locked and pre-routed;
the remaining circuit is placed but unrouted. Add `--route` to attempt the full
pipeline (the current nRF example still fails package-access gates; see its
linked report/remaining-work guide). This is not production signoff.

Python 3.11 or newer is required. From the repository root:

```console
python -m copperscript check examples/valid_board/board.copper
python -m copperscript check examples/invalid_board/board.copper
python -m copperscript power-check examples/valid_board/board.copper
```

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
projects](docs/kicad-project-export.md) for portability and safety details.

[Managed footprint dependencies](docs/footprint-dependencies.md) resolve exact
GitHub/GitLab `.kicad_mod` asset URLs or KiCad `Library:Footprint` identifiers
bound through `footprint-library` in `copper.mod`. They share the pinned package
cache and `copper.lock`, so no manual footprint download or installation path
is needed. Try the [standalone example](examples/managed_footprints/README.md).

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
[physical layout workflow](docs/layout-workflow.md) for the research,
consolidated stages, algorithms, and limitations.
The [detailed-routing research](docs/detailed-routing-research.md) explains
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

If a zone pad remains inaccessible, the router first tries a bounded local
rip-up: it reserves a legal plane escape, identifies the ordinary nets that
block it, and reroutes only those nets. If that fails, escape feedback tries
alternative via exits and legal local placement changes from a clean placement.
Use `--zone-local-ripup-trials 0 --zone-escape-trials 0` to disable both, or
adjust `--zone-escape-movement-mm` for the placement step. These trials never
move components under existing copper. Soft plane-aware layer costs and
alternating inner-layer directions can be tuned with
`--layer-preference-cost` and `--direction-preference-cost` (zero disables
either preference); they do not override explicit layer restrictions or DRC.
For intentional diagonal placement, add a
`constraint allowed_orientations(U1) { values = "0,45,90"; }`; unconstrained
parts retain 0/90/180/270-degree candidates.

The command exits nonzero when routing or DRC is incomplete. Even a successful
native check does not qualify proxy footprints or replace KiCad and CAM review.
Use `--progress` for flushed phase/group/trial events during long runs; the
shared Make routing workflow enables this automatically and saves them in
`routing.log`. Full ground-feedback trials can each rebuild the routing.
Progress is not completion or signoff evidence; see [routing progress](docs/routing-progress.md).
`--pitch-mm`, `--passes`, and `--search-budget` bound detailed-routing work;
exhausting the search budget is reported per net rather than silently accepting
an unfinished path.
For difficult boards, `--fanout` pre-escapes crowded SMD pads, `--soft-ripup`
and `--maximum-ripup-blockers 4` try bounded transactional rerouting, and
`--detailed-feedback-trials 4` retries legal placement changes against exact
routing failures. These experiments do not waive physical DRC. Pass
`--verify-plane-fill "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"` to
refill a disposable KiCad copy and include authoritative open-net/island
findings in the report; a zone outline or stitched via alone is not proof of
connectivity. See [routing research tasks](docs/routing-research-tasks.md).
The [routing-review todo](docs/routing-review-todo.md) tracks correctness and
optimization work; the [first-pass repeated review](docs/routing-review-pass1.md)
records independent KiCad results and the remaining six-layer bottlenecks.
The [second-pass review](docs/routing-review-pass2.md) verifies joint ground
escapes with zero KiCad unconnected items; critical-net and production signoff
remain outstanding.
The [third-pass review](docs/routing-review-pass3.md) confirms consistent
duplicate-land closure and feedback reports without changing the verified copper.

### Full-vertical routing from a fresh checkout (Windows / PowerShell)

This runs the complete placement/global-routing, fanout, detailed-routing,
package-ground escape feedback, native DRC and independent KiCad plane-check
workflow used for the [second routing review](docs/routing-review-pass2.md).
It generates a **reviewable PCB draft**, not production Gerbers.

The current example adds explicit critical USB/RF profiles. Those change the
input intent relative to the recorded second/third-pass run. The
[pass-5 joint paired search](docs/routing-review-pass5.md) connected both USB
pairs without vias or hard copper violations. Subsequent source-backed
[RF circuit corrections](docs/rf-layout-audit.md) change the netlist again:
at the old placement, new critical reservations leave the modem pair pending.
The command below is still the full workflow, not a reproduction of the
historical zero-open copper. Run the [cheap critical preflight](docs/full-vertical-example.md#explicit-critical-profiles-and-cheap-preflight)
first; RF cluster placement, return-path and complete-board rerun work remain.

If exact critical routing fails, optionally add `--critical-feedback-trials 1`
to `route-board` or the critical preflight. This bounded
[critical placement feedback](docs/critical-placement-feedback.md) tries legal
whole-unit rotations/moves and rebuilds all critical copper; it does not relax
profile limits or imply fabrication readiness. Each trial can take minutes.
The [pass-9 verification](docs/routing-review-pass9.md) connects both USB pairs
geometrically, but used an incorrect choke winding mapping. **Do not fabricate
those historical USB routes.** [Pass 11](docs/routing-review-pass11.md) corrects
the CopperLib mapping from Coilcraft's schematic and reruns from fresh placement.
Copper DRC cannot validate internal component connections. Ordinary routing and
electrical qualification remain open.
The [paired refinement pass](docs/routing-review-pass10.md) removes redundant
search-step segments while preserving connectivity. It does not yet shorten
the real-board USB detours.
The [access/miter review](docs/routing-review-pass12.md) removes the oblique RF
access segments and small USB backtrack at the unchanged corrected placement.
All eight critical nets remain connected. The corrected-input
[full rerun and six-layer review](docs/routing-review-pass13.md) keeps their
copper and all 51 placements unchanged: KiCad finds zero GND/critical opens,
but three ordinary signal nets remain open (34 unconnected items), alongside
the eight library findings. The review records spare inner/bottom capacity,
guide-cost and tree/seam cleanup tasks. New [per-net policy telemetry](docs/routing-search-policy.md)
reports requested/effective detailed layer costs without changing routing.
Production gates remain outstanding.
The [compatible-escape increment](docs/routing-review-pass17.md) improves the
matched partial package stage from 72 to 74 exits. Two MCU exits remain pending;
the latest full-board routing result above has not been replaced by a full rerun.
The [escape-first increment](docs/package-access-first.md) now reserves ordinary
package exits before critical long routes with `--fanout`. If package access or
critical compatibility fails, ordinary area routing is blocked with zero search
passes. Bounded legal placement feedback rebuilds the reservations; use
`--package-access-trials 0` to disable moves without disabling the gate.
Joint critical/power/ground access and directional placement margins remain
follow-up work; an allocated fanout via is not complete-board connectivity.
The [boundary-capacity increment](docs/package-boundary-access.md) now gates area
routing on mutually compatible ordinary paths beyond package collars. Its
matched-placement diagnostic proves 76/76 local witnesses after a multi-bend
fallback. A ready preflight now commits owned local paths and exact-layer
boundary anchors for detailed routing; blocked preflights retain provisional
witnesses only. Neither local access nor those stubs replace full-board routing.
[Pass 18](docs/routing-review-pass18.md) allocates all 76 ordinary exits at the
unchanged saved placement, but the modem-side USB pair then fails. The access
gate correctly blocks area routing. Subsequent alternate-owner preflight passes
both USB pairs, four RF groups, selected ground contacts and all 76 ordinary
boundary paths. A new profiled full-area/fill/DRC run is still required before
claiming full-board success.

Install Git, [uv](https://docs.astral.sh/uv/getting-started/installation/), and
KiCad 10 with its footprint libraries first. The recorded run used KiCad 10.0.6
and Python 3.12. Only CopperScript needs checking out:

```powershell
# Use consistent package line endings in these new checkouts.
git clone -c core.autocrlf=false -c core.eol=lf https://github.com/andenore/CopperScript.git
Set-Location CopperScript

uv sync --python 3.12
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
[profiling and optimization assessment](docs/routing-performance.md) for details,
current evidence and the prioritized optimization work list.
The detailed [optimization todos](docs/routing-optimization-todo.md) track implementation.
Package preflight can now negotiate alternate ordinary/critical/selected-ground
escape patterns before placement repair. The default is at most two proposals;
use `EXTRA_ROUTE_ARGS="--package-pattern-trials 0"` for a baseline comparison.
Every accepted pattern preserves prior exits and passes fresh owner/native
checks; this does not certify onward area routing. See
[package-access design and remaining work](docs/package-access-first.md).
Local repair now supports bounded blocker-cone expansion; use
`EXTRA_ROUTE_ARGS="--zone-dependency-expansions 0"` to compare without that extension.
Small noncritical placement moves also try bounded incremental repair. Use
`EXTRA_ROUTE_ARGS=--no-incremental-placement-repair` to compare against full placement reroutes;
unsupported moves always retain that fallback.
Inspect a run without changing it using `uv run --no-sync python -m pcbir.routing_benchmark summarize "build/full-vertical/runs/<run-id>"`.
The [performance guide](docs/routing-performance.md) explains guarded before/after comparisons.

For the equivalent explicit CLI invocation, adjust the KiCad paths if your
installation is elsewhere. From the CopperScript repository root, run:

```powershell
$kicadCli = "C:\Program Files\KiCad\10.0\bin\kicad-cli.exe"
New-Item -ItemType Directory -Force "build/full-vertical" | Out-Null

uv run --no-sync python -m copperscript route-board examples/full_vertical/board.copper `
  --locked --offline --layers 6 --fab-profile jlcpcb-six-layer `
  --placement-templates examples/full_vertical/placement_templates.json `
  --candidates 1 --placement-candidate candidate-01 `
  --feedback-iterations 1 --router-iterations 5 --critical-feedback-trials 0 `
  --pitch-mm 1 --passes 2 --search-budget 20000 `
  --soft-ripup --fanout --constrained-pins-first --progressive-guides `
  --repair-budget-multiplier 10 `
  --plane-contact-radius-mm 5 `
  --zone-escape-trials 4 --zone-local-ripup-trials 6 `
  --verify-plane-fill $kicadCli `
  --report build/full-vertical/route-report.json `
  -o build/full-vertical/board.kicad_pcb

$routingExitCode = $LASTEXITCODE
Write-Host "Routing exit code: $routingExitCode (1 means a draft with unmet gates; inspect the report)"
```

This recomputes placement and copper from `.copper` and footprints; it does not
load an earlier routed board. Allow tens of minutes: the recorded pipeline took
about 19 minutes plus final export/verification, and harder feedback trials can
take substantially longer. No external timing wrapper is required. KiCad library
versions can change geometry and results; record the installed version and
footprint inputs when comparing runs.

The explicit CLI outputs are `board.kicad_pcb`, its same-stem `board.kicad_pro`, and
`route-report.json` under `build/full-vertical/`. Open the board with its project
so KiCad uses the exported design rules. The report includes the independent
`plane_verification` result. The historical profile-free run exited **1** despite
zero KiCad unconnected items. The latest corrected-library/template run also
exits **1**, with three ordinary open nets (`MCU_NRF_TX`, `MODEM_EN`, `V3V3`),
34 KiCad unconnected items, zero GND/critical opens and eight library findings.
Neither run is production signoff. An output file or zero airwires is not
manufacturing acceptance; USB/RF/return-path qualification remains outstanding.
Vias avoid all pads by default, including same-net pads and annulus-edge contact.
The shared routing workflow does not enable blanket via-in-pad. Explicit
`--ground-via-in-pad` permits filled-and-capped GND vias for this six-layer
profile; that process must be explicitly qualified with the fabricator before
ordering a board.

For independent copper-layer plots and via/pad/bend metrics, see
[the layer-review workflow](docs/routing-layer-review.md).

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

Keep all reported findings visible. The second review found zero unconnected
items, no shorts/clearance/dangling findings, and four library issues plus four
library mismatches; this is a recorded result, not a guarantee for changed inputs.

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

## Example

```copper
board SensorBoard {
    use library "tiny";
    import sensors "github.com/copperscript/examples/sensors";
    import stm32 "github.com/copperscript/examples/stm32";

    component U1: stm32.STM32G0B1CBT6;
    component U2: sensors.BME280;
    component R1: RESISTOR { value = 4.7kohm; }

    configure U1.I2C1 as SENSOR_BUS {
        SDA = PB7;
        SCL = PB6;
    }

    net I2C_SDA {
        U1.PB7;
        U2.SDA;
        R1.2;
    }

    supply V3V3 {
        voltage = 3.3V;
        external = true;
    }
}
```

See the [design specification](docs/design-specification.md), the
[language reference](docs/language-reference.md), and the complete [valid
example](examples/valid_board/board.copper).

Device-library authors should use the compact, table-driven
[device generation workflow](docs/device-generation.md) rather than writing
large MCU definitions by hand.

The implemented [rich device model](docs/rich-device-model-proposal.md) describes
how CopperScript can represent flexible Nordic pin routing, mode-dependent FX10
pins, differential ADC channels, multi-unit op-amps, and package connection
rules and the cross-vendor acceptance fixtures used to validate them.

The [hierarchical example](examples/hierarchical_board/board.copper) instantiates a
reusable [5 V to 3.3 V buck supply](examples/packages/power/buck_5v_to_3v3.copper)
and imports the sensor part from a separate package.

## Packages

Imports use stable URL-like package paths and an explicit local alias:

```copper
import power "github.com/copperscript/examples/power";
module PWR: power.Buck5VTo3V3;
```

Versions and development replacements are deliberately kept out of source
files. They live in `copper.mod`:

```text
module github.com/anden/CopperScript
require github.com/copperscript/examples v0.1.0
replace github.com/copperscript/examples => ./examples/packages
```

The compiler resolves the longest matching required module, loads all
`.copper` part and module definitions in the selected package directory, and
records the complete downloaded asset inventory in `copper.lock`. Without a `replace`, v0.1 fetches
tagged GitHub modules into `.copper-cache`; package source is parsed as data and
no package code is executed. Local replacements remain editable and retain
their current content hash as IR provenance without being locked in the sum
file.

## Compiler architecture

```text
.copper source
    -> lexer
    -> source-aware syntax tree
    -> hierarchical electrical IR
         |-> JSON IR
         |-> KiCad schematic backend
         `-> derived flat view -> electrical-rules checker
                              `-> power-state analyzer
                              `-> footprint resolver + draft physicalizer
                                      `-> physical IR
                                          |-> legal placement + routability estimate
                                          `-> KiCad PCB backend
```

The stages are intentionally separate so editor tooling and a future language
server can reuse parsing, semantic lowering, and ERC independently.

Key modules:

- `pcbir.lexer` and `pcbir.parser` — dependency-free source frontend.
- `pcbir.syntax` — source locations and syntax-tree declarations.
- `pcbir.compiler` — semantic lowering and typed unit validation.
- `pcbir.packages` — manifests, package resolution, Git cache, and checksums.
- `pcbir.model` and `pcbir.quantities` — immutable semantic IR.
- `pcbir.elaborate` — explicit hierarchy-to-flat derivation for consumers that
  require a global connectivity view.
- `pcbir.erc` — reusable electrical-rules passes.
- `pcbir.power` — explicit steady-state power-domain analysis.
- `pcbir.physical` — immutable, backend-neutral board geometry, footprints,
  placements, nets, tracks, vias, stackup, and design rules.
- `pcbir.footprints` — deterministic `.kicad_mod` reference resolution from
  board-relative paths or explicit KiCad library roots.
- `pcbir.physicalize` — resolved-footprint and typed placement-constraint
  lowering; proxy geometry is an explicit development fallback.
- `pcbir.placement` — hierarchy-aware analytical placement, hybrid
  legalization, route-aware detailed refinement, and deterministic Pareto
  candidate generation.
- `pcbir.layout` — four-gate readiness orchestration and placement reporting.
- `pcbir.routing` — deterministic multilayer global routing with
  negotiated-congestion guides and explicit overflow diagnostics.
- `pcbir.routeflow` — full-route candidate selection, bounded legal placement
  moves, atomic rollback, stagnation control, and fresh-route certification.
- `pcbir.critical` — profile-driven locked copper for critical, differential,
  clock, CAN, RF-feed, power, and length/via-constrained routes.
- `pcbir.detailed` — guide-aware deterministic general routing with legal pad
  escape, exact foreign-copper clearance, locked critical copper, and bounded
  whole-pass rerouting. See [detailed-routing research](docs/detailed-routing-research.md).
- `pcbir.geometry`, `pcbir.any_angle`, and `pcbir.shove` — shared exact integer
  predicates, deterministic visibility walkaround, and atomic recursive line
  shove with fail-safe rollback.
- `pcbir.drc` — fail-closed exact-copper checks, explicit coverage, scoped
  waivers, and content-bound physical signoff tokens.
- `pcbir.flow` — compatible global/critical/detailed routing and DRC
  orchestration with no proxy-to-copper shortcuts.
- `pcbir.manufacturing` — gated KiCad 10 Gerber/drill/netlist releases with
  independent structural CAM parsing, manifests, checksums, and atomic publish.
- `pcbir.process_drc` — separate provenance-bearing fabrication, stencil, and
  assembly capability gates.
- `pcbir.engineering` — evidence-graded screening results that explicitly state
  claim scope and model limitations.
- `pcbir.cam_qualification` and `pcbir.constraint_coverage` — fail-closed
  independent-tool evidence and end-to-end ownership of hard constraints.
- `pcbir.physical` — immutable placement, exact copper, typed zone/keepout
  intent, and content-bound external fill provenance. Manufacturing boards with
  zones are authoritatively refilled and saved by the pinned KiCad toolchain.
- `pcbir.importers.kicad_mod` — dependency-free, fail-safe KiCad footprint
  parser and normalizer.
- `pcbir.devicegen` — compact JSON/CSV device bundles, bounded extraction work
  packets, validation, and deterministic library generation.
- `pcbir.backends` — immutable backend artifacts and the KiCad schematic and
  PCB generators.
- `pcbir.serializer` — versioned JSON IR output.
- `pcbir.cli` — checking, compilation, KiCad export, and footprint inspection
  commands.

Python IR constructions are confined to test fixtures. `.copper` is the only
user-facing source format accepted by the compiler.

The [full-vertical acceptance design](docs/full-vertical-example.md) combines
an STM32G0C1, CAN, an EG800G cellular modem over UART and USB, an nRF52832
Bluetooth slave, low-power motion sensing, GNSS, user I/O, and two SWD ports.
It is the integration fixture for progressing from CopperScript source toward
reviewed manufacturing outputs; its documentation distinguishes implemented
checks from the remaining production-layout work.

## Current ERC checks

- Unknown parts, components, pins, nets, and supply sources
- Duplicate component, net, supply, and interface names
- Pins assigned to multiple nets
- Multiple push-pull or power outputs on one net
- Supply voltages outside declared pin limits
- Invalid, missing, or off-net supply sources
- Disconnected and unsourced power inputs
- I²C signal bindings, pin compatibility, and SDA/SCL pull-ups
- MCU peripheral completeness, package-pin availability, and mux validity
- Duplicate exclusive peripheral/pin selections and incompatible mux-resource settings
- Package DNC/required-pin rules and required net traits
- Functional-unit endpoint resolution to canonical physical pins
- Differential-pair completeness and finite device-mode selections
- Parametric route-rule pad membership and power-domain operating ranges

`power-check` separately evaluates named rail states and warns when a driven net
may back-power an I/O domain declared off. It is intentionally a conservative
steady-state analysis, not firmware or transient simulation.

Physical constraints are separate from connectivity. During physical lowering,
placement regions, keepouts, fixed locations, legal orientations, alignment,
minimum/maximum distance, and explicit placement groups become typed physical
IR and are enforced by `plan-layout`. Hierarchy, interfaces, and proximity
constraints also produce soft semantic placement groups.

The physical Python API also supports [rigid reference-layout clusters](docs/rigid-placement-clusters.md).
Unlike soft groups, these retain identity-bound local footprint poses and
keepouts during whole-cluster placement/refinement and routing feedback,
including explicitly permitted 45-degree rotations. Vendor RF templates and
their qualification are separate work; the example does not yet use a qualified
Nordic/Johanson cluster. An opt-in
`--placement-templates examples/full_vertical/placement_templates.json` scene
now binds the source-extracted Nordic matching macro to pinned KiCad footprints.
It is a provisional adaptation, not the complete vendor support/ground layout;
use the [template preflight command](docs/rigid-placement-clusters.md#source-backed-cli-scene)
before attempting the complete routing workflow with that option.

## Exact assembly-selection prototype

Exact board-side manufacturer/MPN and supplier selections can be prototyped with
`copper assembly snapshot`, `copper assembly check` and `copper assembly bom`.
These are offline checks, not stock lookup or production approval. See
[assembly pinning](docs/assembly-pinning.md) for the workflow and JLCPCB API scope.

## KiCad schematic backend

The initial backend targets KiCad 8's `20231120` `.kicad_sch` format. It embeds
deterministic generic symbols derived from part definitions, places symbols on
a backend-owned grid, and connects pins through short stubs and named labels.
Generated coordinates and UUIDs never enter the CopperScript IR.

This first revision emits one flat sheet. Hierarchical designs are elaborated
explicitly and retain their original qualified component paths in hidden
`CopperScriptPath` properties. Native KiCad hierarchical sheets and mappings to
standard KiCad symbols are planned follow-up work.

## Physical IR and KiCad PCB backend

The physical IR is separate from the electrical IR and stores exact geometry
as integer nanometres. It currently models polygonal board outlines, two-layer
stackups, basic design rules, footprint bodies, pads and courtyards, component
placement, regions, keepouts, fixed/relative rules, semantic groups, physical
net-to-pad assignments, track segments, and vias. It validates cross-references
when constructed.

The KiCad PCB backend targets KiCad 8's `20240108` board format. It emits a
self-contained board with deterministic UUIDs, embedded footprints, net
assignments, copper tracks and vias when present, and a closed `Edge.Cuts`
outline. Each PCB export also writes a same-stem `.kicad_pro` with the physical
IR's minimum clearance and default net-class widths/vias; open the board with
that project for KiCad DRC. Backend tests include a completely routed synthetic
physical board. Generated reference designators are on fabrication layers
until a collision-aware silkscreen labeling pass is available.

The `.copper` frontend supports selected placement, routing, and copper-zone
constraints, while arbitrary board-outline authoring is still pending.
`export-kicad-pcb` resolves selected `.kicad_mod` files into
physical IR, checks that their numbered pads exactly match the electrical part,
and arranges components on a deterministic grid. These unrouted drafts must not
be sent for fabrication. Generic proxy geometry is available only with
`--allow-proxy-footprints` and remains clearly marked in output warnings.

`plan-layout` replaces the inspection grid with a deterministic, clearance-legal
placement and emits coarse routability metrics. It still creates no tracks and
explicitly blocks fabrication sign-off.

## KiCad footprint importer

`load_kicad_mod()` and `parse_kicad_mod()` convert KiCad 6–8 footprint files
into `PhysicalFootprint`. The initial importer covers common SMD, plated and
non-plated through-hole pads, circular and oval drills, rotations, independent
mask/paste presence, round-rectangle ratios, and the usual line, rectangle,
circle, arc, and polygon graphics. Imported geometry can be emitted directly by
the KiCad PCB backend.

The importer is intentionally fail-safe. Custom/trapezoid pads, copper or mask
graphics, drill offsets, and unrepresented fabrication modifiers are rejected.
Presentation-only omissions such as 3D models and user text produce explicit
warnings; `--strict` promotes those warnings to errors. Source format, version,
generator, path, and SHA-256 checksum are retained as footprint metadata.

Managed footprint URLs and manifest-bound `Library:Footprint` identifiers use
the declared provider revision and inventory. Local `.kicad_mod` paths are
relative to the declaring source. Unbound `Library:Footprint` identifiers search
`<root>/Library.pretty/Footprint.kicad_mod` and
`<root>/Library/Footprint.kicad_mod` for each `--footprint-root`. Resolution
fails on zero or multiple matches, and the selected name must agree with the
footprint name declared inside the file. CopperScript never searches an
installed KiCad library implicitly.
