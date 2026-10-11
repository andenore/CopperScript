# CopperScript device-model compatibility lab

This bounded research lab is kept with CopperScript tooling rather than in
CopperLib's reusable component catalogue. It records a provenance-first
`STM32G0B1CBT6` proof, CubeMX reconciliation fixtures and cross-vendor model
gap case studies. The results are incomplete and are not production packages.

From this directory, install the optional test/upstream dependencies and run:

```powershell
python -m copperscript_stm32g0 validate
python -m copperscript_stm32g0 generate
python -m copperscript_stm32g0 coverage
python -m pytest
python -m copperscript_stm32g0 compatibility
```

The normalized bundle is in `data/bundles/stm32g0b1/`; generated files are in
`generated/`. Source acquisition is opt-in and writes only ignored cache data.
The compatibility reports classify support gaps without changing CopperScript's
language or the generic CopperLib package contract.


## Source and extraction rules

- Treat an installed STM32CubeMX `db/mcu` tree as local source only; never
  commit or redistribute ST XML. Hash every consumed XML file and keep compact
  extraction artifacts/manifests in ignored `cache/` or `work/` paths.
- Do not infer alternate-function selector numbers from CubeMX signal names.
  Missing selectors stay unresolved; leave unsupported mux generation out.
- Keep cross-vendor studies bounded and source-first, using official vendor
  sources with exact revision/table/page locators. Never execute downloaded code.
- Non-representable or lossy cases are explicit model-gap fixtures, not fake
  production bundles. Keep both compatibility reports deterministic and run
  `python -m copperscript_stm32g0 compatibility` when changing this lab.
