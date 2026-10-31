# Browser routing lab

`/CopperScript/demo/routing/` runs the actual `copper-route` pipeline in a
separate WASM worker. The compiler playground retains its smaller module.
No routes or outcome geometry are precomputed in JavaScript.

## User interaction

- **Route around it:** two fixed, front-side synthetic resistor footprints;
  two ordinary signals route around a both-layer rectangular keepout.
- **Allow a layer change:** the keepout spans the board on F.Cu only; both
  layers and two vias per net are allowed. Actual vias and B.Cu tracks form
  the crossing. The keepout disappears when viewing B.Cu alone.
- **Block the corridor:** the keepout spans the board on both layers. The
  bounded run returns incomplete results with open nets; the UI never substitutes
  a successful prerecorded route or labels search exhaustion as impossibility.
- **Before / After**, **Layer** and **Net** controls inspect the returned scene.
  Dashed lines show desired connections before routing or remaining open nets.
  Both sides are viewed from above, without mirroring.
- Expand the source, change geometry or constraints, and press **Route board**
  (or Ctrl/Cmd+Enter). Every edit immediately clears the previous geometry and
  findings. A result from an older revision is discarded. Stop or timeout
  terminates the worker; rerunning starts a fresh instance with source preserved.

## Implementation and scope

`copper-web-route` parses and compiles self-contained source, checks ERC,
physicalizes explicit proxy footprints, then calls
`copper_route::flow::run_routing_pipeline`. The pipeline retains source-fixed
poses and uses the same global routing, unified negotiation, exact commit checks
and internal final DRC as the native implementation. No solver algorithm or
recovery policy was changed. `copper-report::scene_from_physical` supplies the
drawn pad polygons, tracks, vias and footprint poses.

The adapter accepts at most 8 KiB, four fixed components, four ordinary nets,
16 pads, eight keepouts with at most 16 vertices each, a 16-vertex outline within
0..40 by 0..30 mm, and no holes, slots, zones, fixed copper polygons, via arrays or macros. Unsupported
requests fail explicitly. Native package resolution and filesystem operations
are never enabled.

The pipeline uses one placement candidate/seed, three global iterations, five
unified iterations, 15,000 search expansions per path and two exact commit rounds.
The base pitch is 0.25 mm; via cost is 0.5 mm. Geometry/clearance rules remain
source-owned. Placement feedback is disabled and every component must have a
fixed position. Native wall-clock routing deadlines are unset; the browser has
a 12-second worker watchdog and a 30-second loading deadline. Work bounds stay
active inside the engine.

This is synthetic teaching geometry, not a manufacturer-backed circuit.
`native_verified` and `qualified_release` are always false. The UI distinguishes
connection completion and internal DRC from native KiCad, electrical/RF/power,
assembly, CAM and hardware qualification. `drc` includes the standard coverage
and findings, without issuing its signoff token to the browser.

## Build and checks

From the repository root, use the [main demo setup](../README.md). The build
script generates ignored `routing/pkg/` assets and the Pages workflow builds
and checks both modules before publication.

```console
cargo test --locked --release -p copper-web-route
node scripts/build_demo.cjs
node scripts/check_routing_demo.cjs
npm ci --prefix tests/browser-demo --ignore-scripts --no-audit --no-fund
node scripts/check_routing_ui.cjs
node scripts/check_site.cjs
node scripts/serve_demo.cjs
```

The WASM check exercises all three cases, fixed-pose preservation, no initial
tracks, actual B.Cu tracks and vias, incomplete routing, internal DRC, input
limits, and recovery. It also loads the actual worker with Node message/fetch
adapters to verify revision IDs and recovery from malformed source. This does
not replace browser visual/interaction QA.

`check_routing_ui.cjs` runs the actual page logic against the real WASM router
in a test-only DOM emulator. It checks SVG visibility, before/after switching,
layer and net filters, blocked outcomes, stale-result rejection, and Stop/restart.
It does not measure browser layout. The DOM tool is never shipped to visitors.

The existing benchmark ladder now includes a bounded `browser-routing-demo`
case. It intentionally uses the CLI's proxy-footprint option and establishes
only the same synthetic routing scope. Compare ordinary ignored build outputs:

```console
copper bench bench/ladder.json --only browser-routing-demo --out build/bench/browser-routing-before
copper bench bench/ladder.json --only browser-routing-demo --out build/bench/browser-routing-after --baseline build/bench/browser-routing-before/results.json
```

When the first report is present, `check_routing_demo.cjs` also compares the
native CLI's detailed metrics with the WASM detour case. Production consumer
cases in the ladder are unchanged. Timings shown in the lab include pipeline
execution, scene conversion and JSON transfer preparation, not module download,
rendering or a claim about large-board performance.
