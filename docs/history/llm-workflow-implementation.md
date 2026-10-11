# LLM PCB workflow improvements

> Historical record, archived 2026-10-11. Claims and commands below apply to
> their recorded revisions, not current usage or approval. Read only when
> investigating that history; use the current reference guides for new work.

Requested 2026-10-11. CopperScript owns generic algorithms, reports and acceptance
fixtures; library repositories own reusable parts and circuits. CopperLib is one
such library, not a required central catalogue. This work does not change
consumer board sources, dependency pins or manufacturing artifacts.

The objective is fewer total model interactions while retaining explicit,
artifact-bound evidence of what was checked. Source brevity alone is insufficient:
research, failed attempts, report inspection and recovery count toward effort.

## 1. Future: measure the LLM workflow

Do not implement or claim token savings in this iteration.

- Freeze a task corpus: simple sensor board, constrained two-layer controller,
  four-layer mixed-interface board, unfamiliar part integration, replacement
  part, connector relocation, and a changed operating envelope.
- Compare CopperScript and direct KiCad automation with the same model, initial
  specifications, part evidence, tools, compute budgets and acceptance criteria.
  Include both a cold start and reuse of already reviewed library assemblies.
- Record input/output tokens including documentation retrieval, unsuccessful
  tool calls and retries; human interventions; elapsed time; search limits;
  completed connections; engineering findings; and unresolved requirements.
- Separate one-time library authoring cost from marginal board construction cost.
  Publish both rather than hiding preparation in one comparison arm.
- Use multiple attempts per task. Publish distributions, failures and exact
  input/tool identities; a selected successful run is not a success rate.
- Have independent reviewers assess functionality requirements and layout quality.
  Hardware-function results remain pending until item 6 becomes available.
- Deliver a machine-readable evaluation manifest, replay instructions and a
  concise result table. No numerical token-saving claim before this evidence.

## 2. Portable library contracts and selective retrieval

Do not add components to CopperLib as part of this improvement work. Components
may be added in other library repositories when useful, including to demonstrate
and validate the portable contract and retrieval mechanisms. Improve those shared
mechanisms independently of catalogue size; project demand guides prioritization.

- When a project needs a new part or circuit, check its selected library
  dependencies first. Add the reusable definition to an appropriate library
  repository and import it; keep project-specific composition in the project.
- Each assembly needs exact manufacturer/package identities, source revisions
  and locators, required support parts, operating limits, permitted variants,
  pin-associated bypasses and host integration obligations.
- Supply reviewed RF/switched-power macros where applicable, with immutable
  copper, external ports, keepouts and explicit adaptation limitations.
- Model known facts separately from assumptions and unresolved facts. Carry
  qualification requirements with the package; board measurements stay local.
- Add a small searchable capability/index contract so an agent can retrieve one
  assembly's interfaces, requirements and evidence without reading the catalogue.
- Validate pad mapping, hierarchy, exact procurement identity, contract failures,
  macro rotations/ports and deterministic regeneration before publication.
- Publish versioned reviewed bundles; never use a catalogue-wide pass flag to
  disguise an unresolved package obligation.

### Multiple repositories and dependency identity

Treat every library as a package provider under the same versioned contract.
CopperLib, vendor libraries and organization-specific libraries must use the same
discovery, inspection and verification interfaces. No tool should assume that a
component lives in CopperLib or require merging libraries into one repository.

- Build on URL imports, aliases, `copper.mod`, `copper.lock`, cached inventories
  and local replacements already supported by `pcbir.packages`. Current remote
  fetching supports GitHub/GitLab; other sources use local replacements. This is
  a foundation, not a claim of complete Go-style dependency graph handling.
- Identify a definition by module path, resolved revision/content digest,
  package path and exported symbol. A part number or local alias alone is not
  identity. Preserve this identity in retrieved contracts and board evidence.
- Let each library publish a small versioned capability index. Build a cached
  aggregate index from the project's resolved dependency set; return bounded
  search results and load only the selected contracts/evidence. Key cached
  entries by actual content digest, including editable local replacements.
- Keep package summaries descriptive. Resolve source declarations and assets
  from the pinned package and verify index bindings before accepting claims.
  Surface ambiguous matches with their provider identities; never silently
  select a similarly named definition from another repository.
- Design transitive requirements, version conflict handling and cycle diagnostics
  explicitly before supporting cross-library assemblies. Record the complete
  resolved graph and contract schema compatibility; do not silently substitute
  a dependency version or assume Go's version-selection policy fits hardware.
- Keep footprints, macros, models and source evidence tied to their owning
  package and exact consumed asset bytes. Library review evidence remains scoped
  to its declared configuration; board-specific qualification stays local.

Implementation order: define the contract/index schema, add inspection and
selective search over existing resolved packages, then extend dependency-graph
handling where needed. Verify with two small synthetic library repositories,
including same-name exports, a cross-library assembly, pinned updates, offline
replay, stale indexes and local replacements. Additional components in other
libraries are permitted; this work does not expand CopperLib's catalogue.

### Implemented retrieval increment (2026-10-11)

The [bundled agent guide](../agent-guide.md) now exposes a short workflow bootstrap
through `copper agent-guide`, with separately retrievable syntax, library,
verification and recovery topics. Consumer project pointers lead agents to this
entry point without requiring a compiler checkout or copying the full manual.

The [compact library context interface](../library-context.md) now provides a common
source-derived index schema, bounded search over declared providers and selective
inspection of one definition. It preserves source/module digests, resolves
cross-library imports under the existing host-manifest policy, and paginates
interfaces, requirements, properties, notes, composition and exact source.
Source comments and oversized declarations remain explicitly accessible.
Locked/offline is the default; updating a lock or fetching requires explicit flags.
No CopperLib catalogue entries or consumer pins were changed.

Author-reviewed contract bundles, a persistent aggregate cache and automatic
transitive graph/version handling remain future extensions. Current discovery
derives metadata from pinned source instead of trusting independently authored
indexes. The implementation minimizes exposed text without claiming measured
model token savings or electrical qualification from a retrieval result.

## Research foundation for immediate work

The current code already has `routing_benchmark.summarize_run`, streamed phase
events, physical DRC coverage, normalized semantic constraint metadata, exact
explicit-copper connectivity, relative-placement diagnostics, SI return-path
screens and content-bound native refill evidence. Build on these mechanisms.

The gaps are a compact standard-command report, an integrated per-declaration
verification ledger, and repeatable diverse routing acceptance with explicit
quality measurements. Do not equate DRC coverage with a source requirement pass.

Primary external reference: [KiCad 10 PCB editor documentation](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html),
especially design-rule checking and custom rule constraints. KiCad separately
reports rule violations and unconnected items; length/skew rules evaluate stated
geometry and do not establish electrical performance. Follow the same separation.
See also [the IPC API limitations](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/for-addon-developers/):
use the existing qualified native adapter rather than inventing a new backend.

## 3. Immediate: compact, actionable engineering reports

### Design

Add a report projection over existing routing/layout evidence, not another solver.
Default `route-board` output should show routing closure, requirement status,
native verification and geometric quality in a bounded number of lines. Retain
the complete report and native artifacts for inspection.

Emit a small sibling summary JSON with a versioned schema and SHA-256 of the
full report's exact bytes. Include its path in standard output. Add a read-only
`summarize` command accepting an existing routing/layout report, with text/JSON
output and a configurable bounded issue count. It must not create or modify PCB
geometry or upgrade an old report into verified evidence.

Issues carry stage, stable code, subject, source origins when known, explanation,
measurements, and a concrete next investigation. Preserve explicit failed,
blocked and not-run states. Aggregate omitted issue counts without concealing
the existence of failures. Avoid full polygons, grids, traces and repeated search
history in the compact projection. Diagnostics may suggest inspecting or changing
source intent; they must never automatically relax electrical/fabrication rules.

### Integration and acceptance

- [x] Implement a deterministic routing/layout projection and text formatter.
- [x] Generate the summary by default for routing and saved layout reports.
- [x] Add the read-only CLI with validation of supported schema and issue bounds.
- [x] Summarize incomplete package access, ordinary opens, decoupling failures,
  native failures, and failed/blocked requirement rows.
- [x] Bind sidecars to exact full-report bytes and final physical digest when known.
- [x] Test success, missing native evidence, failure truncation, malformed reports,
  unsupported schemas, deterministic output and unchanged input bytes.

No summary is a new signoff certificate. A historical report remains historical;
the compact reader cannot establish that external PCB/source files are unchanged.

## 4. Immediate: per-requirement verification ledger

### Design

Evaluate the elaborated source constraints against the actual final Physical IR.
Preserve declaration ID, kind, targets, mode, source origins, parameters,
implemented verifier, measurements, result and explanation. Bind the ledger to
the serialized electrical source, final physical digest and exact applicable
native evidence. Reject duplicate IDs and stale DRC/native bindings.

Do not infer a pass from the presence of a rule in IR or from a whole-board
`decision=pass`. Reuse existing lowering and geometry predicates for actual
placement, routing and length-match checks. Check each original declaration,
including repeated constraints on a single net; a last-rule-wins map is not
sufficient proof of all requirements.

Placement verification covers pin/component distances, alignment, fixed
position/rotation/side, allowed orientations, footprint containment in regions
and placement keepouts. Placement groups and notes are organizational/advisory:
their declaration does not define a numerical acceptance criterion.

Routing checks cover the implemented geometric limits (width, clearance, allowed
layers, via budgets, total explicit track length and supported pair/group skew).
Parameter coverage must be explicit. Impedance, unsupported return-path/topology
or other engineering claims remain blocked until an applicable verifier exists.
No accepted `impedance_evidence_digest` string alone proves electrical impedance.

Copper zones require exact current native fill evidence as well as preserved
source zone geometry/settings. Via-in-pad permission is distinct from actual
required array multiplicity and confirmed fabrication process. Scoped hole
clearance requires the existing exact DRC check. `assume` never becomes `pass`;
`external` remains blocked without a supported external evidence adapter.
Custom consumer/verifier labels cannot grant themselves authority.

### Integration and acceptance

- [x] Implement a read-only ledger with conservative unsupported-parameter handling.
- [x] Attach it to routing and placement reports. Show future-stage checks as
  blocked during placement rather than claiming routing was executed.
- [x] Keep routing closure distinct from all-requirements completion; offer an
  explicit strict intent gate for users who require all hard declarations verified.
- [x] Test source locations/hierarchy, weaker final net rules, duplicate IDs, missing
  targets, stale evidence, violated and passing placement, routing limits,
  advisory/assumed/external modes and unsupported parameter coverage.
- [x] Ensure changed geometry invalidates old evidence and report bytes.

This ledger covers explicit semantic constraints. ERC, mechanics, decoupling,
macro ownership, package qualification and release requirements also have their
own gates. Do not claim that enumerating this ledger proves all design knowledge.

## 5. Immediate: routing acceptance and geometric quality

### Design

Add deterministic metrics from final explicit copper: total/per-net/per-layer
track length, track/via counts, route-angle distribution, connectivity and
pin-associated surface bypass evidence. Measurements are geometric evidence,
not electrical limits. Never invent universal acceptable bypass lengths or via
counts. Exclude via barrels, zone outlines, filled pours and component-internal
connections from a claimed via-free surface path.

Report return-reference checks from supported filled-copper screening separately;
missing native/normalized fill means incomplete. Preserve hard macros and source
rules; use existing ownership tests to guard immutability.

Create a repeatable bounded acceptance runner using the shared routing pipeline,
with authored finite pad/courtyard geometry rather than proxy footprint emission.
The corpus must include two- and four-layer constrained layouts, obstruction
detours, branched nets, pin-associated bypasses and intentionally blocked access.
Exercise renamed/rotated variants to expose hidden selectors. Expected failures
must remain failures; completing an expectation is not completing a board.

Persist input/final geometry identities, settings, tool identities, native
evidence, quality measurements and per-case expectations. Independent native
verification is an explicit dimension: missing tools yield incomplete rather
than success. Use fresh output directories and retain failures. Commit small
fixture definitions/tests, not bulky generated boards or reports.

### Integration and acceptance

- [x] Add final-copper quality measurements to the standard route report/summary.
- [x] Add an acceptance CLI and diverse bounded fixture corpus, including expected
  failures; publish exact cases/layers/limits and repeat instructions.
- [x] Test metric changes for extra vias/detours, missing surface paths,
  interior track junctions, rotated/renamed cases and stale fill.
- [x] Run the corpus and independent native checks with the installed KiCad.
- [x] Run existing ownership, arrays, package-access, routing/report and CLI
  regressions; report skips and unrelated failures accurately.

These fixtures establish a repeatable software acceptance baseline, not a
population success rate. Larger real consumer boards and future cold-start LLM
tasks supplement the corpus; item 1 owns the statistical workflow evaluation.

## 6. Deferred: fabrication and prototype measurements

Wait for a later user request. No fabrication, component order or hardware test
campaign is initiated by this plan. Future acceptance should cover rail startup,
burst loads, programming, interfaces and applicable RF/thermal/enclosure behavior.

## Implementation evidence

Record actual implemented scope, test commands, results and retained artifact
locations here after verification. Unchecked work is not a completed capability.

### Implemented scope and limits

`pcbir.engineering_report` implements the projection, exact report-byte binding,
bounded issues and next-investigation text. `copper summarize` is read-only;
successful reading returns 0 even when the reported board has failed gates.
Unsupported/malformed input returns 2. The default issue cap is eight; callers
can request 0–100. Historical reports without new fields display `not_reported`.

`pcbir.requirement_verification` evaluates elaborated declarations using existing
physical lowering and exact predicates. Placement distance decisions use squared
integer distances, avoiding display-rounding errors. Fixed poses, orientation,
alignment, footprint regions and keepouts are checked independently. Routing
width, total explicit length, via count, allowed layers and supported pair/group
skew are checked against original source intent. Exact clearance coverage is
reused only when the current bound DRC's effective rule is at least as strict;
otherwise it is blocked rather than falsely passed. The physical model already
rejects duplicate routing rules; a weaker mutated final rule also cannot hide an
original stricter requirement.

Unsupported routing parameters (including impedance and complete pair gap,
return-policy, stub, topology and tuning obligations) remain blocked. Source
custom consumer/verifier labels have no registered authority. A required via
array can report present geometry while retaining a blocked fabrication-process
obligation. Hole-clearance finding attribution is conservatively board-wide.
Notes/groups are advisory; assumptions retain `assumed`. External obligations
are hard blocked, including explicit external notes. No new waiver/evidence
adapter or manufacturing authorization is introduced.

Routing reports retain their existing routing `status` and add
`requirements_complete`, `requirements` and `routing_quality`. Default exit codes
retain the existing routing gates; `--require-verified-intent` adds the hard
declaration gate. Saved layout reports show route-stage declarations as blocked.

`pcbir.routing_metrics` measures final tracks, vias, layers and angles, rather
than summing intermediate solver counters. Surface bypass connectivity uses the
shared exact contact graph with only the chosen surface tracks and without vias,
polygons, zone fills or component-internal edges. It handles interior junctions.
The reported connected-component track length includes branches: it is explicitly
not the shortest pad-to-capacitor path. Pad distance is a separate measurement;
neither has an invented universal electrical limit.

Reference coverage uses the existing conservative native polygon screen only
with current export-bound fill and an explicit reference net/layer. It states
projected-envelope coverage at zero extra margin, not impedance/return inductance
qualification. Missing geometry support or reference declarations is incomplete.

`pcbir.routing_acceptance` routes eight cases through `run_routing_pipeline`,
with fixed authored intent and recorded bounded settings. Base cases: two-layer
obstacle detour, four-layer crossing, two-layer branched bypass, and deliberately
blocked two-layer access. Each also has a renamed/270-degree-rotated variant.
Per-case via/length bounds are explicit fixture expectations, not general design
rules. The runner records source/final physical digests, implementation content
digest, Python/platform, quality, native tool/report/artifact identities and
expectations. It rejects reused output directories and retains expected failures.
Missing native evidence makes the overall result incomplete and CLI exit 1;
an expectation regression makes it fail. Expected-open cases are never described
as connected boards.

Existing `routing_benchmark.compare_runs` now records final-copper deltas and
blocks neutral speedup claims when measured length/via costs increase. Such an
increase is an explicit layout tradeoff, not an invented electrical defect.
Lost surface/reference/required-declaration evidence is a regression. Legacy
reports can still be compared with other legacy reports; mixing a measured and
unmeasured final-copper result blocks that comparison.

### Verification (2026-10-11)

- Broader regression command: new reporting/requirement/acceptance suites plus
  `test_cli`, `test_routing_benchmark`, `test_physical_drc`, `test_native_checkpoint`,
  `test_pad_via_arrays`, `test_hard_macros`, and `test_package_access`:
  **304 passed, 1 optional skip**. This is not a full-repository test claim.
- Final focused check after native-binding, hierarchy, comparison, deferred-zone
  summary reconciliation and standard-command strict-gate integration:
  `test_requirement_verification`, `test_engineering_report` and
  `test_routing_benchmark`, plus `test_routing_acceptance`: **97 passed**.
  Counts overlap the broader run.
- Independent KiCad 10.0.6 acceptance: **8/8 case expectations passed**. Six
  intended connected results each have zero opens/violations; two intentionally
  blocked results each retain one native open and zero other violations.
  Evidence: ignored `build/routing-acceptance-final-d68ddf7ed65d429e856beeb1dbef9d95/`.
  The implementation digest remained stable throughout that run.
  The first attempt exposed a synthetic bypass silk/mask overlap. Its fixture
  body envelope was corrected; no design rules were weakened or findings waived.
- Test temporary directories use fresh ignored `build/` paths because this
  session's default sandbox temporary root could not create pytest directories.

Scope remaining for future extensions: measured shortest bypass paths, complete
final pair/return/stub/tuning verifiers, supported external evidence adapters,
larger dense real-board acceptance and statistical success rates. Missing checks
are reported explicitly. Item 1 and item 2's reviewed-bundle/graph extensions
remain future work; item 2's retrieval increment is implemented. Item 6 remains
deferred as requested.
