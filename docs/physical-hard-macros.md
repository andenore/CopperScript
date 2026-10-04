# Physical hard macros

Decision CS-136. The initial implementation is **experimental**, not an
RF-qualified library or another electrical source of truth.

## Accepted contract

A hard macro is a physical realization attached to an electrical module or
explicit component set. Electrical hierarchy, pins and nets remain authoritative
in electrical IR. Geometry belongs only in physical IR/assets. The intended
reusable unit is an electrical module plus a separately selected physical macro;
the first trial explicitly binds component instances, without new frontend
syntax or automatic component-name inference.

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
  assembly and RF behavior. Unsupported local pours, arcs, mirrored instances
  and arbitrary vendor CAD import fail closed in this first asset format.

## Implemented lifecycle

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

The normal `route-board` pipeline accepts repeatable `--hard-macro SCENE.json`.
Planning materializes owner copper in a scratch board, includes occupied copper
and private reservations in capacity/access searches, and recognizes already
connected internal nets. Package fanout skips bound private pads. Critical and
detailed stages retain the immutable prefix; feedback rebuilds the entire macro
at the accepted unit pose. Source recovery rejects arbitrary non-owner copper
and filled input boards, rather than discarding it. Fingerprints identify the
same pose/net topology before and after materialization.

Plane stitching reuses exact private-pad-to-existing-via graph continuity; it
never creates shortcuts through protected ground returns. Without a usable
existing macro via, private ground pads remain pending. A prospective contact
does not prove filled-plane connectivity: independent KiCad refill is required.
Single-ended pre-routed critical owner nets retain length/via budgets. Entirely
pre-routed differential macros fail closed until paired geometry/return-path
certificates are supported. Multiple alternative ports for one private group,
automatic boundary allocation and RF/stackup qualification remain out of scope.
This does not automatically replace the full-vertical board's electrical circuit.

## Nordic/Johanson trial

CopperLib owns `packages/circuits/nordic/nrf52832-johanson-reference/assets/nrf52832-johanson-six-layer-trial.json`
and its package-local generator.
The electrical probe/bindings are `examples/nrf_antenna_macro/board.copper` and
`examples/nrf_antenna_macro/hard_macro.json`.

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
evaluation values does not tune the combined system. The normal full-vertical
board remains unchanged pending qualification and pipeline integration.

From CopperScript with installed KiCad footprints (CopperLib is URL-resolved):

```powershell
uv sync --extra test
uv run python -m examples.nrf_antenna_macro.hard_macro_trial `
  --footprint-root "C:\Program Files\KiCad\10.0\share\kicad\footprints"
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
