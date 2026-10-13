# Exact assembly selections (prototype)

Electrical library revisions and procurement identities are separate pins.
`assembly.lock` binds a board's compiled electrical IR to explicit manufacturer
MPNs and supplier ordering codes. It never resolves substitutes from stock.
Every assembled component must be covered; non-assembled library parts are
excluded. Hierarchical references retain their qualified paths.

```sh
copper assembly snapshot board.copper --locked --offline -o assembly.lock
# Fill manufacturer, mpn, supplier, supplier_part and optional catalogue URL.
# Review exact package, ratings, pin mapping and footprint before reviewed=true.
copper assembly check board.copper --locked --offline --lock assembly.lock --report build/assembly.json
copper assembly bom board.copper --locked --offline --lock assembly.lock -o build/bom.csv \
    --hand-assembly build/hand-assembly.csv
```

`--hand-assembly` writes the components the supplier will not place: every
`consigned` selection (to buy and fit by hand) and every `populate = "none"`
selection (to leave empty). Together with the supplier BOM it covers each
populated component exactly once. Consigned selections may record `supplier`,
`supplier_part` and `source_url` (any supplier, not only `jlcpcb`); the list
carries them so it doubles as the order sheet for the hand-fitted parts.

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

Locks written before `populate` (`copperscript-assembly-lock/v0.1`) still load;
every selection in them means `supplier`. New locks are written as `v0.2`.

Snapshot creation refuses to overwrite an existing lock. Circuit/library changes
invalidate the electrical digest; review differences and deliberately create a
new snapshot rather than silently accepting new parts. Missing fields, unknown
or duplicate references, mismatched parts/footprints, and unreviewed selections
fail the check. BOM export requires both ERC and assembly checks to pass.
Constraint diagnostic source locations are excluded from this digest, so the
same locked design works across checkout paths and operating systems. Actual
constraints, hierarchy, document evidence and library revisions remain bound.
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
There are no board-specific rules or selections in `pcbir`.
