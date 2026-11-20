# Output and input schemas

Every JSON document CopperScript writes or reads names its format in a
`schema` field. This page lists each identifier and where it is defined, so
a consumer can find the format's code and a contributor can see what a
change touches. A test (`crates/copper/tests/schemas.rs`) fails when a source
file names an identifier that is not listed here, or when a listed one no
longer appears in any source.

Readers reject a document whose schema they do not support rather than
guessing (see [the design specification](design-specification.md)). A
change to a document's shape takes a new version; readers may keep accepting
older versions, as `copperscript-assembly-lock` does.

| Schema | Crates | Defined in |
| --- | --- | --- |
| `copperscript-antenna-synthesis/v0.1` | `copper-antenna` | `copper-antenna/src/report.rs` |
| `copperscript-assembly-lock/v0.1` | `copper-assembly` | `copper-assembly/src/assembly.rs` |
| `copperscript-assembly-lock/v0.2` | `copper-assembly` | `copper-assembly/src/assembly.rs` |
| `copperscript-assembly-lock/v0.3` | `copper-assembly` | `copper-assembly/src/assembly.rs` |
| `copperscript-assembly-lock/v0.4` | `copper-assembly` | `copper-assembly/src/assembly.rs` |
| `copperscript-assembly-offers/v0.1` | `copper-assembly` | `copper-assembly/src/offers/mod.rs` |
| `copperscript-assembly-report/v0.1` | `copper-assembly` | `copper-assembly/src/assembly.rs` |
| `copperscript-bench/v0.1` | `copper-lang` | `copper-lang/src/bench.rs` |
| `copperscript-bench/v1` | `copper-route` | `copper-route/src/ladder.rs` |
| `copperscript-build/v1` | `copper` | `copper/src/build.rs` |
| `copperscript-cam-comparison/v0.1` | `copper-cam` | `copper-cam/src/compare/mod.rs` |
| `copperscript-critical-replay/v1` | `copper` | `copper/src/replay_critical.rs` |
| `copperscript-critical-route/v0.1` | `copper-route` | `copper-route/src/critical.rs` |
| `copperscript-detailed-route/v0.1` | `copper-route` | `copper-route/src/detailed.rs` |
| `copperscript-drc/v0.1` | `copper-cam` | `copper-cam/src/drc.rs` |
| `copperscript-editor-overlay/v0.1` | `copper-editor` | `copper-editor/src/overlay.rs` |
| `copperscript-editor-route-intent/v0.1` | `copper-editor` | `copper-editor/src/overlay.rs` |
| `copperscript-editor-scene/v0.1` | `copper-editor` | `copper-editor/src/scene.rs` |
| `copperscript-engineering-summary/v0.1` | `copper-place`, `copper-report` | `copper-place/src/summary.rs`, `copper-report/src/summary.rs` |
| `copperscript-filled-feedback/v0.1` | `copper-route` | `copper-route/src/filled_feedback.rs` |
| `copperscript-final-geometry/v0.1` | `copper-place` | `copper-place/src/requirements.rs` |
| `copperscript-footprint-audit/v0.1` | `copper` | `copper/src/audit_footprints.rs` |
| `copperscript-global-route/v0.1` | `copper-route` | `copper-route/src/routing.rs` |
| `copperscript-ir/v0.1` | `copper-lang` | `copper-lang/src/serializer.rs` |
| `copperscript-layout-report/v0.1` | `copper-place`, `copper-report` | `copper-place/src/layout.rs`, `copper-place/src/summary.rs`, `copper-report/src/summary.rs` |
| `copperscript-manufacturing-files/v0.1` | `copper-assembly` | `copper-assembly/src/manufacturing.rs` |
| `copperscript-panel/v0.1` | `copper-panel` | `copper-panel/src/lib.rs` |
| `copperscript-physical-drc/v0.1` | `copper-route` | `copper-route/src/drc_report.rs` |
| `copperscript-placement-templates/v0.1` | `copper-pcb` | `copper-pcb/src/templates.rs` |
| `copperscript-plane-verification/v0.1` | `copper-route` | `copper-route/src/plane_verify.rs` |
| `copperscript-requirement-verification/v0.1` | `copper-place` | `copper-place/src/requirements.rs` |
| `copperscript-route-board/v0.1` | `copper`, `copper-place`, `copper-report` | `copper-place/src/summary.rs`, `copper-report/src/issues.rs`, `copper-report/src/summary.rs` … |
| `copperscript-route-error/v1` | `copper-route` | `copper-route/src/iteration_checkpoints.rs` |
| `copperscript-route-summary/v1` | `copper-report`, `copper-route` | `copper-report/src/issues.rs`, `copper-report/src/summary.rs`, `copper-route/src/route_summary.rs` |
| `copperscript-route-timings/v1` | `copper-route` | `copper-route/src/iteration_checkpoints.rs` |
| `copperscript-routing-acceptance/v0.1` | `copper-route` | `copper-route/src/routing_acceptance.rs` |
| `copperscript-routing-quality/v0.1` | `copper-route` | `copper-route/src/routing_metrics.rs` |
| `copperscript-emi-check/v0.1` | `copper-drc` | `copper-drc/src/emi.rs` |
| `copperscript-si-check/v0.1` | `copper-drc` | `copper-drc/src/signal_integrity.rs` |
| `copperscript-sim/v0.1` | `copper-simulate` | `copper-simulate/src/results.rs`, `copper-simulate/src/noise.rs` (`noise`), `copper-simulate/src/variation.rs` (`variation`); a component simulated by a `sim spice` block has fidelity `vendor` and model `spice SUBCKT (file, dialect, sha256 ...)` (`copper-simulate/src/spice.rs`); a transient bench's `analysis` kind `tran` (optional `uic`), step `transient` and plot axis `time_s`, a dc sweep's `analysis` kind `dc` (`sweep`, `units`, `dimension`, `from`, `to`, `points`) and plot axis `sweep`, a scoped limit's optional `where`, `covered_steps` and per-step `covered`: `copper-simulate/src/results.rs` |
| `copperscript-signoff/v0.1` | `copper-route` | `copper-route/src/drc_report.rs` |
| `copperscript-source-macro/v0.1` | `copper-place` | `copper-place/src/requirements.rs` |
| `copperscript-via-process/v0.1` | `copper-pcb` | `copper-pcb/src/kicad_pcb.rs`, `copper-pcb/src/project.rs` |
| `copperscript.assembly-sheet/v1` | `copper-assembly` | `copper-assembly/src/assembly.rs` |
| `copperscript.body-input/v1` | `copper-web` | `copper-web/src/body.rs` |
| `copperscript.body/v1` | `copper-web` | `copper-web/src/body.rs` |
| `copperscript.check/v1` | `copper-lang` | `copper-lang/src/diagnostics.rs` |
| `copperscript.emi-model/v0.1` | `copper-lang` | `copper-lang/src/emi.rs` |
| `copperscript.escape-diagnostics/v0.1` | `copper-route` | `copper-route/src/escape_plan.rs` |
| `copperscript.library-context/v1` | `copper-lang` | `copper-lang/src/library_context.rs` |
| `copperscript.library-index/v1` | `copper-lang` | `copper-lang/src/library_context.rs` |
| `copperscript.package-lock/v1` | `copper-lang` | `copper-lang/src/packages.rs` |
| `copperscript.playground/v1` | `copper-web` | `copper-web/src/lib.rs` |
| `copperscript.project-plan/v1` | `copper-web` | `copper-web/src/lib.rs` |
| `copperscript.project-snapshot/v1` | `copper-lang` | `copper-lang/src/snapshot.rs` |
| `copperscript.query/v1` | `copper-lang` | `copper-lang/src/query.rs` |
| `copperscript.review-analysis/v1` | `copper-web` | `copper-web/src/lib.rs` |
| `copperscript.routing-playground/v1` | `copper-web-route` | `copper-web-route/src/lib.rs` |
| `copperscript.run-issues/v1` | `copper-report` | `copper-report/src/issues.rs` |
| `copperscript.state/v1` | `copper-state` | `copper-state/src/lib.rs` |

The route report (`copperscript-route-board/v0.1`) may carry one optional
top-level key, `emi`: a `copperscript-emi-check/v0.1` document, or
`{ "evidence_grade", "error" }` when the check could not run. `route-board`
writes it for designs with a switching cell, a finding or an executed signal or cable check (section 15.12), and `copper emi-check BOARD --run RUN
--write` adds it to an existing run (`copper/src/emi_section.rs`); `copper
issues` reads it as stage `emi`. See [EMI noise analysis](emi-noise-analysis.md).

### Additions to `copperscript-emi-check/v0.1` (the signal and cable checks)

The return-path, mode-conversion and cable checks (sections 15, 16 and 18 of [EMI noise
analysis](emi-noise-analysis.md)) only add values and keys to the document; no existing field changed its meaning or its
shape, so the version stays `v0.1`. A reader that ignores unknown findings, estimate kinds, coverage entries and
`measures` keys reads the new document as it read the old one.

- **Finding codes.** `EMI-RETURN-PATH` (one per signal or pair with something to report), `EMI-MODE-CONVERSION` (one per
  differential pair), `EMI-CABLE-COMMON-MODE` (one per cable connector with something to report) and `EMI-CABLE-SHIELD`
  (a connector that declares `cable_shielded = true` and no shield pin). They have `cell: null`; a board with no
  switching cell has `cells: []` and the document is still written (section 15.12).
- **`estimate.kind`.** `ranking_score` (existing; now also the `slot_ranking_score` of `EMI-RETURN-PATH`, in dB relative to a
  reference product, not a field), `voltage_ratio` (`mode_conversion_ratio`, dB, a ratio of two voltages of one network)
  and `common_mode_voltage` (`common_mode_voltage_at_connector` or `first_arrival_voltage`, dBuV against the board reference at
  the connector). None of them has a `margin_db`, and a cable or conversion finding has no field and no current. Findings
  are ordered by kind of estimate: absolute estimates, conducted levels, ranking scores, voltage ratios, connector voltages,
  findings with no estimate, `EMI-ACCEPT-UNUSED` last.
- **Coverage.** The entries `EMI-RETURN-PATH`, `EMI-RETURN-PATH:<net>`, `EMI-MODE-CONVERSION`,
  `EMI-MODE-CONVERSION:<positive net>`, `EMI-CABLE-COMMON-MODE` and `EMI-CABLE-COMMON-MODE:<connector>`, each `executed`,
  `not_applicable` or `unsupported` with a reason (the status vocabulary is the old one). The `EMI-RETURN-PATH` and
  `EMI-CABLE-COMMON-MODE` entries existed as `unsupported`; they now carry the real status. The `EMI-MARGIN` entry says
  cable common mode is voltages and a ranking only.
- **`measures` keys.** `EMI-RETURN-PATH`: `subject`, `reference`, `routed_length_mm`, `uncovered_length_mm`, `gaps[]`
  (`position_mm`, `slot_length_mm`, `resonance_hz`), `reference_changes[]`, `members[].layers[]` (`plane_edge`,
  `board_edge` with `at_mm`), `thresholds`, `ranking` (section 15.8). `EMI-MODE-CONVERSION`: `pair`, `subject`, `ratio`,
  `references`, `sections[]`, `parts[]`, `elements_in_order[]`, `observation_planes[]` (section 16.5).
  `EMI-CABLE-COMMON-MODE`: `connector`, `part`, `cable`, `shield`, `differential_pairs[]`, `ground_noise[]`,
  `ground_noise_not_counted[]`, `single_ended_signals[]`, `switch_node_coupling[]`, `ranking` (section 18.4); the per-driver
  spectrum lines carry `quantity` (`continuous_bound`, `pattern_lines:<pattern>`, `random_data_in_rbw`, clock lines),
  `frequency_hz` and `voltage_dbuv`. `EMI-CABLE-SHIELD`: `connector`, `kind`.

The `issues` listing of stage `emi` (`copperscript.run-issues/v1`, `copper issues RUN --stage emi`) groups the findings by
subject kind (converter cells, then signals and pairs, then cable connectors) and adds `details.causes` to
`EMI-RETURN-PATH` (`kind`, `at_mm`, `text`), `EMI-MODE-CONVERSION` (`kind: routed_section`, `section`, `text`; the measures
carry no coordinates, so the issue is located by the pads of the pair's nets) and `EMI-CABLE-COMMON-MODE` /
`EMI-CABLE-SHIELD` (one cause per driver: `kind`, `name`, `status`, `quantity`, `termination`, `value_dbuv` and, for a driver
that is ranking only, `ranking_value` with `ranking_unit`; the issue is located at the connector's pads).
