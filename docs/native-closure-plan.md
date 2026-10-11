# Native closure of fresh source-only boards

Success means the latest 91-component CopperAssetTracker revision closes using
the standard source-only CLI, with zero native opens, violations and islands,
while the existing 83-component benchmark still closes. This is routing
verification, not manufacturing or RF/USB/power/prototype approval.

## Checklist

- [x] Preserve baseline source/board/native findings and add renamed small
  real-geometry regressions for dependency escape-via continuity and cleanup.
- [x] Preserve usable package-exit routing anchors during dependency rerouting;
  make bridge/reroute/ownership-checked cleanup/native refill one transaction.
- [x] Share bounded native repair trials across distinct opens; deduplicate
  equivalent proposals without hard-coded component or net selectors.
- [x] Fix relative native-evidence paths and cover Windows working-directory
  behavior; preserve all retained evidence and input hashes.
- [x] Follow actual explicit copper components from native-reported items to
  existing plated transition anchors, without borrowing source-zone outlines
  or installed-component internal connections.
- [x] Regenerate a trimmed owned boundary witness only when its launch via is
  retained and exact land/clearance checks pass; reject blocked regeneration.
- [x] Run affected ownership, arrays, macro, escape, cleanup and native tests.
- [x] Run the fresh 91-component full source-only workflow to native closure;
  if repair stalls, investigate generic filled-connectivity placement feedback
  with fixed/rigid intent preserved, rather than consumer repair scripts.
- [x] Rerun the unchanged 83-component source benchmark and compiler suite.
- [x] Publish exact commands, hashes, counts, limitations and retained outputs
  in the project's build folder and update this checklist with measured results.

## Baseline, 2026-10-11

Tracker `build/latest-power-review-20261011/route-rerun.json`: all 61 ordinary
nets routed, 76 package exits retained, both USB pairs routed; native KiCad
10.0.6 finds two supply opens, zero other violations and zero islands. Eight
trials all target the first supply; candidates reduce opens but introduce four
native dangling-via warnings on displaced ordinary dependencies and roll back.
Source SHA-256: `2d0a5dcda3e61cc0066c0c8938e2c3547479f275c182ed67704c61dc47412205`.
The earlier native-clean 91-component result used targeted repairs; autonomous
closure previously covered only the older 83-component source.

## Safety

Do not change electrical topology, source widths/clearances/keepouts, placements
fixed by source, macros or required via-array multiplicity. No warning waivers,
historical pose seeds, or board-specific routing selectors. Mutability must be
occurrence-counted and explicit. Cleanups require exact native object mapping,
fresh connectivity and refill checks, complete rollback and bounded work.
Ordinary disposable escapes are distinct from source, macro, semantic-feed,
required-array, critical and accepted bridge/contact ownership.

## Implementation evidence

The first eleven new regression cases pass with the installed KiCad 10.0.6.
They exercise renamed retained-port dependency reroutes, two-rail trial fairness,
relative evidence paths, actual disposable-via cleanup while an unrelated zone
remains open, stale certificates, caller ownership, macro ownership and retained
required-array occurrences. The required arrays retain valid source return zones;
the native dangling-via negative control is macro-owned, not a fabricated array
violation. Full compiler and 83/91-component source-only runs are pending.

The finalized affected replay has 226 passing tests and one optional skip across
dependency closure, escape feedback/terminals, native feedback/CLI/scope,
fanout/native verification, hard macros, pad arrays, route/bend cleanup and
autonomous routing quality (`build/native-closure-affected-verified.xml`).
Tracker's consumer suite separately passes 55 tests and 34 subtests, no skips,
with the existing CopperLib review root explicitly selected. These focused
counts overlap the forthcoming full compiler run and must not be added to it.

Dependency transactions now use retained routing ports rather than flexible pad
alternatives when they do not own the prefix cleanup. Native cleanup is limited
to 64 exact mapped ordinary ends and only the displaced eligible nets, with
occurrence-counted source/prefix, macro and required-array protection. It checks
explicit connectivity and independently refills once before accepting cleanup.
The enclosing bridge transaction still requires strictly fewer native opens,
no newly open net, no islands and no non-library violations.

Four opposite-layer native cases pass: existing transitions permit closure,
while absence of a transition remains unresolved, including renamed variants.
The combined native-feedback replay passes 49 tests. The next board increment
exposed two retained launches whose original collar witnesses were replaced by
actual area copper. Before the witness fix, both renamed reduced cases reconnect
the logical signal but create two native dangling-via findings; after the fix,
all four intact/settled-witness cases pass fresh native verification. Regenerated
witnesses use the original bounded geometry, retain the launch/land ownership,
and must clear the current immutable bridge/prefix. Missing launch vias are not
recreated. The shared allowance is 64 regenerated witnesses across the entire
dependency cone; later expanded nets use the same validation and allowance.

The witness increment passes 73 affected escape/native-feedback cases and both
explicit blocked-witness rollback controls. The formerly skipped native array
case also passes with KiCad's directory placed on this process's PATH.
The v5 board replays and full compiler run use the same isolated source snapshot.
Concurrent compact-report/quality changes are preserved: their four runtime
modules were copied into that snapshot before their lazy imports, and 97
report/requirement/acceptance/benchmark compatibility tests pass separately.
These concurrent reporting changes are not routing-decision changes from this
closure task; their new test modules are not retroactively counted in the full
run's already collected inventory.

## Fresh 91-component closure

The v5 standard source-only `balanced` run exits 0 with `routing_complete=true`.
It routes all 61 ordinary nets; fresh KiCad 10.0.6 fill verifies zero opens,
zero non-library violations and zero islands. Four bounded native trials close
both supply rails: the In3.Cu V3V3 bridge reroutes SIM_CLK and V1V8_MODEM using
retained exits; two surface attempts reject before the In1.Cu V3V8 bridge uses
existing plated transitions. No protected vias are removed, no historical poses
or copper are imported, and no project repair entry point is invoked.

Tracker `build/latest-power-review-20261011/closure-v5-checkpoint.json` independently
reads the retained filled board against current source, legal poses, immutable
macro copper and required arrays. All four modem bypass feeds pass actual
via-free F.Cu chain checks; read-only native DRC has zero violations and opens.
The board contains 91 components, six copper layers, 921 tracks and 314 vias.
Its filled PCB SHA-256 is
`b63dd47ae84b9d9fea381420208dd6ffa113d89f1d9ecff8af07968907e3c3b1`.
Exact commands and frozen compiler/input hashes are in `closure-v5-inputs.json`;
the accepted filled board is in `native-closure-v5/verification-003/`.
Native top and In2.Cu SVG/PNG views are retained beside the reports. Earlier
failed reports remain intact. The independently completed older source benchmark
is recorded below. Production approval is not inferred from routing success.

## Compiler inventory verification

The frozen full run completed with 2504 passing tests, 17 optional/platform
skips and two nRF52 example failures caused by an initially absent pinned
offline dependency cache. After the existing online example cases populated
that cache, both exact failed cases pass with unchanged frozen code
(`offline-cache-replay-v5.xml`); all six example cases also pass together
(`example-replay-v5.xml`). Thus 2506 distinct cases in the full run's inventory
are verified passing, with no unresolved failures. This is full-suite plus
exact failed-case replay evidence, **not** a claim that the original full run
was all green. The original `full-suite-v5.xml` is retained unchanged.

The separate 97-case compatibility replay includes the new concurrent reporting
tests collected after the full run started; it has no failures or skips. Counts
overlap and are not added to the full inventory. Tracker's suite passes 55 tests
and 34 subtests. The 17 full-suite skips are explicitly platform permissions,
Linux defaults, optional vendor/native-Python fixtures and absent ngspice
integration; the new native closure cases run against actual KiCad 10.0.6.

## Unchanged 83-component benchmark and handoff

The locked v5 `balanced` benchmark also exits 0: all 60 ordinary nets connected,
no failed critical routes or package exits, and native fill verifies zero opens,
non-library violations and islands. Read-only current-source/land/pose/macro/
required-array validation passes and leaves the PCB unchanged. Counts are
83 components, six layers, 877 tracks and 304 vias. Its physical digest and
exported PCB byte hash reproduce the preceding historical successful shared run;
native filled-board bytes are freshly hashed, not assumed identical.
Retained files: Tracker `build/native-closure-final-83-20261011/`.

Tracker `build/latest-power-review-20261011/closure-v5-result.json` binds the
latest and older results, exact commands, independent checks, test reports and
output checksums. `compiler-source-v5.json` hashes all 600 frozen source/assets
files; every entry rechecks unchanged. Canonical Tracker source and release locks
remain unchanged. CopperLib's dependency commit is unchanged and this task did
not edit it; concurrent agent-guidance changes in its checkout are preserved. No project
repair algorithm, rule waiver, production export, purchase, commit or push is
part of this closure result. All checklist items are complete for routing scope;
RF/USB/power, CAM, prototype and final release approval remain separate gates.
