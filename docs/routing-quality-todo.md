# Routing quality obligations

The 2026-10-11 visual-review follow-up has a checkable
[implementation plan](assettracker-review-plan.md). Existing package-escape
preferences do not establish that antenna, supply-width and area-topology
comments on the latest candidate have been resolved.

Open joint decoupler/ordinary-area topology improvements are in the
[routing backlog](routing-backlog.md). Current implemented behavior:

- Decoupling intent names the protected pin/pad; local supply feeds prefer direct
  same-side access. Do not infer roles from designators or forbid ground vias.
- Package escapes reject gratuitous same-layer interior X crossings while
  permitting endpoints, T junctions and intentional trunks. Straight outward
  access is preferred with exact diagonal/multi-leg fallback.
- Failed ordinary nets retain verified exits; successful nets release unused
  generated witnesses transactionally. Macros and required arrays remain protected.
- Local surface and via-search paths honor declared breakout regions and recheck
  emitted pieces; a narrow trunk outside its region is not a wider valid route.
- Ordinary rip-up requires exact mutable multisets and restoration of displaced
  nets. Redundant-copper deletion proves affected pad connectivity before/after;
  failed proof returns no candidate.
- Host/macro GND-zone overlap requires explicit permission; native filled copper
  proves contact. Foreign clearance, protected copper and RF returns stay binding.

Full routing/native verification is required for every changed input. Board
status belongs in its project; these rules do not certify electrical performance.
