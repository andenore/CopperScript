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
copper assembly bom board.copper --locked --offline --lock assembly.lock -o build/bom.csv
```

Snapshot creation refuses to overwrite an existing lock. Circuit/library changes
invalidate the electrical digest; review differences and deliberately create a
new snapshot rather than silently accepting new parts. Missing fields, unknown
or duplicate references, mismatched parts/footprints, and unreviewed selections
fail the check. BOM export requires both ERC and assembly checks to pass.
Constraint diagnostic source locations are excluded from this digest, so the
same locked design works across checkout paths and operating systems. Actual
constraints, hierarchy, document evidence and library revisions remain bound.
The CSV contains JLCPCB's component-selection columns plus manufacturer/MPN.
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
