# Physical hard macros

Decision CS-136. The initial implementation is **experimental**, not an
RF-qualified library or another electrical source of truth.

## Accepted contract

A hard macro is a physical realization attached to an electrical module or
explicit component set. Electrical hierarchy, pins and nets remain authoritative
in electrical IR. Geometry belongs only in physical IR/assets. The intended
reusable unit is an electrical module plus a separately selected physical macro;
the first trial explicitly bound component instances through JSON scenes.
Source bindings now use `physical_macro` declarations with explicit member/net
mappings; automatic component-name inference is not supported.

- Pin asset content, exact member/footprint identities and explicit pad/net
  role bindings. Isolated lands remain present and unassigned. Different packages
  require reviewed adaptations, not automatic stretching.
- Transform poses, copper, ports and keepouts together in one local frame,
  including declared 45-degree rotations. No implicit mirroring/layer reversal.
  Fixed poses/orientations remain constraints. Explicit positive member edge
  margins support edge mounting without relaxing copper-to-edge DRC.
- Preserve local tracks/vias and reserve private regions against **all** new
  routing, including same-net shortcuts and soft rip-up. Owner lead-ins reach
  external ports outside the regions. Verify actual port/private-pad copper
  continuity, not just equal net names.
- Specify fill exclusions separately. Ground-return geometry may be essential:
  generic fill/stitching must not bypass a matching capacitor's prescribed path.
- Check layer/span/technology compatibility and bind geometry/ownership into
  fingerprints, export and signoff evidence. Layer names alone do not qualify a
  stackup. Failed materialization cannot partially mutate the board.
- Qualification also covers impedance/reference planes, mounting edge/corner,
  assembly and RF behavior. The v0.1 format rejects local pours. The v0.2
  format described below admits bounded owned zones and pending plane returns;
  arcs, mirrored instances and arbitrary vendor CAD import remain unsupported.

## Implemented lifecycle

Definitions can also be authored as `macro NAME { ... }` Copper documents and
exported from ordinary packages. Bind a named export with
`physical_macro UNIT: alias.NAME { ... }`. Lowering generates footprint digests
from verified assets and the current v0.5 JSON contract; physical binding uses
the same immutable lifecycle. `copper compile-macro` emits optional compatible
JSON. Source/footprint revisions invalidate previous evidence. See
[definition syntax](macro-definitions.md) and [requirements](macro-definition-plan.md).

Boards/modules can select assets through ordinary imports and declare bindings:

```copper
import layout "github.com/vendor/library/assets/layout.json";
physical_macro UNIT: layout on CIRCUIT {
    member chip = U1;
    member passive = R1;
    net signal = port.SIGNAL;
}
```

This illustrative asset has two members and one net role. The declaration
references existing circuitry. `on` is optional; unscoped bindings can use
explicit descendant paths. Dependency versions live in `copper.mod`; hashes
come from `copper.lock`, without a source SHA field. `copper lock` prepares the
selected JSON assets; shared physicalization binds them for placement, routing,
export and editor rebuilds without an extra CLI scene argument. Local imports
resolve from the declaring source and retain package ownership. See the
[language reference](language-reference.md#physical-macro-bindings).

Electrical-only checks keep asset/geometry checks pending. Source requirement
verification checks current selected assets, member/pad/net bindings and
materialized immutable ownership. Missing macros or changed input bytes cannot
reuse old evidence; this check does not establish RF/power/native-fill or
manufacturing qualification. The existing JSON scene interface remains supported
with explicit hashes; source and scene bindings must have disjoint owners and
distinct identities.

`bind_hard_macro()` reads pinned JSON without execution, fetching, rewiring or
movement. It creates a `RigidPlacementCluster` plus typed `PhysicalHardMacro`,
`MacroPadBinding` and `MacroPort` records. `cluster_placements()` supplies the
whole-unit pose; `materialize_hard_macros()` atomically commits transformed copper,
checks native physical DRC and proves port continuity. Repetition is idempotent.

Rigid-unit/private-region placement checks apply before materialization.
Afterwards owner copper must remain exactly present at its transformed pose.
Movement requires rebuilding from the unrouted source, never stripping arbitrary
existing input copper. Ordinary detailed routing preserves the locked prefix,
recognizes connected owner nets, and substitutes proved **pad, free-track and
plated-via ports** for private terminal groups. Via terminals expose only their
actual barrel span. Actual electrical/physical netlists retain every private
pad; collapsing groups is a routing view, not rewiring. Overlapping port pad
groups and ports without proved owner connectivity fail closed. Default
no-via/pad-overlap and ordinary DRC rules remain mandatory.

KiCad export locks member footprints, tracks and vias and emits ordinary
layer-specific keepout/fill exclusions in the self-contained project. Private
owner-aware access reservations are enforced by CopperScript, not exported as
blanket track keepouts that would reject owner copper. Manually unlocking/editing
KiCad output does not preserve this contract; revalidate authoritative physical IR.

An asset via may specify `finish = "filled-capped"`; omission keeps the legacy
`standard` finish. A filled/capped macro via overlapping an SMD land requires
the scoped `via_in_pad` permission, complete annulus containment and the
supported six-layer GND process. Finish survives transformation and replay.
Because native KiCad board geometry does not encode the fabrication process,
exports with non-standard vias include a same-stem `.via-process.json` sidecar
listing net, position, diameter, drill, layers, finish and fabrication profile.
Keep it with the project: the export digest covers it, but it grants no
manufacturing qualification. Re-export removes only a marked, generated stale
sidecar; unrelated process records are preserved.

The normal `route-board` pipeline accepts repeatable `--hard-macro SCENE.json`.
Planning materializes owner copper in a scratch board, includes occupied copper
and private reservations in capacity/access searches, and recognizes already
connected internal nets. Package fanout skips bound private pads. Critical and
detailed stages retain the immutable prefix; feedback rebuilds the entire macro
at the accepted unit pose. Source recovery rejects arbitrary non-owner copper
and filled input boards, rather than discarding it. Fingerprints identify the
same pose/net topology before and after materialization.

## v0.2 local copper and plane-backed returns

The v0.2 asset may declare local `zones` with explicit net, layer, polygon,
priority, clearance, minimum width and solid/thermal pad connection. Binding
pins them to the asset digest; materialization rigidly rotates them with the
tracks and vias. A later change or deletion fails owner validation. KiCad
exports them as locked zones, and source recovery removes only those exact
owner zones. Each local polygon must stay inside its protected region. Host
zones overlapping that region on an owned zone layer fail before export unless
the host zone explicitly opts into an exact same-net overlap; keepouts blocking
an owned zone always fail. A local VIN/VOUT pour does not make that rail a
deferred distribution-plane net:
external terminals still need ordinary routing.

A board host zone may set
`allow_same_net_hard_macro_overlap = true` in its `copper_zone` constraint to
share a macro-owned zone on the same net and layer. This is opt-in per host
zone, remains subject to the macro's zone permissions and copper keepouts, and
does not allow a foreign-net zone into the protected region.
Blocking keepouts are scoped to that macro region when checking the overlap
permission. A remote antenna/matching exclusion on a whole-board host pour must
not reject an otherwise compatible regulator GND overlap; the native filler still
clips that pour at the remote keepout. Separate zone objects can abut as continuous
copper rather than have positive area overlap. Verify actual filled polygons:
the overlap option or zero unconnected items alone does not prove top continuity.

An optional `plane_returns` group declares every private pad on one net and
the inner plane layers it must reach. The pre-fill proof requires each pad's
explicit copper root to contain a via inside a same-net plane zone; dedicated
contacts additionally identify distinct owner vias for named pads. It does
not join separate roots or claim filled-plane continuity. Independent native
KiCad refill and zero unconnected items are required to close that pending
claim. Removing a pad via, changing its net, losing the plane, or adding a
host zone on an owned layer into the private region fails. The TPS62130A
single-regulator probe at `examples/tps62130a_macro/` checks the filled local
polygons and native DRC without a larger board placement run.

Plane stitching reuses exact private-pad-to-existing-via graph continuity; it
never creates shortcuts through protected ground returns. Without a usable
existing macro via, private ground pads remain pending. A prospective contact
does not prove filled-plane connectivity: independent KiCad refill is required.

## v0.3 fixed copper polygons

The v0.3 asset adds `polygons`: each entry has an `id`, net role, one copper
`layer`, and `vertices` for a simple closed ring. It represents an exact,
single-layer conductor rather than refill intent. Binding validates the ring
and protected-region containment. Materialization rigidly transforms it,
fingerprints it, preserves it as immutable owner copper, and includes its
positive-area contacts with pads, tracks and vias in the explicit connectivity
proof. Source recovery removes only exact owner polygons.

KiCad 10 export emits locked, filled, netted `gr_poly` items. CopperScript checks
polygon clearance and board-edge spacing; independent KiCad DRC remains
required, particularly for polygon-to-zone fill interactions. Use fixed
polygons for short prescribed copper shapes and zones for broad pours. Holes,
arcs, auto-generated polygon routing and mirrored instances remain unsupported.
The one-regulator probe exercises the SW island without the former wide SW
track trunk.
Single-ended pre-routed critical owner nets retain length/via budgets. Entirely
pre-routed differential macros fail closed until paired geometry/return-path
certificates are supported. Multiple alternative ports for one private group,
automatic boundary allocation and RF/stackup qualification remain out of scope.
The full-vertical board now opts into board-specific, digest-bound adaptations
of the TPS62130A and Nordic/Johanson assets. The assets retain their original
local copper; the board supplies the matching electrical nets and added parts.
See [the integration plan](full-vertical-example.md).

## v0.4 scoped track-width contracts

The v0.4 asset retains the v0.3 fields and requires `width_contracts` (possibly
empty). Each entry names one zero-based **asset track-row** index and contains
`minimum_width_nm`, `purpose` and nonempty `evidence`. The binder expands that
row's polyline into typed segment contracts. Indices must be unique/in range;
dimensions are positive integer nanometres; the fixed track must meet its
declared minimum. Purposes are `pin_entry`, `control_supply`, `output_sense`,
`enable` or `main_power`. Duplicate contracted geometry fails closed.

```json
{"track_index": 12, "minimum_width_nm": 200000,
 "purpose": "output_sense",
 "evidence": "Authored low-current feedback sense; cite package evidence and unresolved adaptations."}
```

After materialization and immutable-owner validation only, a matching exact
owner segment uses the greater of its local minimum and the board fabrication
minimum instead of the host net-wide width rule. All undeclared owner tracks
and ordinary host copper retain the net profile, including breakout rules.
This is not a net-, layer-, region- or same-net waiver. Additional matching
host occurrences gain no extra permission. Clearance, layers, route length,
via limits, ownership and connectivity checks are unchanged. Each application
is an informational `DRC-MACRO-WIDTH-CONTRACT` finding recording the segment,
owner, asset digest, purpose, evidence, host minimum and applied/actual widths;
typed contracts are included in physical geometry fingerprints/signoff evidence.
Legacy v0.1–v0.3 behavior is unchanged and rejects the additional field.

The contract states authored geometry, not an established current capacity.
It must identify why a branch or pad entry has different geometry from host
distribution, retain unresolved current/thermal evidence and stay unqualified.
KiCad exports preserve locked owner copper; independent native DRC is still
mandatory. Manual edits to an exported PCB do not preserve CopperScript's
owner-bound contracts and need authoritative IR revalidation.

## v0.5 host-perimeter openings

The v0.5 format retains v0.4 and requires `edge_openings` (possibly empty):

```json
{"keepout": "antenna-corner", "edge_indices": [0, 1]}
```

Indices select consecutive-vertex edges of a local simple zone-blocking keepout
ring, including its implicit closing edge. They must be distinct, nonnegative
integers within that ring. Missing keepouts, non-clearance regions, duplicate
contracts and malformed indices fail at binding. The entire transformed
keepout must remain within the host outer polygon, and each declared edge must
coincide fully with the outer perimeter. Point contact, partial overlap and
internal cutout boundaries are insufficient. Curved host boundaries currently
fail closed rather than certify sampled polygon approximations.

Whole-unit rotation transforms keepouts and requirements together. Shared
placement acceptance, macro materialization and immutable-owner replay enforce
the check; physical fingerprints include the typed contract. Legacy assets
remain unchanged and reject the new field. This geometric contract is not an
impedance, matching, radiation or manufacturing qualification.

CopperLib's separate `nrf52832-johanson-managed-corner.json` adaptation declares
the Johanson two-edge opening while preserving its source-backed Nordic copper,
all six-layer exclusions and isolated NC land. It does not silently replace the
older installed-footprint trial or authorize 45-degree corner mounting on a
rectangular board.

## Nordic/Johanson trial

CopperLib owns `packages/circuits/nordic/nrf52832-johanson-reference/assets/nrf52832-johanson-six-layer-trial.json`
and its package-local generator.
The electrical probe and its URL-imported `physical_macro` binding are both in
`examples/nrf_antenna_macro/board.copper`. The coin-cell board selects the same
pinned library asset in its own source. The former example binding scenes have
been removed; legacy scenes remain supported by the compiler.

Seven members: nRF52832 QFAA, Nordic 0.8 pF/3.9 nH chip matching, a **separate**
Johanson evaluation-style tee (1 pF series, 2.7 nH shunt, 3.9 nH series), and
2450AT18A0100001E antenna. Seven chip-matching strokes are verified against the
pinned Nordic LDO top Gerber; actual vertices/widths are retained and their
contact with installed KiCad lands is DRC-tested. Adapted ground extensions
connect C3 through VSS 31 to EP/VSS 45/off-pad access; two through vias connect
local ground across B.Cu. Inner/bottom copper is excluded under chip matching.
The antenna NC mechanical land remains isolated.

The antenna runs along the upper-right edge of a 50 x 40 mm probe, with an
adapted tee/feed and 6.5 x 6.5 mm corner fill/via exclusion. This is **not** a
digitized Johanson reference PCB. Ground shape, via fence, feed width/impedance,
tee values and enclosure/radiation behavior remain unqualified. Copying
evaluation values does not tune the combined system. Full-vertical now includes
this physical trial as a prototype; its antenna tuning remains unqualified.

From CopperScript with the pinned Git footprint providers (CopperLib is
URL-resolved):

```powershell
uv sync --extra test
uv run python -m examples.nrf_antenna_macro.hard_macro_trial --offline
```

Ignored `build/nrf-hard-macro/` contains PCB/project/local library, a diagnostic
physical snapshot, native DRC report, explicit `erc.json` and default cumulative-time profiling
(`profile.pstats`, `profile.txt`). Add `--rotation 45 --output-dir
build/nrf-hard-macro-45` for a larger centred transform probe, **not** qualified
45-degree corner mounting.

Library-author maintenance only (not required for using the examples): from
the CopperLib repository, regenerate using the locally cached official source:

```powershell
python packages/circuits/nordic/nrf52832-johanson-reference/generate_trial.py `
  cache/rf-reference/nrf52832qfaxreflayoutv11.zip
```

On 2026-10-02 the 0°/45° probes passed native physical and KiCad 10.0.6 DRC with
zero reported violations/unconnected items. All seven footprints and 22 copper
objects are locked. Only declared RF/GND nets are checked: the powered MCU,
oscillator and decoupling circuit are absent. No operational-radio or
manufacturing-readiness claim follows from these results.

Normal `copper check/compile` intentionally rejects this incomplete electrical
probe with three unsourced VDD input errors. The dedicated geometry trial retains
those findings in `erc.json` without waivers; its locked/offline parser loading
is not an ERC success or permission to release the design.

An independent KiCad refill regression adds blanket GND zones, queries actual
filled polygons (with a positive fill control), and confirms that matching C3
and antenna-corner exclusions remain empty on F.Cu, In1.Cu and B.Cu. All locked
tracks/vias survive the refill. This is exclusion evidence, not plane/impedance
or full-board RF qualification.

## Sources

The [coin-cell example](nrf52-coin-cell.md) adds the powered MCU support
circuit, SWD and user controls without changing this macro's electrical or
physical contract. It remains a placed draft; the non-RF connections are not
routed by its dedicated builder.

- [Nordic reference guidance](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/ref_circuitry.html?contentId=Deg~HuyXWjteAChpH2NzWw):
  chip matching, C3 return through VSS 31 and inner-layer exclusions.
- [Nordic QFAx archive](https://nsscprodmedia.blob.core.windows.net/prod/software-and-other-downloads/reference-layouts/nrf52832qfaxreflayoutv11.zip):
  QFAA LDO v1.1 Gerber/placement, with archive/entry identities in the asset.
- [Johanson 36S0021A Rev. 4.0](https://www.johansontechnology.com/docs/3827/Antenna-2450AT18A0100001E-Rev4.0.pdf),
  pp. 2–3: isolated terminal, corner clearance, evaluation tee and PCB-dependent tuning.
- [KiCad PCB format](https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/):
  lock flags and copper/keepout objects.
