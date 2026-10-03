# Device and part generation workflow

## Internal contact review

Optional `part.json` field `"internal_pad_groups": [["1"], ["2"]]` generates
`internal_pad_groups = "1; 2";`. Unknown numbers and overlapping groups fail
bundle validation. Review the exact manufacturer's internal circuit and map it
to the selected footprint; repeated land numbers are not sufficient evidence.
Keep momentary switch contact pairs separate and never treat required thermal
or parallel-power contacts as optional external connections.

CopperLib's [agent checklist](https://github.com/andenore/CopperLib/blob/main/internal-pad-connectivity.md)
records the Keystone 3034 and TL3342 examples, source drawings and ground-contact
layout comparisons. Source fields remain optional in CopperScript, but agents
must not invent internal shorts. Groups are installed-component electrical facts
and carry no coordinates or routing geometry.

Real devices should be maintained as compact normalized data, not as large
hand-written `.copper` files or repeated model output. The generator turns a
small bundle of JSON metadata and CSV tables into deterministic CopperScript
device and part definitions.

The example bundle is [`device-data/stm32g0b1`](../device-data/stm32g0b1).

## Why this minimizes model tokens

- Repetitive pads, pins, peripheral signals, and mux choices are CSV rows.
- Rendering and formatting are deterministic Python operations.
- `summary` reports coverage without loading the tables into a conversation.
- `packet` selects only one table, peripheral, pin group, or unresolved row.
- `?` explicitly marks uncertain cells, so an agent can work only on missing
  facts instead of rereading completed data.
- Local validation resolves names and enums before the compiler or a reviewer
  sees generated source.

The normalized bundle is authoritative library input. Generated `.copper`
files are reviewable build artifacts and must not be edited by hand.

## Bundle layout

```text
device-data/stm32g0b1/
  device.json                 device metadata, provenance, domains, resources
  pads.csv                    silicon pads and orthogonal electrical profiles
  peripherals.csv             peripheral signals and required directions
  mux.csv                     irregular pad-to-peripheral mux choices
  parts/
    stm32g0b1cbt6/
      part.json               orderable package metadata
      pins.csv                physical pins, policies, and device-pad bonds
```

Profile fields and bonds use `|` inside CSV cells. Part manifests use an open
`category` string rather than a closed kind enum. Optional source provenance is
stored in `device.json` or `part.json`; it remains optional to the language and
generator.

## Commands

From the repository root:

```console
python tools/devicegen.py summary device-data/stm32g0b1
python tools/devicegen.py validate device-data/stm32g0b1
python tools/devicegen.py generate device-data/stm32g0b1 --out-dir examples/packages/stm32
python tools/devicegen.py check device-data/stm32g0b1 --out-dir examples/packages/stm32
```

An editable installation also provides `copper-devicegen` with the same
subcommands.

`check` fails if generated files are missing or stale. Tests run the same check
for the example bundle.

## Small work packets

Ask an agent to inspect only the relevant rows rather than an entire device:

```console
python tools/devicegen.py packet device-data/stm32g0b1 --section mux --match I2C1
python tools/devicegen.py packet device-data/stm32g0b1 --section pins --part STM32G0B1CBT6 --match PB6
python tools/devicegen.py packet device-data/stm32g0b1 --section pads --missing-only
```

The first output line is compact JSON containing identity, optional provenance,
allowed enum values, and row count. The rest is the selected CSV fragment.

## Recommended datasheet workflow

1. Prefer vendor XML, CMSIS-SVD, CSV, or other machine-readable sources over
   PDF extraction whenever possible.
2. Create the bundle skeleton and use `?` for facts that still need evidence.
3. Run `summary`, then generate a `--missing-only` packet for one section.
4. Give an extraction agent only that packet and the relevant datasheet pages
   or tables. Never provide the whole device bundle unless cross-table context
   is genuinely required.
5. Replace `?` with extracted values and run `validate`.
6. Generate `.copper`, run `check`, then run the compiler tests.
7. Review semantic changes in the compact tables and generated-source diff.

For multi-terminal passives, explicitly review internal signal paths and
polarity as well as pin/land coverage. A common-mode choke must pass each
signal through a separate winding with matching dot polarity; simply listing
four passive pins cannot establish this. The source-backed
[USB choke correction](routing-review-pass11.md) includes topology-specific
CopperLib regressions and a physical-net integration test. Copper DRC cannot
detect an incorrect internal winding mapping; such corrections invalidate old
electrical acceptance and require regenerated routing. Visually inspect source
schematics when PDF text order is ambiguous. This adds review guidance, not a
mandatory-source requirement to the language or a general internal-circuit solver.

For large MCUs, divide work by peripheral family or pin range. Merge compact
CSV rows, not prose or generated CopperScript. Automatic PDF extraction should
record a source revision when available, but missing provenance must not block
experimental definitions.
