# Routing refactor: handoff (2026-10-21)

Where the routing refactor stands, what is open, and how to get it merged to
`main`. The plan, phase table and decisions are in
[routing-refactor-plan.md](routing-refactor-plan.md); this note is the
starting point for the next session.

## Goal for the next session: merge everything to `main`

Everything on `refactor/routing` should end up on `main`. The branch is
55 commits ahead of `main` (187 files) and is merged up to date with
`origin/main` at `32363be` (PRs #1–#12). See "Merging to main" below for the
order and the checks.

## Where things are

| What | Where | State |
| --- | --- | --- |
| CopperScript code | GitHub `andenore/CopperScript`, branch `refactor/routing` | pushed; open a PR from https://github.com/andenore/CopperScript/pull/new/refactor/routing |
| CopperProbe main board | `dev/CopperProbe` (owner's PC) | local commits `f989beb`, `9ddffc0`; **no GitHub remote yet** |
| Previous CopperProbe base design | `dev/CopperProbe/design-candidates/base-original-20261016` | kept, committed |
| Benchmark results so far | cloud session only | lost with that session; rerun locally (below) |

Leftovers to delete in `dev/CopperScript`: `copperscript-refactor-routing.bundle`
and `main-2ec5ee3.bundle` (transport files from the cloud session, superseded
by the pushed branch).

## Running the benchmark locally

Expected folder layout (as the board sources already assume): `dev/CopperScript`,
`dev/CopperProbe`, `dev/CopperProbeParts-hybrid`, `dev/CopperLib`,
`dev/CopperAssetTracker`, `dev/CopperLedRing`.

```sh
cd dev/CopperScript
git fetch origin && git checkout refactor/routing
export COPPER_BOARDS=..            # the dev folder
export KICAD_FOOTPRINTS=<path to a kicad-footprints checkout>
copper bench bench/ladder.json --only asset-tracker,nrf52,full-vertical,cm4,led-ring,round-led-ring \
    --extra-arg=--router --extra-arg=unified --label ladder --out build/bench/ladder
copper bench bench/ladder.json --only copper-probe \
    --extra-arg=--router --extra-arg=unified --label probe --out build/bench/probe
copper issues build/bench/probe/copper-probe        # grouped failures, located
copper view build/bench/probe/copper-probe --issue R1
```

`copper bench` is `pcbir/ladder.py` (renamed from `pcbir/bench.py` in the
merge: `main`'s `pcbir/bench.py` is the KiCad/PCBWorld routing benchmark).
To iterate on the router alone, add
`--extra-arg=--unified-save-inputs --extra-arg=STATE.json` once and then use
`pcbir.unified_router.replay_unified(STATE.json, **option_overrides)`.

## Benchmark numbers (unified router, 2026-10-21)

| Board | Completion | Open | DRC | Notes |
| --- | ---: | ---: | ---: | --- |
| asset-tracker | 1.0 | 0 / 120 | 0 | 0 estimated plane opens (pour re-check) |
| full-vertical | 1.0 | 0 / 86 | 0 | neck-exit fix |
| nrf52 | 1.0 | 0 / 35 | 0 | |
| cm4 | 1.0 | 0 / 21 | 0 | |
| led-ring | 1.0 | 0 / 51 | 0 | |
| round-led-ring | 0.98 | 1 / 51 | 0 | DEC3; example is deprecated on `main` |
| copper-probe | 0.873 | 30 / 237 | 1 | new main board, all critical nets routed |

CopperProbe history: 0.82 (start) → 0.844 → 0.857 (21 badly fixed parts
released) → 0.873 (as committed as the main board). The one DRC error is
DRC-MAX-LENGTH on SENSE (max 4 mm).

## What the branch adds (since the plan's phase table)

All in `pcbir/unified_router.py` unless noted; options are on
`UnifiedRouterOptions`.

- Pour re-check (`pour_recheck`, on): after commit, the raster pour estimate
  finds plane pads walled off by signal copper; their plane net is rerouted,
  then with explicit copper (`explicit_pads`) after ripping up the crowd.
- Neck exits: a node legal only at neck width is never left towards a node
  outside its breakout region (Python search and `native/astar.c`; kernels
  rebuilt for Linux and Windows).
- Buses first (`buses_first`, on): members of a length-match group negotiate
  among themselves first. Measured on CopperProbe replays: +2.1 points on one
  placement, +0.9 on another (with length priority on).
- Length priority (`length_priority`, **off**): a max_length net that detours
  too far retakes its shortest path. It cost 2 points on CopperProbe
  (0.873 → 0.852), so it is off until it is smarter.
- Placement (`pcbir/placement.py`): automatic differential-pair corridors;
  satellite seeding; refinement swaps treat escape-width differences under
  0.1 mm as ties (crystal caps were swapped 12 mm away for 10 um).
- Diagnostics: placement findings in route reports (`REMOTE_FIXED_PART`,
  `FIXED_PART_IN_PAIR_CORRIDOR`), plane-estimate cut-off pads, both shown by
  `copper issues` (W and E ids).
- Plane stitch: `repair_cut_off_plane_pads` (`pcbir/plane.py`) re-stitches
  pads the estimate finds cut off.

## Open items, in priority order

1. **Buses first with length priority off.** The last A/B (buses first on,
   length priority off) on the CopperProbe main board was still running when
   the cloud session ended. Rerun it and keep `buses_first` on only if it
   does not lose against 0.873.
2. **CopperProbe ADC bus (about 11 of the 30 opens).** The data lines route
   alone on F.Cu with no via, but lose the corridor south of the FX10 to
   other nets (max_vias = 2, max_length = 35 mm). Next lever: an escape plan
   or corridor reservation for the bus at the FX10 south edge, like the pair
   corridors.
3. **Length rules in the router.** The router ignores `max_length` (only DRC
   sees it). Length priority as built hurts; a better form would be a
   per-net detour budget in the search, or ripping up only the nets that
   block the short path.
4. **Small parts under a BGA.** The FX10 kit puts R_RESREF on B.Cu under the
   package, 1.2 mm from its ball; the placer never changes a part's side, so
   it sits about 7 mm away on F.Cu. Either allow side changes for satellite
   passives of ball-field pins, or author `fixed_placement(R_RESREF) { side = back; }`.
5. **Remaining CopperProbe opens:** CC1/CC2/CC_OUT*, VBUS, FB, FX_VBUS, FX_V33
   (power with width rules), ACQ_EN, SWDIO, three GND balls the pour
   re-check could not reach.
6. **round-led-ring DEC3.** Low priority: the example is deprecated on `main`;
   consider dropping it from `bench/ladder.json`.
7. **Freerouting comparison (D2)** and a Windows run of the native DLL are
   still not done.

## Merging to main

The aim is one PR, `refactor/routing` → `main`, merged once the checks hold.

1. Rebase or merge `origin/main` again right before the PR (it moves fast).
2. Tests: the full suite (2026-10-21, after the merge with `origin/main`) has
   149 failures, all already failing on the branch before this work (power-transition repair, autonomous plane
   access, CAM corpus, CI bundle). Check which of them also fail on `main`;
   any that fail only on the branch must be fixed or explained before merging.
3. Default router: `copper route-board` still defaults to `--router legacy`.
   Decide whether the merge switches the default to `unified` (all ladder
   numbers above are with `unified`). If it does, the legacy-only paths that
   the unified flow skips (late zone-escape repair, power-preparation
   allocator) need a decision: keep behind `--router legacy` or retire (plan
   decision D4 allows deleting retired code).
4. Run `main`'s CI on the branch: `routing-benchmark.yml` (PCBWorld D3 boards
   scored by KiCad, baseline 81 / 99 clean in
   `.github/routing-benchmark-baseline.json`) and `board-routing.yml`. The
   D3 benchmark routes with the detailed router through `pcbir.bench`; if the
   default router changes, compare both.
5. Run the ladder (above) on the PR head and record the table in
   `routing-refactor-plan.md` section 5a.
6. Squash or keep history: the branch contains a large early snapshot commit
   (`ceda96c`); a squash merge keeps `main`'s history readable.

## CopperProbe repository

- Create a GitHub repository for CopperProbe (it has none), add it as
  `origin` and push `main`.
- Decide whether `design-candidates/` stays tracked. Proposal: keep
  `base-original-20261016`, untrack the other 29 agent-made variants
  (`git rm -r --cached`), or drop them from the first commit before pushing.
- `verification/` (42k generated files) is git-ignored.
