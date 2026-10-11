# Compact LLM design and verification workflow

Start with `copper agent-guide` and request `syntax`, `libraries`, `verification`
or `recovery` when needed. Search the consuming project's declared dependencies,
then retrieve selected contracts; see [library context](library-context.md).
Guidance and discovery metadata do not qualify a design.

## Report inspection

```console
copper summarize build/route-report.json
copper summarize build/route-report.json --json --issues 4
```

Saved `route-board` and `plan-layout` reports have same-stem `.summary.json`
sidecars. Summaries bind the full report's exact bytes and bound the issue list;
read the full report only for an identified issue or required omitted evidence.
`summarize` is read-only and exits 0 on successful reading even when the board
has failed gates; malformed/unsupported input exits 2. Older reports show missing
fields as `not_reported`. The issue cap defaults to eight and accepts 0–100.

## Requirement coverage

Routing reports retain their routing status and add `requirements_complete`,
`requirements` and `routing_quality`. Add `--require-verified-intent` to the
project's `route-board` invocation to require every hard declaration to pass,
as well as the existing routing gates. Layout reports block route-stage checks.
Check each requirement's status, provenance, measurements and explanation.

Supported checks include placement poses/orientations/alignment/distances,
footprint regions/keepouts, routing width, explicit length, via count, layers
and supported skew. Clearance coverage requires a current bound check at least
as strict as source intent. Original declarations cannot be weakened by a final
rule or display rounding. Unsupported complete pair-gap, return-policy, stub,
topology, tuning and impedance obligations remain blocked. External obligations
are hard blocked; assumptions retain `assumed`, and notes/groups are advisory.
Custom verifier names do not grant authority. Required-array geometry does not
establish fabrication-process qualification; hole findings are conservatively
attributed board-wide. No new waiver or external-evidence adapter is implied.

## Final-copper quality and acceptance

Metrics describe final tracks, vias, layers and angles. Surface bypass checks
use exact track contact, including interior junctions, excluding vias, polygons,
zone fills and component-internal joins. Connected-component length includes
branches; it is not a shortest bypass path or a universal electrical limit.
Reference coverage needs current export-bound native fill and an explicit net/
layer. Its conservative projected-envelope screen is not impedance qualification.

```console
copper benchmark-routing -o build/new-acceptance --kicad-cli /path/to/kicad-cli
```

Use a fresh output directory. Eight bounded two/four-layer cases include renamed/
rotated variants and intentionally blocked access. Authored synthetic lands are
test fixtures. Missing native tools make evidence incomplete; expected-open
cases remain open. Identities, digests, settings and expectations are recorded.
The suite is not a statistical success rate for arbitrary real boards.

Benchmark comparison reports final-copper tradeoffs. Increased measured length/
via costs block a neutral speedup claim; lost surface/reference/required-intent
evidence is a regression. Mixing measured and unmeasured reports blocks that
comparison. See [performance guidance](routing-performance.md).

These interfaces reduce the amount of material an agent must retrieve. Total
LLM token savings have not been measured. Remaining work is in
[the future plan](llm-workflow-improvements.md); engineering and prototype
qualification still require their own evidence.
