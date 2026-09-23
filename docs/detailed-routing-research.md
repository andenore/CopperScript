# Detailed routing: research and implementation decision

The global route is a capacity estimate and a set of guides, not copper. The
previous detailed router treated shared grid nodes/edges as a negotiable cost,
then wrote tracks without checking their actual width, via diameter, or nearby
pads. On the full-vertical example this produced hundreds of physical shorts.

## Methods examined

- [KiCad 10's interactive router](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html)
  separates walk-around and shove behavior. Both refuse to fix a route that
  violates the configured copper clearances; pads and locked items are not
  shoved. Its pad-connection optimization and collision highlighting are
  separate concerns from coarse route guidance.
- [TritonRoute](https://vlsicad.ucsd.edu/Publications/Journals/j133.pdf)
  prepares a region query and pin-access candidates, then uses guided maze
  search and iterative search-and-repair with design-rule information. Its
  detailed router maintains a connected tree for each multi-terminal net.
- [FreeRouting's architecture](https://github.com/freerouting/freerouting/blob/master/docs/architecture.md)
  separates fanout, autorouting, and optimization. Its board search tree,
  geometry, rules, and DRC are shared headless services. Missing connections
  are routed through legal free space, with bounded passes.
- [PathFinder](https://janders.eecg.utoronto.ca/1387_2015/readings/pathfinder.pdf)
  explains negotiated use of scarce graph resources. It is useful for global
  congestion and for reroute priorities, but overlap of two physical PCB
  traces cannot be accepted merely because its cost is high.
- [JLCPCB's published capabilities](https://jlcpcb.com/capabilities/Capab)
  list a 0.09 mm minimum trace/space for four-layer 1 oz/0.5 oz copper.
  The previous generic 0.20 mm clearance is incompatible with some installed
  0.5 mm-pitch footprints whose adjacent pad gap is only 0.125 mm. The
  explicit `jlcpcb-four-layer` prototype profile therefore uses 0.09 mm
  minimum clearance and 0.20 mm ordinary track width. The minimum is an
  escape-rule floor, not a recommendation for all nets or fabrication proof.

## CopperScript decision

Use a deterministic *walk-around* first pass. A spatial hash is only a broad
phase; exact integer copper-shape predicates decide whether each candidate
track, via, and pad escape meets foreign-net clearance. Committed routes become
obstacles for later nets. If no legal route exists at the current placement,
grid pitch, and net order, leave that net unrouted and report a partial result.
Rebuild the pass from locked copper with a different deterministic net order
rather than retaining violating tracks. Explicit copper keepouts remain hard
obstacles; footprint courtyards do not stand in for copper geometry. Pads with
no assigned net remain physical copper obstacles and are included in the
independent spacing check.

On a multilayer board without a declared blind/buried/microvia technology,
both critical and general routing now materialize transitions as through-vias
spanning every copper layer. Search checks their full copper disk on every
spanned layer. A named technology is used only when its span and dimensional
limits match; no special via is inferred from a global guide.

For multi-terminal nets, preserve every grid-tree junction while materializing
the route. Straight-line cleanup is currently limited to two-terminal paths
and must pass the same copper-clearance query. The independent physical DRC
remains the final authority; detailed-router success alone cannot authorize
manufacturing export.

This is an incremental implementation, not a full shove router. Planned work
still includes multiple legal pin-access candidates, pad neckdown rules,
geometry-aware rip-up of selected nets, via technology/span validation for
each fabrication profile, zone-aware power routing, and KiCad DRC comparison.

## Full-board checkpoint

On the provisional full-vertical placement, one bounded pass on a 0.5 mm grid
with the explicit four-layer profile initially connected 41 of 58 nets. That
checkpoint preceded the keepout, through-via, and unconnected-pad fixes, so it
is not a result for the current router. A fresh one-pass run of the current
implementation on a 1 mm grid connected 32 of 58 nets. Native DRC found 26
open nets and `DRC-ROUTE-INCOMPLETE`, with zero shorts, clearance errors, or
copper-keepout violations. The remaining opens show that a legal global guide
is not enough: fine-pitch pin access, grid resolution, and net-order
interactions still need work. None of these results qualifies the board for
manufacturing.

The exported `.kicad_pcb` is an inspection artifact. KiCad's project-level
design rules are not yet emitted from the selected profile, so independent
KiCad DRC must use a reviewed matching project configuration before its
result can be compared with native DRC.
