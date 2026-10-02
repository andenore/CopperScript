# GitHub board inspection builds

`.github/workflows/board-routing.yml` runs on relevant `main` pushes, all pushed
tags and **Actions → Route and publish board inspection drafts → Run workflow**.
It invokes the real complete routing workflow, not a dry run or saved historical
PCB. Tags also route the nRF52/CR2032 hard-macro example. Manual runs can select
that example too. All generation stays in ignored `build/` directories.

## Download and inspect

Each run uploads an Actions artifact, retained for 90 days. Download and unzip
it, then open `board-inspection-drafts.zip`. Each board directory retains its
PCB, project, `fp-lib-table` and project-local `.pretty` footprint library:
keep these together when opening KiCad. `inspection/` contains a separately
refilled copy, native `kicad-drc.json`, command logs and direct SVG exports of
F.Cu, In1.Cu, In2.Cu, In3.Cu, In4.Cu and B.Cu. SVGs open in a browser.
Reports, streamed routing logs, available cProfile stats/phase timings, exact
input hashes and the unchanged authoritative package lock are included. The
artifact manifest hashes every packaged file; `SHA256SUMS.txt` covers the outer
release assets. Source wheel/sdist and both repositories' source ZIP snapshots
accompany the board ZIP when their build stages succeed.

Tag pushes create a GitHub **prerelease** with every asset produced by the build,
even if routing remains incomplete. This is a lasting, publicly downloadable
inspection snapshot, not a production acceptance. There are no implicit
Gerbers/drills or gate bypasses. `READ-ME-FIRST.txt` always labels the ZIP an
experimental inspection draft. A zero routing exit alone is insufficient:
Actions also requires a passing route report/ERC and independent KiCad refill/DRC
without violations or unconnected items. Missing reports, timeouts and failed
gates fail the run without discarding diagnostics. RF, USB, battery, assembly
and fabrication qualification remain separate.

An interrupted run may have only logs, provenance and partial profiles: the
router does not necessarily write a final PCB before finishing. Missing files
are not replaced by an old result. Cancelling the whole Actions job or losing
the runner can prevent even the final upload; artifacts cannot be guaranteed
after an externally killed job.

## Dependencies: one checkout, pinned URLs

Only CopperScript is checked out. The normal package resolver downloads the
`github.com/andenore/CopperLib` URL requirement from `copper.mod` into the
project-managed `.copper-cache/pkg/` directory. Its full Git commit and exact
asset inventory must match `copper.lock`. CI refuses a local CopperLib
replacement or a floating tag/branch. Library physical assets and custom
footprints use the same cache; no sibling directory, manual library checkout
or manually maintained KiCad library table is required.

Preparation fetches the pinned URL with locked resolution, then compiles both
examples offline and records provenance. It **never rewrites or refreshes
`copper.lock`**, nor accepts changed bytes through line-ending normalization.
Routing thereafter uses `--locked --offline`. A missing/unpublished commit,
inventory mismatch or cache corruption fails setup rather than substituting
the latest library. Push the pinned CopperLib revision before tagging
CopperScript; GitHub cannot fetch a commit that only exists locally.

The committed `.github/board-toolchain.json` pins Python 3.12.12 and the official
KiCad 10.0.6 x86_64 installer by SHA-256. The latter installs matching footprint
libraries, avoiding floating external footprint downloads. Actions are pinned
by full commit and Python build/test tools by exact version. The fresh Windows
runner checks out source with canonical LF bytes. The generated project keeps
its own exported footprints, so recipients do not need CopperLib or matching
global KiCad library registrations to inspect it.

Ensure Actions is enabled and repository/org policy permits the tag-only
publish job's `contents: write` permission. No PAT or self-hosted runner is
needed. Routing/build steps only have read permission. A separate tag-only job
gets the write token, verifies downloaded asset checksums, creates a private
draft release, uploads every asset, then publishes it as a prerelease. An upload
failure leaves a private draft rather than a partially advertised release.
Existing releases are never silently overwritten: resolve a failed draft
deliberately or use a new tag. There are no PR or `pull_request_target` triggers
and no persistent self-hosted machine exposed to contributed code.

## Budgets and honest failures

GitHub-hosted routing uses a 270-minute full-vertical budget and a 20-minute
nRF52 budget, leaving time within the six-hour job limit for installation,
source distributions and inspection. Manual runs may reduce the full-vertical
budget to 1–270 minutes. A complete attempt is not a guarantee that the search
finishes on a hosted runner; timeouts remain visible as failures. The wrapper
first interrupts the worker process group, allowing Python profiling finalizers
to save, then force-terminates it after 45 seconds if needed. Blocking native
calls or forced termination may leave no profile. `ci-process.json` records
timeout and forced termination separately. Independent DRC and every layer
export also have bounded timeouts. Profiling stays enabled; these timings are
not uninstrumented benchmarks. Runs serialize per branch/tag without cancelling
earlier runs.

## Local inspection packaging

The packaging stage can inspect an existing fresh run locally:

```powershell
uv run python scripts/ci_board_bundle.py package `
  --output build/ci-routing --dist build/ci-dist `
  --kicad-cli "C:\Program Files\KiCad\10.0\bin\kicad-cli.exe"
uv run python scripts/ci_board_bundle.py check --output build/ci-routing
```

CI routing generates `build/ci-routing/full-vertical/` and (if selected)
`build/ci-routing/nrf52-coin-cell/`. Do not mix previous runs into these trees.
The helper tests cover locked URL preparation, refusal of local replacements,
real process interruptions with saved cProfile data, deterministic self-contained
ZIPs, per-layer exports and refusal to publish corrupted assets or overwrite
releases. Native smoke testing of the original placed nRF52 fixture produced
all six SVGs, zero geometry violations and 52 intentionally unrouted items;
the inspection gate correctly rejected completion.

## References

- [GitHub workflow syntax and permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)
- [GitHub Actions limits](https://docs.github.com/en/actions/reference/limits)
- [GitHub release REST API](https://docs.github.com/en/rest/releases/releases)
- [Official KiCad 10.0.6 release](https://github.com/KiCad/kicad-source-mirror/releases/tag/10.0.6)
- [KiCad CLI documentation](https://docs.kicad.org/10.0/en/cli/cli.html)
