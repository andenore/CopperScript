# Exact octilinear detailed pin access

Detailed routing previously checked a direct chord between a pad center and
its chosen grid access node, then emitted that chord. Octilinear maze edges
did not make those terminal connections octilinear. The full-vertical RF
preflight exposed four genuinely oblique access segments, not rounding noise.

`detailed._access_path` now constructs the complete local connection during
both candidate selection and copper emission. It tries a direct axis/45-degree
segment where applicable, otherwise both diagonal/straight orders followed
by two orthogonal-corner alternatives. These are four bounded local proposals,
not an unbounded escape search. Orthogonal corners remain a last conventional
fallback; this is an axis/45-degree segment contract, not a ban on every
90-degree junction in the general router's budget fallback.

Every leg retains the actual endpoint, original width, net and layer and is
checked against board-edge erosion and exact foreign copper/keepouts. There
is no oblique fallback and no snapped terminal. If all local proposals fail,
the access node is unavailable and search must choose another eligible node
or report failure. Sector diversity, candidate limits, pad-side eligibility,
physical via spans and maze budgets retain their existing owners.

Tentative blocker-aware rip-up may cross removable tracks, using the same
blocker query as the router. Pads, locked reservations, keepouts and board
edges remain immutable. A tentative path is not accepted copper: existing
transactional reroute/native checks must still succeed. On emission the helper
recomputes the deterministic legal path against the unchanged clearance index;
missing legality fails the attempt rather than emitting a stale chord.

Actual emitted lengths and tracks update normal detailed/critical metrics.
The general detailed router and single-ended exact critical fallback share
this construction. Global guide access, fanout/via/zone closure helpers and
imported existing copper are not silently rewritten by it. Differential/CAN
members remain owned by the joint paired engine, never independent access repair.

Tests cover all directional octants, exact endpoint continuity, alternate
leg order, locked/removable copper, keepouts, board edges, no oblique fallback,
off-grid exact critical routing and shared-node physical connectivity. See
CS-124 and [the matched review](routing-review-pass12.md).
