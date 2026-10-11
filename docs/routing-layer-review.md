# Via policy and per-layer review

Ordinary vias may not touch even the edge of a same-net pad. Native KiCad DRC
normally accepts that electrical connection, so CopperScript also checks it as
`DRC-VIA-PAD-OVERLAP`. Explicit, qualified `filled-capped` via-in-pad remains a
separate opt-in process; the full-board script does not opt in. This is not a
blanket same-net clearance rule: tracks must still join their pads and vias.

Cleanup of ordinary and local surface-closure paths trims only degree-two
perpendicular octilinear corners, trying decreasing
chamfer sizes. Every new diagonal passes exact copper/keepout/board-edge tests.
Contacts in the cut-away area block cleanup; obstacle-constrained corners are
retained. It neither independently alters paired USB geometry nor edits a saved
KiCad board by hand.

Run the audit with KiCad's bundled Python after a routing run (PowerShell):

```powershell
& 'C:\Program Files\KiCad\10.0\bin\python.exe' scripts/review_routing_layers.py `
  build/full-vertical/<run>/board.kicad_pcb `
  --output-dir build/full-vertical/<run>/layers `
  --kicad-cli 'C:\Program Files\KiCad\10.0\bin\kicad-cli.exe'
```

Outputs include one saved-copper SVG per enabled copper layer and
`layer-review.json`: exact native via/pad collision locations, track lengths,
degree-two sharp turns (90 degrees or greater), branches, non-octilinear segments
and coarse 10mm local track-area estimates. Plane fill is not recomputed by the
audit. Refill/DRC separately before interpreting plane connectivity. These style
and density metrics are not electrical, impedance or manufacturing signoff.
