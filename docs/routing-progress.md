# Routing progress and long runs

`route-board --progress` streams flushed `PROGRESS {json}` lines. The complete
`scripts/route_full_vertical.py` workflow enables it automatically and persists
these lines in its ignored run directory's `routing.log`. Direct CLI commands
remain quiet unless explicitly enabled.

Each event has `phase`, `event`, monotonic `elapsed_seconds` since routing
started, and a `details` object. Events cover placement/global routing, ordinary
package exits, critical groups, package-access decisions and placement trials,
ordinary area routing, physical-land closure/native DRC, ground contacts,
local ground repairs, incremental/full ground-feedback trials, final native checks,
independent KiCad refill/DRC, and export. Trial decisions expose failures rather
than reporting a candidate as the selected completed board.

Full ground trials rebuild routing from a clean placement and can each take
minutes. An accepted trial with fewer pending ground contacts can cause another
trial; the configured trial limit still applies. A start event without a finish
event means that phase is still active or execution stopped. It is not proof of
a hang, completion, successful connectivity or signoff.

These are operational event checkpoints, **not resumable geometry checkpoints**.
They do not enter physical IR, route fingerprints, artifact contents, candidate
ordering, search bounds, acceptance decisions, fabrication rules or signoff.
Only the completed report and independently checked exported board establish
closure. The run manifest remains `running` until the subprocess actually ends;
its final exit status and elapsed time remain authoritative for completion.

Telemetry does not yet expose each ordinary-net search, within-pair maze progress
or reusable serialized stage geometry. Those parts of R9 remain open.

The full-run wrapper also enables function profiling and derives inclusive
phase/trial intervals from the persisted log. See
[routing performance](routing-performance.md) for artifacts, profiling overhead,
unprofiled comparisons and the optimization assessment. Direct CLI routing is not
automatically function-profiled.

Local repair dependency searches emit `zone_subset_search` start/finish events:
`kind` distinguishes actual transactions from noncommitting probes; `affected_nets`,
`expansion`, `failed_nets` and `overflow` expose bounded work. A successful probe
is only a proposal, not a selected route or connectivity/signoff result.

Placement transactions emit `zone_incremental_trial` spans and a fresh
`zone_moved_global` span. `zone_incremental_guard` records why an unsupported or
unsuccessful transaction falls back to `zone_full_trial`. An incremental start
does not imply acceptance; closure/DRC and ground-contact improvement are checked
before its finish decision. Nested phase times are inclusive and overlap.
