# CopperScript v0.1 language reference

## Mechanical geometry status

A board may declare a separate `mechanical` section containing a circular,
rectangular or polygonal outline, polygonal cutouts, NPTH holes and physical
rule overrides. See [the mechanical language specification](mechanical-language.md)
for the complete grammar, validation rules and API separation.

```copper
mechanical {
    outline circle { diameter = 50mm; center = (25mm, 25mm); }
}
```

Electrical connectivity remains geometry-free. Generic `export-kicad-pcb`,
`plan-layout`, `route-global` and `route-board` consume the source outline.
Source geometry takes precedence over rectangle fallback dimensions.

The `mechanical` block may also declare the board stack-up, top to bottom:

```copper
stackup {
    copper F.Cu { thickness = 0.035mm; }
    dielectric P1 { thickness = 0.1mm; er = 4.1; loss_tangent = 0.02; material = "3313"; type = prepreg; }
    copper In1.Cu { thickness = 0.0175mm; }
    ...
    copper B.Cu { thickness = 0.035mm; }
}
```

It lowers to `Stackup.physical_layers`; the selected `--layers` must name
exactly the declared copper layers. The stack-up feeds impedance screening
(see [signal-integrity screening](#signal-integrity-screening)) and the KiCad
board stack-up. See [the mechanical language specification](mechanical-language.md#board-stack-up)
for its validation rules.

## Permanent component-internal pad connections

Parts may declare `internal_pad_groups = "1; 2";`. Semicolons separate
independent groups; commas/spaces separate numbers within a group. A singleton
joins every physical land with that number. `"1, 3; 2, 4"` joins lands numbered
1/3 and, independently, 2/4. Numbers must exist and groups must not overlap.
Different-net assignments fail ERC and physical validation. An otherwise
unmentioned numbered terminal inherits its group's net during physical lowering.

This is a verified installed-component connection, not a net tie, switched
connection, resistor or copper-clearance waiver. Do not declare groups where
every external land is required for power/current sharing or thermal reasons.
Source references remain optional. With `assembled = false`, component-internal
connections are disabled. Repeated footprint numbers alone never imply a group.

The router may reach any land of a declared group and does not add a redundant
PCB bridge. All solder lands remain in the footprint. Assembly-aware and bare
copper connectivity are separate (`include_internal_connections=False` requests
the latter). Exports containing groups require KiCad 10 and preserve explicit
jumper-pad/pin groups in generated PCB, footprint and schematic files. Via-in-pad
permission is independent and remains explicitly scoped.

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
module to a local directory, relative to `copper.mod`. `copper lock board.copper`
writes the canonical `copper.lock` inventory. The lock contains a module digest
and an individual SHA-256 and size for each consumable `.copper`, `.kicad_mod`,
3D-model, JSON, and CSV asset. Local replacements are locked exactly like
downloaded modules. Use `--locked` to reject missing or changed content and
`--offline` to reject remote cache misses. Remote resolution supports
tagged or commit-pinned GitHub and GitLab repositories and caches them in `.copper-cache`; package code is
never executed.

A package is a directory. Every `.copper` file directly in it exports one
`device`, `part`, `module`, or `board_profile`; boards cannot be exported from packages.
Project-local imports such as `import carrier "./mechanics";` are relative to
the importing file and confined to its source module. Local source retains
content-hash provenance but is not an external locked dependency.

Reusable [mechanical profiles](mechanical-profiles.md) supply outlines, holes,
keepouts and explicit connector roles without changing electrical connectivity:

```copper
mechanical {
    use carrier.Carrier as host { debug = J_DEBUG; }
}
```

## Part definitions

Packages can define electrical parts independently of component instances:

```copper
part BME280 {
    category = "sensor.environmental";
    manufacturer = "Bosch";
    footprint = "LGA-8";

    pin VDD {
        number = "1";
        domains = "power";
        directions = "input";
        voltage_min = 1.71V;
        voltage_max = 3.6V;
    }
    pin GND {
        number = "2";
        domains = "ground";
        directions = "input";
    }
}
```

Supported part properties are open `category` and `traits` strings,
`manufacturer`, `assembled` (boolean, default `true`), one `footprint`, and an
optional package-independent `device`. Every pin requires a quoted `number`;
`voltage_min` and `voltage_max` are optional typed voltage quantities (the
operating range), and `absolute_min` and `absolute_max` are optional
absolute-maximum ratings (see [Voltage limits](#voltage-limits)).
Standalone parts declare `domains` and `directions`, with optional
`drive_modes` and `traits`. Domains are `digital`, `analog`, `power`, `ground`,
`clock`, and `rf`. Directions are `input`, `output`, `bidirectional`, and
`passive`; drive modes are `push_pull`, `open_drain`, and `high_impedance`.
Pins may use `connection = required`, `do_not_connect`, `optional`, or `normal`
and may declare comma-separated `required_net_traits`.
As an exception, `optional` or `do_not_connect` pins may omit their electrical
profile to preserve a known physical pad whose function has not yet been
verified. An unmodeled optional pin cannot be connected to any net: ERC reports
`UNMODELED_PIN`. This supports complete footprint-pad coverage without
asserting invented electrical facts.
Set `assembled = false` for bare-board targets such as Tag-Connect programming
pads; the KiCad schematic then excludes them from its BOM while retaining them
on the board. The selected footprint must separately carry KiCad's
`exclude_from_bom` and `exclude_from_pos_files` attributes for PCB export.

### Voltage limits

Package pins and device pads carry two independent voltage ranges:

```copper
pad VDD {
    domains = "power"; directions = "input";
    voltage_min = 1.0V;   voltage_max = 1.3V;    // operating range
    absolute_min = -0.3V; absolute_max = 1.5V;   // absolute-maximum ratings
}
pad AUX {
    domains = "power"; directions = "input";
    voltage_max = 1.3V;
    absolute_max = "VTERM+0.1V, 1.36V";          // relative, capped at 1.36 V
}
```

An absolute limit is either a voltage (`V` or `mV`) or a quoted
pad-relative limit `"PAD"`, `"PAD+OFFSET"` or `"PAD-OFFSET"`, for example
`"VTERM+0.1V"` or `"VDDIO - 300mV"`. A relative limit may add one fixed
voltage after a comma; the tighter of the two applies, so
`absolute_max = "VTERM+0.1V, 1.36V"` means `min(VTERM + 0.1 V, 1.36 V)` and
`absolute_min = "VSS-0.3V, -0.5V"` means `max(VSS - 0.3 V, -0.5 V)`. `PAD`
names another package pin of the same part or, for a device-backed part, a
device pad of the same device; a device pad's reference must name a pad of its
own device. Unknown references are rejected at compile time (`CMP122`),
malformed values with `CMP120`. Fixed absolute limits (including the fixed part
of a capped relative limit) must enclose the fixed operating limits, and
`absolute_min` may not exceed `absolute_max` (`CMP121`).

ERC compares the voltage of every declared supply with the limits of each pin
on its net:

| Supply voltage | Diagnostic |
| --- | --- |
| outside the absolute range | error `SUPPLY_VOLTAGE_ABSOLUTE_LOW` / `SUPPLY_VOLTAGE_ABSOLUTE_HIGH` |
| outside the operating range, inside a declared absolute bound on that side | warning `SUPPLY_VOLTAGE_LOW` / `SUPPLY_VOLTAGE_HIGH` |
| outside the operating range, no absolute bound on that side | error `SUPPLY_VOLTAGE_LOW` / `SUPPLY_VOLTAGE_HIGH` |

Parts that declare only `voltage_min`/`voltage_max` therefore keep their
operating range as a hard limit. A relative limit is evaluated per component:
the reference pin (or every package pin actively bonded to the reference
device pad in the component's modes) must sit on a net with a declared supply,
and the limit is that supply's voltage plus the offset. When it cannot be
resolved, because the reference is not on a declared supply, is connected to
supplies with different voltages, or names no pin or active pad, ERC reports
the error `VOLTAGE_LIMIT_UNRESOLVED`, a fixed cap still applies on its own, and
the operating bound on that side stays a hard limit. Limits are only evaluated
for pins on nets with a declared supply; signal nets carry no voltage in the
IR. When several active profiles of one package pin declare a range, the
package pin's own range wins, then the first bonded device pad's.
Power-domain `voltage_min`/`voltage_max` remain operating limits checked as
errors. The serialized IR carries the absolute range as
`profile.absolute_voltage` (`rating = "absolute"`); a relative bound is
`{"reference", "offset", "limit"}`.

Part and device provenance is optional. Definitions may use
`source_document`, `source_revision`, `source_location`, `source_url`, and
`source_checksum`; none is required to compile.

## Devices and peripheral selection

A `device` describes package-independent silicon capabilities. Device pads are
separate from package pins. Peripheral signals are typed, required by default,
and mux options map them to device pads:

```copper
device STM32G0B1 {
    vendor = "STMicroelectronics";

    power_domain VDDIO1 {
        supply_pads = "VDD";
        voltage_min = 1.7V;
        voltage_max = 3.6V;
    }

    pad VDD {
        domains = "power";
        directions = "input";
    }
    pad PB6 {
        domains = "digital";
        directions = "bidirectional";
        drive_modes = "push_pull,open_drain";
        power_domain = VDDIO1;
        unpowered = clamped;
        voltage_max = 3.6V;
    }
    pad PB7 {
        domains = "digital";
        directions = "bidirectional";
        drive_modes = "push_pull,open_drain";
        power_domain = VDDIO1;
        unpowered = clamped;
        voltage_max = 3.6V;
    }

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

A package-specific MCU part refers to its underlying device and declares its
physical pins plus explicit device-pad bonds:

```copper
part STM32G0B1CBT6 {
    category = "semiconductor.mcu";
    footprint = "LQFP-48";
    device = STM32G0B1;

    pin VDD { number = "24"; bond = VDD; }
    pin PB6 { number = "45"; bond = PB6; }
    pin PB7 { number = "46"; bond = PB7; }
}
```

`bond` is a comma-separated list when one package pin connects to multiple
device pads. A conditional entry uses `PAD@MODE=CHOICE`. Multiple package pins
may also name the same device pad. A device-backed package pin combines its
electrical profile with its active bonded pads, that is the bonds whose
condition holds and whose device pad's own `when` condition holds in the
component's effective modes (see below).

Devices may define functional units, differential groups, finite modes, and
regular routing rules:

```copper
unit A: std.opamp { INP = A_INP; INN = A_INN; OUT = A_OUT; }
group AIN0: differential_pair { positive = AIN0P; negative = AIN0N; }
mode_group PORT0 { choices = "LVCMOS,LVDS"; default = LVCMOS; }
pad_set GPIO_PSEL { pads = "P0_00,P0_01"; }
route UARTE0.TX { pad_set = GPIO_PSEL; selector = nrf_psel; }
```

Unit terminals may be used as net endpoints, for example `U1.A.OUT`. The
compiler resolves them to canonical package pins before ERC. Components select
non-default modes with `modes = "PORT0=LVDS"`. Conditions are comma-separated
equality selections; general boolean expressions are intentionally unsupported.

A component's effective modes are its explicit `modes` selections layered over
the `default` of every mode group it does not select. They govern every mode
condition: bond conditions (`PAD@MODE=CHOICE` and a pin-level `when`), pad
`when`, mux options, route rules and signal groups, in ERC, pin resolution,
power-state analysis, physical lowering and the KiCad backends alike. A device
can therefore bond its package pins for the default mode without each
component repeating `modes`:

```copper
mode_group PHY { choices = "DPHY,CPHY"; default = DPHY; }
group DA0: differential_pair { positive = DA0P; negative = DA0N; when = "PHY=DPHY"; }

// in the package part
pin L0 { number = "3"; bond = "DA0P@PHY=DPHY,TA0A@PHY=CPHY"; }
pin L1 { number = "4"; bond = "DA0N@PHY=DPHY,TA0B@PHY=CPHY"; }
```

`component U1: BRIDGE;` uses the D-PHY bonding and checks the `DA0` pair;
`component U1: BRIDGE { modes = "PHY=CPHY"; }` uses the C-PHY bonding instead.
A mode group without a default and without a selection has no active choice:
no condition on it holds and ERC reports `MODE_NOT_SELECTED`.

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

Selections are exclusive by default. For hardware modes that firmware switches
at runtime, add `usage = firmware_managed;` inside each relevant `configure`
block. This permits sharing of peripherals, pins, and resource settings between
those modes, while invalid muxes, missing signals, and electrical mismatches
remain errors. If no peripheral-level validation is wanted, omit `configure`
and treat the connection as ordinary GPIO.

## Power states

Boards may describe named steady-state rail scenarios:

```copper
power_state NORMAL {
    VBUS = on;
    V3V3 = on;
    GND = on;
}

power_state STANDBY {
    VBUS = on;
    V3V3 = off;
    GND = on;
}
```

Each name must refer to a declared supply and each value is `on`, `off`, or
`unknown`. Run `python -m copperscript power-check board.copper` to validate
these scenarios. The current analysis warns about possible back-power into an
off device domain; it is not transient or firmware simulation. Power states are
board-level declarations in v0.1.

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

Board-owned physical constraints can explicitly address a resolved descendant:

```copper
constraint fixed_placement(PWR/U1) { x = 12mm; y = 8mm; }
constraint fixed_placement(J1) { rotation = 90; side = front; }
```

Position (`x` and `y` together), rotation and side are independent locks.
Omitted properties remain free; explicit allowed-orientation rules still apply.
Conflicting imported and board-owned poses are errors. `/` is an instance path
separator for constraint targets; electrical nets continue to use module ports.

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

A footprint can be a direct KiCad `.kicad_mod` path relative to the board:

```copper
component R1: RESISTOR {
    footprint = "footprints/R_0402_1005Metric.kicad_mod";
}
```

It can also use KiCad's `Library:Footprint` identifier form. Bind the library
to a versioned provider in the consuming project's `copper.mod`:

```text
require github.com/KiCad/kicad-footprints 7ebfa6b23cc292a56f751b7b5f4a0e12eeef69dd
footprint-library Resistor_SMD github.com/KiCad/kicad-footprints/Resistor_SMD.pretty
```

`Resistor_SMD:R_0402_1005Metric` then resolves its exact footprint through the
managed cache and lock without `--footprint-root`. A part or component may also
name an exact `.kicad_mod` module asset URL, with or without `https://`:

```copper
footprint = "github.com/KiCad/kicad-footprints/Resistor_SMD.pretty/R_0402_1005Metric.kicad_mod";
```

Relative `.kicad_mod` paths authored in imported parts/modules resolve from the
declaring package and remain confined to its source module. Entry-board paths
retain their board-relative behavior. Local roots remain explicit development
overrides; a locked bound namespace always selects its managed provider.
`copper lock BOARD` prepares selected managed footprints as well as source
imports. See [footprint dependencies](footprint-dependencies.md) for complete
syntax, provenance and independent `--locked`/`--offline` behavior. Missing and ambiguous
references are errors. Imported numbered pads must exactly match the electrical
part's physical pin numbers; unnumbered non-plated mounting holes do not
participate in that comparison.

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

The checker verifies both bindings, compatible pin capabilities, and one pull-up
resistor from each signal to the named supply.

## Constraints

Constraints remain separate from electrical connectivity. The physicalizer
lowers the placement kinds below into typed physical IR, and `plan-layout`
enforces them. A target may be a component (`U1`) or, for distance and
alignment rules, a component pin (`U1.VDD`), which is resolved to its physical
pad.

```copper
constraint max_distance(C1, U1.VDD) {
    distance = 3mm;
    weight = 10;
}

constraint min_distance(U1, J1) {
    distance = 8mm;
}

constraint placement_region(U2) {
    name = "radio";
    x = 45mm;
    y = 2mm;
    width = 20mm;
    height = 15mm;
    side = front;
}

constraint keepout() {
    name = "antenna-clearance";
    x = 60mm;
    y = 0mm;
    width = 15mm;
    height = 8mm;
    side = both;
}

constraint fixed_placement(J1) {
    x = 2mm;
    y = 18mm;
    rotation = 90;
    side = front;
}

constraint allowed_orientations(U1) {
    values = "0,45,90,135,180,225,270,315";
}

constraint align(U1, U2) {
    axis = y;
    tolerance = 0.5mm;
}

constraint placement_group(U1, U2, C1) {
    name = "controller";
    anchor = U1;
    priority = 50;
}

constraint routing(USB_DP) {
    id = "usb.dp";
    mode = require;
    consumers = "critical_router,physical_drc";
    verifier = "DRC-DIFF";
    kind = differential;
    partner = USB_DM;
    width = 0.18mm;
    clearance = 0.15mm;
    pair_gap = 0.2mm;
    max_skew = 1mm;
    allowed_layers = "F.Cu,B.Cu";
    target_impedance_ohms = 90;
    require_return_vias = true;
    return_via_net = GND;
    maximum_return_via_distance = 2mm;
    impedance_evidence_digest = "<64 hexadecimal SHA-256 characters>";
}
```

Known constraint kinds are `max_distance`, `min_distance`,
`placement_region`, `fixed_placement`, `allowed_orientations`, `align`,
`placement_group`, `keepout`, `routing`, `copper_zone`, `via_in_pad`,
`length_match`, `hole_clearance`, and `note`. Coordinates and rectangle dimensions
are lengths in the physical board coordinate system; orientation values are
unitless degrees. The legalizer accepts any explicitly permitted angle,
including 45-degree increments; unconstrained components still default to
0, 90, 180, or 270 degrees. Use `allowed_orientations` when a diagonal
placement is acceptable for assembly and interface alignment. `note` remains
metadata and has no placement effect.

All constraints accept the ownership metadata `id`, `mode`, `consumers`, and
`verifier`. Modes are `require`, `target`, `prefer`, `assume`, and `external`.
Hard `require` and `external` constraints must have a complete consumer/verifier
pair when either is stated, and the normalized coverage gate supplies the known
placement/routing defaults when neither is stated. Routing parameters lower to
`NetRoutingRule`; supported values include `kind`, `priority`, `width`,
`clearance`, `allowed_layers`, `max_vias`, `max_length`, `partner`, `pair_gap`,
`max_skew`, `topology`, `target_impedance_ohms`,
`maximum_uncoupled_length`, `maximum_stub_length`,
`tuning_amplitude_limit`, `require_return_vias`, `return_via_net`,
`maximum_return_via_distance`, `return_via_policy`, `shared_reference_layer`,
`impedance_evidence_digest`, `target_single_ended_ohms`,
`impedance_tolerance_percent`, `layer_group`, `breakout_length`,
`breakout_width`, `breakout_gap`, `breakout_clearance`, `tuning_style`,
`tuning_spacing` and `tuning_group` (see
[signal-integrity routing intent](#signal-integrity-routing-intent)).

USB/differential profiles need not be top-layer-only. For example,
`allowed_layers = "F.Cu,In2.Cu"; max_vias = 2;` permits matched terminal
transitions and another-layer paired middle route. `max_vias` counts signal
vias **per member**, not across both nets; return-net vias are reported
separately. Required return vias must satisfy `return_via_net` and
`maximum_return_via_distance` at each transition by default
(`return_via_policy = "always"`). For an explicitly declared common reference,
opt in with:

```copper
require_return_vias = true;
return_via_net = "GND";
maximum_return_via_distance = 2mm;
return_via_policy = "reference_change";
shared_reference_layer = "In1.Cu";
```

Both paired rules must agree. Only actual signal layers adjacent to that same
unambiguous dedicated return plane can omit a stitching via; unknown or
different references still require one. The owning router checks the entire
pair against declared plane bounds/voids, not just the transition sites.
`shared_reference_layer` requires `reference_change` and an available layer
which is not a permitted signal layer. Native filled-plane validation remains
required; this policy is not impedance or return-path qualification.
Dedicated plane layers
remain unavailable to foreign signal tracks. See
[paired layer transitions](paired-layer-transitions.md) for the bounded
implementation and its impedance/return-path limitations.

The routing property `reserve_corridor` (boolean, default `false`) reserves
the space between a differential pair's terminal lands during placement:

```copper
constraint routing(CSI_D0P) {
    kind = differential;
    partner = CSI_D0N;
    width = 0.1mm;
    pair_gap = 0.1mm;
    clearance = 0.1mm;
    reserve_corridor = true;
}
```

It must be set on a profile that declares `partner`, and it applies to the
pair when either member sets it. The corridor is derived only when every
component with a land on either pair net has a `fixed_placement` with both
position and rotation. It is the convex hull of all terminal lands of both
nets, expanded on every side by the pair's `width` + `pair_gap` + clearance.
Clearance is the larger of the rule's `clearance` and the board minimum
clearance. A missing `width` falls back to the board default track width,
and a missing `pair_gap` to the board minimum clearance. The corridor applies
on the terminal components' side, or on both sides when they are on
different sides. Placement treats it like a hand-drawn `keepout()` for every
component except the pair's own terminal components. Tracks may still pass
through it. Component legality checks enforce it, including the placement
editor, placement feedback and the DRC placement check. If a terminal
component is movable, or a net has fewer than two lands, the corridor is not
derived. `plan-layout` then reports a `CORRIDOR_NOT_RESERVED` warning with
the reason. A fixed component that is not one of the pair's terminals and
overlaps the corridor is an error that names the corridor and the component.
`plan-layout --report` lists each derived corridor under
`reserved_corridors`, sorted by net pair, with its nets, side, margin,
terminal components and polygon in nanometres. Skipped requests are listed
under `skipped_corridors`.

A provisional plane can be declared separately from electrical connectivity:

```copper
constraint copper_zone(GND) {
    id = "ground-return-plane";
    layers = "In1.Cu";
    inset = 0.5mm;
    pad_connection = solid;
}
```

`copper_zone` targets exactly one existing net and lowers to an unfilled
physical zone inside the declared board outline. `layers` is required;
`inset`, `clearance`, `minimum_width`, and `pad_connection` are optional. The
`island_policy` optionally selects `remove_all`, `keep_all`, or
`remove_below_area` (the existing default, with a 10 mm² threshold). Use
`remove_all` to discard disconnected fill rather than waive native warnings.
The selected layers must exist in the chosen physical stackup. A zone declaration
does not establish electrical connectivity or fabrication readiness: its
actual fill and connected copper require later verification.

A zone may instead have an explicit regional boundary. Choose exactly one:
`x`, `y`, `width`, `height` (all typed lengths); `region` referencing a named
`placement_region`; or `polygon_mm`, a semicolon-separated string of `x,y`
coordinates in millimeters. Named regions may be declared after the zone.
`priority` is an optional nonnegative integer for overlapping zone intent.

```copper
constraint copper_zone(V3V3) {
    id = "logic-power";
    layers = "In2.Cu";
    polygon_mm = "4,4; 35,4; 35,28; 4,28";
    priority = 1;
    pad_connection = solid;
}
```

The polygon must be simple and wholly inside the board. A completely enclosed
board cutout is retained; a partially intersecting cutout is rejected because
general polygon clipping is not implemented. Nonzero `inset` currently requires
a rectangular or circular boundary. Boundary modes cannot be combined. A
regional pour does not automatically distribute power to distant pads: deferred
zone terminals must still reach prospective contacts, and native refill must
prove that all contacts actually join. Regions never authorize split reference
planes or waive current/impedance review.

Set `reserve_routing = true` to reserve a zone's polygon for its own net's
tracks on its declared layers. This boolean defaults to `false`. Foreign
tracks must remain outside the region by their half-width plus the largest
applicable board, zone or net clearance. Other layers and foreign through-vias
retain their normal routing rules; native fill supplies via antipads. The
reservation is net-aware compiler intent, not a blanket KiCad keepout, and it
does not override hard-macro ownership. Distinct-net reservations on a common
layer may neither overlap nor touch, regardless of zone priority.
Dense general-net pads on reserved regions retain their normal joint fanout
exits before plane stitching. By default, stitching first tries an existing
nearby via, including a bounded, legal track to its own zone, before adding a
new drill.
These contacts survive ordinary-routing cleanup and still require native fill
verification. Unreserved plane nets continue directly to plane stitching,
without new joint fanout.

The same region checks apply to terminal access, detailed routing, repair,
smoothing and final physical DRC. Global routing excludes unavailable planar
guide resources for each foreign net; it does not reserve the whole layer.
Enabled reservations enter physical/global fingerprints and the editor scene.
For a surface power pour, use `--stitch-surface-zones` to enable contacts from
the other side. Neither reservations nor contacts establish filled connectivity:
native refill must still prove each consumer joins its source.

Physical placement derives overlapping power-domain membership from nonzero
supply nets and active power-pin profiles. Ground is excluded. Distinct nets
remain distinct even at the same voltage. `--power-domain-weight` on
`plan-layout`, `route-global` and `route-board` sets the soft source-to-load
distribution attraction (default `0.25`, range `0..1`, `0` disables it).
Source-free rails use a consumer centroid; consumers are normalized by
component, not supply-pin count. Fixed/relative/macro constraints and escape
space remain stronger objectives. Multi-domain parts are not forced to a
geometric midpoint. Layout reports expose `power_domain_penalty_nm` and
`weighted_wire_length_nm`; raw HPWL remains separately reported.

With fanout enabled, package preflight now reserves declared plane contacts
before ordinary area routing by default. Critical routes and plane contacts
are revalidated together with ordinary escape-pattern alternatives. Use
`--no-early-plane-stitch` for an explicit late-contact comparison, or
`--early-plane-pad` to select contacts. These options never certify zone fill.
Use `--prefer-local-ground` to apply the local-contact preference to every GND
pad, or repeat `--prefer-local-ground-pad REF.PAD` to select individual pads.
The preference applies to eligible SMD pads outside protected hard macros;
through-hole pads already provide a plated contact. It compares a nearby legal
GND via with reuse of an existing contact, using surface escape length and a
small penalty for another drill or shared primary contact. A local via is not
guaranteed when no legal site exists. Native refill still has to prove the GND
plane connection.

A pad-scoped fabrication permission is separate from connectivity:

```copper
constraint via_in_pad(BT1.NEG) { process = "filled-capped"; }
```

The target is one semantic component pin, resolved through the part/device pin
mapping to `PadViaInPadRule` in physical IR. This initial implementation supports
only connected SMD GND lands, an inner GND zone, and the six-layer JLCPCB profile.
The only process is `filled-capped` (also the default); unsupported processes,
unknown targets/parameters, duplicate permissions and incompatible profiles
fail closed. The plane-contact stage may use a checked centred 0.30/0.20 mm
through-via as a fallback when off-pad escape fails. The annulus must fit the
land and may not contact other lands, including same-net lands. Existing copper,
all-layer keepouts, board edges, foreign-copper and drill spacing remain checked.
This permits via-in-pad; it does not require insertion if an ordinary contact
already exists. Pad size never enables it implicitly. Signal fanout and all
unselected pads retain the no-pad-overlap default. The route report records
the filled/capped fabrication requirement; geometry alone does not order that
manufacturing process or prove filled-plane continuity.

`rows` and `columns` (positive integers, both or neither) turn the permission
into a required array, such as the thermal/ground vias of an exposed pad:

```copper
constraint via_in_pad(U1.EP) { process = "filled-capped"; rows = 3; columns = 3; }
```

The array is a centred grid of the same 0.30/0.20 mm filled/capped
through-vias inside the pad's single SMD land; columns run along the land's
own x axis, rows along its y axis. The default pitch puts the vias at the
centres of equal cells, using the smaller cell (rounded down to 0.01 mm) for
both axes, so the outer annuli keep half a cell of margin; a `pitch` length
overrides it. A pitch below the drill spacing (drill plus the board's
`minimum_hole_clearance`) or a site outside the land is rejected when the
board is lowered. The array is fixed copper, placed with hard-macro copper
before package access, critical and ordinary routing, and it is the pad's
plane contact: stitching adds no other. Every site must lie in the inner GND
zone and pass the fallback via's checks, including drill spacing between the
array's own vias; one failed site rejects the whole array, naming the pad and
the site. Route and preflight reports list each array (`via_in_pad_arrays`:
count, pitch, positions), and `via_in_pad_count` and the fabrication
requirements include its vias.

A component-scoped drill clearance is a documented exception to the board's
`minimum_hole_clearance` (mechanical `rules`, default 0.25 mm), for a vendor
land pattern that puts pads closer to the part's own non-plated holes:

```copper
constraint hole_clearance(J1) {
    clearance = 0.19mm;
    reason = "Vendor land pattern: GND pads 0.194 mm from the receptacle's own plastic locating pegs";
}
```

The target is exactly one component, local (`J1`) or a module descendant
(`OUT_PORT/J`). `clearance` (a positive length, at most the board
`minimum_hole_clearance`) and `reason` (a nonempty string) are required;
there are no other parameters besides the ownership metadata. It lowers to
`ComponentHoleClearance(reference, clearance_nm, reason)` in
`PhysicalBoard.component_hole_clearances`, sorted by reference and bound by
the physical signoff digest.

The scope is deliberately narrow. The value applies only between the copper
pads of that component's footprint and the non-plated holes of the same
footprint instance. Every other pair keeps `minimum_hole_clearance`: tracks
and vias of any net (the component's own nets included) near those holes,
other components' pads near them, and the component's pads near any other
hole. Board-owned mechanical `hole`s and slots are never covered. Routing is
unchanged: the router only adds tracks and vias, which keep the board value.

Physical DRC checks each of these pad/hole pairs against the scoped value. A
pad still below it is the usual `DRC-HOLE-CLEARANCE`; the message says it was
checked against the component's scoped `hole_clearance` and the finding
carries `required_nm` (the scoped value) and `measured_nm`. A pair that passes
only because of the scoped value is recorded in
`PhysicalDrcReport.hole_clearance_relaxations` and under
`hole_clearance_relaxations` in the report JSON: the component, the objects
(pad and hole), the pad's net, the board value it does not meet
(`required_nm`), the scoped value it meets (`relaxed_nm`), the measured
pad-copper-to-drill spacing (`measured_nm`) and the reason. The coverage
entry `component_hole_clearance` appears when any scoped rule exists. Boards
without the constraint give byte-identical reports, digests and KiCad exports.

The KiCad export keeps the board value in Board Setup and writes
`<board>.kicad_dru` beside the same-stem `.kicad_pro`, one custom rule per
component:

```
(version 1)
# Generated by CopperScript from hole_clearance constraints; do not edit.
# J1: Vendor land pattern: GND pads 0.194 mm from the receptacle's own plastic locating pegs
(rule "J1 hole clearance"
  (constraint hole_clearance (min 0.19mm))
  (condition "A.memberOfFootprint('J1') && B.memberOfFootprint('J1') && (A.Pad_Type == 'NPTH, mechanical' || B.Pad_Type == 'NPTH, mechanical')"))
```

Checked with KiCad 10 `kicad-cli pcb drc`: a matching custom
`hole_clearance` rule replaces the Board Setup value, also below it, so Board
Setup is not a floor and stays at the board value. Tracks, vias, zones and
other footprints (including exported board holes) are never members of the
footprint, so they keep the board value. The condition uses the exported
KiCad reference (`OUT_PORT/J` becomes `OUT_PORT_J`), which must be unique on
the board. KiCad silently ignores a malformed rules file, so keep the
generated file unedited beside the project. An export without scoped rules
removes a stale generated rules file of the same stem; a hand-written one is
kept. `export-manufacturing` copies the file with the project. Proxy
footprints (`--allow-proxy-footprints`) have no holes, so the constraint is
not lowered there.

Errors carry the constraint's source location. `CMP112` is a malformed
declaration (not exactly one target, a pin target, a missing or invalid
`clearance` or `reason`, an unknown parameter), `CMP113` a target that is not
a component and `CMP114` a second `hole_clearance` for one component,
including one declared inside its module. These are compile errors.
`CMP115` (the component has no footprint with non-plated holes) and `CMP116`
(`clearance` exceeds the board `minimum_hole_clearance`) need the resolved
footprint and rules, so physicalization reports them.

Package escape sampling is configurable on `route-board`: `--fanout-step-mm`
defaults to 0.5 mm and `--fanout-refinement-step-mm` defaults to 0.1 mm. Empty
coarse domains and conflicting selected escapes receive bounded finer radial
and, when enabled, two-leg alternatives against the same immutable input.
Easy pins keep their coarse choices. Refinement must be no coarser than the
initial step; equal steps disable the finer pass. Candidate counts and both
steps are reported. This remains a bounded search, not proof that an empty
domain is physically unroutable.

A pin of an ordinary net that may not change layer (`allowed_layers` with one
layer, or `max_vias = 0`) gets no via. It escapes on its own pad layer to
beyond the package collar, and the router continues from there on that layer
(CS-172); the route report lists it under `fanout.surface_escaped_pads`.

### Critical routing and qualification

`routing` profiles, not net-name heuristics, select critical geometry. The
full-vertical example explicitly declares both USB pairs on either side of its
common-mode choke. Pair members are accepted together only after exact native
geometry and connectivity checks. A rejected candidate contributes no locked
tracks/vias and does not fall back to independent D+/D- routing. Reports state
the routing `strategy` and whether a materialized candidate was rejected.
Measured candidate lengths may remain in a rejection report for diagnosis;
accepted track/via counts are zero.

Aligned terminals use a midpoint channel with 45-degree tapers. Terminals are
aligned when, at each end, the two lands sit side by side across one straight
channel on the same layer, both ends share one midpoint and the members keep
their order. The land pitch may differ from the pair pitch (width + gap) at
either or both ends, for example 0.5 mm package lands to 0.4 mm connector
lands: each land then tapers at 45 degrees onto its lane, symmetrically about
the channel centre. Tapers are uncoupled length, so they count in
`uncoupled_lengths_nm` and against `maximum_uncoupled_length`. Native DRC and
connectivity still gate the channel. Other pair geometries (for example
midpoints that need a jog) depend on coarse-guide candidates and the joint
searches and can fail preflight; joint package-access search remains necessary. Single-ended critical nets can
use bounded exact search while earlier critical copper stays immutable.
General fanout/subset repair skips critical nets. Duplicate-land cleanup may
reuse an existing critical connection but leaves new bridges pending for the
owning critical router rather than altering pair skew or adding RF stubs.

Critical groups are routed one at a time by priority, kind and net name, and
accepted copper is immutable. A *bundle* is two or more differential (or CAN)
groups of the same kind and priority whose members each connect the same two
components, for example the data and clock lanes between a package and a
connector. A bundle is routed outermost first along the terminal row instead
of by name: each group's depth is its distance in ranks from the nearer end of
the row at both components (summed), with the lower row position at the
alphabetically first component, then the net names, breaking ties. The bundle
keeps the slots its groups held in the default order, so other groups keep
their place. A group with a different priority is not part of the bundle.

When a bundle group still fails, and a rejected candidate's first-failing gate
is a spacing finding (`DRC-CLEARANCE`, `DRC-SHORT`, `DRC-HOLE-CLEARANCE` or
`DRC-DRILL-SPACING`) against copper of an already accepted group of the same
bundle, the router tries a bounded repair: it removes that group's copper,
routes the failed group first, then routes the removed group again. Both use
the same coarse candidate, joint searches, state budget and atomic validation
as the main pass. Both are kept only when both are accepted; otherwise the
state is restored exactly. Candidates are the blocking groups in acceptance
order, one per attempt, at most 4 attempts per bundle (the
`bundle_repair_limit` argument of `route_critical_nets`; `BUNDLE_REPAIR_LIMIT`
in `pcbir.critical_bundles`). Spacing findings come from the coarse candidate
and from exact candidates. When the failed group has no spacing finding at all
(its exact searches found no candidate, as for a middle pair squeezed between
pairs routed before it), the candidates are instead its two physically nearest
accepted groups of the bundle, by the distance between the pairs' mean land
positions at both components. A repaired group keeps
its place in the report and is committed after the failed group; progress
reports it again as `started`/`finished`. The critical report's `bundles` list
gives, per bundle, the `components`, `kind`, `priority`, the `order` used,
the default `name_order`, `repair_limit`, `repairs_attempted`,
`repairs_accepted` and each repair (`failed`, `ripped_up`, `accepted`,
`reason`). `python -m pcbir.critical_preflight` prints one line per bundle.

When the pair order of a bundle is inverted between its two components, as on
a mirrored connector pinout, some pairs must cross others. Each pair's rank at
a component is read by angle around the component's courtyard centre, starting
on the side that faces away from the other component, in one rotational sense
for both components; two pairs must cross when their order differs at the two
ends. The surface keeps a set of pairs with no crossing among them: as many
pairs that cannot change layer as possible, then as many pairs as possible
(the inner pairs, later in the bundle order, on a tie). Every other pair gets
one planned paired layer swap: a layer both members allow (and every member of
their `layer_group`), other than the terminal surface and reachable by a via;
layers next to a declared `copper_zone` come first; never the layer of another
moving pair it crosses. Moving pairs are routed first in the bundle, using
only the paired-via search on that layer: matched vias at both ends, return
vias within `maximum_return_via_distance` or a declared shared reference, no
transition via inside a breakout region, transitions inside the pair's
reserved corridor first. They skip the coarse candidate and the surface
search, so each member gets exactly two vias; every candidate passes the same
profile, plane and native DRC gates, and bundle repair keeps the plan. A
member whose `max_vias` is below 2, or a pair with no crossing layer left, is
reported impossible and fails without copper. A bundle with crossings has a
`crossings` list: per pair its `group`, the pairs it `crosses`, `surface`,
`layer`, `reference_planes`, `status` (`routed`, `failed` or `impossible`),
`reason` and `transitions` (`component`, each member's via centre in `vias`,
`reference` = `return_vias`, `shared_reference` or `not_required`,
`return_vias` and `shared_reference_layer`). The preflight prints one line per
crossing. Bundles without crossings route and report exactly as before
(`route_critical_nets(plan_crossings=False)` turns planning off).

Each group in the critical report records why exact pair candidates were
rejected. `rejections` counts the first-failing gate of every rejected joint
search candidate, for example `{"DRC-CLEARANCE": 4, "skew": 2}`, and
`rejection_examples` keeps at most three messages, one per gate first. The
profile gates `length_budget`, `via_budget`, `via_pairing`, `return_via`,
`skew` and `uncoupled_length` are evaluated first, then `plane_reservation`,
then native DRC, whose codes are reported verbatim. `connectivity` (the pair is
left open) counts only when no other gate failed. The counts are kept when a later candidate is accepted, appear
in the "none accepted" diagnostic, and are streamed in `route-board --progress`
`critical_group` events and preflight checkpoints.

The coarse global-guide candidate places no layer transitions for a pair with
`max_vias = 0` or a single common allowed layer. Otherwise every guide
transition (both signal vias and any return via) must clear all lands, drilled
holes, via keep-outs and the board edge, or the coarse candidate is rejected
without copper and the exact paired searches decide. When `max_skew` and
`tuning_amplitude_limit` are both set, skew beyond the limit is compensated on
the shorter member: bumps no taller than the amplitude limit, 3 × width wide
and 3 × width from each other and from the segment ends, on the axis-aligned
segments nearest the terminal or bend where the length difference arises,
bulging away from the partner, with 45° corners (below; the chamfer leg is at
most the member's narrowest width). At most eight bumps are used; when they do
not fit, the geometry is unchanged and the skew gate reports it. On a board
with breakout regions the compensation aims 32 nm inside `max_skew`, because a
45° piece re-cut at a region boundary may round a nanometre differently. Tuned
candidates pass the same atomic budget and native DRC gates.

Tuning bumps have no 90° corners (D-PHY plan R9): each keeps its perpendicular
legs, and every corner is a 45° chamfer. For a bump of height h on track width
w, the member inside a turn gets chamfer leg c = min(w, ⌊h/4⌋, ⌊(h − d)/2⌋)
and the other member of a pair c + d, where d = ⌊(2 − √2) × lane spacing⌋ (0
for a single net). These are the offset chamfers of a coupled 45° bend, so
parallel pieces of the two lanes are never closer than the lane spacing and at
most 2 nm farther apart. Each member of a pair is inside the turn at two
corners of a bump and outside at the other two, so both gain exactly the same
length, 2h − 4(2c + d) plus four rounded 45° pieces: about
2h − 2(2 − √2)(2c + d), or 2h − 4(2 − √2)c for a single net. A bump too low
for c ≥ 1 has one 45° ramp per side. The tuners count this loss when choosing
bump counts and heights, so a pair needs more or taller bumps than with square
corners (a 0.2/0.2 mm pair gains about 63% of 2h at 1 mm height).

Once every critical group is accepted, each `length_match` group whose skew
exceeds its `max_skew` is tuned, group by group in declaration order. Tuning
works on accepted critical groups (*units*). A differential pair is one unit:
both members get the same bumps, bent together at the pair's lane spacing, so
each gains exactly the same length and the pair's own skew and `max_skew` are
unaffected. A single-ended critical net is tuned alone.
Each unit is first lengthened so that its longest group member matches the
group's longest member; when that does not fit or does not validate, by the
least length that brings its shortest member within `max_skew` (32 nm inside
it on a board with breakout regions). Bumps are no taller than the unit's
`tuning_amplitude_limit` (the smaller of a pair); a unit without one is not
tuned. They sit on straight axis-aligned runs (for a
pair, where both members are parallel at the pair spacing), at least
3 × width from either end of the run and 3 × width apart, and are 3 × width
wide (the outer member of a pair 3 × width + 2 × spacing), on either side of
the run. Placement rule: the room of each slot is the tallest bump, up to the
amplitude limit, whose swept area (the convex outline of the chamfered bump)
clears all other copper, lands, holes, keep-outs and the board edge by the
applicable clearance; slots with the most room are used first, then those
farthest from the unit's terminals, then by position. The fewest bumps that
provide the length are used, at most 16 per unit (`MATCH_TUNING_BUMP_LIMIT` in
`pcbir.critical_tuning`), with levelled heights. Each tuned unit passes the same profile gates (length and via
budgets, pair skew and `maximum_uncoupled_length`) and atomic native-DRC
validation against all other copper as a routed candidate. In the coupled-length
measure, a pair bump adds up to about 3.3 lane spacings of uncoupled length per
member, so a tight `maximum_uncoupled_length` can rule tuning out.
A group's copper changes only when every unit is accepted and the group ends
within `max_skew`. Otherwise its copper and results are kept exactly as
routed, the group is reported `failed` with the reason, the critical stage
fails, and final verification reports `DRC-LENGTH-MATCH`. Groups within their
limit are untouched. Groups with a member that is not connected after critical
routing (for example an ordinary net) are `incomplete` and left to final
verification. Candidate validation during critical routing does not reject a
candidate for `DRC-LENGTH-MATCH`, because group skew depends on every member
and on this tuning pass. The critical report's `match_tuning` list gives, per
group, `id`, `max_skew_nm`, `status` (`within_limit`, `tuned`, `failed` or
`incomplete`), `skew_before_nm`, `skew_after_nm`, the `members` with
`length_before_nm`, `length_after_nm` and `added_length_nm`, the number of
`bumps` and the `reason`. The `nets` entries and `lanes` table report the
tuned copper. `python -m pcbir.critical_preflight` prints one line per group.

A unit whose rules declare `tuning_style = "serpentine"` (D-PHY plan R10)
snakes about its original line instead of bumping off it. Its pieces are first
joined into straight lines where collinear pieces of equal width meet end to
end with nothing else of the net at the joint (for example where a line was
cut at a breakout-region boundary). On a straight line, legs cross the line
perpendicular to it at a pitch of the lane width (both members and their gap)
plus the leg gap (`tuning_spacing`), on a grid centred in the run
3 × width from its ends; the *tops* between adjacent legs alternate sides,
each no higher than `tuning_amplitude_limit` off the line and at least
d + 2 nm (4 nm for a single net), so every corner has a 45° chamfer. A top of
height a adds 2a. Each leg has one corner at each end, and every member is
inside one and outside the other, so with one chamfer leg c for the whole
unit (R9's rule on every leg, and small enough to leave a straight piece on
every top) both members of a pair gain exactly 2 × (sum of heights) −
(n + 1)(f(c) + f(c + d)) from n tops, where f(k) = 2k − round(k√2), and the
pair's skew is unchanged. A top's room on its side is measured like a bump's
over its two legs, with the outline taken at the least top and largest foot
chamfer. Each line takes at most one serpentine: a window of two or more
consecutive tops with room on their sides; windows with the most capacity
come first, and the last one is trimmed to the fewest tops that add the
length. Lines without a serpentine keep one-sided bumps, and a unit whose
serpentines cannot provide the length falls back to bumps alone (also when the
length is too small for tops of that height). At most 32 legs are used per
unit (`MATCH_TUNING_LEG_LIMIT`, a bump counting two); heights are levelled
across tops and bumps, and c is lowered until it suits every leg and makes the
added length exact. Adjacent legs couple, so a serpentine's delay is slightly
shorter than its length suggests; reports keep the geometric length. Units
without the property tune exactly as described above.

Bundle pairs (same two components, kind and priority) of a `length_match`
group are also tuned while the bundle routes (plan R12). Each time one of
them is accepted, every accepted pair of the bundle that is short of the
group's longest accepted member by more than `max_skew` is tuned toward it,
in acceptance order, with the same targets, geometry, gates and atomic
validation as above. A pair is thus tuned while its inner neighbours are
still unrouted, and later pairs route around its copper; the pairs routed
before the longest one are topped up as soon as it is accepted. A pair that
cannot be tuned keeps its copper and is retried only after it is re-routed or
the target grows. When a later bundle pair then finds no candidate, the
tuning added while routing is removed from the bundle and that pair is
searched once more, before any rip-up repair. The pass after all groups then
tops up what is left. The tuning target is always a measured, accepted
length: global guides are too coarse and land-to-land distances too
optimistic to predict a group's longest member, and a pair tuned past it
cannot be shortened. Groups within their limit, and bundles without a
`length_match` group, route exactly as before. When any step used a
serpentine or ran while routing, the `match_tuning` entry also has `legs` and
`units`: every step's `nets`, `stage` (`routing` or `final`), `style`
(`bumps` or `serpentine`), `added_length_nm`, `bumps`, `legs` and
`amplitudes_nm`, and its lengths and skew before tuning are those of the
copper as routed. The preflight line then lists each step and its style.

Units whose rules name the same `tuning_group` (plan R14) are bent together,
like a DDR byte lane, where each alone is hemmed in by its neighbours. Their
collinear pieces are joined into lines; a *run* is a stretch along one axis
and layer where every net of the group has one straight line, each unit's
lanes are adjacent (a pair's at its lane spacing), and the lanes span W across
it. Every lane rises the same height off its own line through every bump or
serpentine top, so every lane gains the same length; on each leg the lanes
turn one after another, each keeping its distance to the next, so for a
one-sided bump the outermost lane's interval along the run is the innermost
lane's plus 2 × W. Corners are R9's 45° chamfers for N lanes: at each end of a
leg the lane inside the turn takes chamfer leg c and each lane further out c
plus ⌊(2 − √2) × gap⌋ per gap crossed, lowered by a few nanometres where
needed so that every lane's rounded length gain is exactly equal (a pair's
skew is unchanged; a piece re-cut at a breakout-region boundary may still
round a nanometre differently). A leg must hold both corners, so a group top
is at least D + 2 nm high (D the sum of those offsets across the span; about
0.59 × W), and each leg costs every lane the same length. Room is measured
like a unit's, on the outline of the outermost lane's chamfered top together
with the innermost lane's feet, against all other copper; bumps keep the
innermost lane 3 × width wide and 3 × width from each other and from the run's
ends, on the densest grid, centred in the run or shifted along it in steps of
at least 3 × width (most room first, then farthest from the terminals). Where
every member declares `tuning_style = "serpentine"`, tops lie at a pitch of W
+ width + `tuning_spacing` and alternate sides where both sides have room,
else the group takes one-sided bumps. Heights are levelled, no taller than the
smallest `tuning_amplitude_limit` of the members. The group adds what the unit
nearest the group's longest member needs to match it, so no unit overshoots,
else the least that brings one unit within `max_skew`; each unit's remainder
is then topped up on its own as above. A group is tuned as soon as all its
units are accepted in one bundle (until then its pairs wait) and again in the
final pass, each step with the profile gates of every unit and one atomic
native-DRC validation; later pairs route around it. When a unit of a group
tuned while routing is re-routed, the others return to their copper as routed
and the group is tuned again. A group that does not fit leaves the copper
unchanged, and its units are tuned one by one instead. Each group step is a
`units` entry with `group` and `lanes` (the nets across the run, in order),
and the `match_tuning` entry gains `tuning_groups`: per group its `name`,
`nets`, `status` (`tuned`, `failed`, `within_limit` or `incomplete`) and
`reason`, which for a group that does not fit gives the longest run against
the run a group bump needs, the amplitude against the least group height, or
the room on each side. The preflight line names each group step and every
group that was not tuned.

The critical report also has a `lanes` table with one row per critical net:
`routed_length_nm`, `layer_lengths_nm`, `layers` in stack-up order and the net's
own `via_count` (return vias count for their reference net). When the board
declares a physical stack-up, `estimated_delay_ps` is a quasi-TEM screening
estimate (`delay_evidence_grade: "screening"`), never sign-off. Outer layers use
the Hammerstad-Jensen microstrip effective permittivity of the inward dielectric
and the net's dominant width. Inner layers use the thickness-weighted
permittivity of both adjacent dielectrics. Via barrels are excluded. Without a
stack-up the delay is `null` with `delay_reason: "no stack-up declared"`.
`python -m pcbir.critical_preflight` also prints one line per lane.

The `route-board` report's `critical_lane_review` section reviews the exported
copper of the critical nets against common layout guidance; the critical
preflight report has the same section for its critical copper. It changes no
copper. `nets` has one entry per critical net: its `lanes` row, `partner`,
`breakout` (the rule declares breakout properties), `spacing`, coupling and
`bends`. `spacing` is the least edge-to-edge distance from the net's tracks and
vias to another net's tracks, vias and pads on a shared layer, for its copper
`inside_breakout` (in one of its own breakout regions) and `outside_breakout`,
each against `critical` neighbours (a non-general rule) and `signal`
neighbours (all other nets). A minimum names the `neighbour`, `object`
(`track`, `via` or `pad`, with the `pad`), `layer` and `at_nm`, the nearest
point of the net's centreline; `null` means nothing within `search_radius_nm`
(1 mm). The pair partner, pads without a net and nets that own a `copper_zone`
(`ignored_zone_nets`) are ignored. For pair members, `coupled_length_nm` is the
track length outside the net's breakout regions that lies closer than
`coupling_threshold_nm` (2 × `pair_gap`), edge to edge, to copper of another
critical pair (`coupled_nets`). `bends` gives the `sharpest_degrees` direction
change where exactly two of the net's tracks meet on one layer, and each bend
over 45° (`sharp`). `pairs` gives each pair's `lengths_nm`, `skew_nm`,
`max_skew_nm` (the smaller member value) and `status` (`pass`, `fail`,
`no_limit` or `incomplete`); `match_groups` gives each `length_match` group's
member lengths, `skew_nm`, `max_skew_nm`, `status` and `tuning_status`. With
critical nets, both commands print one line with the outside-breakout minima
and the total coupled length:

```text
CRITICAL LANES: 5 nets; pairs=2, worst skew=0.800 mm, over max_skew=1; match groups=1, over max_skew=0; min spacing outside breakout: critical=0.300 mm (A_N to B_P), signal=0.507 mm (A_P to S); coupled length=20.763 mm; bends>45deg=2 (sharpest 90.0 deg)
```

`route-board --debug` and `python -m pcbir.critical_preflight --debug` print the
full traceback of an error before the usual one-line message. Without the flag,
output is unchanged.

An impedance target is not proof that the provisional width/gap meets it.
Missing stackup/field-solver evidence stays an explicit assumption. Nordic's
chip-side matching connection is not labelled a generic 50-ohm RF feed; its
multi-terminal antenna/matching network remains unqualified critical geometry.
The prototype does not yet certify matching-network topology or reference
layout, RF isolation, antenna keepouts, or the continuous return path.

### Signal-integrity routing intent

These routing properties describe D-PHY-style intent. All are optional; none
adds a limit the source does not declare.

```copper
constraint routing(CSI_SRC_CKP) {
    kind = differential; partner = CSI_SRC_CKN;
    width = 0.14mm; pair_gap = 0.26mm; clearance = 0.52mm;
    allowed_layers = "F.Cu"; max_vias = 0;
    target_impedance_ohms = 100;          // differential target for a pair
    target_single_ended_ohms = 50;        // each member's single-ended target
    impedance_tolerance_percent = 15;     // default 10
    layer_group = "csi-src";              // clock and data on the same layers
    breakout_length = 1.2mm;              // breakout region around terminal pads
    breakout_width = 0.12mm;
    breakout_gap = 0.2mm;
    breakout_clearance = 0.15mm;
}

constraint length_match(CSI_SRC_CKP, CSI_SRC_CKN, CSI_SRC_DA0P, CSI_SRC_DA0N) {
    id = "csi-src-lanes";
    max_skew = 1.5mm;
}
```

- `target_impedance_ohms` is the **differential** target on a `differential`
  or `can_bus` rule and the single-ended target on any other kind.
  `target_single_ended_ohms` (positive integer) adds the single-ended target
  of each pair member; on a single-ended rule it must agree with
  `target_impedance_ohms` when both are given.
- `impedance_tolerance_percent` (number, greater than 0 and below 100,
  default 10) applies to both targets and requires one of them.
- `layer_group` (nonempty name) groups nets that should route on the same
  layers, such as a D-PHY clock and its data lanes.
- `breakout_length`, `breakout_width`, `breakout_gap` and
  `breakout_clearance` (positive lengths) describe the pin-field breakout:
  within `breakout_length` of a terminal pad, the breakout values replace
  `width`, `pair_gap` and `clearance`. Breakout values may only relax the
  profile, never tighten it: `breakout_width` may not exceed `width` (or the
  board default track width), `breakout_gap` requires and may not exceed
  `pair_gap`, and `breakout_clearance` may not exceed `clearance` (or the
  board minimum clearance). Width and clearance may not go below the board
  minimum track width and clearance. `breakout_length` is required with any
  other breakout value and is meaningless alone. The router, its clearance
  checks and physical DRC apply them as described under
  [breakout regions](#breakout-regions).
- `tuning_style` is `"bumps"` (the default: one-sided bumps) or
  `"serpentine"` (S-shaped legs on both sides of the line, where both sides
  have room) for `length_match` tuning; both members of a pair must agree.
  `tuning_spacing` (a positive length, only with `"serpentine"`) is the least
  edge gap between adjacent legs, by default the larger of 3 × `width` and
  `clearance`. `tuning_amplitude_limit` stays the largest excursion on each
  side of the original line. Intra-pair skew compensation keeps one-sided
  bumps either way.
- `tuning_group` (nonempty name, critical kinds only) tunes adjacent units of
  one `length_match` group together: every lane bends through the same bumps
  or serpentine (see [critical routing](#critical-routing-and-qualification)).
  Every net with the property must belong to a `length_match` group, all nets
  of one tuning group to the same one, and both members of a pair must name
  the same tuning group.
- `length_match(NET, NET, ...)` lowers to `NetMatchGroup(id, nets,
  max_skew_nm)` on the physical board. It needs at least two distinct nets
  (not pins), each existing net may belong to only one group, and
  `max_skew` (a positive length) is the only parameter besides the ownership
  metadata. A group without `id` is named `length_match:<index>`. A member's
  length is its total routed track length (via barrels are not counted) and
  the group skew is the longest minus the shortest member. Once every member
  net is connected, a skew above `max_skew` is the hard DRC finding
  `DRC-LENGTH-MATCH`; before that the group is reported as incomplete. The
  critical router tunes critically routed members toward the group's longest
  member (see [critical routing](#critical-routing-and-qualification)).

Invalid values fail with source locations (`CMP110` for routing properties,
`CMP111` for `length_match`); semantic errors found while lowering (unknown
nets, a net in two groups, duplicate group ids) carry the constraint's
location, and `CMP117` with the routing constraint's location marks a
`tuning_group` outside any `length_match` group, one spanning two of them,
or pair members that disagree.

### Breakout regions

A breakout region lets a pair leave a fine-pitch pin field (a 0.4–0.5 mm
pitch package or connector) with small spacing while the channel beyond keeps
the full clearance, and lets a wide ordinary net reach such pins. The
routers, the routing clearance checks and physical DRC share one definition:

- **Region.** Every land of every pad on the net is a terminal land. Its
  region is every point within `breakout_length` of the land's copper
  outline: straight-line distance in plan view, the same on every copper
  layer. A point on the land is at distance 0, so the length counts from the
  land's edge, not its centre. The region is the land swept by
  `breakout_length`, so it is convex.
- **Inside.** Copper is inside when its whole centreline is inside one
  terminal land's region: both ends of a track segment, the centre of a via,
  every vertex of a pad outline. Each pad of the net is inside its own region.
  A segment that crosses the boundary is outside and keeps the normal values.
  The router cuts its tracks at the boundary instead: an octilinear segment is
  cut at its last whole-nanometre step inside, so the outside piece starts on
  the region's edge. Segments in other directions are not cut.
- **Values.** Inside, the net's copper uses `breakout_width` as its minimum
  width, `breakout_clearance` toward foreign copper and `breakout_gap` toward
  its partner. Outside, `width`, `clearance` and `pair_gap` apply to every
  foreign object. An undeclared breakout value leaves the normal one in
  force. Two objects of different nets keep the larger of the two nets'
  values and the board minimum clearance, each net's value taken at its own
  copper. A lane inside its region still keeps another net's normal
  clearance from that net's copper outside its region.
- **Pair members.** When either member of a pair declares breakout
  properties, the members are spaced by `pair_gap` (`breakout_gap` inside a
  region) instead of `clearance`. The channel clearance can then exceed the
  gap, for example `pair_gap = 0.26mm; clearance = 0.52mm`. A pair without
  breakout properties keeps `clearance` between its members as before. Each
  member contributes its own values, so declare the same breakout values on
  both. Matched transition vias of such a pair are spaced by the via size plus
  the larger of `pair_gap` and the clearance.
- **Router.** The critical pair candidates (coarse, aligned, joint, paired-via
  and shortcut proposals) and the single-ended guide candidate cut their
  tracks at the region boundary. Pieces inside carry `breakout_width`, a
  neck-down at the land; the rest keeps `width`. The coupled channel keeps its
  `width + pair_gap` pitch. Inside a region the members may come as close as
  `breakout_gap`, for example on a taper from narrow-pitch lands. Every
  routing clearance query (pad access, fan-out, the pair search, the detailed
  router) applies the region values to copper inside a region; a queried
  segment that crosses the boundary is outside. Global-routing demand still
  uses `clearance`.
- **Ordinary nets.** A `general` net with breakout properties necks down
  too, so a wide rail stays wide in the open field and narrows at
  fine-pitch pins. For example, `width = 0.4mm; breakout_width = 0.2mm;
  breakout_length = 1mm` lets a supply rail reach the middle pin of a
  0.4 mm-pitch row, where no 0.4 mm track fits between the neighbouring
  lands. The detailed router's search edges and line-of-sight shortcuts,
  pad access paths (including fan-out lead-ins and package escapes) and the
  global router's pin access are each checked as their pieces, cut at the
  region boundary: `breakout_width` and `breakout_clearance` inside, `width`
  and `clearance` outside. The router emits exactly those pieces. Straight
  runs merge only when the result stays outside every region or inside one,
  and a necked corner is chamfered only inside its region. After stub
  pruning and the release of unused escapes the changed copper is cut
  again, unless narrowing would split the net. Route smoothing checks and
  emits its new segments as pieces, with width changes as anchors. A necked
  piece is thus always inside a region, and copper outside always has
  `width`. Global-routing demand and the package-escape maze keep `width`.
  Nets without breakout properties route exactly as before.
- **DRC.** Track width, copper spacing (tracks, vias, pads) and zone-fill
  spacing apply the breakout values only to copper inside its region, and the
  pair gap between the members of a breakout pair. A violation is the usual
  `DRC-TRACK-WIDTH`, `DRC-CLEARANCE` or `DRC-SHORT`; between pair members the
  message names the `pair gap`. A check that passes only because of a breakout
  value is recorded in `PhysicalDrcReport.breakout_relaxations` and under
  `breakout_relaxations` in the report JSON. Each entry gives the check
  (`track_width`, `clearance` or `pair_gap`), objects, nets and layers; the
  normal value it does not meet (`required_nm`), the breakout value it meets
  (`relaxed_nm`) and the measured width or copper-to-copper spacing
  (`measured_nm`, `null` for zone fills); and, for each relaxed net, the
  terminal land whose region holds its copper and the breakout length. For
  example, a 0.2 mm gap from a lane to a neighbouring land lists the lane's
  net, `pad:U1.3:2` and `1000000`. The coverage entry `breakout_regions`
  appears when any rule declares breakout properties. Boards without breakout
  properties give byte-identical reports.

### Signal-integrity screening

`copper si-check BOARD [--locked] [--offline] [--layers N] [--fab-profile P]
[--footprint-root R] [--allow-proxy-footprints] [--json]` physicalizes the
board and runs the pre-route screening below, then summarises the
`length_match` groups and layer groups. It prints one line per estimate and
warning (or deterministic JSON with `--json`, schema
`copperscript-si-check/v0.1`) and exits 0 when it reports only warnings; a
compile or physicalization error exits 2. Every result is screening evidence
(`EvidenceGrade.SCREENING`), never sign-off: confirm impedance with the
fabricator's calculator or a field solver and record it in
`impedance_evidence_digest`.

- **Impedance (`SI-IMPEDANCE`, `SI-NO-STACKUP`).** For every rule with an
  impedance target, on every allowed layer (all copper layers when
  `allowed_layers` is empty), the declared stack-up gives the line geometry.
  Outer layers are microstrip over the adjacent dielectric: Hammerstad
  single-ended impedance and IPC-2141A edge coupling,
  `Zdiff = 2·Z0·(1 − 0.48·e^(−0.96·s/h))`. Inner layers are stripline between
  the dielectrics above and below: the IPC-2141A symmetric stripline, or the
  offset stripline (two symmetric lines in parallel) when the heights differ,
  with `Zdiff = 2·Z0·(1 − 0.347·e^(−2.9·s/b))`. Stripline permittivity is the
  thickness-weighted mean of both dielectrics. Width defaults to the board
  default track width. An estimate outside `target ± tolerance` is the
  warning `SI-IMPEDANCE`; a target without a declared stack-up is
  `SI-NO-STACKUP`. For example, JLCPCB's six-layer outer layer (0.10 mm
  prepreg, εr 4.1, 0.035 mm copper) with 0.14 mm width and 0.26 mm gap screens
  to about 99.7 Ω differential and 51.9 Ω single-ended.
- **Reference planes (`SI-NO-REFERENCE-PLANE`).** A `differential`, `clock`
  or `rf_feed` rule that allows a layer with no `copper_zone` on an adjacent
  copper layer is warned about. Whether the plane is continuous under the
  route is checked after a fill: `signal_integrity.return_path_report` runs
  `engineering.return_path_continuity` for every critical net of a board with
  normalized zone fills and returns the covered fraction and the uncovered
  track segments. The reference is the rule's `return_via_net`, otherwise
  every zone net.
- **Layer groups (`SI-LAYER-GROUP`, `SI-LAYER-GROUP-SPLIT`).** Before
  routing, members of one `layer_group` that allow different layer sets (or
  a group with a single member) are warned about. After routing,
  `signal_integrity.layer_group_report` lists each member's routed layers and
  main layer (the layer with the most track length) and warns when members'
  main runs differ.

### Native copper connectivity and package escape

Native physical DRC checks contact between actual track/pad/via shapes on
common copper layers, including interior T junctions and pad-edge contacts.
Vias connect only their physical spans; open drill holes are not solid copper.
Repeated pad numbers do not create virtual connections between separate lands.
Zone outlines still do not establish connectivity: use independent filled-zone
verification. A late package-escape feedback trial reserves the entire failing
package's same-zone-net pin group before ordinary routing, including neighbour
pins that already escaped; other packages and rails remain unaffected.

Full and subset routing candidates close duplicate lands before connectivity
scoring. The stitcher reuses existing exact copper paths across physical layers
and adds bounded surface bridges only between disconnected land groups. A net
with one logical pin can still have disconnected physical lands and fail DRC.
Failed searches and zone deferrals are not converted into routed signals.

The route report separates `surface_pending_pads` from effective `pending_pads`
and `zone_verified_pads`. A pending zone-pad reference can be resolved only by
fresh board/export-bound KiCad fill evidence with zero opens/islands and no
non-library violations. `zone_connectivity_verified` can be true while overall
`zone_fill_verified` and signoff remain false because of library findings.
Native open/route-completeness findings and fabrication gates remain unchanged.
Duplicate-land `added_track_count` counts final-stage additions;
`pipeline_added_track_count` records additions in the selected pipeline closure.

45-degree search/shortcut rays follow physical coordinates, not distorted
index-space diagonals. Narrow concave outline crossings and actual track
keepouts block them; off-ray blocked coordinates and via-only keepouts do not.
Width/clearance checks remain mandatory. Ray and search-local physical-span via
caches do not change geometry rules. Ray caches contain only static obstacles;
no copper-clearance result is reused after changing the clearance index.

### Routing search costs

`route-board --layer-preference-cost` and `--direction-preference-cost` are
soft physical-search rates per millimetre, not electrical properties or hard
layer restrictions. Global search uses half the selected detailed rates.
Both searches use a baseline distance rate of 10 cost units/mm; via and bend
events have separate fixed costs. Changing grid pitch or inserting pad-access
coordinates does not change the cost of an identical straight run. Physical
45-degree successors can cross inserted axis splits, subject to blocked-grid
and exact copper clearance checks. This does not guarantee that every route
will be octilinear: short pad accesses and orthogonal-budget fallbacks remain.

Opt-in `--layer-assignment-passes N` (default 0) relabels ordinary
via-bounded guide runs onto another permitted signal layer after global
negotiation when that avoids forced same-layer crossings (priced as one via
pair each) or, with `--local-demand-cost` (per mm at a full edge, counted in
`--pitch-mm` lanes), a crowded channel. Tile paths and via counts do not
change; planes, explicit layers and critical/paired/power guides are fixed.
Opt-in `--guide-escape-mm` lets the detailed corridor change layer next to a
reserved fixed-layer terminal. Accepted ordinary copper is straightened by
default; `--no-route-smoothing` keeps it as searched. Smoothing never moves
vias, reserved or immutable copper. With `--fanout`, a reserved boundary port is
one terminal of its pin, beside the pin's surface access and its launch via;
afterwards, escape copper the net does not need is removed transactionally
(CS-163; `--no-escape-terminals` keeps port-only terminals and all escape
copper). Opt-in `--package-destination-ports` prefers boundary ports facing
each pin's nearest terminal on another component.

## KiCad schematic export

Generate a KiCad 8 schematic after ERC succeeds:

```console
python -m copperscript export-kicad board.copper -o board.kicad_sch
```

The generated schematic is a derived artifact, not CopperScript source. The
current backend embeds generic symbols generated from each part definition,
uses named labels for connectivity, and owns all visual placement. It emits a
single flat sheet; hierarchical component paths are preserved as hidden
`CopperScriptPath` properties. `--no-check` permits diagnostic fixtures to be
exported despite ERC errors.

## Quantities

Quantities have no whitespace between their number and unit.

| Dimension | Units |
|---|---|
| Voltage | `V`, `mV` |
| Resistance | `ohm`, `kohm`, `Mohm` |
| Capacitance | `F`, `uF`, `nF`, `pF` |
| Inductance | `H`, `mH`, `uH`, `nH` |
| Length | `m`, `mm`, `um` |
| Current | `A`, `mA`, `uA` |
| Frequency | `Hz`, `kHz`, `MHz`, `GHz` |

Values are normalized to SI base units in the IR while retaining their source
display unit.

## Grammar sketch

```ebnf
document       = ("board" | "module" | "part" | "device" | "board_profile"), name, "{", item*, "}" ;
item           = library | package_import | port | pin | pad | power_domain
               | part_property | peripheral | mux | route | pad_set | resource
               | unit | signal_group | mode_group | device_property
               | configuration | module_instance | component | net | supply
               | power_state | interface | constraint | mechanical ;
mechanical     = "mechanical", "{", (mechanical_item | stackup)*, "}" ;
stackup        = "stackup", "{", stackup_layer*, "}" ;   (* board mechanical only *)
stackup_layer  = ("copper" | "dielectric"), qualified_name, properties ;
profile_use    = "use", qualified_name, ["as", name], properties ;
(* board_profile bodies permit imports and mechanical items, including
   profile_use and connector roles, but no electrical declarations. *)
library        = "use", "library", string, ";" ;
package_import = "import", name, string, ";" ;
port           = "port", name, ":", pin_type, ";" ;
pin            = "pin", name, properties ;
pad            = "pad", name, properties ;
power_domain   = "power_domain", name, properties ;
part_property  = name, "=", scalar, ";" ;
device_property = name, "=", scalar, ";" ;
peripheral     = "peripheral", name, ":", name, "{", signal+, "}" ;
signal         = "signal", name, ":", pin_type, (";" | properties) ;
mux            = "mux", name, ":", name, ".", name, properties ;
route          = "route", name, ".", name, properties ;
pad_set        = "pad_set", name, properties ;
unit           = "unit", name, ":", qualified_name, properties ;
signal_group   = "group", name, ":", qualified_name, properties ;
mode_group     = "mode_group", name, properties ;
resource       = "resource", name, ";" ;
configuration  = "configure", name, ".", name, "as", name, properties ;
qualified_name = name, (".", name)* ;
module_instance = "module", name, ":", qualified_name, ";" ;
component      = "component", name, ":", qualified_name, (";" | properties) ;
net            = "net", name, "{", endpoint*, "}" ;
endpoint       = name, ".", qualified_name, ";" ;
supply         = "supply", name, properties ;
power_state    = "power_state", name, "{", (name, "=", ("on" | "off" | "unknown"), ";")*, "}" ;
interface      = "interface", name, ":", qualified_name, "{", interface_item*, "}" ;
constraint     = "constraint", name, "(", targets, ")", properties ;
properties     = "{", (name, "=", scalar, ";")*, "}" ;
scalar         = string | boolean | name | number, unit? ;
```
