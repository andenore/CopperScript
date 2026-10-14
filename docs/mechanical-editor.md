# Physical, mechanical and definition editor

Status: implemented contract in CopperScript; this guide describes supported
behavior and current limitations. The source and scene protocols remain authoritative.
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

## Opening boards, macros and profiles

`copper edit design.copper` opens the shared physical editor; `edit-mechanical`
remains a compatible alias. The top-level declaration selects board mode, macro
authoring in local coordinates, or standalone `board_profile` authoring. Macro
view bounds are a viewport, not a manufactured board outline. Placement overlays,
templates and externally supplied hard-macro scenes remain board-only options.

Omitting `--layers` selects the declared source stackup, or the smallest supported
2/4/6-layer stack containing the source's copper layers and macro requirements.
An explicit `--layers` remains strict. VS Code's layer setting defaults to `0`
(automatic). Fabrication profiles remain explicit; layer inference does not select
a vendor or certify its processes. Use a board's configured Make `edit` target
when its fabrication profile or other physical settings are supplied by the build.

For example, the full-vertical build requires six layers and its selected profile:

```console
copper edit examples/full_vertical/board.copper --fab-profile jlcpcb-six-layer
```

Open a macro's `.copper` definition rather than an individual `.kicad_mod`
footprint. Managed-cache definitions use the consuming project's dependency pins
and remain read-only. When authoring a separate local library definition, use
`--project PATH` to select the consumer manifest supplying its pinned dependencies.

On initial open, the editor prepares every dependency explicitly declared in the
nearest `copper.mod`, including source packages, footprints and macro assets.
It downloads missing modules into the normal package cache, verifies existing
inventory pins, and atomically adds missing entries to `copper.lock` only after
preparation succeeds. It preserves unrelated entries and rejects changed pinned
content. Missing manifests or undeclared URL dependencies remain explicit errors;
the editor does not choose versions or create manifest requirements.

Use `--offline` for cached/local inputs only, or `--locked` to require all pins
already exist. After preparation, opening geometry, previewing, reviewing and
saving use locked/offline inputs. Reopen after a dependency failure to retry.
In VS Code, `copperscript.mechanical.offline` provides the same cache-only option;
initial preparation has a cancellable notification. Native buffer updates do not
trigger downloads or upgrades.

## Physical feature authoring

The physical-feature panel exposes source-owned placement and routing constraints,
copper zones, via-in-pad and hole-clearance rules, stackup declarations, and board
artwork. Select an existing feature or a catalog entry, edit its named properties,
then review the source diff before saving. Values use CopperScript expressions
and engineering units; this panel is not a connectivity autorouter or a filled
copper verifier. Stackup changes are reviewed as a complete declaration.

Use **Stage change** for related edits and **Review staged changes** to validate
them atomically, such as a new macro net and its pad assignment or paired routing
rules. Staged forms are temporary and are cleared when the source revision changes.
The shared source diff, Save and undo controls apply to the complete batch.

Macro members can be dragged into a reviewed source pose. **Pick a point on the
canvas** updates a position or an indexed track/polygon vertex; pad-referenced
endpoints require an explicit literal edit before they can be detached. Copper
zones also offer **Draw a polygon boundary**. Distance/alignment dimensions use
resolved physical anchors. Layer-stack rows and zone/corridor/breakout overlays
show source intent, not verified fill or signal-integrity qualification.

Select an imported macro/profile feature and choose **Open definition** to open
its owning file. Cached dependency definitions remain read-only. Standalone
opens a separate authenticated local view; VS Code uses its native document host.

Macro authoring exposes member poses, tracks, vias, polygons and contract geometry
through the same source-review workflow. Profiles expose their mechanical items
and connector roles independently of a consuming board. Existing source comments
and unrelated declarations are retained. Validation rejects malformed definitions
and invalid geometry; consumer-dependent integration and qualification remain
pending. Board instances retain their rigid ownership while definitions are
authored separately. Source changes make downstream generated outputs stale.

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
may regenerate movable poses. After dragging or auto-placement, the top action
**Review placement to save…** proposes position, rotation and side locks for
the current layout. Inspect the source diff, then **Save reviewed changes**.
This is an explicit decision to fix the layout: edit **Saved source locks** to
move these components later. Rigid macros save their anchors only; their members
and private copper remain immutable. Existing imported/attachment ownership is
preserved, and ambiguous ownership is reported before saving. Invalid placement
cannot be saved. Discarding a source review retains the placement draft.

The **Placement unsaved** indicator compares the layout with source-derived
poses. Saving an unrelated feature does not claim to save temporary placement;
the indicator remains until placement is saved or the source is reloaded.

## Mechanical editing

The initial geometry palette uses supported circles, rectangles, simple polygons,
polygonal cutouts and named round NPTH holes. Holes expose finished diameter and
optional screw-head placement clearance. Distinguish substrate voids, placement
keepouts and copper keepouts; creating one must not silently create the others.
Plated/net-connected holes remain component pads, not unconnected NPTH features.

Named datums, dimension/edge-relative attachment, rounded corners, bounded exact
arc paths and NPTH slots are supported as described below. Unsupported geometry
fails rather than being silently approximated. References to
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

Auto-placement reuses the existing connectivity-aware estimator,
legalizer and refinement engine. 'Place remaining' retains manual/source locks,
regions, allowed orientations, fixed sides and rigid geometry. Candidate
selection is deterministic; preview is cancellable and cannot mutate accepted
state on failure. Interactive hints may be approximate, but apply/save checks
use authoritative material/hard-geometry predicates; source saves and automatic
placement additionally enforce represented relative constraints. Validation reports
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
older pending previews. Async jobs are bound to revisions and cancelled
or discarded when their input changes; stale results cannot apply.

## Launch and source editing

```console
copper edit-mechanical board.copper --locked --offline --footprint-root <root>
```

Keep the terminal alive for the local UI; Ctrl+C stops it. `--no-browser` prints
a private loopback launch URL. `--scene-output <new.json>` supports headless
inspection; `--layers`, `--fab-profile`, `--placement-templates` and repeatable
`--hard-macro` select the physical build. Proxy footprints are inspection-only.

Terminal startup reports source checking, footprint/dependency resolution and
session preparation before printing the private launch URL. Locked dependency
verification reads current relevant file bytes; it skips ignored Git/cache trees
without changing the inventory or lock checksum. Unnumbered NPTH pads remain
non-electrical and render their round or slotted drills normally.

## Reviewed persistent editing

The local editor now supports reviewed source saves. Select a component, enable
**Edit source locks**, choose position/rotation/side independently (rotation and
side are under **Rotation and side locks**), enter the intended pose, and choose
**Review pose and locks**. Review the complete source diff and geometry before
**Save reviewed changes**. Clearing
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

Refreshing or reopening the browser view retains pending
placement and source reviews in the running session, including the proposed
geometry and apply/save controls. The save hint explains which changes persist;
a successful save is explicitly reported. Restarting the service reconstructs
only persisted source intent.

The sidebar has **Components**, **Mechanics**, and **View** tabs. Left/right arrows
and Home/End switch tabs with keyboard focus. Component selection on the board
opens Components; switching tabs preserves typed pose values. Leaving Mechanics
turns off active polygon drawing and measurement so hidden tools cannot consume
board gestures.

The top bar contains board identity, source status and reviewed Save. **More**
contains source reload/history and a view refresh; temporary placement Undo/Redo
stays above the board. In VS Code, **Open board…** uses the native file picker.
Standalone board selection stays with the CLI launch command. Pending placement
Apply/Discard appears beside the board, while pending source diffs and their
Discard action appear under the file bar. These are separate transactions.

Advanced drag, ratsnest and lock settings are collapsible. The mechanics form
offers only the supported shapes for its feature type, hides shape/name selectors
where they do not apply, and shows absolute/relative and pad/mating-face anchor
fields only for the selected mode. Hidden incompatible fields are excluded from
review requests; exact source literals and existing validation remain authoritative.

Contributor browser verification uses disposable copies and fresh sessions:

```console
python tests/browser/run-editor-smokes.py ui save placement keyboard curves anchors references artwork --footprint-root <installed-KiCad-footprints>
```

Install Playwright/Chromium in the test environment first, or set
`COPPER_PLAYWRIGHT_MODULE` and `COPPER_BROWSER_EXECUTABLE` to existing local tools.
The runner never downloads tools and writes only disposable `build/` copies and
screenshots. The browser checks assert source-byte preservation or exact undo.

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

Automatic placement runs in a cancellable process without a time limit, with
phase progress, elapsed time and a cumulative performance profile. It continues
until the planner completes, reports an error or the user presses Cancel placement.
The service remains usable during computation. Manual edits cancel older work, and a changed source/session
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
Initial opening prepares declared dependencies; subsequent editing remains offline.
The editor executes no source-provided code and opens no server port in this host.
Component source links navigate to the root declaration
or hierarchical instance. Follow the extension README to stage and open it.
Advanced mechanical intent is specified in
[advanced mechanical intent](advanced-mechanical-intent.md) and its supported
source forms in [mechanical language](mechanical-language.md).

## Responsiveness and keyboard access

Ratsnest island MST uses an exact spatial Boruvka search. Bounding boxes and
uniform-component labels eliminate unnecessary distance checks while preserving
the exhaustive Kruskal tree and its deterministic tie ordering. Memory remains
linear; pathological coincident/interleaved islands may still need quadratic work.
Incident-net caching remains in place. Immutable geometry and full placement
legality are also reused for unchanged job-status polls.

Footprints expose accessible button names/selection and a roving keyboard focus.
Use arrows (Shift: ten snaps), R for the next explicitly allowed rotation, +/− for
zoom, F for fit, and Escape to cancel/reset gestures/measurement. Each partial
source lock restricts only its own dimensions. Source edits require review as
before. SVG is assembled off-document before one DOM replacement. Standalone
and VS Code share these behaviors. Offline VSIX packaging and CI artifact details
are in the extension README.

## Advanced mechanical authoring

Named datums/edges attach component origins, pads or audited mating-face points.
Dependency/conflicting-owner checks prevent silent retargeting. Body-only overhang
allowances never relax copper containment. Enclosure height limits reject unknown
heights; moving tool-access regions are distinct from copper keepouts.

Rounded outlines and closed ordered line/arc paths retain exact manufacturing
primitives. Edit all joined endpoints in the Closed line/arc path panel and
review/save one transaction; invalid intermediate joins never enter accepted
state. Initial curved queries require convex paths/arcs <=180 degrees; unsupported
geometry fails. Board-owned NPTH slots exclude material and export oval/G85 drills.

Locked DXF guides use explicit units/frame/transform and SHA-256, never material
or copper credit. The view toggle and side filter affect display only. Guide
geometry cannot be dragged; project-owned declaration transforms can be reviewed
like other source edits. Imported owners stay read-only. Changed asset bytes stale
the guide and block edits until reload, with no silent checksum update. Native
manufacturing excludes guides entirely. 3D enclosure inspection is deferred.


## Inspection and temporary moves

Board silkscreen, generated Cu/name/version branding and imported footprint
silkscreen graphics are visible by default. The View tab offers separate
silkscreen/branding and decorative copper-artwork toggles, both respecting the
front/rear filter. Footprint graphics follow component poses; board artwork stays
in its accepted source position. Native geometry is used for shapes and source
version labels; text uses a preview font rather than KiCad's stroke font.
Visibility filters preserve unreviewed numeric pose drafts.
Source-owned board artwork is editable through the physical-feature panel;
imported footprint graphics remain owned by their footprint definition. Neither
grants connectivity or manufacturing credit. Existing copper reservations and
process checks still apply.

Wheel zoom is cursor-anchored; right-button drag pans. Numeric pose changes
require Apply/Discard; optional preview drags use the same revision protocol.
Shift-click selects a group; dragging or arrow movement translates it atomically.
Rigid members expand automatically. Unknown, source-position-locked, temporarily
locked or structurally invalid member edits reject the whole move without replacing
the current draft. Rigid ownership and source locks remain authoritative.

Outside-board, overlapping, keepout-conflicting or disallowed-orientation poses
remain visible as red temporary drafts with rule explanations. Continue dragging,
editing coordinates or rotating that draft; it cannot be applied while hard
geometry is invalid. Valid default drags apply normally. Manual previews remain
editable until Apply/Discard, while automatic-placement candidates still require
Apply/Discard before manual adjustment. Escape discards a manual draft and restores
the accepted session poses. Refreshing the view retains the pending draft.

**Rotate left/right** and `R` / `Shift+R` cycle the selected component's supported
orientations, including while correcting an invalid draft. Keyboard focus follows
component selection on the canvas. Source rotation locks still require the
explicit source-lock workflow.

Apply checks hard-geometry legality. Relative distance/alignment
violations remain visible with measurements, limits and participating references;
automatic placement and source-save validation still enforce all represented
constraints. A temporary inspection pose is not saved intent.

Hovering a pad shows its electrical name, or physical number when unnamed; the
complete reference/net tooltip remains available. Visual body envelopes preserve
offset footprint origins and derive from graphics/pads, with a conservative proxy
fallback. Courtyards remain the placement authority.

| Shortcut | Action |
| --- | --- |
| `L` | Lock the selected temporary pose |
| `Shift+L` or `U` | Unlock the selected temporary pose |
| `R` / `Shift+R` | Rotate to the next / previous allowed orientation |
| `S` | Toggle front/back when the side is not constrained |
| `Ctrl/Cmd+Z` | Undo |
| `Ctrl/Cmd+Shift+Z` or `Ctrl/Cmd+Y` | Redo |
| `Home` or `F` | Fit the board |
