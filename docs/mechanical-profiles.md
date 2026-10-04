# Importable mechanical board profiles

Status: implemented initial vertical, unreleased v0.1. See the
[implementation checklist](mechanical-profiles-plan.md) and CS-149.

Profiles describe reusable physical contracts, not electrical modules or textual
inclusion. A board owns its components/nets and explicitly binds components to
profile connector roles. Electrical `Board` stays free of mechanical geometry;
compiled `Design.mechanical` owns the expanded geometry, role bindings and source
provenance. A profile never creates a component, pin, net or copper connection.

## Files and use

`mechanics/carrier.copper` exports a package member:

```copper
board_profile Carrier {
    outline rectangle { width = 50mm; height = 35mm; }
    hole H1 { position = (4mm, 4mm); diameter = 2.5mm; }
    keepout host polygon {
        vertices = [(20mm, 20mm), (40mm, 20mm), (40mm, 30mm), (20mm, 30mm)];
        side = back;
    }
    connector debug {
        anchor_pad = "1";
        position = (10mm, 8mm);
        rotation = 90;
        side = front;
    }
}
```

The board imports and applies it:

```copper
import carrier "./mechanics";
// J_DEBUG is an ordinary component declared by this board.
mechanical {
    use carrier.Carrier as host { debug = J_DEBUG; }
    hole LOCAL { position = (45mm, 4mm); diameter = 2mm; }
}
```

Imports select directories, not individual files; each `.copper` member exports
one part, device, electrical module or `board_profile`. URL-like package imports
use existing `copper.mod` requirements, hashes and `copper.lock`; downloaded code
is never executed. Profiles authored for reuse belong in CopperLib, not `pcbir`.
The compiler repository contains only deliberately generic examples/tests.

Relative imports (`./mechanics`, `../common`) are relative to the importing
source file. They remain inside its source module (the nearest manifest root;
without a manifest, the entry source directory). They cannot escape through
`..` or symlinks. Workspace source is mutable like the entry board, with content
digests in compiled dependency provenance, and does not need a generated lock
entry. `--locked --offline` still permits local source edits; remote dependencies
remain authenticated by their module inventories. Relative imports inside a
downloaded/replaced module stay inside that dependency, retaining its digest.

## Composition and ownership

- `use package.Profile { ... }` defaults the instance name to `Profile`;
  `as name` chooses it explicitly. Imported feature IDs become `name/H1` etc.
- Profiles may import/use other profiles. Nested connector bindings map child
  roles to exported parent roles, e.g. `use base.Header { debug = host_debug; }`.
  Instances and feature source ownership remain recorded, rather than losing
  the composition tree during expansion. Cycles and duplicate instances/roles
  fail explicitly. Profiles may be fragments without an outline.
- The final board must have exactly one outline. Local additions may add holes,
  cutouts, placement keepouts and copper keepouts, not replace an imported outline.
  Rule blocks may supply disjoint fields; duplicate rule ownership is an error,
  even when values match. No declaration-order override exists.
- Every connector role must be bound exactly once; unknown/missing roles or
  components fail. Hierarchical components can be bound by quoted physical path,
  e.g. `debug = "HOST/J1"`. Connectivity still belongs in ordinary module ports/nets.
- Imported features/roles are read-only in the editor, with source locations.
  Future source persistence must edit project-owned additions, never dependencies.

## Connector anchor contract

`anchor_pad`, typed `position`, numeric `rotation` and `side` are required.
Position refers to the centre of one uniquely identified electrical physical
land, not the footprint origin. Physicalization uses the shared rotation/mirror
transform to calculate an origin, then creates a full fixed-pose placement rule.
Repeated pad numbers with multiple physical lands are ambiguous and rejected.
An omitted footprint, missing anchor pad or conflicting fixed pose/side/explicit
allowed orientation fails instead of dropping the role. An optional `footprint`
requires an exact selected library identifier; use it when connector geometry
must not change. Real footprint resolution remains necessary for fabrication;
proxy results are inspection-only, never proof of mating compatibility.

## Keepouts and integration

`keepout NAME rectangle|polygon` accepts shape dimensions/vertices, optional
`side = front|back` and typed `maximum_height`. Omitted side applies to both.
`copper_keepout NAME rectangle|polygon` requires comma-separated `layers`, e.g.
`"F.Cu,B.Cu"`, with optional boolean `block_tracks`, `block_vias`, `block_pads`,
`block_zones`, `block_footprints`. Defaults match physical IR. Stackup mismatches
fail. These feed the existing placement/router/DRC/KiCad paths, not a second
floorplan. Compile JSON and editor scenes retain ownership and role information.
The editor displays board-level placement and copper keepouts with source-owner
tooltips. Copper keepouts show their layer scopes in tooltips, not a routed-copper
overlay; footprint-local copper keepouts are not displayed by this viewer yet.

## Limits and next increments

Profiles share the board coordinate frame; no use-site translation, scaling or
parameter substitution yet. Rounded outlines/arcs, connector mating-face/axis
constraints, required assembly height and exact 3D host interference need their
own supported geometry/contracts. Unsupported declarations fail, not approximate.
No Raspberry Pi standard is encoded in the compiler or certified by this feature.
A real HAT/HAT+ profile must use official revision-specific geometry/requirements;
mechanical fit alone does not establish electrical/HAT compliance. Production
profiles should be reviewed and published in CopperLib separately.
