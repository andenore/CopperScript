# Physical editor and VS Code backlog

Findings from the 2026-10-20 review of `copper edit` and the VS Code extension with
its language server (integrations/vscode), made on the former Python
implementation. The editor is now `copper edit-mechanical` (`crates/copper-editor`)
and the server `copper lsp` (`crates/copper-lsp`). Measured on
examples/mechanical_editor_demo and examples/full_vertical (55 parts). Most
important first; check current code before starting an item.

## Defects

- [x] Language server: decode VS Code file URIs before reading the drive letter.
  VS Code sends `file:///c%3A/...`; the server's URI conversion turned that into `\c:\...`,
  so imports and `copper.mod` do not resolve and published diagnostics do not match
  the open document on Windows. Tests only used `file:///C:/`.
- [x] Language server: publish diagnostics and definitions under the URI the client
  used. Paths are canonicalised with `resolve()`, so files opened through a
  symlink, junction or `subst` drive get no diagnostics and definitions open a
  second tab.
- [x] Reject constraints whose targets do not exist. `max_distance(GHOST, NOPE.VDD)`,
  `align(R1, GHOST)` and `placement_region(GHOST)` pass `copper check`, and the
  editor reviews and saves them, so a typo silently drops placement intent.
- [ ] Editor latency: each drag release, discard or review took about 2.5 s on
  full_vertical. Macro preview copper is now cached and the source parsed once per
  revision: a move or discard takes 0.25-0.36 s (was 2.2-3.2 s). Remaining:
  `placement_rejection_reasons` (about 0.7 s under the profiler) recomputes every
  part's legality explanation per move; make it incremental. Source reviews still
  take about 2 s (recompile and validate). Target: under 200 ms per move.
- [x] Editor: reloading source after an external save discards the unsaved
  placement draft (`reload_source` builds a fresh state). Rebase the draft with
  `seed_placements` and report conflicts.
- [x] Editor: the browser focus outline of the selected component is scaled with
  the board view and draws a large black/white disk after every drag.
- [x] Language server: use `diagnostics.locate` (origin, occurrence) instead of its
  own subject mapper, so DUPLICATE_NET points at the duplicate and
  NO_CONNECT_CONNECTED at the `no_connect`; carry machine-applicable fixes as code
  actions (quick fixes).
- [x] VS Code: restart a crashed or timed-out language server with backoff and show
  its state; today every feature returns nothing until a manual restart.
- [x] VS Code: closing the editor tab while it prepares (up to 300 s) leaks the
  editor backend (`panel.onDidDispose` is registered after preparation). The
  custom editor was removed in extension 0.4.0.
- [x] Grammar: `no_connect` is not highlighted; add a test comparing parser keywords
  with the grammar.
- [x] Editor: a stale `.editor-lock` blocks saving forever; record PID/time, detect
  stale locks and offer to break them.
- [x] Editor: startup progress lines were removed but a test and the docs expect them;
  opening full_vertical is silent for about 7 s.
- [x] Editor: auto-placement elapsed time keeps counting after the job finishes.
- [x] Editor: inserted source leaves whitespace-only lines and extra blank lines.
- [x] Editor: relative-constraint messages print raw nanometres and do not always
  name the parts; use the shared quantity formatter.
- [x] Editor UX: pose problems are listed for the accepted layout too; rigid-macro
  members inherit their anchor's source locks (rotate/move disabled, with a notice);
  discarding a preview says so instead of "Source/session updated".
- [ ] Editor UX: "Review placement" saves every part (needed so the source reproduces
  the layout) at full precision such as `28.333333mm`; offer grid snapping or
  rounding for parts that were never fixed.
- [x] VS Code: severity mapping is error/else warning; the file watcher reports every
  change as "changed"; unknown notifications are logged as errors; the offline
  packager (since removed; packaging uses `vsce`) hardcoded the engine version.
- [x] Docs: the rear view is described as mirrored (the UI says it is not); the tab list
  omits Properties.

## Improvements

- [x] Quick fixes: apply diagnostic replacements (`U3.SDAA` -> `U3.SDA`) and declare
  `no_connect` for UNMARKED_PIN.
- [ ] Quick fix: remove a pin from a net (or the `no_connect`) for NO_CONNECT_CONNECTED.
- [ ] Hovers from `copper query` data: a component's pins with nets and unconnected
  pins, a pin's pad and net, a diagnostic code's explanation.
- [ ] Inlay hints: pin name after numeric endpoints (`R1.1 <VIN>`), pad number after
  named pins.
- [ ] VS Code commands: check (into Problems), explain code, query component/net at
  the cursor, issues/view for a run directory; a task definition with a problem
  matcher.
- [ ] Headless placement for agents: `copper place board.copper --json` applying
  move/lock/prepare operations from a file and printing the source diff, legality
  and constraint results without a server.
- [ ] Editor placement conflicts as `copper issues`-style records so `copper view` and
  CopperReview can frame them; a compact scene summary output.
- [ ] Editor: a persistent problems panel with click-to-zoom; live distance readouts
  while dragging; save only changed or selected parts; snap seeds to the grid.
- [ ] Completion: unconnected pins first after `no_connect REF.` and in nets; snippets
  for component, net, no_connect, supply and interface.
- [ ] Outline: nested document symbols, distinct kinds for supplies; workspace symbols.
- [ ] Semantic tokens for components, nets, pins and parts; scope-aware rename/references.
- [ ] Add an Extension Development Host test with `@vscode/test-electron` to Windows
  CI (the earlier `tests/vscode/integration.cjs` was deleted with the mechanical
  custom editor).
- [x] `language-configuration.json` word pattern including `-`.

## Follow-ups from the fix review

- [ ] Language server: `file://server/share/...` (Windows UNC, as VS Code writes it) is
  rejected as a remote host, so projects on network shares get no language tools.
- [ ] Language server: `_CLIENT_URIS` is process-wide; two URIs for one path publish
  under the last one opened.
- [ ] ERC checks device-unit constraint pins with `active_bonded_pads`, physicalize with
  `pins_bonded_to_pad`; confirm they agree under configurations.
- [ ] Editor: macro preview copper is not re-accepted when a part outside the macro
  moves onto it (placement legality reports the overlap instead).

