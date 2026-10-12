# Board silkscreen

Boards support one `silkscreen` block, separate from `mechanical` and electrical
connectivity. Text and artwork export as native KiCad board drawings on
`F.SilkS` or `B.SilkS` by default, and therefore appear in those layers' Gerber
exports. They do not create components, BOM items or electrical nets.
Opt-in copper artwork also reserves placement and routing space.

Every source board includes the bundled CopperScript Cu logo, `CopperScript`
and the compiler package version by default, even without a silkscreen block.
The version identifies the compiler used to build the board, not the board's
revision. It is captured in the physical artwork, so later exporting that
physical board does not substitute a different installed compiler version.

Branding defaults to `layout = horizontal`. Use `layout = stacked` for a compact
mark with the name and version centred below the logo; its labels are one tenth
of the logo size, with strokes one fiftieth of that size. An 8 mm logo therefore
has 0.8 mm labels and 0.16 mm strokes. Position, rotation and side apply to the
whole mark in either layout.

Opt out explicitly:

```copper
board Sensor {
    silkscreen {
        branding { enabled = false; }
    }
}
```

Opting out removes only this default mark. Authored artwork and footprint
silkscreen remain. An empty `silkscreen {}` keeps the mark enabled.

The default logo is 5 mm square on the front, with 1 mm lettering alongside it.
A bounded search starts near the outline's upper-left bounding-box corner and
tries to fit a conservative mark envelope inside board material. It does not
reserve placement space or guarantee clearance from pads, holes, components or
other artwork. When the board is too small, the mark remains at the initial
position for review; move/resize it or opt out. Review the final native board's
silkscreen and DRC before manufacturing, especially for small or shaped boards.

Move, resize, rotate or change the side of the default mark:

```copper
silkscreen {
    branding {
        position = (8mm, 30mm); // centre of the logo; labels follow it
        size = 4mm;            // logo width and height; lettering scales with it
        rotation = 90;         // degrees in board coordinates
        side = back;
    }
    text REVISION {
        text = "Sensor board\nRev A";
        position = (25mm, 10mm);
        size = 1.2mm;
        thickness = 0.18mm;
        side = front;
    }
    line DIVIDER {
        start = (10mm, 15mm);
        end = (35mm, 15mm);
        thickness = 0.15mm;
    }
    rectangle FRAME {
        start = (10mm, 18mm);
        end = (35mm, 25mm);
    }
    circle TEST_MARK {
        center = (40mm, 10mm);
        radius = 1mm;
    }
    arc CURVE {
        start = (40mm, 15mm);
        mid = (41mm, 16mm);
        end = (42mm, 15mm);
    }
    polygon POINTER {
        vertices = [(40mm, 20mm), (42mm, 20mm), (41mm, 22mm)];
        filled = true;
        thickness = 0mm;
    }
    logo EXTRA_CU {
        position = (45mm, 30mm);
        size = 5mm;
    }
}
```

All features require unique ids. `__copperscript` ids are reserved for the
generated mark. All coordinates and dimensions are typed lengths and lower to
exact integer nanometres. Shape coordinates are absolute board coordinates on
both sides. Text is centred on its position. Back text is mirrored in the PCB
file so it reads correctly when viewing the back of the board; back logos also
mirror their geometry and relative labels. Positive rotation turns clockwise
in board coordinates (x right, y down).

| Declaration | Required properties | Optional properties |
| --- | --- | --- |
| `branding { ... }` | none | `enabled`, `position`, `size`, `rotation`, `side`, `layout` |
| `text ID { ... }` | `text`, `position` | `size`, `thickness`, `rotation`, `side` |
| `logo ID { ... }` | `position` | `size`, `rotation`, `side` |
| `line ID { ... }` | `start`, `end` | `thickness`, `side` |
| `rectangle ID { ... }` | `start`, `end` | `thickness`, `filled`, `side` |
| `circle ID { ... }` | `center`, `radius` | `thickness`, `filled`, `side` |
| `arc ID { ... }` | `start`, `mid`, `end` | `thickness`, `side` |
| `polygon ID { ... }` | `vertices` | `thickness`, `filled`, `side` |

`side` defaults to `front`; `rotation` to 0; text `size` to 1 mm;
`thickness` to 0.15 mm; and `filled` to `false`. Zero thickness is allowed only
for filled shapes. A `logo` uses the bundled Cu mark without the name/version
labels. External images and arbitrary SVG imports are not supported.

The complete design JSON includes a separate `silkscreen` section; its
electrical projection remains geometry-free. Physical board digests include
the generated vector/text artwork. Changing its content, position, side or
compiler version invalidates previous physical evidence. Use the complete
`Design` API when preserving source artwork through physical lowering;
`compile_source` intentionally returns only electrical intent.

## Opt-in copper artwork

Every artwork declaration, including `branding`, accepts `material = copper`.
It uses `F.Cu` on the front and `B.Cu` on the back. The default remains
`material = silkscreen`; each declaration chooses independently.

Put the default logo, name and compiler version in copper:

```copper
silkscreen {
    branding {
        material = copper;
        position = (15mm, 15mm);
        size = 10mm;
        exposed = true;
    }
}
```

Or keep the default branding in silkscreen and add a copper logo or label:

```copper
silkscreen {
    logo COPPER_CU {
        position = (20mm, 20mm);
        size = 10mm;
        material = copper;
        exposed = true;
        side = back;
    }
    text COPPER_REVISION {
        text = "Rev A";
        position = (40mm, 20mm);
        size = 2mm;
        thickness = 0.25mm;
        material = copper;
    }
}
```

`exposed` defaults to `false`: copper remains covered by solder mask. Setting
`exposed = true` adds a matching drawing on `F.Mask`/`B.Mask` so the copper
surface is visible. It adds no solder paste. There is currently no mask
expansion setting; the opening matches the artwork. `exposed` is valid only
for copper artwork. Internal layers and assigning an electrical net to artwork
are not supported.

Copper artwork is floating decorative copper, exported as locked native board
drawings. Conservative bounding envelopes, including stroke width and the
largest declared board/net/zone clearance, become immutable copper keepouts
for ordinary placement and routing. Pads, tracks, vias and zone fills must stay
out; filled zones receive native rule areas. Reservations are regenerated when
the artwork or rules change. No fixed component or immutable macro is moved to
make room. Circles/arcs reserve their entire circles and text uses generous
boxes, so these reservations can be larger than the visible mark. Default
copper branding searches for space using these envelopes; final review is
still required. Board edge, hole, fixed-copper and filled-zone conflicts remain
errors. Artwork strokes must satisfy the board's minimum copper width.

Copper text, minimum feature widths of filled artwork, and exposed-mask
registration/sliver checks currently remain **mandatory unsupported checks** in
physical signoff. Native KiCad loading, copper/mask Gerber export and routing
reservations are supported, but these cases are for layout inspection and keep
internal fabrication signoff incomplete. Existing native checkpoints that do
not normalize decorative copper also report it as unsupported. Native DRC and
final CAM/process verification must not be replaced by passing envelope checks.
Changing material or exposure invalidates previous physical evidence.

Internal process DRC checks board-owned lines against mask openings on their
side when the fabrication profile supplies a silkscreen-clearance capability.
Text and complex board graphics report `FAB-SILK-ARTWORK-UNSUPPORTED` and keep
that fabrication gate incomplete. Native DRC and final artwork inspection
remain necessary; export alone is not manufacturing qualification. The
mechanical editor currently does not render or edit board silkscreen.
