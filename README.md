# CopperScript

CopperScript is a semantic, strongly typed language for describing PCB
connectivity, electrical intent, and design constraints.

The v0.1 compiler parses `.copper` source into a typed intermediate
representation and runs electrical-rules checks (ERC). It deliberately does not
perform placement or routing yet.

## Quick start

Python 3.11 or newer is required. From the repository root:

```console
python -m copperscript check examples/valid_board.copper
python -m copperscript check examples/invalid_board.copper
python -m copperscript power-check examples/valid_board.copper
```

Compile a valid design to normalized JSON IR:

```console
python -m copperscript compile examples/valid_board.copper -o board.json
```

For a development installation with the `copper` command:

```console
python -m pip install -e ".[test]"
copper check examples/valid_board.copper
python -m pytest
```

## Example

```copper
board SensorBoard {
    use library "tiny";
    import sensors "github.com/copperscript/examples/sensors";
    import stm32 "github.com/copperscript/examples/stm32";

    component U1: stm32.STM32G0B1CBT6;
    component U2: sensors.BME280;
    component R1: RESISTOR { value = 4.7kohm; }

    configure U1.I2C1 as SENSOR_BUS {
        SDA = PB7;
        SCL = PB6;
    }

    net I2C_SDA {
        U1.PB7;
        U2.SDA;
        R1.2;
    }

    supply V3V3 {
        voltage = 3.3V;
        external = true;
    }
}
```

See the [design specification](docs/design-specification.md), the
[language reference](docs/language-reference.md), and the complete [valid
example](examples/valid_board.copper).

The [hierarchical example](examples/hierarchical_board.copper) instantiates a
reusable [5 V to 3.3 V buck supply](examples/packages/power/buck_5v_to_3v3.copper)
and imports the sensor part from a separate package.

## Packages

Imports use stable URL-like package paths and an explicit local alias:

```copper
import power "github.com/copperscript/examples/power";
module PWR: power.Buck5VTo3V3;
```

Versions and development replacements are deliberately kept out of source
files. They live in `copper.mod`:

```text
module github.com/anden/CopperScript
require github.com/copperscript/examples v0.1.0
replace github.com/copperscript/examples => ./examples/packages
```

The compiler resolves the longest matching required module, loads all
`.copper` part and module definitions in the selected package directory, and
records downloaded content in `copper.sum`. Without a `replace`, v0.1 fetches
tagged GitHub modules into `.copper-cache`; package source is parsed as data and
no package code is executed. Local replacements remain editable and retain
their current content hash as IR provenance without being locked in the sum
file.

## Compiler architecture

```text
.copper source
    -> lexer
    -> source-aware syntax tree
    -> hierarchical electrical IR
         |-> JSON IR
         `-> derived flat view -> electrical-rules checker
                              `-> power-state analyzer
```

The stages are intentionally separate so editor tooling and a future language
server can reuse parsing, semantic lowering, and ERC independently.

Key modules:

- `pcbir.lexer` and `pcbir.parser` — dependency-free source frontend.
- `pcbir.syntax` — source locations and syntax-tree declarations.
- `pcbir.compiler` — semantic lowering and typed unit validation.
- `pcbir.packages` — manifests, package resolution, Git cache, and checksums.
- `pcbir.model` and `pcbir.quantities` — immutable semantic IR.
- `pcbir.elaborate` — explicit hierarchy-to-flat derivation for consumers that
  require a global connectivity view.
- `pcbir.erc` — reusable electrical-rules passes.
- `pcbir.power` — explicit steady-state power-domain analysis.
- `pcbir.serializer` — versioned JSON IR output.
- `pcbir.cli` — `check`, `power-check`, and `compile` commands.

Python IR constructions are confined to test fixtures. `.copper` is the only
user-facing source format accepted by the compiler.

## Current ERC checks

- Unknown parts, components, pins, nets, and supply sources
- Duplicate component, net, supply, and interface names
- Pins assigned to multiple nets
- Multiple push-pull or power outputs on one net
- Supply voltages outside declared pin limits
- Invalid, missing, or off-net supply sources
- Disconnected and unsourced power inputs
- I²C signal bindings, pin compatibility, and SDA/SCL pull-ups
- MCU peripheral completeness, package-pin availability, and mux validity
- Duplicate exclusive peripheral/pin selections and incompatible mux-resource settings

`power-check` separately evaluates named rail states and warns when a driven net
may back-power an I/O domain declared off. It is intentionally a conservative
steady-state analysis, not firmware or transient simulation.

Physical constraints such as maximum placement distance are retained in IR but
are not enforced in v0.1.
