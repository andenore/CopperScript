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
| `copperscript-bench/v1` | `copper-route` | `copper-route/src/ladder.rs` |
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
| `copperscript-si-check/v0.1` | `copper-drc` | `copper-drc/src/signal_integrity.rs` |
| `copperscript-signoff/v0.1` | `copper-route` | `copper-route/src/drc_report.rs` |
| `copperscript-source-macro/v0.1` | `copper-place` | `copper-place/src/requirements.rs` |
| `copperscript-via-process/v0.1` | `copper-pcb` | `copper-pcb/src/kicad_pcb.rs`, `copper-pcb/src/project.rs` |
| `copperscript.assembly-sheet/v1` | `copper-assembly` | `copper-assembly/src/assembly.rs` |
| `copperscript.body-input/v1` | `copper-web` | `copper-web/src/body.rs` |
| `copperscript.body/v1` | `copper-web` | `copper-web/src/body.rs` |
| `copperscript.check/v1` | `copper-lang` | `copper-lang/src/diagnostics.rs` |
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
