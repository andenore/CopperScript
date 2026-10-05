# Package-access allocation review — 2026-10-05

This is an ordinary-package-escape experiment, not a completed routed board or
filled-plane/manufacturing signoff.

## Reproduced failure

The current locked `examples/full_vertical/board.copper`, six-layer JLCPCB profile
and pinned placement template produce a legal `candidate-01` placement. The
initial ordinary allocator escaped 75 of 76 eligible terminals. `PWR/U_MODEM.13`
had 684 legal alternatives, but its four-pin group (`.11` through `.14`) reached
the 200,000 compatibility-check budget before attempting any assignment.
This was search-budget starvation, not a zero-domain or clearance failure.

The old counter charged conservative disjoint bounding-box comparisons against
distant packages to the same budget as exact copper/drill checks. Domain
expansion and outside-reservation filtering consumed that budget before MRV
search started.

## Change and result

Separated bounds now prove compatibility without spending the exact-query budget
or occupying the exact-pair cache. Exact overlapping pairs retain every existing
copper, layer, via-span and drill predicate. A separate 2,000,000 total-request
budget counts broad-phase and cached requests too; exhaustion still rolls back
atomically. The exact-check limit remains 200,000.

Replaying the same source and verified poses gives:

- **76/76** ordinary exits; no pending ordinary pad.
- **10,726** exact compatibility checks out of **315,565** total requests;
  **303,131** requests were conservative broad-phase acceptances.
- The power-stage group finds a complete compatible assignment in **four**
  attempted states, and final native fanout acceptance passes.
- The placement fingerprint is unchanged:
  `5d1b87bb622bd415def17b3c54bd23db266dee9b4a9a08d8c74632bc9551163e`.

No placement, footprint, width, clearance, via dimensions, fabrication profile
or sampling settings were relaxed. Both ordinary experiments enabled the bounded
multi-bend fallback. The revised allocation also keeps every previous escaped
identity. These instrumented, concurrent runs are not a wall-time speed benchmark.

Local ignored evidence:

- `build/package-pattern-validation/ordinary-20261005/`: baseline report, verified
  placement, KiCad project, phase timings and cProfile data.
- `build/package-pattern-validation/ordinary-budget-fix-20261005/`: matched-placement
  replay, complete ordinary allocation and equivalent diagnostic/profiling files.

The longer `full-vertical-20261005-auto` preflight was started before this budget
fix and is baseline evidence only. It passed global planning, four RF groups and
the MCU USB pair; its modem USB/selected-ground evaluation was still running at
this checkpoint. Do not substitute it for a fresh full-board result under the
updated allocator.

## Next gate

Run the updated ordinary allocation together with unchanged critical profiles
and requested early ground contacts, applying the new bounded pattern negotiation
when necessary. Only a ready package preflight may start ordinary area routing.
Boundary-port/onward-capacity certification and general joint critical/GND domain
enumeration remain open in [the package-access checklist](package-access-first.md).

The shared Make default now selects from actual legal candidates rather than
forcing `candidate-00`, which this placement rejected. Explicit candidate requests
remain supported and still fail if unavailable.
