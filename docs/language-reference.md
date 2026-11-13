# CopperScript v0.1 language reference

## Decoupling role and pin association

```copper
component C_BYPASS: CAPACITOR {
    value = 100nF;
    footprint = "Capacitor_SMD:C_0402_1005Metric";
    role = decoupling;
    decouples = U.VDD;
}
```

`decouples` takes the pin like a net endpoint, `U.VDD`; the quoted form
`"U.VDD"` is equivalent and compiles to the same IR.

This is semantic component intent, not a numeric placement constraint. The
association must name a real connected pin on the capacitor's feed net, with
its other terminal on a declared zero-volt return. Reusable circuits can declare
it once in CopperLib; module expansion qualifies the protected component path.
Do not mark RF matching, crystal load or bulk reservoir capacitors indiscriminately.

Placement prefers same-side proximity using the actual feed land, respecting
fixed poses, allowed orientations, mechanical constraints and hard macros.
Ordinary routing reserves a direct surface path before package via escapes and
plane access. Ground return vias and remote power-feed transitions remain legal.
Explicit critical/RF profiles keep their specialized routing policies; no
universal distance, capacitance, return-loop or power qualification is inferred.
Placement checks actual direct pin access as well as pad distance. Routing tries
short direct candidates, then a bounded octilinear surface search with exact
terminal attachment and clearance/width checks. Computational search bounds are
not electrical length limits. Existing verified surface chains are reused;
neither ground-return vias nor remote power-distribution vias are banned.

Blocked surface paths are exposed in `fanout.decoupling_pending`, rather than
silently reported as direct bypasses. Placement never moves existing routed copper.

See [routing-quality progress and remaining work](routing-quality-todo.md).

## Board silkscreen

Board-level `silkscreen` blocks support text, the Cu logo and line, rectangle,
circle, arc and polygon artwork on either board side. The CopperScript logo,
name and compiler version are included by default. Opt out explicitly:

```copper
silkscreen {
    branding { enabled = false; }
    text REVISION { text = "Rev A"; position = (10mm, 10mm); }
}
```

See [board silkscreen](silkscreen.md) for positioning, defaults and validation.
Each item can opt into `material = copper`; `side` selects the outer copper
layer and `exposed = true` adds a matching mask opening. Copper artwork reserves
placement/routing space without acquiring an electrical net. Copper text,
filled artwork feature widths and exposed-mask qualification currently keep
physical signoff incomplete; see the same reference for the exact limits.

## Board build settings

A board may hold one `build` block. It sets defaults for options of the
physical commands, so a project keeps them in its source instead of in a
Makefile. The block is board-only: a module, part, device or macro file with a
`build` block is an error, and so is a second block.

```copper
board Example {
    build {
        fab_profile = "jlcpcb-six-layer";
        placement { candidates = 2; candidate = "candidate-01"; grid = 0.25mm; }
        routing { effort = balanced; fanout_maze = true; time_budget = 600s; }
        planes { contact_radius = 3mm; stitch_step = 0.1mm; early_stitch = false; }
        router { fine_regions = dense; maximum_expansions = 2000000; }
    }
}
```

Each setting stands for one command line option. Precedence is: command line,
then the board's `build` block, then the compiler default. A command reads the
block from the board file it is given, keeps the settings whose option it
accepts, and evaluates them before its own arguments. A conflict is not an
error: the command line overrides the source. A setting given explicitly, in
the source or on the command line, also outranks what an effort preset implies.

| Command | Settings it reads |
|---|---|
| `route-board`, and so `copper build` | all |
| `plan-layout` | `fab_profile`, and `placement` except `candidate` and `feedback_trials` |
| `si-check`, `synthesize-antenna`, `edit-mechanical` | `fab_profile` |

`route-board` prints one line naming the options it took from the source.
`copper build` shows them in its plan and records them in `run.json` as
`source_build_options`. `copper check` validates the block. The compiled JSON
holds the settings under `build`, with lengths in nanometres (`grid_nm`) and
durations in seconds (`time_budget_s`).

Value types: an integer is a whole number, not negative unless a minimum is
given. A boolean is `true` or `false`. A length needs a unit (`mm`, `um`, `m`)
and must be a whole number of nanometres. A duration needs a unit and must be
a positive whole number of seconds (`600s`). A name may be written bare or as
a string.

Top level:

| Key | Type | Default | Option |
|---|---|---|---|
| `fab_profile` | `generic`, `jlcpcb-four-layer` or `jlcpcb-six-layer` | `generic` | `--fab-profile` |

`placement`:

| Key | Type | Default | Option |
|---|---|---|---|
| `candidates` | integer, at least 1 | 1 (`plan-layout`: 3) | `--candidates` |
| `candidate` | string | the planner's choice | `--placement-candidate` |
| `grid` | length, positive | 1 mm | `--placement-grid-mm` |
| `decoupling_trials` | integer | 64 | `--decoupling-placement-trials` |
| `refinement_passes` | integer | 2 | `--placement-refinement-passes` |
| `escape_spacing_passes` | integer | 2 | `--escape-spacing-passes` |
| `feedback_trials` | integer | 24 | `--placement-feedback-trials` |

`routing`:

| Key | Type | Default | Option |
|---|---|---|---|
| `effort` | `quick`, `balanced` or `thorough` | none | `--routing-profile` |
| `feedback_iterations` | integer, at least 1 | 1 | `--feedback-iterations` |
| `router_iterations` | integer, at least 1 | 5 | `--router-iterations` |
| `fanout` | boolean | `true` | `--fanout` / `--no-fanout` |
| `fanout_maze` | boolean | `false` | `--fanout-maze` when `true` |
| `array_flow` | boolean | `false` | `--array-flow` / `--no-array-flow` |
| `via_lattice` | boolean | `false` | `--via-lattice` / `--no-via-lattice` |
| `congestion_feedback_trials` | integer | 4 | `--congestion-feedback-trials` |
| `filled_copper_trials` | integer | 8 | `--filled-copper-trials` |
| `pair_states` | integer | 6000 | `--package-initial-pair-states` |
| `pitch` | length; 0 selects the default | 0.25 mm | `--unified-pitch-mm` |
| `time_budget` | duration | no limit | `--unified-time-budget-s` |

`effort` sets `filled_copper_trials` and `decoupling_trials` unless they are
given: quick 2 and 4, balanced 8 and 16, thorough 16 and 64.

`planes`:

| Key | Type | Default | Option |
|---|---|---|---|
| `early_stitch` | boolean | `true` | `--early-plane-stitch` / `--no-early-plane-stitch` |
| `surface_zones` | boolean | `true` | `--stitch-surface-zones` / `--no-stitch-surface-zones` |
| `stitch_step` | length, positive | 0.5 mm | `--plane-stitch-step-mm` |
| `stitch_radius` | length, positive | 3 mm | `--plane-stitch-radius-mm` |
| `stitch_detour` | length | 0 mm | `--plane-stitch-detour-mm` |
| `contact_radius` | length; 0 disables it | 5 mm | `--plane-contact-radius-mm` |
| `maze_step` | length, positive | 0.1 mm | `--plane-maze-step-mm` |
| `maze_budget` | integer, at least 1 | 12000 | `--plane-maze-budget` |

`router` is the expert group. Each `key = value;` stands for
`--unified-set key=value`. The key must be a field of the unified router;
`copper route-board --list-unified-options` lists them with their types and
defaults. The value is a number without a unit, `true`, `false`, a name or a
string. `copper check`, `copper build` and `copper route-board` reject an
unknown key at its source location.

Diagnostics: `PAR018` (block outside a board), `PAR019` (second block),
`PAR020` (unknown group), `PAR021` (unknown setting), `PAR004` (duplicate
setting), `BLD001` (wrong type, unit or name), `BLD002` (value out of range)
and `BLD003` (unknown router setting). Each points at the group or key.

Settings that describe the board itself do not belong here: the stackup and
design rules are `mechanical` declarations, and net widths, layers and lengths
are constraints. See [board configuration](board-configuration.md).

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

It lowers to `Stackup.physical_layers` and selects the copper layer count
automatically. An explicit `--layers` must match the declared copper layers.
Without a declared stack-up or explicit count, physical commands select the
smallest supported stack accommodating source layer requirements and the
fabrication profile; they do not invent dielectric properties. See
[board configuration](board-configuration.md). The stack-up feeds impedance screening
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
module to a local directory, relative to `copper.mod`. A remote replacement
names another GitHub/GitLab repository and its own tag or commit instead:

```text
require github.com/KiCad/kicad-footprints 7ebfa6b23cc292a56f751b7b5f4a0e12eeef69dd
replace github.com/KiCad/kicad-footprints => gitlab.com/kicad/libraries/kicad-footprints 10.0.6
```

The module keeps its identity: imports, footprint bindings and macro member
URLs still name `github.com/KiCad/kicad-footprints/...`, but the content comes
from the replacement repository at its revision. The lock keeps the required
version as the module key and records `replace:git:https://SOURCE.git@REVISION`
as its source, so retargeting the replacement is a lock change even when the
bytes are identical. `copper lock board.copper`
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
`manufacturer`, `assembled` (boolean, default `true`), one `footprint`, an
optional package-independent `device`, and the optional 3D fields `model` (a
file path string) and `height` (a positive typed length); see
[3D models](3d-models.md). Every pin requires a quoted `number`;
`voltage_min` and `voltage_max` are optional typed voltage quantities (the
operating range), and `absolute_min` and `absolute_max` are optional
absolute-maximum ratings (see [Voltage limits](#voltage-limits)).
Standalone parts declare `domains` and `directions`, with optional
`drive_modes` and `traits`. Domains are `digital`, `analog`, `power`, `ground`,
`clock`, and `rf`. Directions are `input`, `output`, `bidirectional`, and
`passive`; drive modes are `push_pull`, `open_drain`, and `high_impedance`.
Pins may use `connection = required`, `do_not_connect`, `optional`, or `normal`
and may declare comma-separated `required_net_traits`.

Power-only output pins may explicitly declare
parallel_power_group = "CORE". Matching names permit a manufacturer-approved
external tie only within one component instance. Other output groups, separate
regulators and digital push-pull outputs still conflict. Declare this only from
the applicable manufacturer's external connection contract; it is not evidence
of internal package bonding. It does not join physical pads, omit either contact,
or alter routing/DRC. Use connection = required when each contact is mandatory.
The value is a quoted identifier; profileless, mixed-domain, input and
bidirectional pins cannot use it.

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
for pins on nets with a declared supply by the supply checks above; the logic
levels of signal nets are checked as described in
[Logic levels](#logic-levels). When several active profiles of one package pin declare a range, the
package pin's own range wins, then the first bonded device pad's.
Power-domain `voltage_min`/`voltage_max` remain operating limits checked as
errors. The serialized IR carries the absolute range as
`profile.absolute_voltage` (`rating = "absolute"`); a relative bound is
`{"reference", "offset", "limit"}`.

### EMI roles and switching metadata

Switching converters, clock drivers and their passives can carry optional EMI
facts as flat attributes; nothing is required, and a library without them
compiles to the same IR and produces no new diagnostics. Pins and device pads
take `emi_role` (`switch`, `hot_supply`, `hot_return`, `bootstrap`,
`gate_drive`, `feedback` or `sense`), `rise_time`, `fall_time`, `slew_rate`
(the switch-node dv/dt, written with a slash unit: `3V/ns`, `500V/us` or `2kV/us`),
`current_peak`, and `current_rise_time`/`current_fall_time` (the 10-90 % time in which
the switch current commutates, a different quantity from the voltage edge);
parts take `switching_frequency` or
`switching_frequency_min`/`_max`, `topology`, `shielding`, `esl`, `esr`,
`self_resonance`, `output_capacitance` and `external_cable`. A component may
fix `switching_frequency` inside the part's range, declare `load_current`
(the operating point of a buck converter, a positive current at most the switch
pin's `current_peak`; `CMP133`, also for a converter whose topology is not a
buck) and, for a capacitor, `capacitance_at_bias` (its effective capacitance at
the DC bias of the board, from the datasheet curve; at most its value, `CMP133`):

```copper
component U1: Buck { switching_frequency = 500kHz; load_current = 2A; }
component CIN: Cap { value = 10uF; capacitance_at_bias = 4.2uF; role = decoupling; decouples = U1.VIN; }
```

A component that declares `load_current` without being a switching cell gets the warning
`EMI_OPERATING_POINT_NO_CELL`.

```copper
part Buck {
    category = "power.regulator.buck";
    switching_frequency = 500kHz;
    pin VIN { number = "2"; domains = "power"; directions = "input"; emi_role = hot_supply; }
    pin SW  { number = "8"; domains = "power"; directions = "output";
              emi_role = switch; rise_time = 5ns; current_peak = 4.5A; }
}
```

`slew_rate` is a positive voltage slew rate; it classifies the device (a value
of 20 V/ns or more is reported as wide-bandgap class) and gives the edge time
`0.8 V_swing / slew_rate` when no `rise_time`/`fall_time` is declared. When both
are declared `rise_time` wins for the timing and `emi-check` notes a
disagreement of more than a factor 3. Edge time, device capacitance, ceramic
DC-bias loss and the inductor ripple (when the power inductor's value cannot be read) that the language
leaves open are filled by
rule-of-thumb defaults of `copper emi-check` (see the EMI specification, section
10.1): declared values always win, a default is tagged and printed with its
rule, and a result that rests on one is reported as a band.

Invalid values are `CMP130`. `constraint emission(J1) { standard = "cispr32_class_b"; }`
names cable connectors (`CMP131`; at most one per board) and
`constraint emi_accept(U1.SW) { code = "..."; reason = "..."; }` acknowledges a
reviewed finding (`CMP132`). The compiler derives switching cells and net
classes (`copper query BOARD emi`) and `copper check` warns about declared
cells only: `EMI_HOT_LOOP_NO_CAPACITOR`, `EMI_HOT_CAPACITOR_UNDECLARED`,
`EMI_SWITCH_NODE_NO_MAGNETIC`, `EMI_BOOTSTRAP_NO_CAPACITOR`,
`EMI_SENSITIVE_ON_SWITCH_NODE`, `EMI_SWITCH_NODE_EXTERNAL`,
`EMI_SWITCHING_FREQUENCY_UNKNOWN` and `EMI_OPERATING_POINT_NO_CELL`. The serialized IR
carries `profile.emi` and `part.emi` only when declared, and the component properties
`load_current` and `capacitance_at_bias` only when declared. All of this is screening input, never EMC
evidence; see [EMI noise-source analysis](emi-noise-analysis.md).

### Logic levels

Signal nets carry no voltage in the IR, so the supply checks above never see
them. A pin or pad can instead name its I/O rail and declare logic thresholds,
and ERC then compares the drivers and receivers of every net that has no
declared supply:

```copper
pin IO1 {
    number = "5";
    domains = "digital"; directions = "bidirectional"; drive_modes = "push_pull";
    io_supply = VDDIO;            // the pin or pad whose supply is the I/O rail
    vih = "0.7*VDDIO";            // input-high minimum
    vil = "0.3*VDDIO";            // input-low maximum
    voh = "VDDIO-0.4V";           // output-high minimum
    vol = 0.4V;                   // output-low maximum
    absolute_max = "VDDIO+0.3V";
}
```

`io_supply` on a pin names another pin of the same part or, for a device-backed
part, a pad of its device; on a device pad it names a pad of the same device. Each threshold is either a voltage
(`2.0V`) or a quoted rail-relative level `"[SCALE*]PAD[+|-]OFFSET"`, for example
`"0.7*VDD"`, `"VDD-0.4V"`, `"VDD"` or `"0.3*VDD+0.1V"`. `PAD` is named like the
reference of a relative absolute limit and is evaluated per component. Unknown
references are rejected at compile time (`CMP127`), malformed values with
`CMP120`, and a fixed `vil` above a fixed `vih` with `CMP121`. The thresholds
are optional and appear in the IR as `profile.io_supply` and
`profile.logic_levels` only when declared.

ERC resolves the I/O rail of a pin in this order: its `io_supply`, then, for a
device-backed pin, the `power_domain` of its active bonded pads through that
domain's `supply_pads` (when they all resolve to one voltage). The rail voltage
is the declared supply voltage on the net of the named pin or pad. A pin whose
rail cannot be resolved takes no part in rail-based checks and gets no
diagnostic. If a pin declares `io_supply` or a rail-relative threshold and it
cannot be evaluated, ERC reports the warning `LOGIC_LEVEL_UNRESOLVED`: the levels are not checked,
usually because the rail net has no declared `supply`.

A driver is a pin with direction `output` or `bidirectional`, a `digital` or
`clock` domain and the `push_pull` drive mode. A pin that declares no drive mode
is not assumed to be push-pull. Every other pin on the net is a receiver, except
pins that are only `power` or `ground`; a bidirectional pin is both. A pin is
never compared with itself, and two pins of one component on identical rails are
skipped.

| Condition | Diagnostic |
| --- | --- |
| driver rail voltage above the receiver's absolute maximum | error `LOGIC_HIGH_OVERVOLTAGE` |
| above the operating maximum, inside a declared and resolved absolute maximum | warning `LOGIC_HIGH_OVERVOLTAGE` |
| above the operating maximum, no absolute bound | error `LOGIC_HIGH_OVERVOLTAGE` |
| driver output-high below the receiver's `vih` | error `LOGIC_HIGH_TOO_LOW` |
| driver `vol` above the receiver's `vil` | error `LOGIC_LOW_TOO_HIGH` |

The overvoltage severities follow the supply checks. The driver's output-high
level is its `voh` when declared, otherwise its rail voltage; `LOGIC_HIGH_TOO_LOW`
and `LOGIC_LOW_TOO_HIGH` need the receiver's threshold and, for `vol`, the
driver's, and both sides must resolve. There is one diagnostic per driver,
receiver and code. Messages name both pins, the net and the numbers with their
derivation.

ERC cannot know which way a bidirectional pin is used. When a driver that may
also be an input (`bidirectional`) exceeds the limit of a pin that can itself
drive the net (`output` or `bidirectional`), `LOGIC_HIGH_OVERVOLTAGE` is a
warning that says the excess exists only if the first pin is configured as an
output. Against an input-only pin the severities in the table apply.
`LOGIC_LEVEL_UNRESOLVED` is reported once per component and reference, not
once per pin.

A resistor from a signal net to a net with a declared non-zero supply is a
pull-up. Its supply voltage is a high level on the net and is checked against
the limits of every pin on it with `LOGIC_HIGH_OVERVOLTAGE`, and against every
declared `vih` with `LOGIC_HIGH_TOO_LOW`, so an open-drain
pin gets a high level only through such a resistor. A net with resistors to
supplies of different voltages (a divider or bias network, including one to
ground) has no single high level and is not checked this way.

Limits of the check: it compares logic levels, not analog voltages, and it does
not model timing, hysteresis, current or the signal's rise time. Pins without
`io_supply`, power domain or thresholds are not checked, so unannotated
libraries stay quiet.

### Pull resistors and undriven inputs

A pin or pad can record what its datasheet says about pulls:

```copper
pin NRST {
    number = "7";
    domains = "digital"; directions = "input";
    requires_pull = up;           // the datasheet demands an external pull-up
    internal_pull = "up,down";    // a built-in or firmware-selectable pull
}
```

`requires_pull` is `up` or `down`; any other value is `CMP120`. `internal_pull`
is a comma-separated list of `up` and `down`, parsed like `drive_modes`; an
unknown name is `CMP120`. Both are optional and appear in the IR as
`profile.requires_pull` and `profile.internal_pull` only when declared.

| Condition | Diagnostic |
| --- | --- |
| a pin with `requires_pull` is on a net with neither that pull nor a direct tie | error `MISSING_PULL` |
| every pin on a net is a digital or clock input and nothing drives or pulls it | error `UNDRIVEN_INPUT` (warning when the net has a single pin) |
| every pin that can drive a net is open-drain and the net has no pull-up | warning `OPEN_DRAIN_NO_PULLUP` |

A net has a pull-up when a resistor joins it to a net with a declared non-zero
supply, and a pull-down when a resistor joins it to a net with a declared 0 V
supply. A net that itself carries a declared supply counts as tied high or low.
`MISSING_PULL` is satisfied only by a resistor or a tie of the required
direction; `internal_pull` does not satisfy it, because the datasheet asks for
an external part. A pin that is on no net is left to the unconnected-pin checks.

`UNDRIVEN_INPUT` looks only at nets without a declared supply. Every resolved
pin must be input-only (direction exactly `input`) in a `digital` or `clock`
domain. Any output, bidirectional, passive, power or analog pin, or a pin the
library does not model, counts as a driver, as it does in KiCad, so a connector
or switch contact keeps a net clean. A pull resistor to a supply or ground, or
an `internal_pull` on any pin of the net, also satisfies it. A net with a
single input pin is reported as a warning, like an unmarked pin. A net with an
endpoint that does not resolve gets no `OPEN_DRAIN_NO_PULLUP`. There is one
diagnostic per net.

`OPEN_DRAIN_NO_PULLUP` looks only at nets without a declared supply. A driver
is a pin with direction `output` or `bidirectional` in a `digital` or `clock`
domain; the check applies when every driver has exactly the `open_drain` drive
mode. A pull-up resistor, or any pin with `internal_pull` containing `up`,
satisfies it. The `sda` and `scl` nets of an `i2c` interface are skipped
because `I2C_MISSING_PULLUP` covers them, and a net with a `requires_pull = up`
pin gets only `MISSING_PULL`.

Limits: the resistor value is not checked, and an `internal_pull` is a
firmware obligation that ERC does not verify. `UNDRIVEN_INPUT` and
`OPEN_DRAIN_NO_PULLUP` need only `domains`, `directions` and `drive_modes`, so
they also apply to libraries that declare no pull properties.

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
`unknown`. Run `copper power-check board.copper` to validate
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

## Hard-macro definitions

`macro NAME { ... }` defines a named physical realization exported from an
ordinary package. `import layouts "PROVIDER/PACKAGE";` selects that package;
`physical_macro UNIT: layouts.NAME { ... }` binds its definition to existing
circuitry. Definitions use engineering quantities, explicit pad/net roles,
named fixed geometry and independently resolved footprint assets with generated
digests. See [the full syntax reference](macro-definitions.md) and
[the runnable example](../examples/macro_definition/README.md).

## Panels

`panel NAME { ... }` places already-routed boards on a fabrication panel,
using the macro grammar: root properties (`preset`, `spacing`, `rails`,
`tab_width`, `mouse_bite`, `fiducials`, `fiducial_offset`, `tooling_hole`,
`cutter`, `frame_fill`, `assemble_after_depanel`) and
`board NAME { source; pcb; at; rotation; }` and
`tab NAME { board; side; at; width; }` blocks. A panel is not compiled as a
board: `copper panelize` composes it into one `.kicad_pcb` and refuses tabs
on antenna regions opened to the perimeter or on body overhangs, and bodies
overhanging a rail or another board. `preset = "jlcpcb"` (or `jlcpcb-smt`,
`pcbway`, `eurocircuits`) fills the properties a panel leaves out and checks
the panel against that fab's documented rules. See [panelization](panelization.md) and
[the runnable example](../examples/panel/README.md).

## Physical macro bindings

Import an exact physical hard-macro JSON asset, then map its members and net
roles onto existing circuitry. The asset URL uses the same `copper.mod`
requirements, replacements and `copper.lock` inventory as managed footprints.
No content hash is authored in `.copper`.

```copper
import buck_layout "github.com/vendor/power/assets/buck.json";
physical_macro BUCK: buck_layout on PWR {
    member U = U1;
    member L = L1;
    member CIN = C_IN;
    member COUT = C_OUT;
    net VIN = port.VIN;
    net VOUT = port.VOUT;
    net GND = port.GND;
    net SW = SWITCH;
}
```

The example assumes the asset exposes exactly those member keys and net roles.
`on PWR` resolves targets within that existing module instance; omit it to bind
a component set in the declaring board/module, including explicit descendants
such as `PWR/U1`. Net names, descendant net paths and module port references
select existing electrical nets. Elaboration maps boundary nets to their actual
parent nets; bindings never create components, connect nets or rewrite the circuit.
Member mappings cannot name pins. Net mappings cannot use component-pin selectors.

Declarations are supported in boards and modules. Module-authored bindings repeat
with qualified identities for each instance. Duplicate ownership, unknown
references and distinct asset roles collapsing onto one net are errors; there
is no implicit override or name-based binding. Asset keys can be quoted strings.

`import layout "./assets/layout.json";` selects a local asset relative to its
declaring source. Imported package paths remain confined to that module and
retain its provider identity. Workspace assets are hashed as build inputs;
files outside required modules are not pinned by the package lock.

Electrical checks validate source references without loading geometry and
leave asset/physical validation pending. `copper lock` prepares selected assets
and checks exact mapping coverage. Physical commands bind exact footprints,
pads, nets and layer contracts through the existing immutable macro lifecycle.
See [physical hard macros](physical-hard-macros.md) for routing, export and
qualification limits, and [the implementation plan](physical-macro-syntax-plan.md).

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

An endpoint has the form `component.pin`. A pin pattern `component.PREFIX*`
names every pin of the component's part whose name starts with `PREFIX`, in
the part's pin order, so a module's ground pins need one line:

```copper
net GND {
    U_MODEM.GND*;      // GND, GND_1, GND_27, ... as the part declares them
    C1.2;
}
```

The `*` follows the prefix directly and ends the name; it matches only part
pin names (not device unit terminals). A pattern that matches no pin is an
error (`CMP211`), as is one on a component whose part is unknown (`CMP212`).
A pin the net already names is not repeated. Patterns may name a component
declared further down the file. Check what a pattern selected with
`copper query BOARD net NAME`.

## No-connect pins

`no_connect` records that pins are left open on purpose, so a deliberate
decision is distinguishable from a forgotten connection:

```copper
no_connect U_NRF.DEC2, U_NRF.DCC {
    reason = "QFAA LDO reference: C6 not mounted, DC/DC disabled";
}
no_connect U1.PA0;
```

One declaration lists one or more `component.pin` endpoints; `reason` is the
only property and must be a non-empty string. Boards and modules may declare
no-connects; module declarations are qualified by instance path. Module boundary
ports cannot be targeted (`CMP125`).

ERC reports `NO_CONNECT_CONNECTED` when a declared pin is also on a net,
`NO_CONNECT_REQUIRED_PIN` when the part requires the pin, `DUPLICATE_NO_CONNECT`
(warning) for repeats, and `UNKNOWN_COMPONENT`/`UNKNOWN_PIN` for bad endpoints.
A no-connect does not silence `UNSOURCED_POWER_INPUT`.

`copper check` and the language server also report `UNMARKED_PIN`: one warning
per component listing ordinary modeled pins that are neither on a net nor
declared no-connect. Power inputs, required, optional and part-level
do-not-connect pins, and pins without an electrical profile are excluded because
they are reported or decided elsewhere. Components inside module instances are
excluded: only the module's own source can declare their no-connects, and a
`no_connect` on a module port is an error (`NO_CONNECT_MODULE_PORT`).
`copper check --pins error` makes the
warning blocking; `--pins off` hides it. The `erc.check()` API reports it only
with `unmarked_pins=True`. No-connects appear in the IR as `no_connects` only
when declared, and do not change KiCad export, which already marks every
unconnected schematic pin.

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

A standalone passive part can explicitly declare `dc_power_path = "P1,P2";`
between two passive, power-only pins. ERC follows this path only from an existing
source and only when the part is assembled and both ends connect unambiguously.
An unpowered loop cannot create a source. This describes the intended DC feed
through a ferrite or similar component; it does not merge nets, bond PCB lands,
assign an ideal-inductor model, or prove voltage drop, current, bias impedance,
inrush or resonance. Declare each downstream supply's nominal voltage separately.
Assembly-lock population changes need their own engineering validation; basic
source ERC does not load the assembly lock.

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

`placement_region` also accepts `polygon_mm = "45,2;65,2;65,17;45,17";`
instead of `x`, `y`, `width` and `height`. Concave polygons are supported; the
complete component courtyard must remain inside the region. Rectangle and
polygon boundary parameters are mutually exclusive. `side = both` preserves
either board side. A copper zone can reference the same named polygon region;
nonzero zone inset still requires a rectangular boundary.

Known constraint kinds are `max_distance`, `min_distance`, `courtyard_gap`,
`placement_region`, `fixed_placement`, `allowed_orientations`, `align`,
`placement_group`, `keepout`, `routing`, `copper_zone`, `via_in_pad`,
`length_match`, `hole_clearance`, `antenna` (see
[trace antenna synthesis](#trace-antenna-synthesis)), and `note`. Coordinates and rectangle dimensions
are lengths in the physical board coordinate system; orientation values are
unitless degrees. The legalizer accepts any explicitly permitted angle,
including 45-degree increments; unconstrained components still default to
0, 90, 180, or 270 degrees. Use `allowed_orientations` when a diagonal
placement is acceptable for assembly and interface alignment. `note` remains
metadata and has no placement effect.

`courtyard_gap` declares a hard additional gap between audited component
courtyards. It does not shrink the courtyard or change copper, pad, mask,
paste, routing-access or assembly-process checks:

```copper
constraint courtyard_gap(C1, C2) { additional = 0.1 mm; }
```

Targets must be components, including module-qualified component paths, rather
than pins. `additional` must be a nonnegative length exactly representable in
integer nanometres. Zero permits no additional gap; courtyard interiors still
must not overlap. Only hard `require` (the default) and `external` modes are
supported; this declaration is not a preferred/rework spacing objective.
`placement_region` may include `courtyard_gap = 0.1 mm;` as shorthand for the
same member policy. Repeated declarations combine by maximum, independently
of declaration order, and an absent field never erases an explicit value.

Each ordinary same-side pair uses the maximum of its two member gaps. A member
without an explicit policy retains the ordinary 0.5 mm default, so annotating
one member does not relax its unannotated neighbour. An audited rigid cluster's
internal-clearance contract replaces the generic fallback inside that cluster;
explicit member gaps can only tighten that contract. Source fixed poses,
orientations, sides, regions and immutable macro geometry remain authoritative.
Planner APIs retain their explicit caller fallback behavior for unannotated
members. Changing source policy changes the physical intent and invalidates
evidence bound to the earlier inputs. A declared dense prototype gap is a
design choice, not a guarantee of vendor, assembler, CAM or electrical
qualification; selected parts and manufacturing processes require separate
checks.

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

Length and skew budgets (`max_length`, a pair's `max_skew`, a
`length_match` group's `max_skew`) are review warnings on connected copper,
not opens or geometry errors. Every router keeps searching for a route within
the budget and prefers it; when none is found it keeps the best connected
route (fewest misses, then the smaller pair skew, then the shorter copper)
and reports the miss in the net result's `warnings`. DRC reports
`DRC-MAX-LENGTH`, `DRC-DIFF-SKEW` and `DRC-LENGTH-MATCH` as warnings,
`copper issues` lists them as `review` issues (`B1`, ...) to resolve by
placement or manual review, and requirement verification still fails the
declared constraint. `max_vias`, `maximum_uncoupled_length` and every other
profile gate stay hard. Reused critical copper must meet its current budget.

`maximum_stub_length` limits the stubs of a net's routed copper. The copper is
a graph of junctions joined by tracks and arcs, and its route ends are the
net's pads, pours and synthesized antenna copper. Copper that lies on a simple
path between two route ends is route copper, including every loop that joins
two of them; all other copper is stub copper: a dangling track or arc, an
unused via, or a loop that hangs off the route. A stub's length is the
greatest distance along the copper, by the shortest way, from the junction
where it attaches to the route to any point of it (for a loop, the point
halfway round, which may lie partway along a track), so how the copper is cut
into tracks does not change it; arcs count at their true length and a via
barrel adds nothing. A branch that ends on another pad is
a route, not a stub, so a multi-pad net is not held to the limit on its
branches to pads. Exact DRC reports one `DRC-STUB-LENGTH` error per net, for
its longest stub, when that exceeds the limit. Only nets whose pads are all
connected are measured (an open net is reported as `DRC-OPEN-NET`).

Pieces of copper join where they overlap: ends that overlap share a junction;
an end on the body of another piece, a via or pad on the body of a track, and
a crossing of two centre lines split the pieces there; an end on a via, pad or
pour joins it. Copper laid over copper, such as a duplicated or overlapping
segment, adds a parallel stretch between the same junctions and does not
hide or lengthen a stub. The check does not look for electrical effects: it is
not a via-stub resonance or impedance analysis.

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

For single-ended profiles, the unified router can reuse existing return vias.
`require_return_vias`, `return_via_net` and `maximum_return_via_distance` constrain
both early launch proposals and negotiated transition sites. Exact commit and
independent `DRC-RETURN-VIA` checking require an existing return barrel spanning
the entire physical signal via within the stated center distance. No ground
vias are generated, moved or removed. Missing returns remain unresolved. This
single-ended path conservatively requires actual return vias even when a
shared-reference waiver might otherwise be requested; it does not infer one.
Native filled-reference continuity and electrical SI remain separate checks.

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
}
```

`copper_zone` targets exactly one existing net and lowers to an unfilled
physical zone inside the declared board outline. `layers` is required;
`inset`, `clearance`, `minimum_width`, `pad_connection`, `thermal_gap` and
`thermal_spoke_width` are optional.

Pads join a zone through thermal reliefs by default: the fill keeps
`thermal_gap` (default 0.3 mm) around each same-net pad and bridges it with
four spokes `thermal_spoke_width` wide (default 0.3 mm, or the zone's
`minimum_width` when that is larger). Reliefs keep the pour from sinking the
heat of a hand or reflow solder joint, which matters most for ground pads and
through-hole pins on large pours. `pad_connection` selects `thermal` (the
default), `solid` (pads flood directly, for example an exposed thermal pad that
must spread heat or a high-current contact), `tht_thermal` (reliefs on
through-hole pads only, SMD pads solid) or `none`. A footprint pad's own
`zone_connect` overrides the zone's setting. Vias always connect solidly.
Spokes narrower than `minimum_width` are rejected, since the fill would prune
them; native DRC reports a pad left with fewer than two spokes as
`starved_thermal`.

```copper
constraint copper_zone(GND) {
    layers = "F.Cu,B.Cu";
    thermal_gap = 0.25mm;
    thermal_spoke_width = 0.4mm;
}
```

The
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

Set `allow_same_net_hard_macro_overlap = true` to opt this host zone into
overlapping a hard macro's local zone on the same net and layer. The macro must
allow zones on that layer, and its own copper keepouts still apply; foreign-net
overlap is never permitted. This permits connected native fill but does not
combine the source zone objects or replace native refill/DRC verification.

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
remain distinct even at the same voltage. The soft source-to-load
distribution attraction is 0.25 (`--power-domain-weight` on `plan-layout` and
`route-global`, range `0..1`, `0` disables it).
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
Local-contact preference is on by default for every GND pad. Use
`--no-prefer-local-ground` to restore reuse-first behavior, optionally with
`--prefer-local-ground-pad REF.PAD` to select individual exceptions.
Pads over their own non-GND distribution zone, such as an inner `V3V3` fill,
also prefer a nearby via over a longer surface route to a reusable contact.
The preference applies to eligible SMD pads outside protected hard macros;
through-hole pads already provide a plated contact. It compares a nearby legal
via with reuse of an existing contact, using surface escape length and a
small penalty for another drill or shared primary contact. A local via is not
guaranteed when no legal site exists. Native refill still has to prove the
zone connection.

`--plane-maze-step-mm` (default 0.1 mm) and `--plane-maze-budget` (default
12,000 states) bound the local surface-path fallback when cheap contact
candidates fail. A finer mesh, such as 0.025 mm, can resolve an escape missed
by the coarser search. It never reduces trace width, clearance, drill size or
source routing limits, and a successful contact still requires native refill
and DRC. The report records both controls; early and late stitching use the
same configured bounds.

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

### Trace antenna synthesis

`constraint antenna(COMPONENT.PIN)` reserves a board-edge region and places a
printed antenna inside it, fed from that pin. The target is the feed pad of a
one-land antenna feed component; its net is the antenna feed net. Without a
`geometry`, lowering uses a first-order template; `copper synthesize-antenna`
tunes the geometry in a full-wave model of the board and prints the
`geometry` and `tuned_for` lines to paste back.

```copper
constraint antenna(ANT1.FEED) {
    name = "bt";                      // macro and keepout identity; default antenna:N
    x = 18mm; y = 0mm; width = 22mm; height = 6mm;
    // or region = "radio-corner";    (a named placement_region)
    // or polygon_mm = "18,0;40,0;40,6;18,6";
    ground = GND;                     // the host reference ground net
    bands = "2400 MHz-2483.5 MHz";    // LOW-HIGH with units; ';' separates bands
    max_return_loss_db = -10;         // synthesis target over every band (default -10)
    track_width = 1mm;                // default: the board's default track width
    keepout_margin = 0.5mm;           // extra component clearance around the region
    topology = auto;                  // auto | monopole | meander | inverted_f | pixel
    short = R1;                       // inverted_f: the two-land link that ties the arm to ground
    geometry = "meander height=4.6mm depth=3.1mm pitch=2mm turns=1 tail=6.2mm";
    tuned_for = "sha256:...";         // site digest the geometry was tuned for
}
```

Exactly one region form is required; the region must be a rectangle whose
one edge lies on the straight outer board perimeter while the opposite edge
faces the host ground. When both axes qualify (a corner), the longer run is
used. `ground` must name an existing net (ERC reports an unknown one). An
inverted-F needs `short`: a placed two-land component (a 0 ohm link, or a
tuning inductor) with one land on the feed net and the other on the ground
net; ERC reports an unknown one, and `topology = inverted_f` without it is a
compile error. `loop` is reserved and fails as not implemented, as do
back-side antennas and curved regions or outlines.

#### Geometry

Local coordinates run along the board edge (`u`, from the region's low end)
and across it (`v`, from the ground edge). Every geometry starts at the feed
pad, just outside the region on the ground side, with a stub up the feed's
column:

- `monopole height=H length=L`: the stub to `H`, then a run along the edge;
  `L` is the conductor length from the ground edge.
- `meander height=H depth=D pitch=P turns=N tail=T`: the stub to `H`, one
  pitch along, `N` U-shaped meanders hanging `D` below `H` with legs `P`
  apart centre to centre, then a straight `T`.
- `inverted_f height=H length=L short=S`: the stub to `H`, where the arm
  runs `L` from the short leg to its open end; the leg stands `S` before
  the feed and runs down to the link's feed land. The link is rotated so its
  feed land faces the region, and stands between the region start and the
  feed: `S` must keep both bodies and both legs clear, and may use up to a
  fifth of the region (at most 6 mm). A ground-net stub and a through via at
  the link's ground land return it to the ground plane; the via must sit in
  a ground zone.
- `pixel cell=C columns=M rows=K bits=HEX`: an `M` x `K` grid of `C` tiles,
  row-major from the ground edge, with one column centred on the feed; only
  tiles 4-connected to the tile above the feed are realized. Tiles are
  owned copper polygons, so no `F.Cu` `copper_zone` may overlap or touch the
  antenna region: draw the ground zone with a gap around it.

Lengths are millimetres with an `mm` suffix, on whole nanometres. Every
geometry must keep its copper the board clearance plus half a track inside
the region, and meander legs at least the clearance apart; a geometry that
does not fit fails with the length it needs.

#### Tuning and staleness

`tuned_for` is the site digest: a hash of every input the full-wave model
reads (outline and cutouts, stack-up thicknesses, permittivities and loss
tangents, the ground net's zones, region, frame, track width, clearance,
feed land and position, bands, target, generator version and the model
revision, which changes whenever the simulated model itself changes). When
`tuned_for` is present and differs from the current board's digest,
lowering fails and names both digests: re-run `copper synthesize-antenna`,
or remove `tuned_for` to keep the geometry as an untuned manual one.
Placements of other components, other nets and the enclosure are not in the
model and so not in the digest.

```sh
copper synthesize-antenna board.copper --antenna bt --report build/bt-antenna.json --locked --offline
copper synthesize-antenna board.copper --antenna bt --verify --locked --offline
```

The command physicalizes the board without its antenna declarations,
resolves the site, and searches in a full-wave FDTD model (the pure-Rust
`ems` solver): the length is scaled until the series resonance sits on the
band centre, then a bounded Nelder-Mead search over height and length
polishes the worst-case return loss on a coarse mesh (two cells across the
track), and the winner is verified on a mesh twice as fine with one more
length correction and the radiation efficiency from a near-to-far-field box.
`topology = pixel` runs a seeded genetic algorithm over the region's tiles
instead; an inverted-F searches height, length and the short spacing, and
is scored on its bare return loss, since the short spacing is its own
impedance knob and the matching network is only reported for it. Every candidate is scored by its worst return loss over every band
with the best two-element L-section matching network synthesized from its
simulated feed impedance, unless it already meets the target bare.
`--budget` caps coarse evaluations (default 12); `--threads` sets the
solver threads (default one per physical core: the time loop is
memory-bound, so SMT siblings only slow it); `--no-fine` skips
verification; `--verify` evaluates the declared geometry without searching.
The report (`copperscript-antenna-synthesis/v0.1`) holds the search history,
the final curve, the matching values, efficiency, assumptions and the
unresolved bench work. The model leaves out components, other nets, zone
holes, the enclosure and cables, and its matching parts are ideal: the result
is simulation evidence, not bench or regulatory evidence.

#### Lowering

Lowering generates a one-member hard macro in the `v0.5` asset contract and
binds it through the ordinary immutable binder, so placement, routing, fill
exclusion, export locking and port-continuity proof need nothing new:

- The feed component is the anchor, fixed just outside the region on the
  ground side (its pad at the board clearance from the region, its courtyard
  clear of the component keepout). A conflicting `fixed_placement` fails.
  An inverted-F's link is the second member, fixed the same way; its ground
  land is a private pad with a `plane_return` proved by the macro's via.
- The radiator is locked `F.Cu` copper: the geometry's track of
  `track_width`, and for pixel geometries one copper polygon per tile. The
  template is a monopole whose run sits on the region's centreline, totalling
  one quarter wave at the band centre scaled by the Hammerstad-Jensen
  effective permittivity of the dielectric under `F.Cu`; a stack-up without
  `er` there fails.
- The region is a protected region on every copper layer (no foreign tracks,
  vias or fill), a zone keepout with `edge_openings` for its perimeter edges,
  and a `PlacementKeepout` (both sides, expanded by `keepout_margin`) for
  other components. Other pads of the feed footprint must be unconnected.
- The feed pad is the macro's one external port; ordinary routing reaches it
  from the matching network. The macro's copper is the antenna, not a route:
  a `max_length` (or pair skew) on the feed net measures only the routed
  feed, leaving out the radiator, its feed stub and an inverted-F's short.

KiCad's DRC reports the radiator's open end as a dangling track; native
verification (`route-board --verify-plane-fill`) excludes that one finding
per antenna, by net and region, and records it as an expected open end, also
by point, net and layer (`plane_verification.expected_open_ends`), so
`copper drc FILLED --expected-open-ends ROUTE_REPORT` excuses exactly those
ends on the filled board. A
ground island that the short's via joins only through the zone fill is
proved by that refill, so an inverted-F needs native verification for a
passing route.

`metadata["antenna_synthesis"]` records each synthesis (provenance
`template`, `tuned` or `manual`, geometry, site digest, band, effective
permittivity, path, pose, region) and the macro's `source` repeats it.
Requirement verification passes an antenna declaration only when its
generated copper is present and materialized, and says so without any
return-loss, radiation, matching or regulatory claim; the macro's
`unresolved` list keeps bench tuning in the final enclosure mandatory. The
example is `examples/antenna_synthesis`; the design, validation and
remaining work are in [the research note](pcb-antenna-synthesis-research.md).

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

Package escape sampling on `route-board` steps 0.5 mm, and
`--fanout-refinement-step-mm` (default 0.1 mm) sets the finer pass. Empty
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
When the unified router is selected, clock and general `critical` single-net
fallbacks use that engine with their original source rules. Each fallback is
bounded to at most 16 iterations, two commit rounds and 60 seconds (or the
smaller caller limits). Pair routing retains its coupled engine; RF/power
profiles retain their existing specialized behavior.
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
in `copper_route::critical_bundles`). Spacing findings come from the coarse candidate
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
`reason`). `copper replay-critical` prints one line per bundle.

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
moving pair it crosses. Moving pairs are routed first in the bundle, then
the pairs they cross, then the rest; among the surface pairs, those nearest a
moving pair along the terminal rows come first, so the pairs beside the
transition vias can step aside before the outer pairs take the room. Moving
pairs use only the paired-via search on that layer: matched vias at both ends,
return vias within `maximum_return_via_distance` or a declared shared
reference, transitions inside the pair's reserved corridor first and
transition vias inside a breakout region last. They skip the coarse candidate and the surface
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
segments nearest the terminal or bend where the length difference arises
(segments inside a breakout region last), bulging away from the partner, with 45° corners (below; the chamfer leg is at
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
pair, where both members are parallel at the pair spacing; where they run
parallel up to three times wider, as beside the pair's vias, both members
bulge apart over the same interval as single nets), at least
3 × width from either end of the run and 3 × width apart, and are 3 × width
wide (the outer member of a pair 3 × width + 2 × spacing), on either side of
the run. Placement rule: the room of each slot is the tallest bump, up to the
amplitude limit, whose swept area (the convex outline of the chamfered bump)
clears all other copper, lands, holes, keep-outs and the board edge by the
applicable clearance; slots outside breakout regions come first (inside one
the clearance is relaxed for the dense lands, and a bump there takes the room
the neighbouring lands' lanes need to escape), then slots with the most room,
then those farthest from the unit's terminals, then by position. The fewest bumps that
provide the length are used, at most 16 per unit (`MATCH_TUNING_BUMP_LIMIT` in
`copper_route::critical_tuning`), with levelled heights. Each tuned unit passes the same profile gates (length and via
budgets, pair skew and `maximum_uncoupled_length`) and atomic native-DRC
validation against all other copper as a routed candidate. In the coupled-length
measure, a pair bump adds up to about 3.3 lane spacings of uncoupled length per
member, so a tight `maximum_uncoupled_length` can rule tuning out.
A group's copper changes only when every unit is accepted and the group ends
within `max_skew`. Otherwise its copper and results are kept exactly as
routed, the group is reported `failed` with the reason, the critical stage
warns, and final verification reports the `DRC-LENGTH-MATCH` review warning
(requirement verification fails the group). Groups within their
limit are untouched. Groups with a member that is not connected after critical
routing (for example an ordinary net) are `incomplete` and left to final
verification. Candidate validation during critical routing does not reject a
candidate for `DRC-LENGTH-MATCH`, because group skew depends on every member
and on this tuning pass. The critical report's `match_tuning` list gives, per
group, `id`, `max_skew_nm`, `status` (`within_limit`, `tuned`, `failed` or
`incomplete`), `skew_before_nm`, `skew_after_nm`, the `members` with
`length_before_nm`, `length_after_nm` and `added_length_nm`, the number of
`bumps` and the `reason`. The `nets` entries and `lanes` table report the
tuned copper. `copper replay-critical` prints one line per group.

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
length is too small for tops of that height). Serpentines and then bumps are
tried outside breakout regions first, and only then anywhere, so a serpentine
beside the lands never wins over bumps in the open run. At most 32 legs are used per
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
`copper replay-critical` also prints one line per lane.

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
  length is its total routed track length plus, for each of its vias, the
  depth between the outermost copper layers its tracks end on there (from the
  declared stack-up, else the layers spread evenly over the board thickness),
  and the group skew is the longest minus the shortest member. Once every member
  net is connected, a skew above `max_skew` is the DRC review warning
  `DRC-LENGTH-MATCH` (requirement verification fails it); before that the
  group is reported as incomplete. The
  critical router tunes critically routed members toward the group's longest
  member (see [critical routing](#critical-routing-and-qualification)).

Invalid values fail with source locations (`CMP110` for routing properties,
`CMP111` for `length_match`); semantic errors found while lowering (unknown
nets, a net in two groups, duplicate group ids) carry the constraint's
location, and `CMP117` with the routing constraint's location marks a
`tuning_group` outside any `length_match` group, one spanning two of them,
or pair members that disagree.

### Arched corners

```copper
constraint routing(RF_FEED) { kind = rf_feed; width = 0.3mm; corner_style = "arc"; corner_radius = 1mm; }
constraint routing(LVDS_P) {
    kind = differential; partner = LVDS_N; width = 0.1mm; pair_gap = 0.1mm;
    corner_style = "arc"; corner_radius = 0.3mm; tuning_style = "serpentine"; tuning_amplitude_limit = 1mm;
}
```

- `corner_style` is `"octilinear"` (the default: 45° chamfers where the
  router can place them) or `"arc"`. After routing, smoothing, tuning and
  cleanup, the arc-corner stage replaces every corner of an arc-style net
  where exactly two of its tracks of one layer and width meet, away from
  vias, lands and immutable macro copper, by a circular arc tangent to both
  legs. KiCad receives native `(arc ...)` tracks.
- `corner_radius` (a positive length, only with `"arc"`) is the arc radius,
  by default three times the net's routing width. Where a piece between two
  corners cannot hold both tangents (r·tan(θ/2) each), the larger radii on
  that piece come down to a common level that fits; a corner whose legs
  cannot hold half the width keeps its octilinear shape. Consecutive corners
  that turn the same way, such as the two 45° corners of a chamfer, form one
  compound turn of at most 90° when an arc of the radius tangent to the
  outer legs swallows the pieces between them, so a chamfered 90° bend
  becomes one quarter circle and a chamfered U-turn two; a compound turn
  that does not fit or is refused is arched corner by corner. Both members
  of a pair must declare the same `corner_style` and `corner_radius`; the
  member inside a turn takes the radius and its partner the radius plus the
  lane spacing, so the pair stays concentric and keeps its gap. A pair
  member's compound turn needs its partner's. Where the legs are too short
  for concentric arcs, the outside member first gives up its extra radius,
  down to equal arcs (translates of each other, which keep the pair's skew),
  and only then does the radius shrink; the gap never narrows and widens
  mid-turn by at most 1/cos(θ/2) − 1 (8 % at 45°, 41 % at 90°). A member
  turning alone takes its own radius.
- An arc is emitted only when its copper is clear of foreign copper,
  keep-outs, macro regions, routing reservations and the board edge by the
  clearance plus 2 nm, with breakout values inside breakout regions as the
  exact DRC applies them (the partner's copper keeps its gap by
  construction). A refused corner keeps its octilinear shape, as does one
  blocked by a via or land. The route report's `arc_corners` entry counts the arcs and lists
  every retained corner with its reason; `summary.copper.arcs` counts them
  and `track_length_mm` includes their length. The stage is the last change
  to the copper before the exact DRC and the native checks, so KiCad sees
  the arcs; a net the filled-copper feedback loop reroutes afterwards loses
  its arcs (its new copper is octilinear).
- Arc-style nets are routed and tuned exactly as octilinear ones (45°
  route chamfers, chamfered bumps and serpentines); the stage only rounds
  their corners, so arching never makes a corner sharper. One difference:
  both members of an arc-style pair take the same chamfers on bumps and
  serpentines (octilinear pairs widen the outside member's by the lane
  spacing), so every right-angle turn of a tuning feature becomes one arc
  per member and the chamfers stay skew-neutral if the stage refuses one. Lengths count an
  arc at radius × sweep: length-match and pair-skew tuning of arc-style nets
  measure every unit as arched, and a step may gain up to 4 nm beyond its
  even target. `tuning_group` requires octilinear corners. The final `DRC-LENGTH-MATCH` check measures the emitted copper,
  so a corner the stage refused shows up there.
- Exact DRC measures arc distances in floating point from the exact centre
  of the three stored points and reports a violation when a distance falls
  more than 8 nm short (the rounding of those points moves a circle by a few
  nanometres); board-edge checks use the inscribed chord chain widened by its
  sagitta, which is conservative. A `DRC-ARC-GEOMETRY` error marks an arc
  whose points are collinear or span more than 180°.

Invalid values fail with `CMP110` (`corner_style` outside the two names,
`corner_radius` without `"arc"`, `tuning_group` with `"arc"`), and pair
members that disagree on corner style or radius fail when the board is
physicalized.

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

### EMI screening

`copper emi-check BOARD [--locked] [--offline] [--layers N] [--fab-profile P]
[--footprint-root R] [--allow-proxy-footprints] [--run RUN] [--plot-spectrum OUT.svg]
[--plot-field OUT.svg [--field-height MM]] [--fullwave [--fullwave-fidelity coarse|fine]
[--fullwave-budget SECONDS] [--fullwave-margin MM] [--fullwave-single-mesh] [--fullwave-zone-outline]]
[--extract [--extract-budget FILAMENTS] [--extract-margin MM] [--extract-cell MM]
[--extract-single-mesh] [--extract-no-switch-bar]] [--json]`
physicalizes the board like `si-check` and screens every switching cell of the
EMI model (`copper query BOARD emi`; see [the EMI specification](emi-noise-analysis.md))
for `EMI-HOT-LOOP`, `EMI-SWITCH-NODE` and `EMI-PROXIMITY`, applies `emi_accept`
constraints (`EMI-ACCEPT-UNUSED` for a stale one) and lists what it could not
measure as coverage. It prints one block per cell with ranked findings and the
measured numbers, or JSON (`copperscript-emi-check/v0.1`), and always exits 0
when the board loads. Every result is screening evidence, and margins against an
`emission` standard are never a pass or a fail. No accuracy figure is claimed:
the closed forms, the circuit solver and the receiver emulation are checked against
hand calculations and analytic cases, and no estimate has been correlated with a
measurement (`limits.validation`). What an estimate leaves out is stated with it in
three separate parts: `estimate.numerical_sensitivity` (`null` for a closed form;
the full-wave model reports its mesh sensitivity in `measures.fullwave`),
the parameter band that rule-of-thumb defaults open (`band_db`, `basis`), and
`estimate.not_modelled`, the named mechanisms the estimate does not contain. Cells whose roles come from the structural
fallback are marked `inferred`. A four-switch buck-boost cell
(`topology = buck_boost`) gets an `input loop` and an `output loop` finding;
the output pins are the controller's `VOUT*` pins. Among a cell's hot
capacitors one that declares `decouples` of a hot-supply pin is preferred.

For each `buck` cell it also predicts the conducted emission at the input
(`EMI-CONDUCTED`, 150 kHz to 30 MHz, dBuV at a 50 uH V-network, differential
mode only; common mode is not modelled) of a fixed-frequency buck in continuous conduction, from the declared
supply voltages of the hot-supply and output nets, the switching frequency and one of two scenarios named in
`measures.scenario`. `declared_operating_point`: the converter's component declares `load_current`, and the input
current is a ramp from `I_out - dI/2` to `I_out + dI/2` during the on-time (zero in the off-time),
`dI = (Vin - Vout) D / (L f)` with the value of the power inductor on the switch net (when it cannot be read, a
ripple of 0.2, 0.3 and 0.4 x the load current, tagged `rule_of_thumb_ripple`); the switch pin's `current_peak` then
appears as a secondary worst case, `measures.secondary_scenario`. The hot-loop estimates of the same cell
(`EMI-HOT-LOOP` radiated field, stored energy and overshoot, and the near-field map) use the same scenario
(`measures.scenario`, `operating_point` and `loop_current` of the hot-loop finding; tags `scenario_declared_operating_point`
or `scenario_current_limit`): the field of a declared point is the exact harmonic lines of the high-side switch current,
the stored energy and overshoot are figured at the turn-off current `I_out + dI/2` (the turn-on valley and the current
limit also given), and the map draws that current. Radiated findings are ordered by their headline level in dBuV/m, each in
its own cell's scenario, and `limits.notes` says so when a board holds both scenarios. A declared point with `I_out < dI/2` is
discontinuous conduction, pulse skipping or forced-PWM light-load operation, which the model does not cover: the
finding says so (`measures.operating_point.status = outside_model`, tag `operating_point_outside_model`) and the current-limit
scenario is the only one. `current_limit`: no operating point is declared, and the switch pin's `current_peak` is the amplitude of
a flat-top pulse, a worst case that bounds operation and is not a prediction of it (tag `worst_case_current`; declare
`load_current` on the component to give the cell an operating point). The current commutation time is the switch pin's
`current_rise_time`/`current_fall_time`, else the voltage edge stands in for it (tag `current_edge_from_voltage_edge`),
and drives the conducted spectrum and the hot-loop radiated estimate; the voltage edge (`rise_time`/`fall_time`,
`slew_rate`) keeps the switch-node common-mode score and the ringing. Every capacitor of the hot net and of the supply side of a
recognised series input filter (one inductor or ferrite between the hot-supply net and an upstream net with capacitors to
the hot return) is its own branch with its ESR and ESL (the part's `esr`/`esl`/`self_resonance`, else package defaults tagged
`default_capacitor_parasitics`) and, for the capacitors other than the hot capacitor, an estimated connection
inductance (tag `estimated_connection_inductance`); the hot-loop inductance sits between the converter and the hot
capacitor's node and does not change the level at the LISN. A ceramic capacitor's DC-bias loss is a package rule of thumb
(tag `rule_of_thumb_dc_bias`) unless its component declares `capacitance_at_bias`. The finding reports, per capacitor, which
values were declared and which defaulted, the capacitor with the largest share of the ripple current at the fundamental and at the worst
harmonic (a ratio |I_branch| / |I_source| of complex-current magnitudes, not a partition: the ratios can add to more than
100 %), and every anti-resonance of the input network between 150 kHz and 30 MHz (`measures.capacitor_branches`,
`largest_current_share_*`, `anti_resonances`, each with its height over the best single capacitor branch); only one that
raises a reported harmonic at the LISN by at least 6 dB over that reference is tagged `anti_resonance_in_band` and named in the
headline. A missing edge time
becomes a default slew band (3 V/ns nominal, 10 V/ns fast, 1 V/ns slow on the
hot-supply voltage, tagged `rule_of_thumb_edge`) and every harmonic to 30 MHz
is reported with `low_dbuv` and `high_dbuv`; the headline and the margin use
the high end. The radiated estimates (`EMI-HOT-LOOP`, `EMI-SWITCH-NODE`) use
the same band: the nominal level with `band_db` `[slow, fast]` and the margin
from the fast bound. Other missing inputs skip the prediction with the reason
as coverage. With a `cispr32_class_a` or `cispr32_class_b` `emission`
constraint the worst harmonic carries a margin to the quasi-peak and average
lines, but only where the levels are detector readings: `measures.detector_applicability`
is `isolated_lines` when the switching frequency is one constant value and the harmonic
spacing exceeds twice the receiver bandwidth (9 kHz in Band B), each harmonic then being an
isolated line with peak = quasi-peak = average. When the spacing is too small but the frequency is one constant value it is
`receiver_emulation`: at most 48 harmonics (the least-margin ones, the highest ones and a spread over the band, stated in
`measures.receiver_emulation.selection_rule`) are run through a time-domain receiver model of the periodic waveform, with
the Gaussian IF filter of each CISPR band and quasi-peak time constants remembered from CISPR 16-1-1 and unverified; only
those harmonics carry `limit_qp`, `limit_avg`, margins and a `receiver` object with the peak, quasi-peak and average
readings, the finding stays `conducted_emission` tagged `detector_receiver_emulation`, `margins_scope` is
`evaluated_harmonics_only`, and the other harmonics are line amplitudes. Otherwise it is `not_applicable` (with
`detector_reason` and the per-band `detector_bands`): the part
declares `switching_frequency_min`/`_max` without the component fixing the frequency. The
finding is then `conducted_line_amplitude` instead of `conducted_emission`, tagged
`detector_not_applicable` and `line_amplitudes_only`, the levels are line amplitudes and are
not detector readings, and no limit, margin or `EMI-CONDUCTED` wording about a limit is
produced (`limit_qp`, `limit_avg` and the margins are `null`; `--plot-spectrum` draws the
panel without limit lines). A declared frequency range is evaluated as two separate
fixed-frequency scenarios, its lower and its upper bound (one `EMI-CONDUCTED` finding each,
coverage `EMI-CONDUCTED:<cell>@min` and `@max`, tags `frequency_scenario_min`/`_max`); the swept
or modulated case is not modelled. Declare `switching_frequency` on the component to fix the
frequency and get the detector comparison. Conducted levels (dBuV) are not comparable with the
radiated ones (dBuV/m).

`EMI-HOT-LOOP` is the only absolute radiated estimate (the differential-mode small-loop field,
`kind = absolute_screening`). `measures.radiated_validity` states its conditions at the frequency
and distance used: the loop path at most a tenth of a wavelength (the sweep stops at that
frequency, tag `radiated_band_truncated` and a sentence in the message when it is below 1 GHz),
the distance beyond lambda/2pi at 30 MHz, and the distance at least three times the loop's
longest dimension; the tag `small_loop_validity_failed` and the message say which fails.
`EMI-SWITCH-NODE` reports the voltage-driven common mode as a coupling-risk SCORE
(`cm_coupling_score`, `kind = ranking_score`, unit dB, tag `ranking_only`):
`20 log10(f C V / (100 MHz x 1 pF x 1 V))` at its worst point, with `measures.cm_drivers`
giving the island capacitance, swing, edge time, frequency and duty it comes from. It is
relative, not a field: it has no distance, no field-per-current constant, no margin, never
appears against a limit line, and ranks after the absolute estimates. An absolute prediction
needs the cable and attached-conductor geometry, the board-to-environment coupling and the
return impedance of the common-mode path (`estimate.not_modelled`,
`measures.missing_for_absolute_prediction`).

`--fullwave` adds, for each `buck` cell (asynchronous or synchronous), the hot-loop
inductance from a region full-wave (FDTD) model of the loop on the pinned `ems`
solver (`copper-fullwave`): the finding gains a `fullwave:` line and
`measures.fullwave` (`inductance_nh`, `status`, `mesh_sensitivity_pct`, `mesh_difference_pct`, `not_modelled`,
`meshes`, `closed_form_nh`, `ratio`, `port_strap_nh`,
`inductance_static_nh`, `band_mhz`, `flatness`, `cells`, `steps`, `seconds`, `flags`,
`solver`, `model`, ...), and coverage `EMI-FULLWAVE:<cell>` says `executed`,
`not_applicable` (other topologies) or `failed` with the reason (for example a model
over `--fullwave-budget`, which is checked before the solver starts; a run that
takes half as long again as the budget is stopped by a wall-clock guard and reported
as `budget_exceeded` with its partial statistics, `measures.fullwave.status`). The
mesh is refined (about 0.2 to 0.25 mm) around the loop and coarser elsewhere. Unless
the budget is short, two meshes are run, the standard one and one with the refined
cell divided by 1.5 (or, when that does not fit, multiplied by 1.5): the reported
`inductance_nh` is that of the finer of the two and `mesh_difference_pct` is their
relative difference, a measured difference between two discretisations, not an error bound.
`mesh_sensitivity_pct` is the numerical sensitivity of `inductance_nh` to the mesh: the larger of that
difference and a floor of 4 % (`mesh_sensitivity_floor_pct`), the scatter of L over meshes of one loop that
two meshes cannot see (the placed example reads 4.62 to 4.90 nH over eleven meshes while its standard and
refined meshes differ by 0.5 %); it is a sensitivity, not an accuracy, and it leaves out the model's
omissions, which `not_modelled` names (components, copper loss, zone fills, via barrels, the window's cut nets,
the assumed stack-up and the strap bias where they apply). When the record did not decay and the ring-down
fit did not pass its checks (`energy_not_converged` without `ring_fitted`; `ring_fit_rejected` when a fit was tried), `status` is `indicative`,
`mesh_sensitivity_pct` is `null`, `mesh_sensitivity_unavailable` and `status_reasons` give the reasons, and `inductance_nh` is the plain transform of the truncated record, a marked number. A fit is accepted only when it predicts
the withheld last 30 % of its tail to 10 %, its model orders agree to 5 %, none of its poles grows over the tail and the completed response is passive; `measures.fullwave.ring_fit` states each figure and its limit.
The pours are the native zone fill of the exported board (`zone_fill_used`) unless `--fullwave-zone-outline` asks for the zone outlines minus clearances or the export fails (a board with an unmaterialised hard macro, for one);
`inductance_lowest_nh` and `lowest_frequency_mhz` give L at the lowest analysis frequency, which the extraction's cross-check reads. `--fullwave-single-mesh`,
or a budget that fits one mesh only, gives one mesh: `mesh_difference_pct` is `null`, the sensitivity is the
floor and the flag `single_mesh` is set. A loop whose record has not decayed (it rings) is completed by a fit of its
ring-down, ends when the fit is stable instead of at the 3x time cap, and gets the flag `ring_fitted` (and a
note with the ring frequency and Q) instead of `energy_not_converged`. A port whose pads do not face each other across free space is
joined to them by short copper straps (`port_strap`; in the air above a package's
exposed pad, `port_strap_in_air`); their closed-form inductance is `port_strap_nh`, a
bias that the inductance includes. A loop that resonates under 750 MHz gets
`band_near_resonance` and `inductance_static_nh`, L(f) with the resonance factor
removed. It is never part of the default run and takes minutes per cell;
`--fullwave-fidelity fine` halves the general copper cell: about twice the work on the placed example. Screening
evidence, see [the EMI specification](emi-noise-analysis.md#11-region-of-interest-full-wave-phase-4).

`--extract` adds, for each `buck` cell, the hot-loop **interconnect impedance** from a quasi-static
partial-element (PEEC) extraction of the real filled copper (`copper-extract`): the board is exported in memory
and its zones are filled with the native filler (`copper fill`; thermal reliefs, clearances, openings and
islands as in the board), the tracks, pads, vias and fills of the loop's nets and of the planes that reach the
window are meshed layer by layer into rectangular filaments, and the loop equations of `R + jwL` are solved at
1, 10 and 100 MHz. The finding gains an `extract:` line and `measures.extract`
(`inductance_nh`, `resistance_mohm`, `frequencies_mhz`, `closed_form_nh`, `ratio_to_closed_form`, `fullwave_nh`
and `ratio_to_fullwave` where `--fullwave` ran, `status`, `status_reasons`, `numerical_sensitivity_pct`
(the difference of the two finest meshes of a series of three, per frequency; `null` with
`numerical_sensitivity_unavailable` for a single mesh) and `numerical_sensitivity_earlier_pct` (the step before),
`convergence_threshold_pct`, `cross_check` (with `--fullwave`: the extraction's 100 MHz value beside the full wave's at its lowest frequency with both numerical
sensitivities and a plain statement of agreement within them, disagreement beyond them, or none; never a pass or fail), `port_labels`, `inductance_matrix_nh`,
`numerical_sensitivity_basis`, `parameter_assumptions` (inputs the board does not fix), `not_modelled`
(the omissions), `flags`, `notes`, `meshes`, `via_currents` (vias that carry at least 1 % of the port current,
with their share of the net's via current), `nets`, `window_mm`, `layers`, `copper`, `seconds`, `method`), and
the coverage entry `EMI-EXTRACT:<cell>` says `executed`, `not_applicable` (other topologies) or `failed` with
the reason. The status is `indicative` when a terminal had to be joined by its nearest node, when a via joins fewer
than two layers, when the two finest meshes differ by more than 3 % at 10 MHz or when only one mesh was run. The
capacitor and the catch diode are copper bars between their pads, the switch is the port in a
gap in a bar between its pads (`--extract-no-switch-bar` puts the port between facing pad sides instead), and
no lumped element is modelled. The other capacitors of the cell that hang on the hot node get a port across
their pads in the same extraction; the resulting inductance matrix (the hot loop and those capacitors, at 10 MHz)
replaces the estimated connection inductances of `EMI-CONDUCTED` and becomes mutual inductances between them (tag
`extracted_connection_inductance`, `measures.extracted_connections`, the estimate kept in
`connection_inductance_estimate_nh`; tag `extraction_indicative` when the extraction is only indicative);
capacitors on the supply side of an input filter keep the estimate. The dense inductance matrix bounds the mesh:
`--extract-budget` (default 9000 filaments, at most 20000) is the most filaments of the finest mesh, and a larger
mesh is coarsened by 1.3 until it fits (flag `coarsened`); the two other meshes of the series have pitches 1.5 and 2.25
times larger. `--extract-margin` (default 3 mm) sets the window margin and the corridor the zone fills are cut
to, `--extract-cell` the fine pitch (default 0.2 mm), `--extract-single-mesh` runs one mesh. Screening
evidence, never part of the default run; see
[the EMI specification](emi-noise-analysis.md#13-quasi-static-network-extraction-phase-5).

The hot loop is the polygon through pad centres (`placement_estimate`) until
copper is present on its nets. Where tracks, arcs, vias, polygons or the copper
of hard macros join the loop's pads, each copper leg is replaced by the
shortest copper path (`traced_copper` when every copper leg is traced,
`partially_traced` with `unrouted_leg` otherwise; a path through a polygon or
zone is a straight line, `plane_shortcut`), and the finding reports traced
length, enclosed area, via count, layer changes, narrowest width and the
inductance of the traced bars. `--run RUN` analyses the routed board of a
`route-board --checkpoint-dir` run (`final-board.state.json`, a file or the
directory holding it) instead of the placed prototype; a run that holds only a
`.kicad_pcb` cannot be loaded. Positions that no `fixed_placement` fixes are
marked `provisional_placement`.

`--plot-spectrum OUT.svg` writes the conducted spectrum of every cell with an
`EMI-CONDUCTED` finding as an SVG (harmonic markers and envelope 150 kHz to 30 MHz in dBuV, stems for the first ten, the worst harmonic ringed with its margin to each limit, the
quasi-peak and average limit lines across the whole band when a conducted `emission` standard is
declared and the levels are detector readings, and the assumptions as plain sentences; a `receiver_emulation` panel marks the harmonics
the receiver model read with diamonds and draws the limits through them only; a panel whose levels are only line
amplitudes is headed "NOT a detector reading" and draws no limit line or margin). The common-mode score is
a ranking and is never plotted.
`--plot-field OUT.svg [--field-height 1mm]` (and `copper view RUN --issue ID
--emi-field` for an `EMI-HOT-LOOP` issue of a run) draws the quasi-static
magnetic near field `|H|` of the hot-loop current path, in dB(uA/m) in a plane
above the top copper, over the layout, in dB relative to the map maximum with 10 dB contours, the loop drawn from its traced
path (solid on the component layer, dashed on another layer, circles at vias), and the level below the peak at the pads of `sensitive`
nets and cable connectors. Both are screening estimates: the field uses the amplitude and scenario of the hot-loop
finding (`I_out + dI/2` at a declared operating point, else the loop's `current_peak`, per ampere without one; the map
names which) with an ideal ground plane by images, and does not include the power inductor's own field (coverage
`unsupported EMI-FIELD:inductor`). See [the EMI specification](emi-noise-analysis.md).

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

### Routing search costs

The global guide search adds soft layer- and direction-preference rates of 2
and 1 cost units/mm on top of a baseline distance rate of 10 cost units/mm;
they are not electrical properties or hard layer restrictions. Via and bend
events have separate fixed costs. The unified router prices its own lattice through the
`--unified-set OPTION=VALUE` options and `--unified-pitch-mm`.

Accepted ordinary copper is straightened by default; `--unified-set route_smoothing=false`
keeps it as searched. Smoothing never moves vias, reserved or immutable copper.

## KiCad schematic export

Generate a KiCad 8 schematic after ERC succeeds:

```console
copper export-kicad board.copper -o board.kicad_sch
```

The generated schematic is a derived artifact, not CopperScript source. The
current backend embeds generic symbols generated from each part definition,
uses named labels for connectivity, and owns all visual placement. It emits a
single flat sheet; hierarchical component paths are preserved as hidden
`CopperScriptPath` properties. `--no-check` permits diagnostic fixtures to be
exported despite ERC errors.

## Quantities

Quantities accept compact or spaced forms (`0.25mm` or `0.25 mm`). Human-facing
engineering values use a space and the shared fixed-point quantity formatter.

| Dimension | Units |
|---|---|
| Voltage | `V`, `mV` |
| Resistance | `ohm`, `kohm`, `Mohm` |
| Capacitance | `F`, `uF`, `nF`, `pF` |
| Inductance | `H`, `mH`, `uH`, `nH` |
| Length | `m`, `mm`, `um` |
| Current | `A`, `mA`, `uA` |
| Frequency | `Hz`, `kHz`, `MHz`, `GHz` |
| Time | `s`, `ms`, `us`, `ns` |

Values are normalized to SI base units in the IR while retaining their source
display unit.

## Grammar sketch

```ebnf
document       = design_document | macro_document ;
design_document = ("board" | "module" | "part" | "device" | "board_profile"), name, "{", item*, "}" ;
macro_document = "macro", name, "{", macro_definition_item*, "}" ;
(* Macro definition items, nested values and geometry blocks are specified
   in macro-definitions.md; electrical items are not permitted there. *)
item           = library | package_import | port | pin | pad | power_domain
               | part_property | peripheral | mux | route | pad_set | resource
               | unit | signal_group | mode_group | device_property
               | configuration | module_instance | component | net | supply
               | power_state | interface | constraint | mechanical | silkscreen
               | physical_macro | no_connect | build ;
build          = "build", "{", (build_setting | build_group)*, "}" ;   (* board only, at most one *)
build_group    = ("placement" | "routing" | "planes" | "router"), "{", build_setting*, "}" ;
build_setting  = name, "=", scalar, ";" ;
silkscreen     = "silkscreen", "{", silk_item*, "}" ;
silk_item      = "branding", geometry_properties
               | ("text" | "logo" | "line" | "rectangle" | "circle" | "arc" | "polygon"), name, geometry_properties ;
geometry_properties = "{", (name, "=", (scalar | coordinate | coordinates), ";")*, "}" ;
coordinate    = "(", scalar, ",", scalar, ")" ;
coordinates   = "[", coordinate, (",", coordinate)*, "]" ;
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
physical_macro = "physical_macro", name, ":", qualified_name, ["on", instance_path],
                 "{", macro_mapping*, "}" ;
macro_mapping  = ("member" | "net"), (name | string), "=", target_reference, ";" ;
component      = "component", name, ":", qualified_name, (";" | properties) ;
net            = "net", name, "{", endpoint*, "}" ;
no_connect     = "no_connect", endpoint_ref, (",", endpoint_ref)*, (";" | properties) ;
endpoint_ref   = name, ".", qualified_name ;
endpoint       = name, ".", qualified_name, ";" ;
supply         = "supply", name, properties ;
power_state    = "power_state", name, "{", (name, "=", ("on" | "off" | "unknown"), ";")*, "}" ;
interface      = "interface", name, ":", qualified_name, "{", interface_item*, "}" ;
constraint     = "constraint", name, "(", targets, ")", properties ;
properties     = "{", (name, "=", scalar, ";")*, "}" ;
scalar         = string | boolean | name | number, unit? ;
```
