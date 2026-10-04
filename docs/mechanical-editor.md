# Mechanical and floorplan editor specification

Status: accepted contract; incremental implementation in CopperScript. See the
[implementation checklist](mechanical-editor-plan.md) for actual delivery status.
This is a physical-intent editor, not a schematic editor or another PCB router.

## Ownership and user workflow

The tool, reusable UI assets, CLI, scene protocol and tests belong in the
CopperScript repository. Generic algorithms belong in `pcbir`; no example part,
reference, path or board geometry may be encoded there. Libraries own reusable
footprints and hard macros; board projects own their source and constraints.

1. Open a board and resolve its pinned packages and actual footprints.
2. Define its outline, cutouts, holes and mechanical clearances.
3. Preview rough automatic placement and apply a chosen result.
4. Enable ratsnest, adjust selected connectors/test points/components and lock
   their intended poses. Use numeric dimensions as well as dragging.
5. Place the remaining movable components while preserving fixed intent.
6. Save reviewed source edits, rebuild, route, refill and run native DRC.

Neither a useful preview nor a short ratsnest establishes routability, electrical
function, impedance, assembly suitability or manufacturing qualification.
Independent CAM qualification is not a prerequisite for editor use.

## Authoritative data and persistence

`.copper` remains authoritative. Geometry lives in `MechanicalDesign`, alongside
electrical `Board` in `Design`; it never becomes schematic geometry or electrical
connections. Existing physical-intent constraints lower to physical IR. The UI
scene is a derived view, not a second netlist or authoritative floorplan.

Saving changes updates the board's mechanical declarations and relevant physical
constraints. Generated KiCad files, JSON scenes, view state and solver reports
are outputs only. Do not rewrite imported library definitions. Hierarchical
targets identify resolved instance paths and lower back to source-level targets.
Rigid clusters/hard macros move as units; a user cannot detach their members or
discard private copper by dragging a component.

Source edits require complete syntax spans, document revision checks and a
comment-preserving patch layer. Update owned existing declarations rather than
adding competing constraints. Present conflicting/unowned declarations for
resolution; declaration order must not decide which constraint wins. Validate
the prospective complete design before an atomic save, preserving file encoding
and line endings. Source undo/redo must use the same edit transactions.

Automatic placement is a preview, with explicit apply/discard. Applying poses
does NOT create fixed constraints for every component. Movable poses are session
seeds. Only deliberate locks are persistent design intent. Reopening a board
may regenerate movable poses; exact reproducible poses require explicit locks
or a separately specified future seed mechanism, not a hidden source of truth.

## Mechanical editing

The initial geometry palette uses supported circles, rectangles, simple polygons,
polygonal cutouts and named round NPTH holes. Holes expose finished diameter and
optional screw-head placement clearance. Distinguish substrate voids, placement
keepouts and copper keepouts; creating one must not silently create the others.
Plated/net-connected holes remain component pads, not unconnected NPTH features.

Later increments add named datums, dimension/edge-relative attachment, rounded
corners, exact arcs and routed slots. Each requires language/IR, legality, export
and regression coverage before enabling its editing controls. References to
arbitrary edges must be stable IDs, not unstable polygon vertex indices.
Explicit connector-body overhang must be separate from copper-to-edge legality.
Enclosure/DXF overlays are reference data with explicit units/transforms and
asset identities; never silently heal a drawing into manufacturing geometry.
Full enclosure solid modelling, panels and general MCAD constraints are deferred.

## Component editing and placement

Render actual pads, courtyards, conservative body envelopes, mechanical holes,
regions, keepouts and footprint provenance. Front and rear share authoritative
board coordinates; a mirrored rear view is presentation only. Include hierarchy,
reference search, zoom/fit, measurement, grid snapping and exact numeric editing.
Footprint origins and pad anchors must be labelled rather than confused with
component body centroids.

Support position-only, rotation-only and full-pose locks, side choice, placement
regions, alignment and allowed angles, including explicitly allowed 45-degree
increments. Honour existing source constraints and macro ownership. Explicit
editing of a source lock requires a reviewed source transaction; an ordinary
auto-placement or drag must not bypass it. Session locks must be visibly distinct
from source locks and must not be described as saved before source persistence.

Initial auto-placement reuses the existing connectivity-aware estimator,
legalizer and refinement engine. 'Place remaining' retains manual/source locks,
regions, allowed orientations, fixed sides and rigid geometry. Candidate
selection is deterministic; preview is cancellable and cannot mutate accepted
state on failure. Interactive hints may be approximate, but apply/save checks
use the existing authoritative placement/material predicates. Validation reports
must explain the reference and rule involved, not merely say 'illegal'.

## Ratsnest

Offer global, selected-component and selected-net views; power/GND and side
filters reduce clutter without changing connectivity. Recompute as components
move or rotate, including nearest physical lands on repeated terminal numbers.

Build a deterministic Euclidean minimum spanning tree between explicit physical
connectivity islands. Edge weights use the nearest pad-centre pair between islands,
not component centres. Use shared copper-contact roots: repeated pad numbers alone
do not imply an internal conductive connection; declared internal pad groups do.
Already connected explicit macro/track/via copper joins islands. Zone outlines do
not. Initial endpoints are pads; arbitrary free-track/via endpoints and verified
native-filled islands are later extensions. Therefore a pad-anchored tree is a
guide, not the minimum physical route or a copper continuity certificate.

Distinguish electrical guides from routed copper. A routed-board overlay must
identify the exact board revision; stale copper is visibly stale and cannot hide
new airwires. Filter state changes only presentation, never compilation/ERC.

## Architecture and local security

Start with a dependency-free Python local service and bundled HTML/CSS/JavaScript
SVG UI. Keep transport, scene, session operations and source editing separate.
The UI can later be hosted by a VS Code webview/custom text editor using the same
scene/operation contract and the editor's document/undo services.

Bind only `127.0.0.1`, default to an ephemeral port, check Host and Origin, require
a random per-session capability header for every API request and reject cross-
origin requests. No CORS, arbitrary filesystem endpoint, shell endpoint or remote
UI/CDN resources. Send the token in the launch URL fragment, not query strings or
server access logs. Require bounded JSON bodies, explicit operation fields and
revision checks; serialize mutations. Asset/package resolution respects explicit
locked/offline options and search roots. A preview must not fetch new dependencies.

The scene contract carries integer nanometre geometry, decimal-angle strings,
source revision, session revision, source/session lock status, footprint warnings,
macro ownership and truthful feature capabilities. Convert to millimetres only
for presentation. Derived scene fingerprints are not manufacturing signoff.

Undo/redo is bounded. A new successful edit discards redo, whereas previewing or
rejecting a candidate leaves history intact. Successful mutations invalidate
older pending previews. Future async jobs are bound to revisions and cancelled
or discarded when their input changes; stale results cannot apply.

## Verification and first delivery scope

Test front/rear and 45-degree transforms, hole/cutout rendering, deterministic
ratsnest, physically separate duplicate lands, declared internal connections,
explicit copper reuse, source/session lock retention, undo/redo, failed/stale
transaction rollback, HTTP origin/host/token/body validation and wheel assets.
Browser verification must exercise auto-place preview/apply and ratsnest filters,
not just load a screenshot. Later test source round-trips, conflicting constraints,
mechanical legality and routing/fill invalidation end-to-end.

First delivery: read-only source, mechanical/footprint view, ratsnest filters,
rough auto-place preview/apply, legal in-session whole-unit pose previews,
temporary full-pose locks and undo/redo. It must explicitly say that edits are
not saved, provide no misleading save control and leave source bytes unchanged.
Mechanical authoring, persistent/partial locks, source-selection links, copper
overlays, measurements, datums, slots and VS Code hosting remain checklist work.

CLI contract: `copper edit-mechanical board.copper --footprint-root <root>`.
`--locked --offline`, physical layer/profile settings, explicit templates/macros,
`--no-browser` and a scene JSON export support reproducible/headless inspection.

## First preview delivery: use and limitations

The launch command is now implemented. From the CopperScript checkout:

```powershell
uv run copper edit-mechanical examples/mechanical_editor_demo.copper --footprint-root "C:/Program Files/KiCad/10.0/share/kicad/footprints"
# Compile a real project's locked scene without starting a service:
uv run copper edit-mechanical ../CopperLedRing/board.copper --locked --offline --footprint-root "C:/Program Files/KiCad/10.0/share/kicad/footprints" --scene-output build/led-ring-scene.json
```

Linux: use your installed footprint root, commonly `/usr/share/kicad/footprints`.
The first command launches the local UI. Keep the terminal alive; Ctrl+C stops
it. `--no-browser` prints a private loopback launch URL for manual opening.
Scene export refuses existing files and cannot overwrite source. `--layers`,
`--fab-profile`, `--placement-templates` and repeatable `--hard-macro` mirror the
existing physical pipeline. `--allow-proxy-footprints` is labelled inspection-only.

Click 'Preview rough auto-placement', inspect, then apply/discard. Select an
unlocked component and left-drag it: the pose applies after legality checking and
can be undone. Enable 'Preview drags before applying' for explicit drag previews.
Numeric X/Y/angle/side edits still require Apply or Discard; a visible pending
banner explains why another move cannot start. Wheel zoom is cursor-anchored;
right-button drag pans, including when a preview is pending. Both navigate only,
without changing placements. A rejected/conflicting operation reloads the accepted
scene revision; 'Reload session scene' also refreshes it explicitly (not source).
Temporary locking preserves that pose during
subsequent placement. Source locks cannot be edited/unlocked in this increment.
The demo's R1 allows 45-degree rotations; other demo parts retain their normal
angle constraints. Nets/side/selected-component toggles filter ratsnest display.

Live drag shifts pad-anchored airwire geometry; the authoritative island graph
and nearest-pad MST are recomputed on release, not continuously during a drag.
Rear view currently uses shared unmirrored coordinates, not a mirrored display.
Footprint bodies are conservative envelopes; pads/courtyards use imported geometry.
Unplaced/illegal macros produce an explicit warning and receive no private-copper
connectivity credit until their pose can materialize legally. Copper overlays,
footprint-local copper-keepout rendering, measurements and detailed per-rule rejection diagnostics
remain future work; the first pose gate uses complete-board legality.
Board-level copper keepouts are now displayed by the profile integration, with
layer-scope and imported-owner tooltips. They are not a routed-copper overlay.

Session state is in memory only. Closing/restarting loses temporary poses/locks;
no action writes the `.copper` source or saves a derived second floorplan. There
is no mechanical drawing tool or save button yet. Source file changes disable
mutations until restart. Dependencies/asset changes during a running preview
are not monitored yet; restart after changing them. Placement runs are synchronous
and serialized in this increment, without a cancellation/progress UI.

Regression tests: `uv run --extra test pytest tests/test_mechanical_editor.py`.
Optional browser smoke: with Node, Playwright and a Chromium browser installed,
run `node tests/browser/mechanical-editor.cjs <private-local-demo-URL>` against
the freshly started demo session (the smoke changes temporary poses/locks).
`COPPER_PLAYWRIGHT_MODULE` and `COPPER_BROWSER_EXECUTABLE` may
point to explicitly installed test tools. These are not runtime dependencies.

### Source-patch foundation (not yet a save workflow)

`pcbir.editor.source` provides immutable UTF-8 snapshots, exact token-end offsets,
revision-bound non-overlapping text edits, and targeted board-owned
`fixed_placement` candidates. Existing comments (even inside a scalar), UTF-8 BOM,
newlines and unrelated source bytes survive. New constraints are inserted
deterministically. Shared/duplicate constraints and unowned imported poses are
rejected; imports are neither fetched nor edited by this pure patch layer.

## Persistent editing and initial mechanical authoring

The local editor now supports reviewed source saves. Select a component, enable
**Edit source locks explicitly**, choose position/rotation/side independently,
enter the intended pose, and choose **Review persistent pose / locks**. Review
the complete source diff and geometry before **Save reviewed source**. Clearing
a lock removes only its owned properties. Imported module/profile poses require
editing their public binding or original owner; the editor reports this conflict.

Board-owned constraints may address resolved descendants as `MODULE/COMPONENT`.
Electrical connections still cross module ports. Movable automatic placements
remain session seeds and are never silently converted into fixed constraints.

Mechanical features have stable IDs and exact CopperScript literal properties.
The palette supports circle/rectangle/polygon outlines, NPTH holes with screw-head
clearance, polygon cutouts, side-specific placement keepouts and layer-specific
copper keepouts. Select a feature to update its properties or review its removal.
Polygon vertices can be entered numerically or clicked on the snap grid. The
measurement tool reports distance and coordinate deltas between snapped points.

Every source candidate is compiled and physicalized in memory using offline,
locked dependencies. Electrical identity, placement rules, substrate topology,
pad/drill clearances and copper keepouts are checked. Saving checks the revision
again and uses a same-directory fsynced temporary file and atomic replacement,
with an exclusive lock between cooperating editor sessions. Source history is
bounded and uses the same checks. **Reload source** explicitly discards session
previews/history after an external edit. No background source merge occurs.

Placement and source history are separate. Successful source changes rebuild
unrouted physical intent and flag routing/fill/manufacturing outputs stale.
Reopening reconstructs source locks; movable seeds can change. Re-run the build
before using existing manufacturing output. These checks are not routing signoff.

Automatic placement now runs in a cancellable process with a wall-time budget,
phase progress and a cumulative performance profile. The service remains usable
during computation. Manual edits cancel older work, and a changed source/session
revision discards the result. Only incident ratsnest nets are recomputed for pose
changes; exact graph semantics remain unchanged.

## Routed reference and connectivity evidence

The compiler writes a route-intent sidecar and the shared Make runner records its
digest. After a fresh `make route`, export an editor reference with native KiCad
Python (on Linux typically `/usr/bin/python3`; on Windows KiCad's `bin/python.exe`):

```sh
make EXAMPLE=mechanical-editor editor-overlay RUN_DIR=build/my-run KICAD_PYTHON="C:/Program Files/KiCad/10.0/bin/python.exe"
make EXAMPLE=mechanical-editor edit EDITOR_ARGS="--overlay build/my-run/editor-overlay.json"
```

Direct CLI equivalent: `copper editor-overlay build/my-run/run.json
--kicad-python <interpreter> -o build/my-run/editor-overlay.json`. The output must
be a new JSON file beside the run manifest; evidence files are never overwritten.
Old builds without the intent sidecar must be rerun. A failure draft may be
inspected, but missing saved-fill/native-DRC evidence grants no plane connectivity.

Actual filled rings (including holes/fractured boundaries) are extracted read-only
from the native board. **Zone intent outlines never count as filled copper.** The
reference has source, electrical/dependency, physical, PCB, DRC and content digests.
The native report applies only to those exact saved PCB bytes. A zero-open matching
report closes presentation airwires; partial reports retain native-open markers and
explicit-copper island airwires. This is connectivity evidence, not release signoff.

Router poses seed the session without freezing components. Moving a part or
changing source, dependencies, footprints or rules fades/labels the reference
STALE and restores unrouted source airwires. Undo/discard can restore the exact
matching state. Persistent edits always rebuild unrouted intent, not old copper.
Layer/side/net/component/signal-supply filters and optional airwire labels/costs
help inspect the result. Costs are straight-line MST estimates, not detailed route
predictions.

## VS Code host

The optional [extension](../integrations/vscode/README.md) stages this same web
core as a custom text editor. A bounded local stdio compiler reads unsaved source
buffers in memory; it cannot write source files. Reviews produce minimal UTF-16
WorkspaceEdit spans. VS Code owns versions, Undo/Redo, encoding, dirty documents
and Save; a stale review cannot commit over a newer buffer. Invalid unsaved text
does not silently fall back to the disk file, and recovers on a valid later version.

Only trusted local workspaces are supported. Configure an explicit locked compiler
command and its exact content fingerprint, along with the physical build settings.
The editor performs no downloads, executes no source-provided code, and opens no
server port in this host. Component source links navigate to the root declaration
or hierarchical instance. Follow the extension README to stage and open it.
Advanced mechanical intent remains in checklist section E.
