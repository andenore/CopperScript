# Exact assembly selections (prototype)

Electrical library revisions and procurement identities are separate pins.
`assembly.lock` binds each assembled component, as compiled, to an explicit
manufacturer MPN and supplier ordering code. It never resolves substitutes from
stock.
Every assembled component must be covered; non-assembled library parts are
excluded. Hierarchical references retain their qualified paths.

```sh
copper assembly snapshot board.copper --locked --offline -o assembly.lock
# Fill manufacturer, mpn, supplier, supplier_part and optional catalogue URL.
# Review exact package, ratings, pin mapping and footprint before reviewed=true.
# After source changes, rebind; only changed components lose their review:
copper assembly update board.copper --locked --offline --lock assembly.lock
copper assembly check board.copper --locked --offline --lock assembly.lock --report build/assembly.json
copper assembly bom board.copper --locked --offline --lock assembly.lock -o build/bom.csv \
    --hand-assembly build/hand-assembly.csv --ordering build/ordering.csv
```

`--hand-assembly` writes the components the supplier will not place: every
`consigned` selection (to buy and fit by hand) and every `populate = "none"`
selection (to leave empty). Together with the supplier BOM it covers each
populated component exactly once. Consigned selections may record `supplier`,
`supplier_part` and `source_url` (any supplier, not only `jlcpcb`); the list
carries them so it doubles as the order sheet for the hand-fitted parts.

`--ordering` writes one sheet for the whole board: every selection, grouped by
designator, with value, manufacturer, MPN, footprint, assembly route
(`supplier-assembled`, `hand-assemble` or `do not fit`) and the recorded
supplier, supplier part and source URL. Use it to quote, buy spares or
re-order; the supplier BOM and hand-assembly list remain the files the
assembler and the hand-fitter work from.

### Alternate offers

A selection may also record where else the same manufacturer part can be
bought, for spares, quotes or a hand build from one distributor:

```json
"alternates": [
  {"supplier": "DigiKey", "supplier_part": "1276-1001-1-ND",
   "source_url": "https://www.digikey.com/en/products/detail/samsung-electro-mechanics/CL05B104KO5NNNC/3886659"}
]
```

Alternates never change what is fitted or what the assembler is given: the
supplier BOM uses only `supplier`/`supplier_part`. They are checked like the
primary offer (a supplier code may not name two manufacturer parts, may not be
listed twice for one component, and JLCPCB codes must be `C` numbers), and they
need the manufacturer and MPN they sell. The ordering list gains a part and URL
column for each alternate distributor, also filled where that distributor is
the primary supplier (matched without regard to case), so a single-distributor
order reads down one column.

## Partially assembled boards

Each selection carries a `populate` mode saying how the component reaches the
board:

| `populate` | Meaning | In the supplier BOM/CPL |
| --- | --- | --- |
| `supplier` (default) | the assembler sources and places it | yes |
| `consigned` | fitted, but supplied and placed outside the supplier order | no |
| `none` | deliberately not fitted | no |

This is what makes a mixed build exportable: a board whose passives and common
ICs the assembler stocks, with a few specialist parts it does not, can still
hand JLCPCB everything it *can* place. Mark the rest and the BOM succeeds.

A consigned component is still fitted, so it keeps a full part identity
(manufacturer, MPN, footprint) for whoever hand-places it, but needs no
supplier offer. A `none` component needs neither. All three still require
explicit review.

Exclusion is never silent: `assembly check` lists the consigned and not-fitted
references, `assembly bom` reports how many it left out, and the report carries
`supplier_assembled_count`, `consigned_count`, `not_fitted_count` plus the
reference lists. Because the CPL is generated from the BOM's designators,
excluded components leave both files together, which preserves the mandatory
BOM/CPL reference equality and keeps the assembler from being asked to place a
part nobody supplied.

## Keeping the lock current

Each selection records `electrical_sha256`, the digest of what it was reviewed
against: the component (value, footprint, properties), its library part and
device, and for every pin the net it is on, that net's supplies and voltages,
and any no-connect. A change to any of these marks that selection
`STALE_SELECTION`; changes elsewhere on the board (placement, routing intent,
silkscreen, other components) leave it current.

`copper assembly update` rebinds the lock in place. A selection whose component
is unchanged keeps its identity and review. A changed one keeps its identity
for reference, takes the new part and footprint, and is no longer reviewed; the
command prints what changed. New components get unreviewed snapshot entries;
selections for components that are gone are dropped and listed. It never marks
anything reviewed.

Locks are written as `copperscript-assembly-lock/v0.4`. Earlier locks bind the
whole compiled board with one top-level `electrical_sha256` (constraint
diagnostic source locations excluded) and still load and check: v0.3 added
`alternates`, v0.2 `populate` (every v0.1 selection means `supplier`). Updating
one rewrites it as v0.4. If its whole-board digest still matches, every review
is kept; otherwise nothing records which components changed, so every review is
reset. To keep the reviews of a stale legacy lock, update it at the revision it
was made for (where `check` reports no `STALE_LOCK`), then update the result
against the current source: only components that changed in between are reset.

Snapshot creation refuses to overwrite an existing lock. Missing fields, unknown
or duplicate references, mismatched parts/footprints, stale and unreviewed
selections fail the check. BOM export requires both ERC and assembly checks to
pass.
The CSV contains JLCPCB's component-selection columns plus manufacturer/MPN,
for the supplier-assembled components only.
No stock lookup, order submission or manufacturing signoff is implied.
The separate [native manufacturing exporter](manufacturing-files.md) can consume
this BOM and the final routed KiCad board to generate a matching JLCPCB CPL.
The footprint reference is pinned, but its geometry checksum still
requires the independent footprint/manufacturing audit.

## JLCPCB access experiment

JLCPCB publishes a [developer portal](https://api.jlcpcb.com/) and
[API access application instructions](https://jlcpcb.com/help/article/jlcpcb-online-api-available-now).
Its Components API covers specifications, inventory and pricing. Account
approval is required; endpoints and authentication must follow the integration
documentation supplied by JLCPCB. Do not guess endpoints, scrape private APIs,
or call a catalogue-page lookup an authenticated API test.

The initial experiment uses public catalogue evidence only. Catalogue presence
is not live stock, a reservation, or approval of a particular assembly order.
Live availability belongs in a timestamped report under ignored `build/`, not
in deterministic compilation. No credentials belong in a lockfile or Git.

Supplier offers should eventually live beside exact reusable CopperLib parts.
This bounded prototype uses explicit board-side selections without changing
the electrical language or claiming generic passives are qualified replacements.
There are no board-specific rules or selections in the compiler.

## Live distributor offers

`copper assembly offers` is the one networked assembly action. It reads the
lock (no compilation), looks up every fitted manufacturer+MPN with each
configured provider and writes a timestamped report:

```sh
export MOUSER_API_KEY=...                            # any provider(s), see below
copper assembly offers board.copper --quantity 5 --distributors DigiKey,Mouser,Farnell \
    --min-stock 100 --within-weeks 8 --currency EUR --country NO
```

### Providers

A provider is enabled when its environment variables are set; `--provider
digikey,mouser` limits the lookup to those (naming an unconfigured one is an
error). Each is queried only when it sells for a requested distributor, and
results are merged per part: one offer per distributor and SKU, a direct API's
offer winning over Nexar's.

| `--provider` | Distributors | Register | Environment | Limits (paced) |
| --- | --- | --- | --- | --- |
| `digikey` | DigiKey | [developer.digikey.com](https://developer.digikey.com): production app with the Product Information API (v4) | `DIGIKEY_CLIENT_ID`, `DIGIKEY_CLIENT_SECRET` | 120/min, 1000/day |
| `mouser` | Mouser | [mouser.com/api-hub](https://www.mouser.com/api-hub/): Search API key | `MOUSER_API_KEY` | 30/min, 1000/day |
| `farnell` | Farnell, Newark or element14, by store | [partner.element14.com](https://partner.element14.com): Product Search API key | `FARNELL_API_KEY`, optional `FARNELL_STORE` (default `uk.farnell.com`; e.g. `no.farnell.com`, `www.newark.com`) | 2/s, 1000/day |
| `nexar` | every authorized seller Octopart lists | [portal.nexar.com](https://portal.nexar.com): application with the Supply scope (needs a plan with a part quota) | `NEXAR_CLIENT_ID`, `NEXAR_CLIENT_SECRET` | per plan |

DigiKey and Nexar take `--country`/`--currency` (default `US`/`USD`);
Mouser prices come in the API account's currency and Farnell's in the store's.
Requests are spaced under each per-minute limit, one per part (Nexar batches
20). An HTTP 429 stops that provider with a rate-limit error. A provider that
fails is reported (`PROVIDER ERROR`, `provider_errors`) and its offers are left
out; the command fails only when every queried provider fails. Daily limits
bound how many parts one run can look up. TME, LCSC, Arrow and other
distributor APIs can be added as further providers.

### Report

- The lock defaults to `assembly.lock` beside the board and the report to
  `build/assembly-offers/offers-YYYYMMDDTHHMMSSZ.json` there (`--lock`, `-o`).
  Schema `copperscript-assembly-offers/v0.1`, marked `"deterministic": false`,
  with the providers used.
- Per part: references, quantity needed (`--quantity` boards, default 1), the
  exact match (MPN equal without case, manufacturer prefix-equal without case
  or punctuation, so `Murata` matches `Murata Electronics`) or the near-miss
  candidates, and for each requested distributor every offer: source
  provider, SKU, stock, factory lead days, MOQ, packaging, URL and price breaks.
- `--distributors` defaults to the suppliers already named in the lock and
  compares names without case or punctuation (`DigiKey` is `Digi-Key`).
- An offer is adequate when stock covers the build and `--min-stock`, or, with
  `--within-weeks`, when the factory lead time fits. Parts with no adequate
  offer are listed as `SHORT` and under `shortfall`; with `--min-stock` or
  `--within-weeks` the command then exits 1.
- `--write-alternates` adds, per fitted selection, one offer for the same
  manufacturer part from each requested distributor not yet listed on it
  (preferring an adequate offer, then the smallest MOQ). It writes only
  `supplier`, `supplier_part` and a non-tracking `source_url`, skips codes the
  lock gives another part, and never changes MPNs, primary offers, `populate`
  or `reviewed`. It needs a canonical v0.4 lock (as `assembly update` writes).

Credentials come only from the environment variables above. They, access
tokens and API keys in query strings travel to `curl` on standard input, never
on a command line, and are never printed or written; errors show URLs without
their query string. `--offline` is refused. Check, compile, lock and BOM
export never touch the network, and the report is never an input to them.
Live stock is not a reservation.

## Unapproved review candidates

`copper assembly candidate-bom board.copper --locked --offline --lock assembly.lock
-o build/assembly-review` creates a new, explicitly unapproved bundle for human
review. It retains every unreviewed selection flag and lists those findings in
`assembly-review.json`; missing, stale, conflicting or incomplete identities and
ERC errors refuse output. `all-suppliers-NOT-APPROVED.csv` lists every fitted
selection with its original supplier identity. `bom-candidate-NOT-APPROVED.csv`
contains only explicitly selected `jlcpcb` offers and is omitted when none exist.
LCSC and other supplier entries are not silently treated as JLCPCB-eligible.
Stock, native placement orientation and ordering approval remain unconfirmed.
The strict `assembly bom` and release qualification gates are unchanged.
CSV designators use the same native-reference normalization as the KiCad PCB
backend (for example `BANK/R1` becomes `BANK_R1`), while lock/diagnostics retain
source-qualified references. Collisions refuse output, and candidate diagnostics
include the exact source-to-native map. No supplier rotation correction is inferred.
Optional `--positions final/positions-kicad.csv` also writes an explicitly
unapproved matching JLCPCB-subset CPL, excluding non-fitted references and binding the native
CSV hash. It retains native coordinates/rotations; a reference match alone does
not verify source/pose identity, pin 1 or supplier-specific placement conventions.
`all-suppliers-positions-NOT-APPROVED.csv` also covers separately procured parts;
missing native positions for any fitted part refuse completion of the review bundle.
