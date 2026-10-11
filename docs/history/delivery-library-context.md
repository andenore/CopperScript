# Compact library context

> Historical record, archived 2026-10-11. Claims and commands below apply to
> their recorded revisions, not current usage or approval. Read only when
> investigating that history; use the current reference guides for new work.

Start with `copper agent-guide`; use `copper agent-guide libraries` for the
compact retrieval workflow. See [agent bootstrap](../agent-guide.md) for project hints.

Use `copper library` to discover and inspect definitions across a project's
declared library dependencies. CopperLib is one provider; no provider name is
hard-coded. Discovery parses source declarations without expanding a board's
entire library or running package code.

## Find, inspect, then follow dependencies

```powershell
copper library search "nrf" --project path/to/board --kind part --limit 3
copper library inspect "github.com/vendor/library/parts/chip::Chip" --project path/to/board
copper library inspect "github.com/vendor/library/parts/chip::Chip" --project path/to/board --section interfaces --limit 10
copper library inspect "github.com/vendor/library/parts/chip::Chip" --project path/to/board --section interfaces --member VDD --member GND
copper library inspect "github.com/vendor/library/circuits/support::Support" --project path/to/board --section requirements
```

Search uses all terms, matches source declarations as well as identities and
metadata, and ranks symbol/metadata matches first. `--provider MODULE` restricts
search to one declared provider and is repeatable. Same-name exports from different
providers remain distinct. `--kind` disambiguates a part and module with the same
export name; duplicate exports of the same kind are errors to repair.

An inspection resolves only the selected provider for discovery. Imports in its
`dependencies` section are resolved on demand using the existing compiler's
policy: the host project declares the required versions in `copper.mod`.
Missing declarations are recorded as blocked, with their resolver diagnostic.
No automatic transitive version selection is introduced.

Default output is compact JSON, including when `--json` is omitted. Every
search result carries its package, exported symbol, kind, provider, selected
version, module content digest, source path, source hash and source line.
Metadata is explicitly a preview. To bind inspection to a search selection, use
`--expect-digest sha256:...` with that result's module content digest.

## Retrieve only what is needed

| Section | Contents |
| --- | --- |
| `overview` | Source comments, requirements, core identity/evidence properties and interfaces; obligations precede pin tables |
| `interfaces` | Pins, pads, ports, peripherals, interface declarations, units and group/domain declarations |
| `requirements` | Constraints, routing rules, supplies, power states and explicit component role/decoupling associations |
| `composition` | Components, submodules, nets, configuration, muxes and mechanical declarations |
| `properties` | All declared part/device properties, including evidence locators |
| `notes` | Source comments, preserving caveats without confusing URL strings with comments |
| `dependencies` | Builtin library names, imported packages and their selected identities, plus a part's device-contract reference |
| `declarations` | Every declaration of the selected source definition |
| `source` | Exact selected source lines, including comments |

Quantities retain exact fractions and readable units, for example `1.8 V`.
Imported and inherited definitions are not automatically expanded. Follow
`dependencies` and inspect the necessary contracts separately. Search and
inspection are descriptive views, not compiler validation, ERC, requirement
verification or electrical qualification. Use the standard checks after
integration. A source-bound declaration is not a reviewed fact merely because
it appears in this output.

The default search returns at most five records; inspection returns at most 20.
Use repeatable `--member NAME` to select exact declaration names, component refs,
constraint kinds or targets within a section. Results distinguish `unselected`
records from paginated `omitted` records and list `unmatched_members`. Selection
does not imply that the rest of the definition is irrelevant; inspect its source
notes and complete requirements before integration. Source and notes sections
use pagination rather than member filtering.

`--limit` accepts 1..100; `--offset` retrieves the next page. Output is bounded by
`--max-chars` (default 8000, range 2048..65536), measured against the actual compact
JSON representation, not an estimated token count. Follow `next_offset` and
check `omitted` and `page_complete`. A single oversized record becomes an explicit
`needs_source` locator; increase the budget or inspect its bound source. It is
never silently shortened into an apparently complete obligation.

Discovery errors remain visible through `discovery_complete`, diagnostic counts
and `copper library diagnostics --project ...`, which is also paginated. Missing
locks, corrupt content or unsupported resolution fail rather than yielding a
verified-looking substitute.

## Locked operation and portable snapshots

Discovery defaults to locked, offline resolution. It verifies cached source and
asset inventories without modifying source or the lock. Resolve dependencies
with the existing project workflow first. Alternatively, explicitly request
`--update-lock` to create/update the lock and `--fetch` to permit uncached remote
fetches. These flags retain the existing resolver's behavior for local
replacements, checksums and remote hosts.

```powershell
copper library index path/to/library -o build/library-index.json
```

A standalone library must declare its identity in its root `copper.mod`.
The versioned `copperscript.library-index/v1` snapshot contains provider identity,
digest-bound source-derived entries, capability hints, counts and diagnostics.
Export it to a new file outside the indexed library; this avoids a digest that
depends on its own generated index. Snapshots can be shared as release artifacts.
Runtime discovery derives its index from current resolved sources rather than
trusting a separately authored or stale snapshot. No registry or persistent
aggregate-index cache is required in this implementation.

## Verification and limits

Fixtures exercise two independent providers, duplicate names, cross-library
assembly compilation, exact limits, source caveats, locked/offline reads, cache
tampering, local replacement updates, nested-module shadowing, output budgets,
pagination and oversized records. Fixture components are not orderable catalogue
additions. No CopperLib definitions or consumer dependency pins were changed.

Validation: the library/package/CLI/report regression passed **72 tests**, with
one host-limited symlink skip. After adding member selection and compacting the
inspection identity, the final library/package/report check passed **49 tests**,
with the same skip. Counts overlap; these are not full-repository test claims.

A read-only replay against CopperLedRing's existing locked dependencies found
37 definitions containing 81,676 source characters. A single `nrf` part search
and five-pin interface page exposed fewer than 4,000 characters together, with the
remaining interfaces explicitly paginated. This illustrates selective exposure;
it does not measure model tokens, success rate or total task effort. Item 1's
controlled evaluation is still needed for those claims.

Future extensions include author-reviewed contract bundles, persistent indexes
bound to resolved content, and explicit transitive graph/version compatibility
policy. The current source-derived interface works across the existing resolver's
providers without expanding CopperLib's catalogue.
