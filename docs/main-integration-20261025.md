# Main integration, 2026-10-25

The integration starts with remote main `7b35df8` and the completed-board
compiler branch `507209d`. Pending changes were captured with alternate Git
indexes; the original checkouts and their staged/unstaged files were preserved.

The primary pending snapshot is `046a268`. Additional snapshots preserve the
organ worktree (`0cf3c29`), probe worktree (`a17f0fa`), site validation worktree
(`8dea4ae`), escape experiment (`59dbcff`) and grid worktree (`326449a`).

Current implementations take precedence over older versions of the same work:

- Exact congestion negotiation uses `be96234`, retaining current partial via
  spans. It includes the earlier grid experiments and snapshot `326449a`.
- Critical cohorts, required source arrays and immutable prefix rebinding are
  already integrated from their later versions. The incremental placement
  array implementation from `21e47f9` is retained.
- Replay state uses upstream `36be777`: accepted-trial input promotion and the
  validated numeric array codec. Additional selected-run and congestion
  round-trip regressions from `dd2fd03` are retained.
- Simulation and RF/CAM work is already present through subsequent commits.
  Historical site snapshot `8dea4ae` contains only previously integrated file
  versions; the current source-bound site assets take precedence.
- The escape experiment's `--escape-bias-rate` depended on a removed experimental
  `escape_plan_options` pipeline API. Its source remains in Git history; it is
  not exposed as a nonfunctional flag on the current router.

Historical results in `routing-performance.md` describe the revisions named
there. In particular, older statements that congestion experiments were not
merged apply to those earlier revisions, not this integration. They do not
qualify the newly combined compiler or renew any consumer board evidence.

Verification results for the final combined source are recorded below.

Integration repairs restore structured via-array errors, the upstream plane
contact alignment tie-break, and specific corridor-conflict diagnostics before
the general fixed-pose rejection. The round LED fixture retains its explicit
central branding position and passes its native silkscreen check.

Two older routing expectations also fail on clean remote main `7b35df8`:
the scarce-contact fixture now succeeds in its first joint pattern, and the
hemmed length-match bundle needs explicit group tuning. Tests retain all owner
and geometry checks and require an honest length-match failure for independent
pair tuning; they do not relax physical rules or label that result successful.

The last local plane-repair regression exposed a keepout-policy mismatch:
pending pad-overlap work also changed track/via tangency, while the router's
existing exact contract allows zero-clearance tangency. Track/via DRC retains
that contract; the pad polygon-crossing correction remains. A boundary regression
now checks tangent and one-nanometre-penetrating tracks/vias against physical DRC.
Bounded nearby-support grouping and route-only report discovery are also restored.

Final integration checks use compiler source `fe9398e`. The seeded-index fixture
now contains a deliberately impassable wall: the improved router completes the
older crossing grid before a seed is needed. The native power tests explicitly
select the legacy router and its legacy rollback gate where those are the
behaviours under test. Their ownership and rollback assertions remain intact.

The bounded final ladder completes round-led-ring (51/51 connections) and nRF52
(35/35), both with zero estimated plane opens and zero physical DRC errors.
These ladder runs did not perform native refill/DRC. The final LED result has
55 vias and 638.284 mm of track; nRF52 has 39 vias and 352.647 mm. The upstream
comparison has 48/42 vias and 604.833/345.115 mm respectively, so routing
completion does not imply shorter routing or fewer vias on every example.

The final ladder ran alongside the full test suite. Its LED time was 225.2 s
versus 165.0 s in the earlier baseline, triggering the ladder timing threshold;
those different machine loads do not establish an isolated performance
regression or speedup. The baseline subprocesses used the clean `7b35df8`
checkout through PYTHONPATH; its harness header records the integration
checkout's then-current commit because that was the harness working directory.

Verification used Python 3.13.1 on Windows with KiCad 10 available on PATH:

- Full compiler suite: 5,086 passed, 25 skipped, six fixture/setup failures in
  1,923.38 s (`pytest tests -o addopts='' -q -n 4 --dist loadfile`). The six
  failures were corrected without changing production behaviour. Complete
  reruns of `test_unified_commit_index.py`, `test_power_config.py` and
  `test_power_preparation.py` then passed 17, 14 and 11 tests respectively.
  Thus all 5,092 executed test cases are verified across the full run and
  corrected-file reruns; this is not a claim of a second green full-suite run.
- Clean remote main reproduces all four original native power-preparation
  failures. The corrected tests retain complete rollback assertions and choose
  the explicitly supported legacy mode. The seeded-index test retains its
  fresh/shared result equality check and now guarantees both connected copper
  and an open connection that requires attempting the seed path.
- Editor/LSP Node client suites: 20 passed.
- CopperLib: 214 passed, three skipped for optional cached Nordic source
  archives; CM4 generator `--check`, compiler smoke ERC and compilation passed.
- The 32 targeted keepout/placement/plane-repair cases passed before the full
  suite. Native checks are included where installed; skipped optional tools
  and source assets are not represented as passed or as hardware qualification.

Local logs and JUnit reports are retained in the ignored
`build/main-integration472-evidence` directory of the original checkout.
The final follow-up commit changes test setup, this record and trailing blank
lines only; functional compiler source remains the tested `fe9398e` source.
