# Bounded local critical-route improvement

The critical stage compares small single-ended surface guides with local
alternatives before locking copper. This is the small-net application of routing
review R5, not a replacement global/detailed router or complete Steiner solver.

## Candidate contract

`pcbir.local_critical.local_surface_candidates` resolves actual physical lands,
including separate lands with the same pad number. It accepts two to eight lands
on a common allowed copper layer; missing, non-copper or mixed-side SMD access
fails without inventing a connection. Clock/RF feed profiles remain logical
point-to-point with exactly two physical lands; multi-terminal critical trees
require explicit tree intent. Repeated lands cannot bypass the RF/clock limit.

For each eligible layer, at most 28 terminal pairs are considered. The shared
surface-path routine tries bounded straight/45-degree-first one-corner paths,
then orthogonal alternatives. Arbitrary-angle fallback paths are excluded from
this candidate graph. Paths must clear actual foreign pads, keepouts, earlier
critical reservations and board edges. A deterministic length-weighted minimum
spanning tree must cover every terminal; a disconnected graph yields nothing.
Intermediate Steiner vertices, maze detours, plane fills, new vias and component
movement are not synthesized here.

`_improve_single_surface` owns comparison with the guide incumbent. Global
transition/access-via guides retain their existing owner, as do differential/CAN
pairs and power/general routing. Local candidates use the original width,
length/via budgets, allowed layers and external qualification assumptions.
Fresh complete-candidate native DRC still checks every land and previous critical
group. A connected incumbent is changed only for strictly shorter legal copper;
an invalid guide may be repaired by a legal local candidate. Failed proposals
preserve the incumbent, and failed local repair still permits the existing
bounded single-net maze fallback. No accepted copper is dragged or deleted.

Critical reports and fingerprints include `local_candidate_attempts` and
`guide_length_nm`. The latter measures the original proposed guide copper,
which can be invalid/uncommitted; it is not an accepted-route length. Attempts
count complete local proposals, including proposals not cheaper than the
incumbent. Selected geometry is marked `local_surface_tree`; metrics are
recomputed from emitted tracks, never inferred from close component placement.

## Progress checkpoints

`route_critical_nets(..., on_progress=callback)` emits `started`/`finished`
notifications for each single net or coupled pair. Notifications are
observational: clocks and progress records are excluded from geometry and route
fingerprints. `pcbir.critical_preflight` saves the running group before its search
and records completed results and elapsed seconds afterward. An interruption
retains `complete: false`; a finished group is not proof of complete board routing.
The scene byte identity is also retained when placement templates are selected.

## Verification and remaining work

Regressions cover legal-detour improvement, full branch/duplicate-land coverage,
keepout and reserved-copper fallback, original budget rejection, equal-length
incumbent preservation, mixed-side and over-limit rejection, malformed shorter
proposals rejected by native connectivity, deterministic routing and observational
progress. The full suite passes 386 tests with 459 upstream CAM-library warnings.

Full-vertical RF/USB results are recorded in [pass 8](routing-review-pass8.md).
General-net wider/projected guides, larger-tree/Steiner improvement and branch-safe
cleanup remain R5/R7 work. Reference matching-ground copper/support, antenna
keepouts, actual stackup/return-path qualification and a complete ordinary-net
reroute remain separate requirements. Short connected copper alone is not RF
performance or production signoff.
