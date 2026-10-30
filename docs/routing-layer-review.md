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

The per-layer audit script (`scripts/review_routing_layers`, run with KiCad's
bundled Python) was removed with the Python implementation. For per-layer
views of a routing run, use `copper view RUN --window ... --filled` (or
`--net`, `--component`, `--issue`); it renders one window per copper layer.
The removed audit also reported via/pad collision locations, track lengths,
sharp turns, branches, non-octilinear segments and local track-area estimates;
`DRC-VIA-PAD-OVERLAP` still reports the collisions. Refill/DRC separately
before interpreting plane connectivity. These style
and density metrics are not electrical, impedance or manufacturing signoff.
