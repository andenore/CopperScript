# Copper macro definitions

This synthetic fixture authors exact copper in `layouts/link.copper`, exports
the named macro through a local package, and binds it in `board.copper`. The
one-land parts and footprint exist solely to demonstrate language behavior.
They are not reusable electrical parts or a qualified board.

From the repository root:

```sh
copper check examples/macro_definition/board.copper --locked --offline
copper compile-macro examples/macro_definition/layouts/link.copper -o build/link.json --locked --offline
copper route-board examples/macro_definition/board.copper -o build/macro-definition.kicad_pcb --report build/macro-definition-route.json --locked --offline
```

Routing consumes the Copper definition directly. The generated JSON is optional
and uses the same immutable asset schema. Production libraries should use
managed footprint URLs pinned by their consumers' `copper.mod` and `copper.lock`.
