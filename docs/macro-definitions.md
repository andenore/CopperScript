# Copper hard-macro definitions

A `macro` document exports one named physical realization. Import its package
normally and select `alias.Name` in an existing `physical_macro` binding.
Versions belong in `copper.mod`; hashes are generated from verified inputs.
Electrical modules remain authoritative for circuitry and connectivity.

```copper
macro LinkLayout {
    source = "Layout reference; qualification unresolved";
    unresolved = ["Current capacity and native DRC review"];
    anchor = U;
    required_layers = ["F.Cu", "B.Cu"];
    allowed_rotations = [0 deg, 90 deg, 180 deg, 270 deg];
    import package "github.com/vendor/library/footprints/Chip.kicad_mod";
    net SIGNAL;
    member U { footprint = package; at = (0 mm, 0 mm); }
    pad U."1" = SIGNAL;
    isolated U."2";
    track lead {
        net = SIGNAL; layer = "F.Cu"; width = 0.25 mm;
        points = [U."1", (2 mm, 0 mm)];
    }
    port signal {
        net = SIGNAL; layer = "F.Cu"; point = (2 mm, 0 mm);
        pads = [U."1"];
    }
}
```

This syntax example requires an actual footprint with those pads and valid
geometry. Physical binding/materialization must prove port continuity. See
[the runnable synthetic example](../examples/macro_definition/README.md).

## Root properties

| Property | Meaning |
| --- | --- |
| `source` | Required nonempty reference/evidence text or structured record; no qualification claim |
| `anchor` | Required existing member used as the placement anchor |
| `required_layers` | Required nonempty unique copper-layer names, including all used layers |
| `allowed_rotations` | Unique angles in `[0 deg, 360 deg)`; default `[0 deg]` |
| `internal_clearance` | Nonnegative exact length; default `0 mm` |
| `unresolved` | List of unresolved evidence notes; default `[]` |

Declare roles with `net ROLE;`, assign pads with
`pad MEMBER."NUMBER" = ROLE;`, and preserve unassigned lands with
`isolated MEMBER."NUMBER";`. Role/member/geometry names may be quoted. Every
declared role requires a pad assignment. Duplicate/conflicting assignments
fail. Referenced pads must exist in the independent footprint; pad endpoints
must select a unique physical land.

Structured source evidence preserves existing locators, revisions, source hashes
and adaptation notes: `source = { url = "..."; revision = "..."; };`.
Records use assignment syntax, allow nested records/lists and reject duplicate
keys. Evidence hashes identify historical source documents; member footprint
digests and selected macro hashes remain automatically generated.

## Geometry blocks

Use `KIND NAME { property = value; ... }`. Properties below are required unless
specified otherwise. Names are unique within each kind.

| Kind | Properties |
| --- | --- |
| `member` | `footprint`, `at`; optional `rotation` (default `0 deg`), `edge_clearance` (default `0.25 mm`) |
| `track` | `net`, `layer`, `width`, `points` |
| `via` | `net`, `at`, `size`, `drill`, `from_layer`, `to_layer`; optional `technology` (default `through`), `finish` (default `standard`) |
| `port` | `net`, `layer`, `point`, `pads` |
| `protected_region`, `keepout` | `layers`, `vertices`; optional `block_tracks`, `block_vias`, `block_zones` |
| `zone` | `net`, `layers`, `vertices`, `priority`, `clearance`, `minimum_width`, `pad_connection` (`solid` or `thermal`) |
| `polygon` | `net`, `layer`, `vertices` |
| `plane_return` | `net`, `layers`, `pads`; optional named `contact` blocks |
| `contact` (inside `plane_return`) | `pad`, `at` for a dedicated pad-to-owner-via contact |
| `width_contract` | `track`, `minimum_width`, `purpose`, `evidence` |
| `edge_opening` | `keepout`, `edge_indices` (zero-based polygon edges) |

Points use `(x, y)` with length quantities; lists use `[...]`. Track and port
points can also use `MEMBER."PAD"`; `pads` is a list of these references.
Widths, sizes, drills and minimum widths are positive. Coordinates must convert
exactly to integer nanometres; fractional nanometres are rejected without
rounding. Angles use `deg`. Bare integers are for priority and edge indices,
not lengths or angles. Quantities use the shared fixed-point length conversion.

Protected regions default to blocking tracks, vias and zones. Keepouts default
to blocking zones only. Regions and copper polygons require simple closed rings.
Via technology identities remain subject to host-stackup compatibility;
`technology = none` preserves an unspecified technology from a JSON asset.
`finish` is `standard` or `"filled-capped"`. Ground-plane proof remains pending
until the existing independent native refill checks succeed.

```copper
plane_return ground {
    net = GND; layers = ["In1.Cu"]; pads = [U."3"];
    contact thermal { pad = U."3"; at = (0 mm, 0 mm); }
}
width_contract pin_entry {
    track = lead; minimum_width = 0.2 mm;
    purpose = pin_entry; evidence = "Reference pad entry; current capacity unresolved";
}
```

Width contracts reference named tracks and lower to existing asset track-row
indices. Purposes are `pin_entry`, `control_supply`, `output_sense`, `enable`, or
`main_power`. Evidence is mandatory, minimum width cannot exceed authored width,
and a track has at most one contract. Existing scoped width-check behavior is
preserved. Dedicated contacts name an exact via position and a pad included in
their plane return. Their physical validity is checked by the existing binder.

## Imports, binding and generated assets

Macro footprint imports name exact `.kicad_mod` assets, including managed URLs
and module-confined relative paths. A member can use an import alias or a literal
footprint reference supported by the existing resolver. Footprint geometry
digests are generated automatically; author no SHA fields.

```copper
import layouts "github.com/vendor/library/packages/power";
physical_macro UNIT: layouts.LinkLayout {
    member U = U1;
    net SIGNAL = SIGNAL;
}
```

Packages can export macros alongside parts/modules. Modules may select sibling
definitions by unqualified name; repeated instances retain independent rigid
owners. Electrical compilation retains only the selection descriptor and leaves
footprint geometry preparation pending.

```sh
copper compile-macro packages/power/layout.copper -o build/layout.json
copper compile-macro packages/power/layout.copper -o build/layout.json --locked --offline
```

The unlocked command may prepare dependency locks; locked/offline operation
verifies existing inventory without fetching or rewriting it. Output is written
only after successful lowering and cannot overwrite source/footprint inputs. Boards consume
the Copper definition directly, so generating JSON is optional.

Both formats lower to the v0.5 asset contract and existing immutable binder.
Source and footprint bytes are rechecked before binding, tracked by editor/build
input snapshots, and compared when replaying requirements. Changed inputs
invalidate old evidence even when generated copper is unchanged. JSON imports
and legacy scenes retain their existing behavior.

Definitions cannot introduce electrical circuitry, parameter expressions,
implicit mirroring, arcs, autorouting, or qualification claims. Unsupported
properties are errors. Assets remain experimental with
`production_publishable = false`. Schema validation does not prove host pad/net
mapping, native DRC/fill, switched-power/RF behavior, assembly or manufacturing
qualification. See [requirements](macro-definition-plan.md) and the
[physical lifecycle](physical-hard-macros.md).
