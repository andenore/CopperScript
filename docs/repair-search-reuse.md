# Exact repair-search reuse — 2026-10-06

This is a detailed-repair work reduction, not a routing-quality change or
fabrication signoff. Every reused answer is the attempt an identical earlier
search produced; selected copper, reports and fingerprints stay unchanged.

## Question

The [full-vertical review](routing-shared-reference-review.md) recorded 77
displaced-net searches (`evicted_net`) taking 842.491 s of a 1,807.862 s run,
with `V5` alone at 20 attempts and 19 failures. Its open item asked whether
these are duplicate states, and to bound any memoization to exact obstacles,
rules, guides, anchors and search options, never reusing a failure after the
blocking geometry changes (CS-087).

## Method

Each `_search_detailed_net` call now has an exact identity:

- the immutable board object (keepouts, zones, placements, footprints, macros,
  locked fanout/critical copper, net rules and board rules) and any resumed
  branch checkpoint, both **by identity**, held so their ids cannot be reused;
- the clearance index as its board, bin size and **ordered** insertions with
  lock flags (`RoutingClearanceIndex.additions()`); all other index state
  derives from the board;
- by value: the net and its terminals, its rule, global guide/corridor, all
  owned fanout/boundary anchors, congestion usage and history, the repair and
  movable-conflict modes, and every `DetailedRouterOptions` field (pitch,
  budgets, costs, heuristic weight, policy switches).

With `--progress`, every `detailed_net` finished event carries a process-local
`search_identity` hash of that key (equal-content boards share a token), so
repeats can be counted in any stage. The search is a pure function of these
inputs: it does not mutate the index, and its grid caches are geometry-only.

Fast iteration used `nrf52` (the smallest example that reaches detailed
repair; `round-led-ring` stops at package access) and synthetic corridor
boards in `tests/test_repair_search_reuse.py`. Full-vertical was run once at the
end, against the unprofiled merged-base baseline on the same machine.

## Measurement

Full-vertical on one Linux machine, CPython 3.12.3, `PROFILE=none`, unchanged
settings. Baseline: the merged base (`build/fv-baseline-noprof/`). Candidate:
this change from a separate worktree (`build/fv-reuse/`). Both are ignored
local evidence; both pass with zero native KiCad violations or opens.

| Detailed work | Baseline | With reuse |
| --- | ---: | ---: |
| All attempts (reused included) | 201 | 201, 16 reused |
| Displaced (`evicted_net`) | 77, 559.3 s | 77, 12 reused, 432.1 s |
| `soft_ripup` | 10, 46.1 s | 10, 4 reused, 22.7 s |
| `final_retry` | 6, 143.2 s | 6, 98.4 s |
| Passes (not memoized) | 98, 233.2 s | 98, 299.6 s |
| `detailed_repair` phase | 783.5 s | 599.5 s |
| Runner wall time | 1,091.5 s | 980.9 s |

The candidate's identity telemetry covers all 201 attempts. Exactly 16 repeat
an earlier identity, all in repair: **12 of 77 displaced searches** and four
`soft_ripup` proposals equal to their `soft_merge` search. When first computed
these searches cost **112.7 s** (91.2 s displaced, 21.6 s soft). No pass or
final retry repeated and the memo evicted nothing (87 of 128 entries).

| Repeated displaced net | Repeats | First-computed cost |
| --- | ---: | ---: |
| `V5` (all failures) | 3 | 50.1 s |
| `USB_C_CURRENT_1` | 2 | 21.1 s |
| `PWR/MODEM_FB` | 2 | 12.1 s |
| `USER_LED_2_A` (failure) | 1 | 6.0 s |
| `MCU_SWDCLK`, `MCU_MODEM_TX`, `USER_LED_1`, `MCU_MODEM_RX` | 1 each | 1.9 s |

Repeated soft proposals were `GNSS_RX`, `GNSS_TIMEPULSE`, `GNSS_TX` (about 7 s
each) and `MCU_NRF_RX`. So duplicates are a minority: 15 of `V5`'s 18 displaced
failures are distinct trial boards.

On `nrf52`, the only repeat is the `XTAL_1` soft proposal (4.7 s); its one
displaced search does not repeat. Its ordinary area routing took 121.4 s
before both changes and 71.1 s after, mostly from cheaper 0.5 mm pass grids
(below); those two runs also saw different machine load.

The pass stage, whose search is unchanged apart from faster grid blocking, took
28% longer in the candidate. The machine carried more concurrent routing load,
so wall and phase times are observations, not a matched speedup. The skipped
search seconds and the grid timings below are the load-independent evidence.
`routing_benchmark compare` finds no quality regression; it withholds a timing
ratio because the checkout paths change the recorded inputs and arguments.

Structure explains the repeats. Every rip-up order rebuilds the same trial
copper, so the first displaced net of two orders is searched twice against an
identical index: with three blockers, three of the six permutations repeat;
with four, the reversed order and the third rotation share a first net. A soft
proposal is also searched twice when no merge changes the board between the
`soft_merge` and `soft_ripup` stages. Repeats deeper in an order are not exact:
the earlier reroutes differ.

## What remains in the displaced searches

Most displaced time is not repeated. `V5` fails by budget exhaustion
("octilinear and fallback search budgets exhausted for U_MODEM.61") against a
different trial board in each order. That cannot be pruned soundly: extra
copper can shrink the explored space so that a budget-limited search that
failed before succeeds, and finite access-candidate sets change with nearby
copper. Even an exhaustive no-path result is not monotone for the same reason.
These failures are left as real searches.

The second driver was grid construction. A displaced net that cannot reach a
terminal refines 1 → 0.5 → 0.25 → 0.125 → 0.1 mm and rebuilt every mesh per
attempt. On the full-vertical outline (100 × 80 mm, six layers, template
placement, concurrent load) the blocked-node scan measured:

| Pitch (mm) | Before (s) | After (s) |
| ---: | ---: | ---: |
| 1 | 0.147 | 0.033 |
| 0.5 | 0.525 | 0.121 |
| 0.25 | 2.225 | 0.463 |
| 0.125 | 8.429 | 1.669 |
| 0.1 | 12.840 | 2.440 |

Material is layer-independent, so each point is now tested once instead of
once per copper layer, and a keepout polygon only inside its inclusive bounding
box (exact: a point outside it is neither on nor inside the polygon). Within a
repair stage, the same board and axes also reuse their blocked-node set from a
16-entry table (63 hits in the candidate); each grid keeps fresh query caches.
In the baseline, `MCU_NRF_TX` failed three displaced searches through this full
chain, an estimated 24 s of meshing each: its four displaced attempts took 93.2 s,
and 24.7 s in the more loaded candidate run.

## Change

`_repair_from_passes` gets one bounded memo per routing run's repair stage
(`--search-reuse-entries`, default 128, LRU; `0` disables). All four repair
stages (`soft_merge`, `soft_ripup`, `evicted_net`, `final_retry`) consult it.
A failure is reused only for an identical key, so any change in blocking copper,
keepouts, rules, guides, anchors, congestion or options is a new search. Pass
searches are not memoized; none repeated on either board, and existing tests
pin their search counts. The single material test per point applies to every
grid build, passes included.

Telemetry only: finished events add `search_reuse` (`hit`/`miss`) and, for a
hit, `reused_search_seconds`; a `detailed_search_reuse` summary follows
`detailed_repair`; `routing_benchmark summarize` reports
`reused_detailed_attempts` and treats the bound like the other explicit
interventions. None of this enters the IR, reports or fingerprints.

## Determinism evidence

Full-vertical, baseline versus candidate:

- The 472 progress events are identical (every stage, net, owner, pitch,
  connectivity, track/via count and diagnostic; elapsed time and reuse
  telemetry and the added reuse summary excluded) except the report path in
  the final export event.
- Global and critical fingerprints, the export digest `077dae49…`, every
  report net/metric and the native KiCad result are identical. The saved
  filled boards differ only in 102 footprint-property UUIDs that KiCad assigns
  on save.
- The detailed fingerprint, board/DRC/filled-board digests and the package
  boundary `source_digest` differ. Each includes `physical_board_digest`, which
  hashes footprint `source_path` metadata: an absolute path into the
  checkout's package cache. The boundary digest differs before detailed
  routing starts, so this is checkout provenance, not routing.

`nrf52` gives an identical 110-event stream for the merged-base grid code with
reuse disabled and for this change;
its build stops at export with a pre-existing `filled-capped via-in-pad`
profile error, after routing, in both runs.

Tests in `tests/test_repair_search_reuse.py` cover: a key change for every
obstacle, board, rule, guide, anchor, congestion, mode, checkpoint and option
field; ordered insertions and lock flags; identical object returned on a hit
without searching; a failure that is searched again after its blocker moves or
disappears; a real displaced-net repair with three exact repeats (13 → 10
searches) and an identical result/event sequence; byte-identical
`route_detailed` JSON and fingerprints with and without reuse; the LRU bound
and disable switch; the blocked-node oracle on keepouts, concave polygons,
holes and cutouts; and benchmark telemetry.

## Limitations

- Scope is one repair stage of one `route_detailed` call. Placement trials,
  zone repairs and the neutral fallback route new board objects or options
  and get their own memo; equal-content boards in different objects are not
  shared (conservative).
- Insertions are keyed in order. A permutation of the same copper is not
  reused, although clearance answers are order-independent.
- Wall times are observations on a shared 14-core machine with other routing
  jobs running; they are not a matched benchmark or a speedup claim.
- This does not reduce budget-exhausted searches, the bulk of displaced time.
