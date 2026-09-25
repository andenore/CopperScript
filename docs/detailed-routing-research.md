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
The first two passes use opposite orders; later passes prioritize nets left
open by the best prior pass. Each pass is transactional, and only the best
complete set of non-overlapping net routes is materialized.

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

The next iteration adds connected pad centers to a nonuniform search grid and
offers several direction-diverse, clearance-checked accesses per pad. A
multi-source/multi-target A* search commits only the selected lead-ins. Track
cost and heuristic use physical distance rather than grid index counts, so
fine local spacing does not make a long detour appear artificially cheap.
Each A* invocation has an explicit state budget. Exhaustion is a distinct
per-net diagnostic, not evidence that no geometric route exists.
Search first tries the global guide corridor under a smaller budget, then
falls back to unrestricted walk-around with the configured budget. Candidate
edge legality and guide membership are memoized within each immutable search.

The next iteration uses a separate pad-aware grid for each net. This avoids a
Cartesian product of every pad's X and Y coordinates across the board while
retaining exact positions for the current net. Resource identifiers use physical
coordinates and layer names, not grid indexes. Passes 5 and later use a stable
hash ordering; repeating only the first two sorts cannot explore new orders
after the best result stagnates. Compatible routes from different passes can be
merged. If a route is blocked by one or two mutable routes, repair can
transactionally rip them up, install the waiting route, and reroute every
displaced net. A slower soft-conflict search can propose such swaps with
`--soft-ripup`; it is opt-in because it did not improve the current example.
`--heuristic-weight` allows bounded weighted-A* experiments without changing
the default search objective. Every committed edge still passes exact clearance
queries; neither soft proposals nor historical routing resources are signoff.

This is not a complete shove router. Planned work still includes pad neckdown
rules, more effective repair and placement coupling, via technology/span
validation for each fabrication profile, zone-aware power routing, and KiCad
DRC comparison.

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

With pad-centered access and a 20,000-state guide-first search, the same
board still routed 32 of 58 nets in one pass. All previously observed
"no legal pin access" diagnostics disappeared: 24 remaining nets exhausted
the search budget and two were unreachable under the chosen route order.
Native DRC again reported only the 26 opens and incomplete-route finding.
This narrows the next work to search efficiency, rip-up/order, and power-plane
strategy rather than loosening copper clearance.

With per-net grids and four deterministic passes, the current provisional
placement reaches 42 of 58 nets. The 16 opens and route-incomplete finding are
the only native DRC findings; there are no shorts, clearance errors, or copper
keepout violations. A one-pass 0.5 mm grid reached 37 nets despite a 20,000
state budget; one-pass weighted A* at 150% reached 39 nets, but four weighted
passes reached only 41. Cross-pass merging and one/two-net rip-up retained 42;
opt-in soft-conflict repair also retained 42. Eight diversified passes reached
43 of 58 with 15 opens and no other native DRC findings. The placement search
also exposes a distinct legal `candidate-01`: eight passes route 49 of 58 there,
with nine opens and no shorts or spacing violations. Its remaining failures
all exhaust the 10,000-state search budget, so a higher-budget run is the next
bounded check. A targeted isolated-net probe
routes 15 of the 16 remaining nets on the same placement without other general
traces. GND alone still exhausts the 10,000-state budget, confirming that
ordinary-net interaction is the main signal-routing problem while GND needs a
separate plane/stitching strategy. The board remains unfit for fabrication.

The full board's many-pad GND net should not be assumed connected by an
unfilled zone. [KiCad's PCB documentation](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html)
states that zone fill and pad-connection rules determine real copper, and the
[KiCad CLI](https://docs.kicad.org/10.0/en/cli/cli.pdf) exposes `--refill-zones`
for DRC. CopperScript currently does not lower a source-level plane intent or
count verified zone fill in native connectivity. Those are required before a
plane can close GND without misleading native signoff.

The exported `.kicad_pcb` is an inspection artifact. KiCad's project-level
design rules are not yet emitted from the selected profile, so independent
KiCad DRC must use a reviewed matching project configuration before its
result can be compared with native DRC.
