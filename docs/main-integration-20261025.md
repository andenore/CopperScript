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

Verification results are recorded after testing the final combined source.

Integration repairs restore structured via-array errors, the upstream plane
contact alignment tie-break, and specific corridor-conflict diagnostics before
the general fixed-pose rejection. The round LED fixture retains its explicit
central branding position and passes its native silkscreen check.

Two older routing expectations also fail on clean remote main `7b35df8`:
the scarce-contact fixture now succeeds in its first joint pattern, and the
hemmed length-match bundle needs explicit group tuning. Tests retain all owner
and geometry checks and require an honest length-match failure for independent
pair tuning; they do not relax physical rules or label that result successful.
