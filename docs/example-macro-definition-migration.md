# Example macro-definition migration

The RF and switched-power layouts used by the current examples are authored in
CopperLib `.copper` macro exports. Board electrical definitions, member/net
mappings, fixed poses and fabrication constraints remain consumer-owned.

| Consumer | Selected exports |
| --- | --- |
| nRF antenna probe / nRF52 coin-cell | `NRF52832_JOHANSON_SIX_LAYER_TRIAL` |
| Full vertical | `NRF52832_JOHANSON_MANAGED_SIX_LAYER`, `TPS62130A_MANAGED_ENABLED` |
| TPS62130A probe | `TPS62130A_MANAGED_ALWAYS_ON` or `TPS62130A_MANAGED_ENABLED` |
| CopperAssetTracker active board | `NRF52832_JOHANSON_MANAGED_SIX_LAYER`, `TPS63020_SIX_LAYER_ALWAYS_ON`, `TPS63020_SIX_LAYER_ENABLED_SERIES_FEEDBACK` |
| Tracker revision builder | Also `NRF52832_JOHANSON_MANAGED_CORNER`, `TPS63020_SIX_LAYER_ENABLED_POWER_OUTPUT` when those revisions are selected |

The managed RF and TPS62130A definitions move the previously reviewed reusable
example adaptations into their owning library packages. The trial RF variant
keeps its distinct source-compatible footprint set; it is not silently rebound
to the managed KiCad variant. Every member now names an independent exact
footprint URL. Hashes come from the selected provider inventory and source/asset
preparation. Existing JSON assets/scenes remain historical compatibility and
regression snapshots; active builds consume the Copper exports.

Migration checks compare all copper rows, via multiplicity/finishes, ports,
protected regions, keepouts, pad/net/isolated roles, zones, fixed polygons, plane
returns/dedicated contacts, width contracts, allowed poses and evidence records.
Source schema upgrades and generated footprint identities change asset hashes,
so old hashes are never relabeled as current evidence. Normalized physical
comparisons exclude only the expected asset-hash change.

The compiler/library commits and refreshed dependency inventories are pinned
before consumer acceptance. Concurrent upstream changes are preserved and tested;
old assembly/routing/manufacturing evidence remains bound to its original inputs.
No source-format conversion approves electrical/RF/power, assembly or production
qualification.

## Validation

CopperLib `50655eff5613690a606c9dd1de456b2adae249df` exports eight migrated variants.
Its complete `tests/` suite passes 168 tests, with 31 explicit optional footprint/
native cases skipped on this host. Compiler feature/LSP/asset/editor checks pass
240 tests (one optional skip); the clean compiler publish checkout passes 146
selected compiler/package/LSP/library tests, plus the 67-test evidence/editor
check after the recovery fix.

The four CopperScript examples compare equal against JSON bindings for electrical
nets, member/net roles, footprint geometry, rigid poses/constraints and immutable
copper under the selected library inputs. Concurrent library changes retain the
same footprint for the XFL4020 substitution; its part label advances with that
upstream change. The example/full-vertical/coin-cell/build suite passes 60 tests
with one platform skip.

Both TPS62130A variants pass independent KiCad 10.0.6 refill/DRC with zero
violations and opens. The enabled fixture's previously exposed EN track now
terminates at a synthetic land outside the rigid unit through an ordinary host
lead. All immutable owner copper is unchanged and no new DRC waiver is added.
The fixture does not model/qualify an EN drive circuit. The pinned KiCad test-point
footprint uses an unsupported legacy `virtual` attribute; the fixture uses an
explicit synthetic land with supported assembly-exclusion attributes instead.
That importer limitation remains visible and unchanged.

CopperAssetTracker consumes the installed published compiler
`17ebc6f488dcc8f80a3e0052a18af35760a9b759`. With the reviewed library supplied,
70 consumer tests and 34 subtests pass with one retained-review-artifact case
skipped. All eight revision-builder tests pass, including real RF-layout and
RF-support source generation using the corner/power-output exports. Both terminated TPS63020
variants pass native refill/DRC at all four cardinal rotations (eight clean reports).
The coin-cell native geometry check passes; ordinary non-RF opens remain explicitly
unclaimed in that placed draft. These results establish format/geometry compatibility,
not fresh full-board or hardware qualification.

Final clean-checkout acceptance passes 128 selected example, compiler-macro,
LSP and build tests with one platform skip, plus independent native refill/DRC
of the enabled TPS62130A fixture. Real example package exports are indexed
locked/offline without preparing footprint assets for unused macro variants.
