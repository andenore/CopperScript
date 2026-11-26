# Hard-macro routing validation and remaining work

The pipeline consumes explicit digest-bound macro scenes via `--hard-macro`.
The `examples.nrf52_coin_cell.nrf52_example --route` command uses that same implementation, not an
alternate example router. See [the example commands](nrf52-coin-cell.md).

Routing contract:

- Immutable owner copper participates in global capacity and exact access.
- Pad-backed, free-track and plated-via boundary ports route external nets.
- Fully connected owner nets are reused, not routed in parallel.
- Package escape skips bound private pads; unrelated pins retain normal checks.
- Single-ended critical budgets remain enforced; unsupported paired macros fail.
- Whole-unit placement feedback rebuilds copper from an unrouted source.
- Plane contacts reuse actual private copper/vias without shortcutting returns.
- KiCad export retains locks, keepouts and self-contained footprint libraries.
- URL library parts, footprints and physical scenes share the revision/cache/lock.

Regression tests cover complete synthetic external-port routing (free and via),
rigid rotations including 45 degrees, immutable copper, exact owner continuity,
private-region exclusion, existing-via plane contacts, rejected copper loss and
native KiCad macro acceptance. The installed nRF52 example is also exercised.

Host-zone ownership is checked against the declared zone outline before fill;
zone-blocking keepouts do not clip that ownership outline. A conflict reports
the host zone, net, layer, macro cluster and anchor, protected region and its
transformed bounds, plus the rejected sharing condition. Both initial
materialization and immutable-copper validation use the same predicate and
diagnostic. Inner-only planes do not claim a private surface region; explicit
same-net sharing still requires a matching macro zone and no blocking region
or intersecting zone keepout.



Current board results and pending native closure are in [the nRF52 example](nrf52-coin-cell.md).
