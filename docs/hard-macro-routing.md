# Hard-macro routing validation and remaining work

The pipeline consumes explicit digest-bound macro scenes via `--hard-macro`.
The `examples.nrf52_coin_cell.nrf52_example --route` command uses that same implementation, not an
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

Initial example/asset work, without waiving clearance or private returns
(see the access-repair checklist below for updated status):

1. Adapt the RF reservation and adjacent decoupling placement to expose ordinary
   MCU terminals legally; alternatively extend the reviewed macro with explicit
   owner lead-ins/ports for affected terminals. Do not allow arbitrary same-net
   shortcuts or pretend a source-backed RF-only asset already covers the MCU.
2. The battery ground contact now explicitly permits filled/capped via-in-pad
   through `constraint via_in_pad(BT1.NEG)`. Other pads retain strict defaults.
3. Rerun all stages, refill, inspect every layer, and require zero native opens
   and violations before claiming a fully routed example.
4. Qualify reference planes, RF feed/antenna matching, actual parts, laminate,
   power/crystal layout and assembly before any manufacturing publication.

GitHub inspection builds package incomplete results with explicit status;
neither a tag nor an uploaded artifact bypasses these gates.

## Access repair (2026-10-03)

- [x] Add typed pad-scoped via-in-pad permission and apply it only to BT1.NEG.
- [x] Refine empty/conflicting escape domains from 0.5 mm to a bounded 0.1 mm
  pass; retain exact geometry gates and expose both step sizes in the CLI.
- [x] Narrow the Nordic top-layer router ownership envelope's upper edge from
  -1.0 mm to -0.6 mm in local macro coordinates, opening DEC3 surface access.
  RF tracks/vias, pad roles, fill/via exclusions and inner keepouts are unchanged.
- [x] Publish the deterministic CopperLib asset and update the pinned URL
  revision, inventory and explicit asset digest; no sibling checkout required.
- [x] Verify real-board global success, no pending signal/ground escapes, and
  one filled/capped contact at BT1.2 with the broad CLI opt-in disabled.
- [x] Attempt the fresh profiled route and independently refill/DRC/audit its
  saved layers in `build/nrf52-access-fixed/` (ignored).
- [ ] Close the two remaining native opens: BUTTON1 at SW_1 / R_BUTTON1,
  and VBAT between the R_BUTTON1 branch and battery positive contact.
- [ ] Recheck complete routing, filled planes and all layers after that repair.

The rerun allocated 15 ordinary exits with no pending signal/plane pads and no
hard preflight findings. Native KiCad 10.0.6 reports zero geometry violations
and two unconnected items (BUTTON1 and VBAT), down from 31 opens/13 dangling
via violations in the original blocked draft. DEC3 and SWDIO are connected;
the only via/pad overlap in the saved six-layer audit is the explicitly
qualified BT1.2 contact at (15,25) mm. RF copper remains locked. The profiled
run took about 410 seconds; native validation does not certify RF performance
or manufacturing readiness. The internal unfilled-copper report also lists
GND as open pending fill evidence; the native opens are not on GND.

The original saved failure remains an historical draft. Resolving package
access is not evidence of complete area routing or RF/manufacturing signoff.

Validation: CopperScript 734 tests passed, one optional skip; final URL-only
nRF52/pad-permission tests 17 passed. CopperLib 35 tests and deterministic
compatibility reports passed before publishing revision `853d32a`. Source,
macro, lock and fabrication-process checks remain strict.

Validation on 2026-10-03: the full CopperScript suite passed (721 tests, one
optional skip); 80 focused routing/library/CI tests also passed. CopperLib's
34 tests and deterministic compatibility reports passed before publication.
A fresh isolated URL-only example downloaded all required data, materialized
27 components/20 owner tracks/two vias, and left its lock byte-for-byte unchanged.
