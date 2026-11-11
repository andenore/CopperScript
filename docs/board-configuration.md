# Board configuration and build settings

Status: first implementation slice, 2026-11-10. The source-driven layer
selection described below is implemented. New fabrication and build blocks
are proposals, not accepted syntax yet.

## What belongs where

| Setting | Owner and representation |
| --- | --- |
| Outline, cutouts, thickness, dielectric properties, copper stack | Board `mechanical` declarations; derive the layer count from `stackup` |
| Minimum clearance, track and drill dimensions | Board `mechanical.rules`; fabrication presets supply defaults, not qualification |
| Net widths, permitted layers, pair gaps, skew, length, via limits, return paths | Existing `constraint routing(...)` and `constraint length_match(...)` declarations |
| Planes, split regions and keepouts | Existing source copper-zone and mechanical declarations |
| Fixed poses, allowed orientations, placement regions, rigid macros | Existing constraints and source-bound macros |
| Library parts, reusable circuits, package exits and manufacturer layout structure | Versioned dependencies; source selects and binds them; `copper.lock` pins their inputs |
| Placement candidate count, routing effort, iteration/search limits | Proposed optional board `build` settings, overridable per invocation |
| Congestion penalties, lattice experiments, individual recovery/search switches | Expert CLI overrides until there is a measured, stable user contract |
| Output/report paths, progress, checkpoint destinations, executable paths, threads | Invocation or machine configuration |
| Locked/offline dependency policy, proxy footprints, skipped checks, verification backend | Invocation/build workflow; never silently enabled by an imported circuit |

The important distinction is whether a setting changes the board's physical
requirements, the amount/strategy of computation, or the environment running
that computation. For example, `max_vias` is a net requirement;
`via_cost_mm` is a solver preference. A search pitch is not a manufacturing
grid. A time budget can change the result across machines; an iteration
budget alone does not promise identical results across compiler versions.

Existing source rules override fabrication preset defaults. Moving preset
selection into source must preserve that existing meaning and must not imply
that a preset certifies the manufacturer's current process capabilities.
The current JLCPCB presets contain rule defaults; they do not supply a
manufacturer-qualified physical dielectric stackup.

## Research and rationale

- CopperScript already lowers stackup, rules, zones and net routing intent
  from source. CLI tools independently defaulted to two copper layers,
  while `copper-editor::configuration` inferred layers from source. This
  duplicated choice was the first inconsistency to remove.
- [KiCad's board format](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/)
  stores physical stackup in board setup. This supports keeping physical
  construction with the board artifact.
- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
  keep build preferences in the project and permit configuration overrides.
  [Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html)
  separately accommodates invocation and environment settings. This is a
  useful precedent for overridable computation preferences, not for allowing
  a CLI flag to override a declared electrical requirement.

## Implemented: one layer-selection policy

Physical CLI commands and the editor use `copper-pcb::configuration`:

1. An explicit `--layers` must agree with a declared physical stackup.
2. Without the flag, a declared stackup selects its exact copper layer count.
3. Without either, choose the smallest supported stack (2, 4 or 6) that
   accommodates source routing/zone/reference-layer constraints, mechanical
   copper keepouts, source-bound hard macros and the selected fabrication
   profile.
4. Explicit selections remain strict. Requirements outside the selected
   stack produce an error; no constraint is rewritten or weakened.

No physical stackup is invented when only a layer count is inferred.
Dielectric thickness and material evidence remain absent until declared.
Macro loading observes the caller's locked/offline policy. Legacy external
`--hard-macro` scene files are still applied after source configuration;
their extra layer needs require an explicit compatible count or source
stackup. Boards with no layer requirements continue to use two layers.

This applies to `export-kicad-pcb`, `plan-layout`, `route-global`,
`route-board`, `si-check`, `emi-check`, `synthesize-antenna`, the editor and panel source
intent loading. The bounded browser routing prototype deliberately retains
its explicit two-layer limit. Low-level `PhysicalOptions::new` callers still
select an explicit stack.

### Verification

The regression tests compare automatic and explicit four-layer exports,
placements, routed boards, global guides, SI output and editor scenes. They
also reject conflicting flags across commands and check inference from routing,
reference layers, zones, keepouts and fabrication profiles. Physicalization
preserves declared dielectric properties; inferred counts add none.

`cargo fmt --all -- --check` passes. The release workspace suite reported
1,503 passing tests and six ignored tests. Six tests encountered Windows
sandbox restrictions on loopback sockets, temporary files or local Git clones;
all six passed when their affected tests were rerun outside the sandbox,
bringing the total to 1,509 passing tests. No code changes were needed for
those reruns. The suite used the same release profile overrides as the ladder.

The bounded benchmark ladder on 2026-11-10 produced byte-identical KiCad boards
before and after this change:

| Board | Opens / connections | Physical DRC errors | Vias | Length (mm) |
| --- | ---: | ---: | ---: | ---: |
| browser-routing-demo | 0 / 2 | 0 | 0 | 48.142 |
| browser-routing-weave | 0 / 8 | 0 | 12 | 332.685 |
| round-led-ring | 1 / 51 | 0 | 44 | 600.718 |
| nrf52 | 0 / 35 | 0 | 44 | 349.166 |
| cm4 | 0 / 21 | 0 | 66 | 605.232 |

Both builds used release optimization with LTO disabled and 16 codegen units.
Both ladders added `--offline`. Timing is not a performance claim because of
concurrent work and cache effects. Native refill/DRC and the large consumer
boards were not part of this bounded comparison. Local ladder results are in
`build/bench/source-configuration-before-offline/results.json` and
`build/bench/source-configuration-after/results.json`.

## Next slices

1. Add board-owned fabrication preset selection, with a located error if an
   explicit CLI preset disagrees. Keep it separate from the physical stackup
   and serialize it into compiled IR. Reusable definitions must not change
   the consumer board's fabrication selection.
2. Add a small typed `build` block, initially placement candidates and routing
   effort. Scope it to the top-level board. Reject unknown keys and units at
   their source locations; never pass arbitrary strings through as CLI args.
3. Define precedence: explicit CLI build settings, board build settings,
   compiler defaults. Expand an effort preset before applying explicit field
   overrides. Requirements remain authoritative regardless of precedence.
4. Expose effective configuration and origins without running the solver.
   Persist resolved configuration with compiler identity in reports and
   checkpoint compatibility checks. Changing physical requirements must
   invalidate incompatible checkpoints; changed search budgets should be
   recorded without pretending to change the board's requirements.
5. Migrate consumer Make settings only after equivalent explicit and
   source-configured builds have matched on the benchmark ladder. Preserve
   CopperAssetTracker, CopperLedRing and the examples throughout.

Proposed future syntax inside a board (not currently supported):

```copper
build {
    placement { candidates = 3; }
    routing { effort = balanced; }
}
```

The first slice deliberately reuses existing source syntax. It provides a
shared place to resolve later board settings without adding another CLI-only
or editor-only interpretation.
