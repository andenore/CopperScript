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
`--offline` to reject remote cache misses. Remote resolution currently supports
tagged GitHub repositories and caches them in `.copper-cache`; package code is
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
`voltage_min` and `voltage_max` are optional typed voltage quantities.
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
electrical profile with its active bonded pads.

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

It can also use KiCad's `Library:Footprint` identifier form. The PCB export
command resolves that form only within explicit `--footprint-root` directories.
For example, `Resistor_SMD:R_0402_1005Metric` maps to
`Resistor_SMD.pretty/R_0402_1005Metric.kicad_mod`. Missing and ambiguous
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
`placement_group`, `keepout`, `routing`, `copper_zone`, `via_in_pad`, and `note`. Coordinates and rectangle dimensions
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
`maximum_return_via_distance`, and `impedance_evidence_digest`.

USB/differential profiles need not be top-layer-only. For example,
`allowed_layers = "F.Cu,In2.Cu"; max_vias = 2;` permits matched terminal
transitions and another-layer paired middle route. `max_vias` counts signal
vias **per member**, not across both nets; return-net vias are reported
separately. Required return vias must satisfy `return_via_net` and
`maximum_return_via_distance` at each transition. Dedicated plane layers
remain unavailable to foreign signal tracks. See
[paired layer transitions](paired-layer-transitions.md) for the bounded
implementation and its impedance/return-path limitations.

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

Package escape sampling is configurable on `route-board`: `--fanout-step-mm`
defaults to 0.5 mm and `--fanout-refinement-step-mm` defaults to 0.1 mm. Empty
coarse domains and conflicting selected escapes receive bounded finer radial
and, when enabled, two-leg alternatives against the same immutable input.
Easy pins keep their coarse choices. Refinement must be no coarser than the
initial step; equal steps disable the finer pass. Candidate counts and both
steps are reported. This remains a bounded search, not proof that an empty
domain is physically unroutable.

### Critical routing and qualification

`routing` profiles, not net-name heuristics, select critical geometry. The
full-vertical example explicitly declares both USB pairs on either side of its
common-mode choke. Pair members are accepted together only after exact native
geometry and connectivity checks. A rejected candidate contributes no locked
tracks/vias and does not fall back to independent D+/D- routing. Reports state
the routing `strategy` and whether a materialized candidate was rejected.
Measured candidate lengths may remain in a rejection report for diagnosis;
accepted track/via counts are zero.

Aligned terminals use a midpoint channel with 45-degree tapers. Other pair
geometries currently depend on coarse-guide candidates and can fail preflight;
joint package-access search remains necessary. Single-ended critical nets can
use bounded exact search while earlier critical copper stays immutable.
General fanout/subset repair skips critical nets. Duplicate-land cleanup may
reuse an existing critical connection but leaves new bridges pending for the
owning critical router rather than altering pair skew or adding RF stubs.

An impedance target is not proof that the provisional width/gap meets it.
Missing stackup/field-solver evidence stays an explicit assumption. Nordic's
chip-side matching connection is not labelled a generic 50-ohm RF feed; its
multi-terminal antenna/matching network remains unqualified critical geometry.
The prototype does not yet certify matching-network topology or reference
layout, RF isolation, antenna keepouts, or the continuous return path.

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
mechanical     = "mechanical", "{", mechanical_item*, "}" ;
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
