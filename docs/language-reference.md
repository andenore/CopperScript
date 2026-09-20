# CopperScript v0.1 language reference

CopperScript source is UTF-8. Comments begin with `//` or `#` and continue to
the end of the line. Identifiers are case-sensitive.

## Boards, libraries, and packages

Each file contains one board. Libraries provide part definitions.

```copper
board SensorBoard {
    use library "tiny";
}
```

`tiny` is the bundled demonstration library. `standard` is currently an alias
for it while the library format is being designed.

Reusable packages are imported by stable URL-like path and assigned a local
alias. The alias qualifies exported parts and modules:

```copper
import sensors "github.com/copperscript/examples/sensors";
component U1: sensors.BME280;
```

The nearest `copper.mod` controls versions and local replacements:

```text
module github.com/example/product
require github.com/vendor/copper-parts v1.2.0
replace github.com/vendor/copper-parts => ../copper-parts
```

`require` selects a module version. `replace` is optional and redirects that
module to a local directory, relative to `copper.mod`. `copper.sum` records
downloaded module content and compilation fails if known content changes. Local
replacements are intentionally mutable and are not locked in `copper.sum`.
Remote resolution currently supports tagged GitHub repositories. Dependencies
are cached in `.copper-cache` and only `.copper` files are parsed.

A package is a directory. Every `.copper` file directly in it exports one
`device`, `part`, or `module`; boards cannot be exported from packages.

## Part definitions

Packages can define electrical parts independently of component instances:

```copper
part BME280 {
    kind = sensor;
    manufacturer = "Bosch";
    footprint = "LGA-8";

    pin VDD: power_in {
        number = "1";
        voltage_min = 1.71V;
        voltage_max = 3.6V;
    }
    pin GND: power_in { number = "2"; }
}
```

Supported part properties are `kind`, `manufacturer`, one `footprint`, and an
optional package-independent `device`. Every pin requires a quoted `number`;
`voltage_min` and `voltage_max` are optional typed voltage quantities.

## Devices and peripheral selection

A `device` describes package-independent silicon capabilities. Peripheral
signals are typed, required by default, and mapped to physical pin identities
through mux options:

```copper
device STM32G0B1 {
    vendor = "STMicroelectronics";

    peripheral I2C1: i2c {
        signal SDA: open_drain;
        signal SCL: open_drain;
    }

    peripheral USART2: uart {
        signal TX: output;
        signal RX: input;
        signal CK: output { required = false; }
    }

    resource USART2_REMAP;

    mux PB7: I2C1.SDA { selector = "AF6"; }
    mux PB6: I2C1.SCL { selector = "AF6"; }
    mux PA2: USART2.TX {
        selector = "AF1";
        resource = USART2_REMAP;
        setting = default;
    }
}
```

A package-specific MCU part refers to its underlying device and declares only
the pins bonded out in that package:

```copper
part STM32G0B1CBT6 {
    kind = mcu;
    footprint = "LQFP-48";
    device = STM32G0B1;

    pin PB6: bidirectional { number = "42"; }
    pin PB7: bidirectional { number = "43"; }
}
```

Boards and modules explicitly select peripheral pins:

```copper
component U1: stm32.STM32G0B1CBT6;

configure U1.I2C1 as SENSOR_BUS {
    SDA = PB7;
    SCL = PB6;
}

net I2C_SDA { U1.PB7; SENSOR.SDA; }
net I2C_SCL { U1.PB6; SENSOR.SCL; }
```

`configure` does not connect pins. It selects pin behavior and lowers each
choice to a concrete selector such as `AF6` in IR. ERC checks required signals,
package pin availability, valid mux mappings, duplicate peripheral or pin use,
electrical compatibility, and incompatible settings of shared resources.
Automatic pin assignment is intentionally not part of v0.1.

## Hierarchical modules

A module is a reusable circuit with a typed electrical boundary. Modules are
exported by packages and instantiated inside boards or other modules.

```copper
// github.com/example/power/supply.copper
module Supply3V3 {
    use library "tiny";

    port VIN: power_in;
    port VOUT: power_out;
    port GND: power_in;

    component U1: REGULATOR_3V3;

    net VIN  { port.VIN;  U1.IN; }
    net VOUT { port.VOUT; U1.OUT; }
    net GND  { port.GND;  U1.GND; }
}
```

The parent imports the module and connects its ports through ordinary nets:

```copper
board Product {
    import power "github.com/example/power";

    module PWR: power.Supply3V3;

    net VBUS { PWR.VIN; }
    net V3V3 { PWR.VOUT; }
    net GND  { PWR.GND; }
}
```

Every declared port must occur on exactly one internal net and every instantiated
port must be connected by the parent. Imported modules may instantiate sibling
modules or modules from other packages; circular imports are rejected.

The compiled IR preserves module definitions, local names, instances, ports, and
boundary connections. Consumers that require a global view may explicitly run
hierarchy elaboration, which derives `/`-qualified names such as `PWR/U1`. This
flat view is not authoritative and is not the default serialized IR.

Module ports do not provide a second connectivity mechanism. `port.VIN` and
`PWR.VIN` are ordinary net endpoints that identify opposite sides of the same
boundary during elaboration.

## Components

```copper
component U1: STM32_LIKE;

component R1: RESISTOR {
    value = 4.7kohm;
    footprint = "0402";
}
```

Supported component properties are `value` and `footprint`. A value can be a
typed quantity or a quoted string.

## Nets

Nets are the single source of truth for connectivity.

```copper
net I2C_SDA {
    U1.PB7;
    U2.SDA;
    R1.2;
}
```

An endpoint always has the form `component.pin`.

## Supplies

Supplies annotate nets with voltage and source information.

```copper
supply V3V3 {
    voltage = 3.3V;
    source = U1.OUT;
}

supply GND {
    voltage = 0V;
    external = true;
}
```

The supply name is also its net name unless `net = OTHER_NET;` is specified.
`external = true` marks a rail established outside the modeled board.

## I²C interfaces

Interfaces add protocol intent without duplicating net connectivity.

```copper
interface SENSOR_I2C: i2c {
    sda = I2C_SDA;
    scl = I2C_SCL;
    pullup = V3V3;

    bind U1 { sda = PB7; scl = PB6; }
    bind U2 { sda = SDA; scl = SCL; }
}
```

The checker verifies both bindings, compatible pin types, and one pull-up
resistor from each signal to the named supply.

## Constraints

Physical constraints are parsed and retained in IR, but not enforced in v0.1.

```copper
constraint max_distance(C1, U1.VDD) {
    distance = 3mm;
}
```

Known constraint kinds are `max_distance`, `placement_region`, and `note`.

## Quantities

Quantities have no whitespace between their number and unit.

| Dimension | Units |
|---|---|
| Voltage | `V`, `mV` |
| Resistance | `ohm`, `kohm`, `Mohm` |
| Capacitance | `F`, `uF`, `nF`, `pF` |
| Inductance | `H`, `mH`, `uH`, `nH` |
| Length | `m`, `mm`, `um` |

Values are normalized to SI base units in the IR while retaining their source
display unit.

## Grammar sketch

```ebnf
document       = ("board" | "module" | "part" | "device"), name, "{", item*, "}" ;
item           = library | package_import | port | pin | part_property
               | peripheral | mux | resource | device_property | configuration
               | module_instance | component | net | supply | interface | constraint ;
library        = "use", "library", string, ";" ;
package_import = "import", name, string, ";" ;
port           = "port", name, ":", pin_type, ";" ;
pin            = "pin", name, ":", pin_type, properties ;
part_property  = name, "=", scalar, ";" ;
device_property = name, "=", scalar, ";" ;
peripheral     = "peripheral", name, ":", name, "{", signal+, "}" ;
signal         = "signal", name, ":", pin_type, (";" | properties) ;
mux            = "mux", name, ":", name, ".", name, properties ;
resource       = "resource", name, ";" ;
configuration  = "configure", name, ".", name, "as", name, properties ;
qualified_name = name, (".", name)* ;
module_instance = "module", name, ":", qualified_name, ";" ;
component      = "component", name, ":", qualified_name, (";" | properties) ;
net            = "net", name, "{", endpoint*, "}" ;
endpoint       = name, ".", name, ";" ;
supply         = "supply", name, properties ;
interface      = "interface", name, ":", "i2c", "{", interface_item*, "}" ;
constraint     = "constraint", name, "(", targets, ")", properties ;
properties     = "{", (name, "=", scalar, ";")*, "}" ;
scalar         = string | boolean | name | number, unit ;
```
