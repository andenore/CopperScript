# CopperScript language server

The LSP belongs to CopperScript and reuses its lexer, strict parser, package
resolver, semantic compiler and electrical checks. CopperLib supplies source
definitions; consumer boards configure the selected compiler and locked inputs.

## Implementation plan and delivered first release

1. **Source frontend — delivered.** Add editor-only recovery around the strict
   parser. Retain complete sibling declarations and exact token ranges when a
   declaration is unfinished. Keep normal compilation strict. Convert source
   offsets to zero-based UTF-16 positions, including Unicode and CRLF documents.
2. **Workspace analysis — delivered.** Maintain versioned unsaved buffers and a
   source-derived index of local declarations and declared package exports. Feed
   all opened imported buffers through an explicit compiler `source_reader`
   hook. Resolve dependencies locked/offline; do not fetch, update locks, write
   source, or run placement/routing/native tools from editor analysis.
3. **Language features — delivered.** Report syntax, compiler and board ERC
   diagnostics; offer declaration/property/type/pin/port completions; show
   source documentation and declared electrical properties; navigate to local
   declarations and imported types, pins and module ports; provide an outline.
4. **Standard protocol — delivered.** Serve bounded JSON-RPC over stdio using
   LSP Content-Length framing, initialize/shutdown/exit, incremental or full
   document changes, save/close and watched-file notifications. Debounce
   semantic checks in one worker; discard obsolete workspace results.
5. **Editor integration and acceptance — delivered.** Extend the existing offline
   VS Code extension, preserving its mechanical editor and dependency-free VSIX
   build. Test actual stdio exchanges, providers, unsaved imports, locked failures,
   malformed frames, Unicode edits, recovery and stale result suppression.

The transport uses Python's standard library, so the ordinary compiler remains
dependency-free and no separate LSP installation extra is required. The small
VS Code client implements only the capabilities advertised by this server.
Other editors can use their existing standard LSP clients.

## Launch

Use a CopperScript release/checkout that includes this server:

```sh
uv run --locked --offline copper lsp
# Equivalent direct entry points:
python -m pcbir.lsp.server
copper-lsp
```

The command is a stdio protocol process, not an interactive terminal command.
An editor launches it and owns its lifetime. Logs go to stderr; stdout contains
protocol messages only. UTF-16 is the advertised position encoding.

For Neovim 0.11+, an example client configuration is:

```lua
vim.filetype.add({ extension = { copper = 'copper' } })
vim.lsp.config('copper', {
  cmd = { 'uv', 'run', '--locked', '--offline', 'copper', 'lsp' },
  filetypes = { 'copper' },
  root_markers = { 'copper.mod', '.git' },
})
vim.lsp.enable('copper')
```

Use the editor's workspace file-watching facility to send
`workspace/didChangeWatchedFiles` for `.copper`, `copper.mod` and `copper.lock`.
Save notifications also invalidate the analysis cache. Missing dependencies or
locks are visible diagnostics; bootstrap them through the normal CLI workflow.

## VS Code

Build/install using [the extension README](../integrations/vscode/README.md).
The extension supplies a TextMate grammar for syntax highlighting, including
engineering units. It works independently of the compiler process; semantic
tokens remain follow-up work in the LSP.

Text `.copper` documents automatically activate the language tools in trusted
local workspaces. `copperscript.lsp.compilerCommand` selects an interpreter argv
array. Its default `[]` discovers the project's `.venv`, then an existing
`pyproject.toml`/`uv.lock` project through locked/offline uv. An explicit array
overrides discovery. Python 3.11 or newer is accepted without an upper bound;
startup checks that the actual LSP module imports successfully.
The extension appends `-u -m pcbir.lsp.server` without using a shell.

Use **CopperScript: Show Setup** to inspect the selected environment and
**CopperScript: Restart Language Server** after changing installed dependencies.
Startup failures offer setup guidance; an incompatible selected environment
does not silently select a different compiler. The
[extension release plan](extension-release-plan.md) covers packaging,
compatibility CI and Marketplace distribution.

Projects pinned to an older compiler must explicitly select an LSP-capable
environment, or disable the LSP with `copperscript.lsp.enabled`. An explicitly
selected newer compiler is editor guidance; it does not upgrade the project's
build pin. The mechanical editor retains its separate compiler digest check.

Each workspace folder uses its configured compiler process. Open Copper buffers
are synchronized to the processes so unsaved local replacements/imports work
across folders. Only a document's owning folder publishes its diagnostics in VS
Code. Close discards that buffer and restores disk-backed import analysis.

## Current limits and follow-up plan

- Recovery retains complete top-level declarations; it does not synthesize
  valid semantics for malformed declarations. An unterminated string can consume
  subsequent source. Syntax errors suppress board ERC until the source parses.
- Library files get syntax diagnostics independently; complete semantic/ERC
  checks run through their consuming board. Built-in Python library pins have
  completions but no `.copper` definition locations.
- Completion offers a useful subset of properties and keywords, not every
  mechanical/constraint property or mode-dependent legal peripheral choice.
  Hovers show source declarations; inherited device-mode profiles are follow-up.
- ERC subjects map to source tokens/declarations when identifiable. Findings
  without a precise source subject use the document start. Imported compiler
  errors attach their original location as related information.
- Document analysis caches are invalidated conservatively across open documents.
  Verified dependency snapshots are reused while typing in a board, then
  discarded on import-buffer, save, manifest, lock or watched-file changes. Imports
  are indexed from the packages actually used; the server does not scan every
  library or build directory. Finer dependency invalidation, incremental
  parsing, background indexing and large-project latency measurements are next.
- Add scope-aware reference indexing before references/rename. Then add tested
  code actions, semantic tokens and comment-preserving formatting. These
  capabilities are deliberately not advertised by this release.
- Requests are served serially; cancellation notifications are accepted. The
  VS Code client suppresses canceled and superseded responses. Cooperative
  cancellation inside long package analysis remains follow-up work.

## Verification

```sh
python -m pytest tests/test_lsp.py tests/test_compiler.py tests/test_packages.py
COPPER_TEST_PYTHON=/path/to/python node --test tests/lsp-client.cjs
node integrations/vscode/build.cjs
python integrations/vscode/package.py -o build/copperscript-language.vsix
```

The Node tests exercise the actual stdio server and the VS Code provider adapter
with a host API fixture; they do not claim a visual Extension Development Host
test. Board ERC diagnostics are source checks, not hardware qualification.

Protocol reference: [LSP 3.17](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/).
