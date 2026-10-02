# Hard-macro routing validation and remaining work

The pipeline consumes explicit digest-bound macro scenes via `--hard-macro`.
The `pcbir.nrf52_example --route` command uses that same implementation, not an
alternate example router. See [the example commands](nrf52-coin-cell.md).

Completed implementation:

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

The first complete example attempt is saved in ignored
`build/nrf52-macro-route-validation/`: ERC passes, owner copper remains intact,
but global/package gates fail before ordinary area search. SWDIO (U_NRF.26) and
DEC3 (U_NRF.33) have empty escape domains; BT1.2 has no prospective plane contact.
Native KiCad confirms 31 unconnected items and 13 dangling fanout vias in that
partial draft. These are not geometry shorts or evidence of successful routing.
The profiling/report outputs retain the failed attempt.

Remaining example/asset work, without waiving clearance or private returns:

1. Adapt the RF reservation and adjacent decoupling placement to expose ordinary
   MCU terminals legally; alternatively extend the reviewed macro with explicit
   owner lead-ins/ports for affected terminals. Do not allow arbitrary same-net
   shortcuts or pretend a source-backed RF-only asset already covers the MCU.
2. Add a legal off-pad contact for the wide battery-holder ground land; consider
   a bounded geometry-aware search radius rather than via-in-pad by default.
3. Rerun all stages, refill, inspect every layer, and require zero native opens
   and violations before claiming a fully routed example.
4. Qualify reference planes, RF feed/antenna matching, actual parts, laminate,
   power/crystal layout and assembly before any manufacturing publication.

GitHub inspection builds package incomplete results with explicit status;
neither a tag nor an uploaded artifact bypasses these gates.

Validation on 2026-10-03: the full CopperScript suite passed (721 tests, one
optional skip); 80 focused routing/library/CI tests also passed. CopperLib's
34 tests and deterministic compatibility reports passed before publication.
A fresh isolated URL-only example downloaded all required data, materialized
27 components/20 owner tracks/two vias, and left its lock byte-for-byte unchanged.
