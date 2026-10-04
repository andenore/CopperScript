# Split-file mechanical profile example

This is a compiler/editor demonstration, not a complete circuit, Raspberry Pi
HAT, or production-ready board. It uses an existing CopperLib debug header.

```text
board.copper                 Components, nets and explicit profile bindings
mechanics/carrier.copper     Reusable outline, holes, keepouts and pad anchor
```

The board applies `carrier.Carrier as host`, binding `debug` to `J_DEBUG`.
Imported holes become `host/H1` and `host/H2`; the board's extra `LOCAL` hole
remains project-owned. The profile fixes header pad 1 at `(15mm,10mm)`, with
90-degree front-side orientation and an exact footprint requirement. It creates
no component or electrical connection. Profiles can be consumed by additional
boards, or published in a versioned library and imported by URL-like package path.

From the CopperScript repository root:

```powershell
uv run copper check examples/mechanical_profile_project/board.copper --locked
uv run copper edit-mechanical examples/mechanical_profile_project/board.copper --locked --footprint-root "C:/Program Files/KiCad/10.0/share/kicad/footprints"
uv run copper export-kicad-pcb examples/mechanical_profile_project/board.copper --locked --footprint-root "C:/Program Files/KiCad/10.0/share/kicad/footprints" -o build/mechanical-profile/demo.kicad_pcb
```

Linux: replace the footprint root with your installed path. Add `--offline` after
dependencies are cached. This example inherits the repository's `copper.mod` and
pin; copying it to a separate project requires that project's own CopperLib
requirement and lock, not a manual CopperLib checkout. Local mechanics remain
normal project source, without a separate locked dependency.

Source saving is not yet enabled in the editor. Imported connector poses cannot
be dragged; imported/local geometry has ownership tooltips. Layer-scoped copper
keepouts are physical obstacles, not proof of routing or assembly qualification.
