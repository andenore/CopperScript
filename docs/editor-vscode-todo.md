# Physical editor and VS Code backlog

Findings from the 2026-10-20 review of `copper edit` (pcbir/editor) and the VS Code
extension with its language server (integrations/vscode, pcbir/lsp). Measured on
examples/mechanical_editor_demo and examples/full_vertical (55 parts). Most
important first; check current code before starting an item.

## Defects

- [ ] Language server: decode VS Code file URIs before reading the drive letter.
  VS Code sends `file:///c%3A/...`; `pcbir/lsp/text.py` turns that into `\c:\...`,
  so imports and `copper.mod` do not resolve and published diagnostics do not match
  the open document on Windows. Tests only used `file:///C:/`.
- [ ] Language server: publish diagnostics and definitions under the URI the client
  used. Paths are canonicalised with `resolve()`, so files opened through a
  symlink, junction or `subst` drive get no diagnostics and definitions open a
  second tab.
- [ ] Reject constraints whose targets do not exist. `max_distance(GHOST, NOPE.VDD)`,
  `align(R1, GHOST)` and `placement_region(GHOST)` pass `copper check`, and the
  editor reviews and saves them, so a typo silently drops placement intent.
- [ ] Editor latency: each drag release, discard or review takes about 2.5 s on
  full_vertical and 2.3 s on the 4-part demo. About 80% is `_preview_copper`
  (hard-macro materialisation with a full physical DRC, twice per scene build);
  the source is also tokenized six times per move. Target: under 200 ms per move.
- [ ] Editor: reloading source after an external save discards the unsaved
  placement draft (`reload_source` builds a fresh state). Rebase the draft with
  `seed_placements` and report conflicts.
- [ ] Editor: the browser focus outline of the selected component is scaled with
  the board view and draws a large black/white disk after every drag.
- [ ] Language server: use `diagnostics.locate` (origin, occurrence) instead of its
  own subject mapper, so DUPLICATE_NET points at the duplicate and
  NO_CONNECT_CONNECTED at the `no_connect`; carry machine-applicable fixes as code
  actions (quick fixes).
- [ ] VS Code: restart a crashed or timed-out language server with backoff and show
  its state; today every feature returns nothing until a manual restart.
- [ ] VS Code: closing the editor tab while it prepares (up to 300 s) leaks the
  Python backend (`panel.onDidDispose` is registered after preparation).
- [ ] Grammar: `no_connect` is not highlighted; add a test comparing parser keywords
  with the grammar.
- [ ] Editor: a stale `.editor-lock` blocks saving forever; record PID/time, detect
  stale locks and offer to break them.
- [ ] Editor: startup progress lines were removed but a test and the docs expect them;
  opening full_vertical is silent for about 7 s.
- [ ] Editor: auto-placement elapsed time keeps counting after the job finishes.
- [ ] Editor: inserted source leaves whitespace-only lines and extra blank lines.
- [ ] Editor: relative-constraint messages print raw nanometres and do not always
  name the parts; use the shared quantity formatter.
- [ ] Editor UX: "Review placement" locks untouched parts at values such as
  `28.333333mm`; pose problems are listed only while a preview exists; rotate stays
  enabled for locked rigid-macro members; "outputs are stale" persists after discard.
- [ ] VS Code: severity mapping is error/else warning; the file watcher reports every
  change as "changed"; unknown notifications are logged as errors; `package.py`
  hardcodes the engine version.
- [ ] Docs: the rear view is described as mirrored (the UI says it is not); the tab list
  omits Properties.

## Improvements

- [ ] Quick fixes: apply diagnostic replacements (`U3.SDAA` -> `U3.SDA`), declare
  `no_connect` for UNMARKED_PIN, remove a pin from a net for NO_CONNECT_CONNECTED.
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
- [ ] Run `tests/vscode/integration.cjs` in Windows CI with `@vscode/test-electron`.
- [ ] `language-configuration.json` word pattern including `-`.
