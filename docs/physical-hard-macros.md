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
recognizes connected owner nets, and substitutes supported **pad-backed ports**
for private terminals. Free/via ports are validated but are not yet router
terminals. Default no-via/pad-overlap and ordinary DRC rules remain mandatory.

KiCad export locks member footprints, tracks and vias and emits ordinary
layer-specific keepout/fill exclusions in the self-contained project. Private
owner-aware access reservations are enforced by CopperScript, not exported as
blanket track keepouts that would reject owner copper. Manually unlocking/editing
KiCad output does not preserve this contract; revalidate authoritative physical IR.

The normal `route-board` package-access/critical/placement-feedback pipeline
**does not yet consume macros** and rejects them explicitly. Future boundary
allocation must account for local occupancy, retain RF/critical ownership,
rebuild whole macros after accepted moves, and recheck boundary connections.
This trial is not automatic full-vertical integration.

## Nordic/Johanson trial

CopperLib owns `data/full-vertical/nrf-antenna-hard-macro.json` and its generator.
The electrical probe/bindings are `examples/nrf_antenna_macro.copper` and
`examples/nrf_antenna_hard_macro.json`.

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

From CopperScript with sibling CopperLib and installed KiCad footprints:

```powershell
uv sync --extra test
uv run python -m pcbir.hard_macro_trial `
  --footprint-root "C:\Program Files\KiCad\10.0\share\kicad\footprints" `
  --footprint-root "..\CopperLib\footprints"
```

Ignored `build/nrf-hard-macro/` contains PCB/project/local library, a diagnostic
physical snapshot, native DRC report, explicit `erc.json` and default cumulative-time profiling
(`profile.pstats`, `profile.txt`). Add `--rotation 45 --output-dir
build/nrf-hard-macro-45` for a larger centred transform probe, **not** qualified
45-degree corner mounting.

Regenerate the asset deterministically without network/model tokens:

```powershell
uv run python ..\CopperLib\scripts\extract_nrf_antenna_hard_macro.py `
  ..\CopperLib\cache\rf-reference\nrf52832qfaxreflayoutv11.zip
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
