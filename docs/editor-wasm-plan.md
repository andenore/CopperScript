# Physical editor in WebAssembly: design plan

Status: proposed (2026-11-02). W0 spike done: go, with two prerequisites
(per-file package verification, incremental legality). P2a (exact legality
speedup) done; nothing else implemented.
This plan changes where the editor's session runs; the editing contract in
[mechanical editor](mechanical-editor.md) stays as it is.

## W0 results (2026-11-02)

Throwaway spike, not in the repository: a crate that opens a board through
`copper_editor::project::document_session` and times compile, footprint
resolution, open, scene, a whole-board legality check per shifted pose
(`placement_rejection_reasons`), and a `move` operation with its scene, then
`discard`. It was built natively and for `wasm32-wasip1`. The wasm build ran in
Node 24 (V8, the engine Chrome uses) on an in-memory file system
(`@bjorn3/browser_wasi_shim`), the way a browser worker would. Release profile,
one Windows machine, median of 10–30 runs.

| Measure | Demo (4 parts) native / wasm | CopperAssetTracker (83) native / wasm | CopperProbe (166) native |
|---|---|---|---|
| Compile | 0.9 ms / 15 ms | 1,100 ms / 333 ms | 1,367 ms |
| Open session (warm disk cache) | 1,250 ms / 572 ms | 6,692 ms / 2,599 ms | 436 ms |
| First scene | 1.0 ms / 14 ms | 323 ms / 399 ms | 1,125 ms |
| Legality check, one shifted pose | 0.02 ms / 0.07 ms | 286 ms / 341 ms | 1,073 ms |
| `move` operation (on release) | 3.7 ms / 10 ms | 326 ms / 373 ms | 1,125 ms |
| Scene after move | 1.5 ms / 1.3 ms | 631 ms / 699 ms | 2,238 ms |
| Scene JSON | 21 KB | 418 KB | 609 KB |

Findings:
- **Compute penalty is small.** On real boards wasm is 1.1–1.2× native.
  Opening is faster in wasm because native open is dominated by disk I/O.
- **Module size.** The full session as `wasm32-wasip1` is 7.0 MB, 2.0 MB
  gzipped (no `wasm-opt`). The compiler-only `copper_web_bg.wasm` is 2.1 MB,
  0.68 MB gzipped. A real `wasm32-unknown-unknown` size needs W1: without it,
  `std::fs` always fails on that target, the optimizer removes everything after
  the first read, and the spike measured 50 KB.
- **Prerequisite 1: package inventory.** `packages::module_inventory` reads and
  hashes every locked file of every module on each compile. For
  CopperAssetTracker that is `kicad-footprints`: 121 MB in about 12,700 files.
  That caused the 91 s cold open and most of the warm open. A browser cannot
  download it. `copper.lock` already records size and sha256 per file, so verify
  only the files actually read against their lock entries. This also speeds up
  the native open.
- **Prerequisite 2: incremental legality.** It is needed natively today, not
  only for wasm. The whole-board check grows faster than linearly (286 ms at 83
  parts, 1,073 ms at 166), and releasing a drag costs about 1 s on
  CopperAssetTracker and 3.4 s on CopperProbe. Live feedback during a drag
  (W6) needs a check of the moved parts against a spatial index; the scene after
  a move also needs to update only what changed.
- **Portability.** `std::fs::canonicalize` is unsupported on WASI;
  `paths::resolve` falls back correctly. Node's own `node:wasi` on Windows lacks
  directory listing, so it is not usable for tests. The in-memory shim works.
- CopperLedRing was not measured: it needs footprint roots from its Make
  configuration.

## Summary

The physical editor is already Rust. `crates/copper-editor` holds the session,
scene, source transactions and placement jobs. `assets/editor.js` (~1,000 lines)
is a view that sends every action to the session over local HTTP. The editor and
its dependency crates (`copper-lang`, `copper-pcb`, `copper-physical`,
`copper-place`, `copper-drc`) already pass
`cargo check --target wasm32-unknown-unknown` without changes.

What stops the editor running in a browser is I/O, not algorithms. The session
reads the source and footprints from disk, writes saves with lock files, runs
auto-placement on a thread, and is reached through a TCP server.

**Proposal:** put that I/O behind two narrow boundaries, a file reader shared by
the compiler and footprint loaders and a session host for saving and jobs. Then
add a `copper-editor-web` crate that runs the same session in a Web Worker.
`copper edit` keeps its native host and behaves exactly as today. The browser
editor and the native one serve the same `editor.js`, which switches between
HTTP and worker messages.

**What it gives:**
- An editor that needs no install or local server and can be hosted statically.
- The base for a VS Code webview editor.
- Placement legality and ratsnest updates during a drag, not only on release.

**Where it lives:** in CopperScript, next to `copper-web`. The scene protocol
changes with the compiler, and [mechanical editor](mechanical-editor.md) already
places the tool, UI assets, protocol and tests here. Consumers such as
CopperReview or the promo site use the built package. CopperReview stays a
read-only review tool; it can link to the editor but does not absorb it.

## Goals and non-goals

Goals:
- One session implementation. Native and browser editors produce the same scene
  JSON for the same inputs, checked by tests.
- Browser editing of board, macro and `board_profile` sources: placement,
  locks, physical features, reviewed source diffs, save, undo.
- Pinned package resolution in the browser, verified against `copper.lock`, using
  the project-snapshot protocol `copper-web` already uses (`ProjectSnapshot`,
  `plan()` requests, resolver).
- No change to `copper edit` behaviour, CLI or outputs.

Non-goals (stay native):
- Routing, fills, KiCad export and native DRC.
- The routed overlay (`overlay.rs` starts a Python probe via `Command`).
- Writing `copper.lock` or downloading unpinned dependencies. The browser editor
  is locked-only at first.
- Firefox/Safari folder saving. They get open-and-download (see Saving).

## Current I/O touchpoints

| Concern | Where | Native today | Browser |
|---|---|---|---|
| Read source, detect external change | `session.rs` `read_source`, `definition_session.rs`, `dependencies::read_source` | `std::fs::read` + sha256 | Host store; re-read the file handle on focus |
| Save reviewed source | `transactions.rs` (lock file, hard link, temp file + rename, permissions) | atomic, revision-checked | Host store write with revision check (see Saving) |
| Compile with packages | `copper-lang` package loading from `.copper-cache` | file system | `ProjectSnapshot` resolver (`copper-lang/src/snapshot.rs`) |
| Footprints | `copper-physical/kicad_mod.rs` `load_kicad_mod(path)` | `std::fs::read` | bytes from the snapshot; `parse_kicad_mod(text)` already exists |
| Macro assets, templates, references | `copper-pcb/hard_macros.rs`, `templates.rs`, mechanical reference assets | `std::fs::read` | bytes from the snapshot |
| Dependency preparation | `dependencies.rs` `prepare_editor_dependencies` | downloads, lock updates | snapshot `plan()` → fetch → verify; no lock writes |
| Auto-place job | `jobs.rs` `std::thread::spawn`; cancel only discards the result | thread | second worker; cancel = terminate |
| Transport | `server.rs` TCP, token header | HTTP | `postMessage` to worker |
| Clock | `jobs.rs` `Instant::now`, `transactions.rs` `SystemTime` | std | host clock (`Instant` panics on wasm32-unknown-unknown) |

## Design

### 1. File reader shared by compiler and loaders

Add a read-only file trait in `copper-lang`, beside `snapshot.rs`:

```rust
pub trait Files {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn is_dir(&self, path: &Path) -> bool;
    fn list(&self, directory: &Path) -> io::Result<Vec<PathBuf>>;
}
```

`NativeFiles` wraps `std::fs`. The `SourceSnapshot` in `snapshot.rs` already has
`read`, `sources` and `is_dir`, so it becomes the in-memory implementation.
`load_kicad_mod`, the hard-macro asset reader, templates and reference assets
take `&dyn Files` instead of calling `std::fs::read`. Each keeps its error text,
`source_sha256` metadata and path display. The snapshot's lock check covers
footprints and assets too, so a browser build reads only pinned bytes.

### 2. Session host

`EditorSession` and `DefinitionSession` take an `EditorHost`:

```rust
pub trait EditorHost {
    fn files(&self) -> &dyn Files;
    /// Current bytes of the edited source (for staleness and reload).
    fn read_source(&self, path: &Path) -> Result<Vec<u8>>;
    /// Replace the source only if its sha256 still equals `expected`.
    fn write_source(&self, path: &Path, expected: &str, bytes: &[u8]) -> Result<()>;
    fn spawn_placement(&self, request: PlacementRequest) -> Box<dyn PlacementJob>;
    fn now_seconds(&self) -> f64;
}
```

- `NativeHost` keeps today's `transactions.rs` lock-file write and the thread job.
- The server, overlay and native transaction code move behind a default
  `native` cargo feature of `copper-editor`.
- The web crate builds with `default-features = false`.

### 3. `copper-editor-web` crate

A `cdylib` beside `copper-web`, with the same pinned `wasm-bindgen` and the same
`scripts/build-wasm.mjs` flow. Exports:

```text
plan(project_json)       -> copperscript.project-plan/v1 requests, or ready
open(project_json)       -> session id + first scene (board, macro or profile by document kind)
scene(id)                -> scene JSON (same schema as GET /api/scene)
operation(id, request)   -> same request/response JSON as POST /api/operation
pending_write(id)        -> bytes + expected revision after save_source, for the page to write
```

The page's worker owns the session. Its `WebHost` keeps the source bytes in
memory. A save leaves the new bytes as a pending write for the page, and the
session's source revision advances only after the page confirms the write.

### 4. Saving in the browser

- **Chromium (File System Access API):** the user opens a project folder. The
  worker gets the files; the page keeps the source's file handle. Before
  writing, the page re-reads the handle and compares its sha256 with the
  expected revision. If they differ, the write is refused as stale, the same
  rule native enforces. The API has no atomic rename, so writing in place is
  the browser contract; document this limitation.
- **Other browsers, or opened from a URL:** read-only open, or edit and then
  **Download source** with the reviewed diff shown. Nothing claims to be saved.
- **VS Code webview (later):** the extension applies the reviewed text as a
  `WorkspaceEdit`. That gives editor undo and uses the existing
  `accepted.document_host` path in `editor.js`.

External changes use the reload behaviour from commit `7113cdf`: on focus the
page re-reads the handle and, if the bytes changed, sends them to the worker and
runs `reload_source`.

### 5. Auto-placement

wasm32-unknown-unknown has no threads, and a worker running a synchronous
placement cannot receive a cancel message. Run `plan_placement` in a second
worker:
- It gets a serialized board and options and returns the placed board.
- Cancel terminates that worker, the same as today's cancel discarding the result.
- Progress is coarse, because no shared memory is used. GitHub Pages cannot send
  COOP/COEP headers, so `SharedArrayBuffer` is out.

This needs `PhysicalBoard` to round-trip through JSON or bytes. Until it does,
the web build reports `capabilities.auto_place = false`, and the UI already
hides the button.

### 6. One UI for both hosts

`editor.js` already routes every request through `api(path, body)`, with a
`postMessage` branch for VS Code. Add a third transport for the worker. Keep one
`index.html`/`editor.js`/`editor.css`: the native server embeds them as today
(`include_bytes!`), and the web build copies them. Native-only controls
(**Open board** dialog, routed overlay) hide on capability flags the scene
already carries.

### 7. Live drag feedback (after parity)

With the session in a local worker, the page can send a no-commit pose check on
each animation frame during a drag. The check returns legality reasons and the
moved component's airwires. The commit on release stays the authoritative check.
This needs a cheap incremental check; measure `placement_rejection_reasons` per
move on the larger consumer boards first.

## Steps

One PR per step. Every step keeps `cargo test --workspace --release` and the
browser editor tests (`tests/browser/mechanical-editor*.cjs`) passing against
the native editor.

| Step | Work | Done when |
|---|---|---|
| W0 | Spike: open the demo board, CopperAssetTracker and CopperProbe, time them natively and as wasm | Done; see W0 results. Go |
| P1 | Verify package files individually against `copper.lock` entries as they are read, instead of hashing whole modules | Same compile results and errors for changed or missing files; CopperAssetTracker open time recorded before and after |
| P2a | Exact speedup of `placement_rejection_reasons`, which the editor scene runs whenever a layout is not fully legal (92–95 % of scene time): one `Checker` for all pairs instead of one per pair; pair checks skip the candidate-only checks already passed; a per-pose cache (`PoseCache`) for hole, via-array and pad-net data | Done 2026-11-02. Reasons byte-identical on 82 layouts (source + 40 perturbed, CopperAssetTracker and CopperProbe; 3,106 reason lines). `placement_rejection_reasons` 277 → 33 ms (CopperAssetTracker), 1,174 → 65 ms (CopperProbe). Editor `move` on release 326 → 70 ms and 1,125 → 108 ms. Planner and router paths unchanged (`Checker::new` and `legal` keep their behaviour) |
| P2b | Incremental: keep pair verdicts and pose geometry across moves in the session, so a move re-evaluates only pairs involving moved parts | Same reasons as P2a after every operation in the editor tests; per-move check under 16 ms on CopperProbe |
| W1 | `Files` trait; footprint, macro-asset, template and reference loaders take it; native wrapper | No behaviour change; scenes byte-identical on the demo and consumer boards; real `wasm32-unknown-unknown` size recorded |
| W2 | `EditorHost`; `native` feature for server, overlay, lock-file transactions, thread jobs | `copper edit` unchanged; `cargo check -p copper-editor --no-default-features --target wasm32-unknown-unknown` in CI |
| W3 | `copper-editor-web` with `plan`/`open`/`scene`/`operation`/`pending_write` | Native test runs the same operation script (move, lock, review placement, save, undo, reload_source) through `NativeHost` and `WebHost` and compares scene JSON |
| W4 | Worker transport in `editor.js`; open folder (FS Access) or URL (snapshot plan); save and download | Browser tests run against both transports; stale-write refusal tested |
| W5 | Board serialization; placement worker with cancel | Auto-place preview matches native for the same seed on the demo board |
| W6 | Live drag feedback (needs P2b) | Per-frame check under 16 ms on CopperProbe |
| W7 | Consumers: host on the promo site (`site/demo` already loads wasm), link from CopperReview, VS Code webview | Each consumer pins a built package version |

## Risks and open questions

- **Bundle size.** The W0 proxy is 2.0 MB gzipped, about three times the compiler
  module. Try `wasm-opt` first. If that is not enough, move placement into a
  lazily loaded second module (it already runs in its own worker in W5).
- **Divergence.** Two hosts can drift. The W3 identical-scene test is the guard,
  and it runs in CI.
- **Uncommitted base.** `copper-lang/src/snapshot.rs` and the `copper-web`
  project API are not committed yet; W1 builds on them.
- **Package fetching.** Opening by URL fetches from GitHub, like CopperReview's
  **Open URL**. Large footprint libraries (`kicad-footprints`) must be fetched per
  file, not cloned. Confirm the snapshot plan requests only the footprints the
  board uses.
- **Definition sessions.** **Open definition** opens a second view. In the
  browser that is a second session in the same worker, with cached dependencies
  read-only as today.
