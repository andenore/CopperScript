# EMI noise-source analysis: language and tool specification

Status: draft specification (branch `emi-noise`). Background and sources are in
the research report `reports/CopperScript EMI noise analysis.md` (outside this
repository). This document fixes the language surface and the phase boundaries
so that each phase can be implemented and reviewed on its own.

Every result described here is **screening evidence**. It ranks likely noise
sources and makes layout review repeatable; it never establishes EMC
qualification, and no check may turn a missing input into a pass.

## 1. Design rules for the syntax

1. Flat `name = value;` attributes on existing declarations, like every other
   pin, part and component attribute. No new block forms.
2. Everything is optional. A library without the new attributes compiles to the
   same IR as today and produces no new diagnostics ("unannotated libraries stay
   quiet", the rule the logic-level attributes follow).
3. Values are datasheet facts with units (`rise_time = 5ns`), never tool
   settings. Thresholds live in the checker, expressed in physical terms.
4. Net classes are derived from pin roles, never declared on nets.
5. The serialized IR carries the new data only when declared.

## 2. Pin and device-pad attributes

Accepted on `pin` in a `part` and on `pad` in a `device`:

| Attribute | Type | Meaning |
| --- | --- | --- |
| `emi_role` | name | The pin's part in a switching cell or its sensitivity; see below |
| `rise_time` | time | 10-90 % output rise time of the pin's edge, datasheet typical |
| `fall_time` | time | 10-90 % fall time; defaults to `rise_time` when only one is given |
| `slew_rate` | slew rate | Switch-node dv/dt, datasheet or measured (`50V/ns`); gives the edge time and classifies the device |
| `current_peak` | current | Peak pulsed current through the pin (switch current limit or rated peak) |
| `current_rise_time` | time | 10-90 % time in which the switch current commutates (turn-on, set by loop inductance and drive; not the voltage edge) |
| `current_fall_time` | time | 10-90 % current commutation time at turn-off; defaults to `current_rise_time` when only one is given |
| `data_rate` | data rate | Bits per second of the pin's signal, NRZ assumed (`2.5Gbps`); see section 14 |
| `clock_frequency` | frequency | The fundamental of a clock output (`24MHz`); see section 14 |
| `signal_swing` | voltage | The loaded voltage at the driver's pins; for a differential pin the differential peak-to-peak (`400mV`); see section 14 |
| `spread_spectrum` | ratio | Down-spread fraction of a spread clock (`0.5%`); recorded, not modelled |
| `output_impedance` | impedance | The impedance behind a driver's open-circuit swing (`50ohm`); used only with `open_circuit_swing`, see section 18 |
| `open_circuit_swing` | voltage | The driver's open-circuit swing, the Thevenin representation of the source (`2V`); exclusive with `signal_swing`, see section 18 |

`passes_to = <pin>` (pins of parts with more than two pins only, not device pads) is described with the signal walk in
section 14. A source is declared **once, in one representation**: `signal_swing` is the loaded voltage at the pin while it drives the
line (the source impedance is already in it, and `I_A = V_pin / Z_A`), or `open_circuit_swing` with `output_impedance` is a Thevenin source
(`V_pin = V_oc Z_A / (Z_source + Z_A)`, `I_A = V_oc / (Z_source + Z_A)`). Declaring `signal_swing` with either of the other two, or one of
them without the other, is `CMP130`: dividing a loaded voltage by `Z_source + Z_A` would apply the source loading twice (6 dB low in the
matched case). `data_rate` takes `bps`, `kbps`, `Mbps` or `Gbps` (`Dimension::DataRate`); `spread_spectrum` takes `%` only
(`Dimension::Ratio`, base value the fraction), and the parser accepts `%` as a unit only directly after a number.

`emi_role` values:

| Value | Class | Meaning |
| --- | --- | --- |
| `switch` | aggressor | The switch node pin (`SW`, `LX`, `PH`): rail-to-rail dv/dt |
| `hot_supply` | aggressor | The pin that carries the pulsed (trapezoidal) current of the cell: `VIN`/`PVIN` of a buck, the output pin of a boost |
| `hot_return` | aggressor | The pulsed-current return pin (`PGND`); for parts with one ground pin, that pin |
| `bootstrap` | aggressor | High-side bootstrap pin (`BOOT`, `BST`) |
| `gate_drive` | aggressor | Gate-driver output for an external switch |
| `feedback` | victim | High-impedance regulation input (`FB`, `VOS`, `COMP`) |
| `sense` | victim | Low-level measurement input (current sense, Kelvin sense, reference) |
| `shield` | victim | A connector pin bonded to the cable's shield or shell (section 18); never an aggressor, it makes no switching cell |

```copper
part TPS54360DDAR {
    category = "power.regulator.buck";
    switching_frequency_min = 100kHz;
    switching_frequency_max = 2.5MHz;
    pin BOOT { number = "1"; domains = "power,analog"; directions = "passive"; emi_role = bootstrap; }
    pin VIN  { number = "2"; domains = "power"; directions = "input"; voltage_min = 4.5V; voltage_max = 60V;
               emi_role = hot_supply; }
    pin FB   { number = "5"; domains = "analog"; directions = "input"; emi_role = feedback; }
    pin GND  { number = "7"; domains = "ground"; directions = "input"; emi_role = hot_return; }
    pin SW   { number = "8"; domains = "power"; directions = "output";
               emi_role = switch; rise_time = 5ns; current_peak = 4.5A; }
}
```

Compile-time validation (new code `CMP130`): unknown `emi_role`; `rise_time`,
`fall_time`, `current_rise_time`, `current_fall_time` not a positive time; `slew_rate` not a positive slew rate;
`current_peak` not a positive current; `rise_time`/`fall_time`/`slew_rate`/
`current_peak`/`current_rise_time`/`current_fall_time` given without `emi_role` being one of the aggressor roles is
allowed (a clock output may declare an edge rate).

`slew_rate` is a quantity written with a slash unit: `V/ns`, `V/us` or `kV/us`
(base unit V/s; `Dimension::SlewRate` in `copper-units`). The lexer yields
`50V` and `/ns` as separate tokens already, so the parser joins an adjacent `/`
and identifier to the unit of a number when, and only when, the joined text is
one of those three units (`3mV/ns` stays a parse error, and `/` elsewhere is
unchanged); this is a few lines in one parser function and needs no lexer
change, which is why the quoted micro-syntax (`"50V/ns"`) was not used. It is
not an enum of device technologies: a silicon or wide-bandgap class is inferred
from the value (20 V/ns or more is reported as wide-bandgap class by
`emi-check`, information only). Declaring both `slew_rate` and `rise_time` on
one pin is allowed: `rise_time` wins for the timing and `slew_rate` still
classifies the device. The compiler cannot know the swing, so it does nothing
when they disagree; `emi-check` knows the hot-supply voltage and notes a
disagreement of more than a factor 3 in the `input:` lines.

IR: `profile.emi = { "role", "rise_time", "fall_time", "slew_rate", "current_peak", "current_rise_time", "current_fall_time" }`, each
key present only when declared; the whole object absent when nothing is. The
`fall_time` default is applied by readers (`EmiProfile::fall`), not stored.
The `emi_role` value is a bare name or a string; time attributes accept `s`,
`ms`, `us` and `ns`, and `slew_rate` accepts `V/s`, `V/us`, `kV/us` and `V/ns`. Only the EMI attributes take time units: the general
quantity grammar (`quantity_dimension`) is unchanged.

## 3. Part attributes

| Attribute | Type | Applies to | Meaning |
| --- | --- | --- | --- |
| `switching_frequency` | frequency | converters, clock sources | Fixed switching frequency |
| `switching_frequency_min`, `switching_frequency_max` | frequency | converters | Adjustable or spread-spectrum range; exclusive with the fixed form |
| `topology` | name | converters | `buck`, `boost`, `buck_boost`, `inverting`, `flyback`, `charge_pump`; overrides what `category` implies |
| `shielding` | name | inductors, transformers | `shielded`, `semi_shielded`, `unshielded` |
| `esl` | inductance | capacitors | Equivalent series inductance |
| `esr` | resistance | capacitors, inductors | Equivalent series resistance (DCR for inductors) |
| `self_resonance` | frequency | capacitors, inductors | Self-resonant frequency |
| `output_capacitance` | capacitance | converters, FETs, diodes | Switch-node device capacitance (Coss, Cj) used for ringing estimates |
| `external_cable` | bool | connectors | A cable leaves the board here (antenna for common-mode current) |
| `cable_shielded` | bool | connectors | The cable the connector takes is shielded (section 18) |
| `cable_common_mode_impedance` | impedance | connectors | The common-mode impedance the cable presents at the connector; absent, the cable is taken as matched to the channel's common-mode reference (section 18) |
| `common_mode_impedance`, `common_mode_impedance_frequency` | impedance, frequency | common-mode chokes, ferrites | The series impedance a common-mode current sees through the part, at the declared frequencies (below) |

Validation (`CMP130`): wrong dimension, non-positive value, `min > max`, fixed
and range frequency together, unknown `topology` or `shielding` name,
`external_cable` that is not `true` or `false`. A range may give only one
bound. The units are the ones of `copper-units`, so an ESR below 1 ohm is
written as a decimal (`esr = 0.01ohm`).

`common_mode_impedance` is declared with `common_mode_impedance_frequency`, either as one point of two quantities
(`common_mode_impedance = 120ohm; common_mode_impedance_frequency = 100MHz;`) or as several points in two quoted lists of equal length,
the frequencies strictly increasing (`common_mode_impedance = "90+j500 ohm, 681.5+j0 ohm"; common_mode_impedance_frequency =
"100 MHz, 1.25 GHz";`). An entry is a magnitude (`120 ohm`, the phase unknown) or a complex value `R+jX` (`X` signed, inductive
positive; `R` not negative). Zero is accepted (a zero-impedance choke changes nothing). The impedance is the series impedance **in the
common-mode loop**, that is, what the total common-mode current through the part sees; the datasheet's common-mode impedance of a
four-pin choke is declared as it is, and a ferrite in each leg is declared per ferrite (the check takes the two in parallel for the common
mode, half of one). Between two or more declared points the magnitude is interpolated on log axes (and a complex value's phase linearly in
the logarithm of frequency); **nothing is extrapolated**, and a part evaluated outside the declared range, or at a frequency other than the
one point it declares, is present with its attenuation not modelled. Validation (`CMP130`): `common_mode_impedance` without
`common_mode_impedance_frequency` or the reverse, lists of different length, frequencies not strictly increasing or not positive, an entry
that is not an impedance or has a negative magnitude or resistance, `cable_shielded` not `true` or `false`,
`cable_common_mode_impedance` not a positive impedance.

IR: `part.emi = { ... }` with only declared keys; `common_mode_impedance` is a list of `{ frequency, magnitude }` or
`{ frequency, resistance, reactance }`.

When `topology` is absent it is taken from `category`:
`power.regulator.buck` -> `buck`, `.boost` -> `boost`, `.buck_boost` ->
`buck_boost`, `.inverting` -> `inverting`. Other categories imply nothing.

## 4. Component attributes

A board fixes what a datasheet leaves open (for example the frequency set by an
`RT` resistor):

```copper
component U7: TPS54360DDAR { switching_frequency = 500kHz; }
```

`switching_frequency` on a component must lie inside the part's declared range
(or equal its fixed value) when there is one (`CMP130` otherwise) and
overrides the part's value. Serialized as
`component.properties.switching_frequency`, as quantity text (`"500 kHz"`).

A board also fixes how a converter is used and what a capacitor does at the
bias the board applies:

```copper
component U7: TPS54360DDAR { switching_frequency = 500kHz; load_current = 2A; }
component CIN: Cap { value = 10uF; capacitance_at_bias = 4.2uF; role = decoupling; decouples = U7.VIN; }
```

* `load_current`: the operating point of a switching converter, the mean inductor
  current of a buck, a positive current (`A`, `mA`). One operating point per cell
  (a second point would be a second declaration of a different kind; the supported
  model has none). The input and output voltages are the declared `supply` voltages
  of the hot-supply net and of the inductor's output net, so no voltage attributes
  exist to disagree with them. `CMP133` when it is not a positive current, when the
  part's topology (declared or implied by its category) is not `buck`, or when it
  exceeds the `current_peak` of the part's switch pin. A component that declares it
  without being a switching cell gets the warning `EMI_OPERATING_POINT_NO_CELL`.
  Serialized as `component.properties.load_current` (`"2 A"`); the cell carries
  `load_current` and `copper query BOARD emi` prints it.
* `capacitance_at_bias`: the effective capacitance of a capacitor at the DC bias of
  its rail (the datasheet curve at the board's voltage), a positive capacitance at
  most the component's value (equal to it for a C0G part). It replaces the
  DC-bias rule of thumb of section 10.1 for that capacitor. `CMP133` when it is not a
  positive capacitance, when the component's value is not a capacitance or is above
  the value. Serialized as `component.properties.capacitance_at_bias`.

Hot-loop capacitors need no new syntax: the existing
`role = decoupling; decouples = U7.VIN;` on a capacitor whose target pin has
`emi_role = hot_supply` marks it as a pulsed-side capacitor.

## 5. Constraints

### `emission`

```copper
constraint emission(J1, J4) {
    standard = "cispr32_class_b";   // cispr32_class_a | cispr32_class_b | cispr25_class_5 | fcc15_class_a | fcc15_class_b
    cable_length = 1m;              // optional, default 1 m
    mode = target;
}
```

Targets are the connector components that carry cables (pin targets are
`CMP131`); a connector whose part declares `external_cable = true` is included
without being named. `standard` is required and one of the five names above;
`cable_length` must be a positive length; no other parameter is accepted.
With no `emission` constraint the checker ranks findings relative to each other
and reports no margins. At most one `emission` constraint per compiled
board or module body (`CMP131`). Unknown targets are reported by ERC as
`UNKNOWN_CONSTRAINT_TARGET`.

### `emi_accept`

Acknowledges one reviewed finding, in the style of `no_connect { reason }`:

```copper
constraint emi_accept(U7.SW) {
    code = "EMI-SWITCH-NODE";
    reason = "Island area fixed by the DDA thermal pad; scanned 2026-11, 6 dB margin";
}
```

The target is the pin or component the finding names; `code` and a non-empty
`reason` are required (`CMP132`). An accepted finding is still listed, marked
`accepted` with its reason, and does not count toward the summary. An
`emi_accept` that matches no finding is itself reported
(`EMI-ACCEPT-UNUSED`), so stale acknowledgements cannot hide.

`emi_accept` is parsed and validated in phase 0 (one target, `code` and
`reason`, no other parameter; ERC reports unknown targets). Matching it against
findings and `EMI-ACCEPT-UNUSED` are implemented with phase 1.

## 6. Derived model (no syntax)

`copper_lang::emi::analyze(view) -> EmiModel` derives, from the flat view:

**Switching cells.** One per component that has at least one `switch` pin:

- `controller`: the component; `topology`; `frequency` (component override,
  else part fixed value, else range, else unknown).
- `switch_nets`: nets of its `switch` pins.
- `magnetic`: a two-pin `passive.inductor*` component (or a part with
  `dc_power_path`) with one terminal on a switch net; `output_net` is its
  other terminal's net (for a buck).
- `hot_supply_net`, `hot_return_net`: nets of the role pins.
- `catch_diode`: for asynchronous cells, a `semiconductor.diode*` component
  between the switch net and the hot return (buck) or hot supply (boost); other
  topologies have none.
- `hot_capacitors`: capacitors joining `hot_supply_net` and `hot_return_net`;
  those declared `decouples = <hot_supply pin>` first. A hot capacitor records its terminal
  on each net.
- `bootstrap_capacitor`: capacitor between a `bootstrap` pin's net and a switch net.
- `provenance`: `declared` when roles come from attributes.

The Rust types (`copper_lang::emi::{EmiModel, SwitchingCell, ...}`) carry the
component reference and `component.pin` endpoints of every role pin, magnetic,
diode and capacitor, plus the first declared `rise_time`, `fall_time`, `slew_rate`,
`current_peak`, `current_rise_time` and `current_fall_time` of the switch pins, the component's `load_current` and the
`capacitance_at_bias` of each hot capacitor (`null` when undeclared; `copper query BOARD emi` prints all of them), so a
layout checker needs no second lookup.
`EmiModel::to_json` prints the cells and nets as
`copperscript.emi-model/v0.1` (keys sorted, absent facts `null`).

**Structural fallback.** A component whose part category starts with
`power.regulator.` and declares no `emi_role` anywhere is examined
structurally: a `power`-domain `output` pin on a net that has no declared
supply and reaches an inductor terminal is taken as the switch pin; power
inputs named `VIN*`, `PVIN*`, `VI*` or `IN*` on a net with a declared supply as
hot supply; ground-domain pins as hot return. A component with no switch pin
found yields no cell. Such a cell has `provenance = inferred`, is used by `copper emi-check`
with every finding marked `inferred`, and produces **no** source diagnostics.

Libraries without categories are covered too: a part with no `category` is
examined the same way when it also has such a supplied input pin, and an
uncategorized two-pin part counts as an inductor or capacitor when its
component `value` is an inductance or capacitance, and as a diode when its pins
are named anode and cathode (`A`/`K`).

**Net classes.** Each net gets at most one aggressor class and one victim flag:

| Class | Rule |
| --- | --- |
| `switch_node` | has a `switch` pin |
| `pulsed_power` | a cell's hot supply or hot return net, between the hot capacitor and the pins |
| `bootstrap` | has a `bootstrap` pin and is not a switch node |
| `gate_drive` | has a `gate_drive` pin |
| `fast_edge` | has an output pin with `rise_time` or `fall_time` below 2 ns, or route kind `clock` |
| victim `sensitive` | has a `feedback` or `sense` pin, or route kind `rf_feed`, or is an antenna feed |
| `external` | reaches a pin of a cable connector directly or through one series resistor, ferrite or `dc_power_path` part that is not an inductor; the hop never enters a net with a declared supply |

Aggressor classes are listed in precedence order; a net takes the first that
applies, and `pulsed_power` marks the whole hot supply and hot return nets in
phase 0. Each net entry also records its `provenance` (declared or inferred),
the `feedback`/`sense` pins on it and the connectors it reaches. A `fast_edge`
output is a pin whose direction includes output with `rise_time` or
`fall_time` below 2 ns; route kinds are read from `routing` constraints
(`kind = clock`, `kind = rf_feed`) and an antenna feed is the net of an
`antenna` constraint's target pin.

`copper query BOARD emi` prints the cells and net classes as JSON
(`copperscript.emi-model/v0.1`); it has no `board` or `query` envelope.

## 7. Source-level diagnostics (phase 0)

Run inside `copper check` from `check_view`, on declared cells only, all
warnings:

| Code | Condition |
| --- | --- |
| `EMI_HOT_LOOP_NO_CAPACITOR` | a cell has a hot supply and hot return net and no capacitor joins them |
| `EMI_HOT_CAPACITOR_UNDECLARED` | capacitors join them but none declares `decouples` of the hot-supply pin, so placement and routing cannot prioritise the loop |
| `EMI_SWITCH_NODE_NO_MAGNETIC` | a switch net reaches no inductor or transformer terminal (charge pumps excluded) |
| `EMI_BOOTSTRAP_NO_CAPACITOR` | a `bootstrap` pin's net has no capacitor to a switch net of the same component |
| `EMI_SENSITIVE_ON_SWITCH_NODE` | a `feedback` or `sense` pin sits directly on a `switch_node` net |
| `EMI_SWITCH_NODE_EXTERNAL` | a `switch_node` net is also `external` (leaves the board unfiltered) |
| `EMI_SWITCHING_FREQUENCY_UNKNOWN` | a declared cell has no fixed frequency on part or component; estimates will use the range or be skipped |
| `EMI_OPERATING_POINT_NO_CELL` | a component declares `load_current` but is not a switching cell (no `switch` pin, and the structural fallback found none), so the operating point is not used |

All are warnings with the component reference, net or `component.pin` as the
subject. `EMI_BOOTSTRAP_NO_CAPACITOR` also fires for an unconnected bootstrap
pin, and `EMI_SWITCHING_FREQUENCY_UNKNOWN` for a part that gives only a range.
Each code gets a `copper explain` entry (the `CMP` codes, like other compile
errors, have none).

## 8. `copper emi-check` (phase 1)

Status: implemented in `copper_drc::emi` (`emi_check(board, view, model)`) and
`crates/copper/src/emi_check.rs`; what is measured and what is coverage only
is listed below.

```
copper emi-check BOARD [--json] [--run RUN] [build options as for si-check]
```

The build options are those of `si-check` (`--locked`, `--offline`,
`--layers`, `--fab-profile`, `--footprint-root`, `--allow-proxy-footprints`).
The command physicalizes the board, derives the EMI model from the flat view
and measures the placed layout. Exit 0 unless the command itself fails
(findings never change the status); a compile or physicalization error exits 2.
JSON schema `copperscript-emi-check/v0.1`:
`{ schema, board, evidence_grade, cells, findings, coverage, limits, summary }`.

The text report is compact: every line is at most 110 characters (a longer one is wrapped at a word with a hanging
indent). It starts with the board and the standard, then groups the findings by subject kind, in the order of the
figures and of `copper issues`: the converter cells (a heading with the converter, its frequency, input and output
voltage, edge and current scenario), then the signals and pairs, then each cable connector (a heading with the cable
facts). Each finding is one line: its number (the number of `M<number>` in `copper issues`), its code without the
`EMI-`, what it is about and how much it is, in the words and the roundings of the figures (the stat tiles of the cell
page, the mode-conversion chart, the connector chart, the legend of the return-path map). A margin is worded as the
figures word it (`+49.6 dB over average`, `23.3 dB under the limit`), a ranking score always carries `(ranking
only)`, a first-arrival voltage is labelled `first arrival`, and an accepted finding carries `(accepted: reason)`.
A finding code with no formatter of its own shows the first clause of its message and its estimate. Two lines
follow for the coverage: the number of checks that ran, did not apply and did not run (the entries `CODE:subject` are
counted under `CODE`), and the checks that did not run by short name, with a few words of reason when it is known
(`needs --run`, `needs routed copper`, `needs --extract`, `needs --fullwave`), the required ones first. `--extract`
and `--fullwave` add one line under the hot loop with their headline number. `--verbose` prints the full text
instead (every finding with its message, estimate, `not modelled:` list and `input:` lines, and every coverage
entry); `--json` is the complete record and is the same with or without `--verbose`.

```
copper emi-check BOARD --run RUN [--write] [--json]
```

`--run RUN` replaces the placed prototype by a routed board: `RUN` is a
`copperscript.state/v1` file holding a `pcbir.physical:PhysicalBoard`, or a
directory holding `final-board.state.json` (or `checkpoint/final-board.state.json`),
which `route-board --checkpoint-dir DIR` writes for its final board. The board
is decoded with `copper_route::state_codec::physical_board`; the electrical view
still comes from compiling `BOARD`, and the command stops with exit 2 when the
routed board places a reference the design does not have, or connects a pad to a
different net than the design does (the message names the first few pins as
`component.pin: source net vs checkpoint net`; net aliases, pads inside a
footprint's internal pad group and pads the design leaves unconnected are not
differences). A run directory that
holds only `board.kicad_pcb` (and `route-report.json`) is rejected with a
message: `copper_cam::board::Board` and the report scene (`copper issues`,
`copper view`) are drawing models, and no converter from them to a
`PhysicalBoard` (footprint pads with numbers and nets, stack-up, zones) exists.
Writing one is the way to analyse a board routed by other tools.

The checks use the copper the `PhysicalBoard` carries (tracks, arcs, vias,
polygons, zones, and the copper of hard macros moved to the pose their cluster
anchor gives). `physicalize` itself produces none, so without `--run` only a
hard macro contributes copper. With `--run`, and a board that has a subject net, the command also fills the zones of the routed board natively and
runs `EMI-RETURN-PATH` (section 15); the option list is unchanged.

A finding is the `copper_drc::Finding` shape (code, severity, message,
objects, nets, layers, `required_nm`, `measured_nm`) plus
`{ cell, provenance, tags, estimate: { quantity, value, unit, method, kind, definition?, numerical_sensitivity, not_modelled, frequency_hz, band_db?, basis? } | null, margin_db?, accepted?: { reason }, measures }`.
`kind` is `absolute_screening` or `ranking_score`; `definition` (what a score is relative to) is present only for a score. The uncertainty of an estimate is three separate things: `numerical_sensitivity` (the estimate's sensitivity to a numerical choice; `null` for every closed-form estimate of `emi-check`, since nothing is meshed or fitted; the full-wave inductance has its own in `measures.fullwave`), the parameter band `band_db` (`[low, high]`) with `basis` (`rule_of_thumb`), present only when the estimate rests on a rule-of-thumb default (section 10.1), and `not_modelled`, the list of named mechanisms the estimate does not contain. `limits.validation` states what has and has not been validated: the closed forms, the circuit solver and the receiver emulation against hand calculations and analytic cases (yes), correlation with a measurement (none). No accuracy figure, confidence interval or bound is claimed anywhere. `measures` holds the further numbers of the finding by name, and `measures.inputs_text` the `input:` lines printed with the finding. `tags` carry
`placement_estimate`, `traced_copper`, `partially_traced`, `unrouted_leg`, `plane_shortcut`, `pads_only`, `inferred` (the cell comes from the
structural fallback), `provisional_placement` (a position involved is the
prototype placement, not fixed by a `fixed_placement` constraint or a fixed
macro anchor) and `macro_relative_placement` (the loop lies in one rigid macro
cluster: its geometry is fixed, the cluster's position is not).

| Code | Status | What is measured | Ranked by |
| --- | --- | --- | --- |
| `EMI-HOT-LOOP` | implemented; placement estimate, traced where copper joins the pads | Per cell (per `input loop` and `output loop` for a four-switch buck-boost), the loop capacitor -> hot-supply pad -> switch pad -> catch diode (switch side -> rail side) or, without a diode, hot-return pad -> capacitor return. The legs between pads of one net follow the shortest copper path when copper exists, else the straight pad-to-pad segment. Enclosed area (shoelace), path length, partial inductance over the nearest inner ground plane (free space without one), differential-mode field, ringing, ring energy; traced length, vias, layer changes, narrowest width | headline differential-mode field in dBuV/m in the cell's scenario (rule below), then loop area |
| `EMI-SWITCH-NODE` | implemented, pads only unless copper exists | Per switch net: copper area of the pads (and of tracks, polygons, zones and macro copper when present), island capacitance to the nearest ground plane, distance to the board edge and to each cable connector, and the voltage-driven common-mode coupling-risk score (`cm_coupling_score`, a relative ranking score in dB, not a field; no margin) | common-mode coupling-risk score, then copper area; after the absolute estimates |
| `EMI-PROXIMITY` | implemented | Inductor and switch-node pads against pads of `sensitive` nets and of cable connectors, closer than a threshold in units of the inductor size | threshold / distance |
| `EMI-CONDUCTED` | implemented for `buck` cells (section 10) | Per buck cell: the conducted level at the input, 150 kHz to 30 MHz, dBuV at a 50 uH V-network, differential mode only, of a fixed-frequency buck in continuous conduction, for the scenario `declared_operating_point` (the component declares `load_current`; the current limit is a secondary worst case) or `current_limit` (a worst case, not a prediction), with every input capacitor as its own branch, the capacitor with the largest share of the ripple current and any anti-resonance of the input network. Worst harmonic (frequency, level, quasi-peak and average margin with a `cispr32_class_a`/`cispr32_class_b` `emission` constraint, **only where the levels are detector readings**: `measures.detector_applicability` is `isolated_lines` for every harmonic, or `receiver_emulation` for the harmonics the receiver model read), the fundamental, harmonics above each line, and every harmonic in `measures.harmonics`; where neither holds the levels are line amplitudes with no limit or margin | margin when margins exist, else level; after the radiated findings (a different quantity, dBuV against dBuV/m) |
| `EMI-ACCEPT-UNUSED` | implemented | An `emi_accept` that matched no finding | listed last |
| `EMI-RETURN-PATH` | implemented with `--run` (section 15); coverage `unsupported` without it | Per subject net (a signal source, or a differential, clock or rf_feed routing rule; a pair is one): reference-plane coverage within 3 H of each routed segment, the gaps crossed with slot length and half-wave resonance, reference changes at vias with the nearest stitching via or decoupling capacitor and the threshold, and the distance to the plane edge and board edge against 20 H; positions in `measures`; a ranking score in dB for the worst slot (`ranking_only`, no field, no margin) | ranking score, then number of breaks; after the absolute estimates |
| `EMI-MODE-CONVERSION` | implemented with `--run` (section 16); coverage `unsupported` without it on a board with a differential pair | Per differential pair: the ratio `|V_cm,out / V_diff,in|` in dB at the fundamental and its worst value up to the edge corner, for the ordered cascade of routed sections (skew from length and via path, amplitude imbalance from return-path exposure) and series parts; each element's contribution in order; the common-mode spectrum at the planes around a choke and at the connector. A choke or ferrite that declares `common_mode_impedance` is evaluated inside its declared frequency range (a band when only magnitudes are declared), one that does not is listed as present and not evaluated. A voltage ratio: no field, no margin | ratio in dB (`voltage_ratio`), after the ranking scores |
| `EMI-CABLE-COMMON-MODE` | implemented (section 18); one finding per cable connector; the standing `unsupported` entry stays on a board with no cable connector | Per cable connector: the common-mode **voltage** (dBuV) each driver leaves at the connector against the board reference there, and a risk ranking of the drivers: differential pairs that leave on the cable (the `V_cm,out` of the section 16 cascade, an absolute value only for a matched cable, else a `first_arrival_voltage`), ground noise `V_B = sum Z_Bk I_k` (a voltage where its inputs exist and it stays within 6 dB across the sense-height band, else a geometry ranking), single-ended signals and switch-node coupling (ranking only); the shield bond where a shield pin is declared. **No current, no field, no margin, no pass or fail** | pair voltage in dBuV (`common_mode_voltage`), after the voltage ratios |
| `EMI-CABLE-SHIELD` | implemented (section 18) | A connector declares `cable_shielded = true` and no `emi_role = shield` pin: no bond is declared | - |
| `EMI-DECOUPLING-PATH` | coverage `unsupported` | needs routed copper | - |

Every check that cannot run on the given input gets a coverage entry with
status (`executed`, `not_applicable`, `unsupported`) and reason; nothing is
reported as a pass for what was not measured. Standing entries:
`EMI-RETURN-PATH` (`executed` with a per-net entry when a routed run supplies the fills and a net is a subject, `not_applicable` without a subject net, else `unsupported` with the reason; section 15) `EMI-MODE-CONVERSION` (`executed` with a per-pair entry when a routed run is given and a differential pair exists, `not_applicable` without a pair, `unsupported` without a run when a pair exists, no entry at all without a run and without a pair; section 16) and `EMI-DECOUPLING-PATH` (unsupported),
`EMI-COPPER-TRACE` (traced copper path: `executed` when at least one copper leg of a hot loop is traced, else `unsupported` with the reason), `EMI-CABLE-COMMON-MODE`
(`executed` with an entry `EMI-CABLE-COMMON-MODE:<connector>` per cable connector when something is reported for at least one of them,
`not_applicable` when no connector has anything to report; on a board with no cable connector the entry stays `unsupported` as before: the board
model has no cable, chassis or return structure; section 18), `EMI-MARGIN` (whether an
`emission` constraint with a limit table gave margins). Per cell or net:
`EMI-HOT-LOOP:<cell>` (and `:radiated`, `:ringing`, `:copper`),
`EMI-SWITCH-NODE:<net>` (and `:common_mode`), `EMI-PROXIMITY:<cell>` and
`EMI-CONDUCTED:<cell>` (with `:common_mode` always `not_applicable` and `:filter`),
each with the missing input when skipped.

### How the numbers are made

- **Pad positions.** Component pin -> footprint pad number
  (`physical_pin_number`) -> placed pad shape. Rigid clusters follow the pose
  their anchor prescribes (`cluster_placements`). A pin with several pads
  contributes all of them; the loop takes the combination of one pad per role
  with the shortest closed path (dynamic programming), and among the cell's
  hot capacitors the one with the least inductance.
- **Hot loop.** Topologies `buck` (and unknown), `boost` and four-switch
  `buck_boost` are supported; the others are coverage `not_applicable`. Buck
  with catch diode: `C.supply, hot-supply pad, switch pad, D.switch side,
  D.rail side, C.return`; without a diode (synchronous): `C.supply,
  hot-supply, hot-return, C.return`, because the switch pad is not on the
  commutation path of a monolithic converter. Boost with a diode: `C.supply,
  D.rail side, D.switch side, switch pad, hot-return, C.return`. Buck-boost
  (two switch nets joined by one inductor, e.g. TPS63020 `L1`/`L2`): the
  `input loop` is `C.supply, hot-supply, hot-return, C.return` over the cell's
  hot capacitors; the `output loop` is `C.supply, output pin, hot-return,
  C.return` over the capacitors between the output net and the hot return net.
  The output pins are the controller's pins whose name starts with `VOUT` (the
  EMI model of `copper-lang` records the inductor's far net but not the
  controller's output pins, so `copper-drc` derives them and the capacitors
  from the flat view; a missing pin or capacitor is coverage
  `EMI-HOT-LOOP:<cell>:output` `not_applicable`). The closing segment crosses
  the capacitor body; its ESL is not added. **Capacitor choice:** among the
  cell's hot capacitors, those declaring `decouples` of a hot-supply pin
  (input loop) or of an output pin (output loop) are preferred and the least
  inductance among them wins; only when none is declared, the least inductance
  among the undeclared ones (`capacitor_selection` says which rule applied).
  Pad-centre version: segment width is the smaller pad dimension clamped to 0.2
  to 1 mm; thickness the outer copper thickness of the stack-up (35 um when
  undeclared); inductance is `copper_emi::inductance::loop_inductance_over_plane`
  with the segments at the centre-to-centre height above the nearest other
  layer that holds a `copper_zone` of the cell's hot-return or a 0 V supply
  net, else `loop_inductance`. Loops with pads on both board sides get no
  inductance.
- **Traced copper** (module `copper_drc::emi_trace`). For each pair of
  consecutive loop pads on one net the shortest path through that net's
  copper replaces the straight segment; legs between pads of different nets
  (through the switch, diode or capacitor body) stay straight and are listed
  as `component` legs. The graph (integer nm): nodes are `(layer, point)` at
  track and arc end points, via positions on every layer of the span and pad
  centres on every layer of the pad; a track is cut where another conductor's
  end point, via centre or pad centre lies inside its copper (a point beyond
  an end does not connect); an arc is one edge (a junction inside an arc is not
  seen); a via joins consecutive layers of its span, weighted by the stack-up
  distance; a pad centre joins every track end or via on its layer inside the
  pad shape by a stub; a polygon or zone joins every node it contains on its
  layer, pairwise, by a straight line (`plane_shortcut`; holes of the zone are
  honoured, fill and other nets' cutouts are not known). A leg without a path
  keeps the straight segment and is flagged `unrouted_leg`. The finding is
  tagged `traced_copper` when every copper leg is traced, `partially_traced`
  when some are, and stays `placement_estimate` when none is (copper that
  joins nothing still lists the unrouted legs; a zone outline alone is not
  routed copper). Reported: `traced_path_length_mm` (copper legs, via barrels
  included), `loop_area_mm2` and `loop_polygon_mm` of the traced polyline of
  all layers projected to the plane (pad polygon kept as `pad_polygon_mm`),
  `trace_via_count`, `trace_layer_changes`, `trace_narrowest_width_mm`
  (tracks and arcs only), per-leg `legs`. Inductance: every traced bar with
  its real width and the layer's copper thickness at its layer's centre height
  over the reference plane (bars on the plane layer or beyond it make the
  whole loop fall back to free space at the true layer heights, stated in
  `inductance_method`; `reference_plane` and `loop_reference_plane_z_mm` are
  then null, so the near-field map images nothing in a plane either); via barrels are added in series with
  `via_inductance(height, drill)` (no mutual terms with the bars; a barrel with
  height under a quarter of its diameter is omitted and counted). Plane
  shortcuts get a bar width of the straight length clamped to 1 to 5 mm. An
  arc contributes its chord chain to the polygon and its arc length to the
  path length. The pad chosen at each loop position is the one with the
  shortest pad-centre cycle, not re-optimised for the traced path.
- **Radiated estimate.** Frequency: the fixed value, else the upper bound of
  a declared range; edge: the current commutation time (the loop current drives
  the field), the faster of `current_rise_time`/`current_fall_time`, else the voltage
  edge standing in for it (tag `current_edge_from_voltage_edge`): the faster of
  `rise_time`/`fall_time`, else the edge
  from a declared `slew_rate` (`0.8 V_swing / slew`, exact), else the default
  slew band of section 10.1 (nominal, fast and slow, on the hot-supply voltage
  as the swing); scenario and amplitude: the cell's loop-current scenario, below; duty: Vout/Vin when
  the hot-supply net and the magnetic's output net both have a declared supply,
  else 0.5. In the `current_limit` scenario the envelope `trapezoid_envelope` of flat-top pulses of
  amplitude `current_peak` is swept on a log grid over 30 MHz to the lower of 1 GHz and
  `c / (10 * path length)`; in the `declared_operating_point` scenario every harmonic line `n f0` of the ramp-on-pedestal
  switch current in the same range is evaluated (the exact lines of `PeriodicSpectrum::buck_input_current`, the source
  of `EMI-CONDUCTED`, not an envelope over them; at most 400 000 lines, else the field is skipped with the reason and not
  replaced by the current limit). The reported level
  is the least-margin point against the `emission` line, or the maximum level
  without one, at the standard's distance (10 m for CISPR 32, 3 m or 10 m for
  FCC 15 per the table; 3 m without a standard). Free-space value; the +6 dB
  ground-plane factor is not applied. Peak envelope against quasi-peak limits.
  Skipped with a reason when the frequency, the edge (no edge declared and no
  hot-supply voltage for the swing) or the current (no usable `load_current` and no `current_peak`) is missing. With a default
  edge (and, in the declared scenario, a rule-of-thumb ripple) the sweep is made at the nominal case and at every
  corner of the fast and slow edge and the low and high ripple: the finding reports
  the nominal level, `estimate.band_db = [low, high]` (slow to fast) and
  `estimate.basis = "rule_of_thumb"`, and the margin is the one at the fast
  bound, the unfavourable end. The band covers the defaulted inputs only; the
  mechanisms the estimate does not contain are `estimate.not_modelled` (common-mode
  currents on cables, ground-plane reflection and enclosure, the inductor's field,
  waveform detail beyond the trapezoid envelope). The voltage-driven common-mode
  score of `EMI-SWITCH-NODE` is banded the same way (it is flat in frequency above the
  edge corner, so its band collapses when the worst point lies below every corner).
  **Validity of the small-loop formula** (`measures.radiated_validity`): the loop path
  is at most a tenth of a wavelength at the evaluated frequency (the sweep stops at
  `c / (10 path)`; when that is below 1 GHz the tag `radiated_band_truncated` and a
  sentence in the message say that emission above it is not covered), the observation
  distance is beyond `lambda / 2 pi` (checked at 30 MHz, the lowest frequency swept) and
  at least three times the loop's longest dimension (a point source). `holds`
  is their conjunction; when one fails the tag `small_loop_validity_failed` and the
  message name it.
- **Scenario of the hot-loop estimates** (`measures.scenario`, `operating_point`, `loop_current`, tags
  `scenario_declared_operating_point` / `scenario_current_limit`). The scenario is a property of the cell,
  decided once exactly as `EMI-CONDUCTED` decides it (`copper_drc::emi::conducted::operating_point`), and the radiated field,
  the stored energy and overshoot, and the near-field map all use it, so one report holds one operating
  assumption per cell. `declared_operating_point`: the component declares `load_current`, the topology is `buck`, the supply
  voltages of the hot-supply and output nets are declared with `Vout < Vin`, the inductor current stays positive
  (`I_out >= dI/2`) and the peak `I_out + dI/2` does not exceed the switch current limit `current_peak` (a limit that
  chops the ramp makes the assumed waveform wrong). The high-side switch current is the hot-loop current: the ramp from `I_out - dI/2` to `I_out + dI/2`
  with the declared current edges. The current limit `current_peak`, when declared, is the secondary worst case
  (`measures.secondary_scenario`: level, frequency, margin, band of the same estimate with flat-top pulses of the limit's
  amplitude, as a trapezoid envelope). `current_limit`: no operating point is declared, or it is outside the model
  (`operating_point.status` `not_declared` / `outside_model` with the `reason` and its machine-readable `reason_code`,
  `discontinuous_conduction` or `peak_exceeds_current_limit`; the tag `operating_point_outside_model`), or
  the cell is not a buck (`how_to_declare` is `null`; the finding says that the operating-point model is for a buck stage only and
  does not advise `load_current`, which `CMP133` would refuse); the amplitude is `current_peak`, a worst case that bounds
  operation and is not a prediction of it (tag `worst_case_current`, the message says so). Without either a usable
  `load_current` or a `current_peak` the field is skipped as before. **Which current the figures use** (`loop_current`
  states each): the radiated field uses the whole ramp; the stored energy `L I^2 / 2` and the overshoot `I sqrt(L/C)` use
  the current commutated at turn-off, `I_out + dI/2` (`ring_current_a`), which is the larger of the two commutations and so
  bounds the ringing; the turn-on commutation of the valley `I_out - dI/2` is also reported
  (`ring_energy_turn_on_nj`, `ring_overshoot_turn_on_v`), and so are the figures at the current limit
  (`ring_energy_current_limit_nj`, `ring_overshoot_current_limit_v`); the diode's default capacitance still uses the current limit
  as its rating proxy (section 10.1). The near-field map draws `|H|` of the static current `field_amplitude_a`: `I_out + dI/2`
  (the largest current the loop carries) in the declared scenario, `current_peak` in the current-limit scenario, per ampere
  without one, and its text names the scenario. **Ranking between cells in different scenarios.** Findings are ordered by
  headline level in dBuV/m, each in its own cell's scenario, ties by loop area (larger first); the order across a
  `declared_operating_point` and a `current_limit` cell therefore compares an estimate of operation with a worst case
  that bounds it as numbers, which `limits.notes` states whenever the board holds both (the secondary scenario of a declared cell
  gives the like-for-like current-limit level).
- **Ringing.** With a computed inductance: `ring_frequency`, `ring_overshoot`
  (need a switch current: `load_current`, else `current_peak`); ring energy `0.5 L I^2` needs the inductance and
  that current, taken at the turn-off peak in the declared scenario (above). The capacitance is the sum of the controller's and the catch
  diode's `output_capacitance`; a device that declares none gets the default
  capacitance band of section 10.1 (tag `rule_of_thumb_capacitance`), and the
  ring frequency and overshoot are then reported as bands
  (`ring_frequency_band_mhz`, `ring_overshoot_band_v`,
  `switch_node_capacitance_band_pf`) beside the nominal.
- **Switch node.** Per switch net: pads, tracks (arcs by chord), polygons and
  zones summed; capacitance is the sum of `island_capacitance` of each element
  over its own layer's nearest ground plane (dielectric height from the
  stack-up, relative permittivity 4.5 when undeclared). The voltage-driven common
  mode is a **coupling-risk score**, `cm_coupling_score = 20 log10(f C V / (100 MHz x 1 pF x 1 V))`
  maximised over 30 MHz to 1 GHz, with `V` the trapezoid envelope of the hot supply's
  declared voltage as swing and `C` the island capacitance, swept like the loop. The
  displacement current `2 pi f C V` is the physical driver; the score keeps it and
  drops everything the board model cannot supply: the earlier field form
  `E = 115 f C V` needed an unverified field-per-current constant and a distance, and a
  field or margin in dBuV/m would imply a cable, chassis and return path the model does not
  have. The score is monotonic in each driver (a doubled swing or capacitance adds 6.02 dB),
  so the ranking between switch nets of one board and between boards stays meaningful, but its
  absolute value has no physical unit: it carries no distance, no margin against an emission
  limit, is never drawn on a limit-line plot, and is sorted after the absolute estimates (tag
  `ranking_only`, `estimate.kind = ranking_score`). `measures.cm_drivers` lists the
  island capacitance, swing, 10-90 % edge, frequency and duty it comes from. What an absolute
  prediction would need, and what the score omits, is `estimate.not_modelled` and
  `measures.missing_for_absolute_prediction`: cable and attached-conductor geometry,
  board-to-environment coupling, the return impedance of the common-mode path, the radiation
  efficiency of the carrying structure and the frequency dependence of the island capacitance. `pads_only` is set when the net has no tracks,
  arcs, vias, polygons or zones.
- **Proximity.** Reference size: the larger of the inductor's body and pad
  extent (the controller's when there is no magnetic). Magnetic source: 3x for
  `unshielded` or undeclared shielding, 1.5x for `semi_shielded`, none for
  `shielded`. The switch-node pads are an additional source at 1.5x whatever
  the shielding. Victims: pads of nets the model flags `sensitive` and pads of
  cable connectors (parts with `external_cable = true` and `emission`
  targets), excluding pads of the cell's own parts. Distance is the pad-to-pad
  clearance. The finding states the threshold factor and millimetres.
- **Acceptance.** An `emi_accept` target matches a finding when it equals a
  component or `component.pin` among the finding's subjects (the controller,
  loop capacitor, diode, role pins, the switch net's components, the
  inductor and the victim components or sensitive pins) and the code is equal.
  Matched findings stay listed with severity `info` and the reason, and do not
  count in the summary; unmatched constraints give `EMI-ACCEPT-UNUSED`.
- **Inferred cells** are analysed like declared ones, every finding tagged
  `inferred`.
- **Margin.** With an `emission` standard that has a table (the two CISPR 32
  classes and the two FCC 15 classes; `cispr25_class_5` has none: coverage
  `unsupported`), `margin_db` is limit minus level at the evaluated
  frequency, for the absolute estimates only (`EMI-HOT-LOOP` differential mode,
  `EMI-CONDUCTED` for the harmonics that are detector readings: every harmonic of an isolated-line finding, the harmonics read by
  the receiver emulation otherwise); the common-mode score has none.
  Nothing is printed as pass or fail. The `cable_length` of the constraint is used only for the quarter-wave
  frequency that `EMI-CABLE-COMMON-MODE` reports (section 18); no cable current or field estimate uses it.

Estimators (module `copper_emi::estimate`, all SI, all documented with their
source):

- Trapezoid envelope: corners at `1/(pi*t_on)` and `1/(pi*t_r)`; harmonic
  amplitude bound `2*A*d*|sinc(n*pi*d)|*|sinc(n*pi*t_r/T)|`.
- Differential-mode loop at distance r: `E = 1.316e-14 * I * f^2 * A / r`.
- Voltage-driven common-mode coupling-risk score (`voltage_driven_cm_drive`,
  `voltage_driven_cm_score_db`): `20 log10(f C V / (100 MHz x 1 pF x 1 V))`, a relative ranking
  score with no field constant and no distance.
- Current-driven common-mode score (`current_driven_cm_voltage`, `current_driven_cm_score_db`):
  `V_cm = 2*pi*f*L_return*I`, scored in dB re 1 V; not used by `emi-check` yet.
- Validity of the small-loop formula (`dm_field_validity`): loop path over wavelength, distance
  over `lambda / 2 pi`, distance over loop size.
- Ringing: `f_ring = 1/(2*pi*sqrt(L_loop*C_oss))`.
- Limit lines for the `emission` standards, as tables.

No accuracy is stated for an absolute level: the estimators are checked against hand
calculations and published closed forms, not against measurements. Margins are screening
margins and never pass/fail.

`copper emi-check BOARD [--run RUN] --extract` adds, per buck cell, the hot-loop interconnect impedance from a
quasi-static extraction of the real filled copper beside the closed form (section 13).

Not implemented from the original plan: the routed-board reader for a
`.kicad_pcb` (see `--run` above), traced copper beyond the hot loop (switch
node copper is counted, not traced) and `EMI-DECOUPLING-PATH`. The checks that
now exist for signals and cables are `EMI-RETURN-PATH` (section 15),
`EMI-MODE-CONVERSION` (section 16) and `EMI-CABLE-COMMON-MODE` / `EMI-CABLE-SHIELD` (section 18): voltages, ratios and
rankings. Three further steps of the signal and cable plan are **not implemented and not committed work**, each starting
with a written definition or a spike: the net current of a cable bundle for a defined termination and environment
(P4b; the voltages of section 18 are not a current and no field follows from them), the ground transfer inductance by
quasi-static extraction (P5) and the full-wave mixed-mode region (P6).

**In the run report, `copper issues` and `copper view`.** The findings are
screening evidence and are read from the run report, `route-report.json`, which
stays the single file of a run (there is no sidecar):

- *The `emi` section.* The report gains an optional top-level `emi` key holding
  the `copperscript-emi-check/v0.1` document (cells, findings with their
  `measures`, including `loop_polygon_mm`, estimate, margin, `accepted`, tags,
  `coverage`, `limits`, `summary`) of the final routed board. A report without
  the key is valid and unchanged.
- *`route-board` writes it.* In its report stage (`crates/copper/src/route_board.rs`,
  after `routing_quality`, before the report is serialised) it runs
  `emi_check` on the final board with the electrical view of the design
  (`crates/copper/src/emi_section.rs`). The report is then written once and the
  summary's `report_sha256` is taken over those bytes, so the digest contract is
  as before. The step is report-only (it does not touch the board), costs the
  elaboration of the design and one pass over the cells, and never fails a
  route: an elaboration error or a panic is recorded as
  `emi: { "evidence_grade": "screening", "error": "..." }`. A design without a
  switching cell, no finding and no executed signal check (`EMI-RETURN-PATH`,
  section 15) gets no `emi` key, so its report is byte-identical to what it was
  before; a board with no cell but a return-path finding or an executed
  return-path coverage entry keeps the section (`cells` empty).
- *`copper emi-check BOARD --run RUN --write`* adds or replaces the section of an
  existing run. `RUN` is the run directory (or a state file in it, or its
  `checkpoint/` directory) and must hold the `route-report.json` of a finished run
  in the run directory, next to or up to two levels above `final-board.state.json`
  (`route-board --checkpoint-dir`); the report is rewritten in the format
  `route-board` uses (sorted keys, indent 2) through a temporary file and a
  rename, and `report_sha256` of `route-report.summary.json`, the only place the
  report's digest is recorded, is replaced the same way (a missing summary is not
  created). The result equals what `route-board` embeds for the same board;
  repeating the command changes nothing. Without `--write` the command stays
  read-only. Nothing checks that the board in `RUN` is the one the report
  describes beyond the references of the design.
- *`copper issues RUN`* lists the section as stage `emi`, ids `M1`, `M2`, ...,
  findings grouped by subject kind: the converter cells in the section's order and the report's ranking inside a
  cell, then the signals and pairs (`EMI-RETURN-PATH`, `EMI-MODE-CONVERSION`: no cell), then the cable connectors
  (`EMI-CABLE-COMMON-MODE`, `EMI-CABLE-SHIELD`), each kind in the report's own order. A board with cells only is listed
  as before, byte for byte. The title
  starts with `screening estimate`, the details carry the estimate with its
  `numerical_sensitivity`, parameter band, `not_modelled` list, margin, tags and the scalar measures, and the text output ends
  with a line saying the issues are screening estimates, not EMC qualification.
  An accepted finding (`emi_accept`) is listed as `ACCEPTED (reason)` with
  count 0, so it does not add to `finding_counts`. Windows: `EMI-HOT-LOOP` the
  bounding box of its `loop_polygon_mm`; every other finding the pad centres of
  its objects as found on the run's board (a proximity finding: the closest pair
  of source and victim pads, the cell's own controller aside; a switch-node
  finding: every pad of the net); a return-path finding its located causes (section 15.12), an `EMI-MODE-CONVERSION`
  finding the pads of its pair's nets (the `measures` of the check hold no coordinates for its sections) and a cable
  finding the pads of its connector. `details.causes` lists, for an `EMI-MODE-CONVERSION` finding, the routed
  sections that convert (`kind = routed_section`: lengths per leg, skew, amplitude imbalance, exposed return path), and for
  `EMI-CABLE-COMMON-MODE` one cause per driver with its `kind` (`differential_pair`, `ground_noise`,
  `single_ended_signal`, `switch_node_coupling`), `name`, `status`, its `quantity` label exactly as the finding gives it
  (`common_mode_voltage_at_connector` under `matched_cable_termination`, `first_arrival_voltage` for a declared,
  unmatched cable, `ground_noise_voltage`, or null for a driver that is ranking only) and `value_dbuv` where a voltage is
  published (a ranking-only driver carries its `ranking_value` and `ranking_unit` instead). `--stage emi` filters; an `error` section or
  an empty one lists nothing.
- *`copper view RUN --issue M2`* frames the window, highlights the issue's nets
  and draws the hot loop over the copper from the finding's traced path
  `loop_path_3d_mm` (SVG group `emi_loop`): legs on the layer of the path's first
  vertex (the component layer) as a solid red line, legs on another layer (a
  different z) as a dashed blue line, a circle at each via (a z step at one
  place), and the `REF.PAD` of every loop pad, nudged clear of the path and of
  each other. The path is drawn in every layer panel, since the view has no
  z-to-layer table. A section written before the path was stored (no
  `loop_path_3d_mm`) keeps the earlier filled pad-centre outline (`SceneArea` of
  kind `emi_loop`, SVG class `emi_loop`, on the layers of the finding); the
  legend names the loop as a screening estimate. Other emi issues are marked at their pad
  centres like the other stages. `view` has no stage filter of its own.

### Visualization

Hand-written, deterministic SVGs (no dependencies, explicit white background, fixed palette). The converter pictures
follow the rules of the signal, pair and connector figures: a title of one line; one subtitle line of at most about 110
characters with the facts needed to read the chart; at most one badge line in the warning colour for the caveat that
matters most (an extraction that did not converge, capacitor ESR and ESL that are defaults); direct labels on the marks
instead of a legend where they fit, a legend of at most four entries of at most three words otherwise; only the one or two
annotations that carry the message; numbers with one decimal for dB and at most three significant digits otherwise, with
their units; and one footer line per document: "Screening estimate, never a pass or a fail and not a compliance
measurement. Details: `copper issues RUN --stage emi`; what is not modelled: `copper explain CODE`." The assumptions, the
lists of what is not modelled, convergence narratives and definitions of colour scales are not drawn: they stay in the
JSON, in `copper issues` and in `copper explain`. The tests check every figure structurally (no text outside its page, no
two texts overlapping, a cap on the number of `<text>` elements that is at most half of what the figure had before it was
simplified, at most four legend entries, no text over 120 characters) and not by eye.

**Conducted spectrum.** `copper emi-check BOARD [--run RUN] --plot-spectrum OUT.svg` writes one panel per cell with an
`EMI-CONDUCTED` finding (renderer: `copper_report::emi_plot`, data read from `measures.harmonics`). Title "U1: conducted
emission at the input"; subtitle "500 kHz switching; dBuV at a 50 uH LISN; limit: CISPR 32 class B". The chart is a log
frequency axis from 150 kHz to 30 MHz against dBuV: a stem and a dot per harmonic (at the high end of its band, its total
level when it has none), in the warning colour above the average limit, a thin whisker for the band where the inputs are
rule-of-thumb defaults, and the quasi-peak and average limit lines drawn across the whole band from the limit table of
`copper_emi` (`estimate::limits::cispr32_conducted`, class from `measures.limit_class`; the 150-500 kHz slope is one straight
line and the step at 5 MHz is vertical), each named at its right end, when an `emission` standard with a conducted table is
declared **and the levels are detector readings** (`measures.detector_applicability = isolated_lines`; for
`receiver_emulation` the lines run through the harmonics the receiver model read, marked by diamonds at their high-end
quasi-peak reading, while the stems stay line amplitudes). One annotation marks the worst harmonic: "95.6 dBuV at 500 kHz,
+49.6 dB over average" (`limit_avg - level`, from the harmonic's own numbers; "6.4 dB under average" when under). The legend
is "over the limit", "under the limit", "band (default inputs)" (with common mode present, "differential" and "common"
instead of the band). A panel whose detector assumption fails (harmonic spacing inside the IF bandwidth, or a declared
frequency range) is titled "conducted line amplitudes", draws no limit line, no warning colour and no excess, and carries the
badge "line amplitudes, not detector readings: no limit applies"; the panels of a frequency range are one per
fixed-frequency scenario, titled with the scenario. A report that does not state the status is drawn the same way. The
common-mode coupling-risk score of `EMI-SWITCH-NODE` is a relative ranking and is never drawn here. A board without an
`EMI-CONDUCTED` finding writes nothing and exits 2 with the reason.

**Magnetic near field.** `copper emi-check BOARD [--run RUN] --plot-field OUT.svg [--field-height 1mm]` and `copper view RUN
--issue M<n> --emi-field [--field-height 1mm]` (an `EMI-HOT-LOOP` issue) draw `|H|` in a plane `--field-height` above the top
copper (default 1 mm) as a heat map in dB relative to the map maximum (0 to -40 dB, a 40-step viridis scale of one dB a
step; cells below it are transparent) with contour lines every 10 dB, over the loop window. Title "U1 hot loop: magnetic
near field, 1 mm above the copper"; subtitle "relative to the map maximum; contours every 10 dB; 4.5 A worst case" (or
"operating point", or "per ampere"). The loop is drawn from `loop_path_3d_mm` (solid on the component layer, dashed on
another, a circle at each via), only the pads of the loop's own components are named, a colour bar with ticks at 0, -20 and
-40 dB and a scale bar complete the map, and the field level below the peak is labelled at the three nearest sensitive pads
and cable-connector pads ("R1.2 -55.5 dB"). The legend lists the loop and the sensitive pad (and the other layer and the
via when the loop has them). The window holds the loop's box plus 3 mm on every side and every cell within 40 dB of the
maximum (it may reach past the board outline; the field is first computed 20 mm round the box and cropped). Labels have a
white halo and are nudged deterministically clear of the rings, the loop and each other. Grid cells are 0.25 mm, with a
larger step when the window would exceed 200 cells a side; the heat map is one compound path per colour step.

The field is computed by `copper_emi::field` from data the finding carries:

- `measures.loop_path_3d_mm`: the closed loop as `[x, y, z]` vertices in mm
  (z = 0 at the centre of the top copper layer, negative below it, from the
  stack-up; a traced loop has a vertex per bar end, so a via is a vertical
  step; the pad centres otherwise), `measures.loop_reference_plane_z_mm`
  (z of the nearest ground plane layer, null without one) and
  `measures.field_victims` (`[{kind, name, pads: ["R1.2", ...]}]`, names only:
  the map looks the pads up on the board it draws).
- The amplitude is the finding's own `measures.loop_current.field_amplitude_a`: `I_out + dI/2` (the peak of the
  high-side switch current) in the `declared_operating_point` scenario, the cell's `current_peak` (the switch current limit,
  a worst case) in the `current_limit` scenario; the map states the scenario and the amplitude. Without a current the map
  is normalised to 1 A and says "per ampere". A report written before the finding recorded `loop_current` falls back to the
  cell's current limit and the map says that the report does not record the scenario.
- Biot-Savart for each straight filament, `H = I/(4 pi d) (sin a2 - sin a1)`
  around the segment axis (zero on the axis, linear inside a 50 um core), summed
  over the loop; an ideal infinite ground plane at the reference plane's z by
  image currents (mirrored segments, reversed current: zero normal field at the
  plane, zero field behind it).

Limits (in the report and in `copper explain`, not on the picture): a ranking and placement aid, not a
field prediction. The model is quasi-static (valid where the region is much
smaller than a wavelength: fine up to a few hundred MHz over a few cm), uses the
one static current named above as the amplitude, round the loop, and only the hot loop
carries current. **The inductor's own field, usually the largest low-frequency
source for an unshielded inductor, is not included** (coverage
`unsupported EMI-FIELD:inductor`). The plane is ideal and infinite, no other
copper shields, and a loop on the far side of the plane from the map shows
nothing. Pads, copper and the loop are drawn schematically from the board scene
(the placed board built in memory, or the routed run's board); components that
a rigid cluster moves are drawn at their placement, not at the cluster pose the
check used. A report written before `loop_path_3d_mm` existed gives an error
telling to rerun `copper emi-check BOARD --run RUN --write`.

**Input-network impedance.** `copper emi-check BOARD [--run RUN] [--extract] --plot-impedance OUT.svg` writes one
picture per `EMI-CONDUCTED` finding (renderer `copper_report::emi_impedance`; data
`copper_emi::conducted::input_network`) that shows why the harmonics are as high as they are: the impedance the
switch current meets and where each capacitor takes it. It is evaluated on the circuit the conducted estimate solves
(section 10), not on a copy: the nominal case of the finding's scenario is kept in memory on the finding
(`EmiFinding::conducted_input`, never in the JSON) and `input_network` solves the same modified-nodal-analysis
circuit at any frequency with a unit current drawn by the converter's source. At a harmonic its branch currents are
the model's (`HarmonicLevel::branch_currents`, to rounding; unit tests
`the_input_network_gives_the_prediction_s_branch_currents_and_node_impedance_at_the_harmonics` with the hot-loop
inductance, a coupling, a filter and a supply-side capacitor, and
`the_node_impedance_of_the_input_network_is_the_one_the_anti_resonance_scan_reports`). The default report is
unchanged: the function is only called by the picture.

Title "U1: input-network impedance seen by the switch"; subtitle "|Z| at the switching node; hot-loop inductance 4.97 nH
(placement estimate)" (the basis is `placement estimate`, `traced copper`, `partly traced copper` or, with `--extract`,
`extracted`). The chart is `|Z|` in ohm on a log axis against frequency (150 kHz to 30 MHz, down to the fundamental when
that is lower): a thick line for the total at the node where the capacitors meet (every source off; what the
anti-resonance scan of the report reads), a thin line for each capacitor branch's own series impedance
`R + j (w (L_connection + ESL) - 1 / (w C))`, named by its capacitor at the right-hand end, a hollow upward triangle on each
anti-resonance of the total (a local maximum with a prominence of 0.5 dB, as in `measures.anti_resonances`) labelled with
its frequency (and "harmonic n" when the model attributes a significant harmonic to it), a hollow downward triangle on each
branch's minimum, its self-resonance `1 / (2 pi sqrt((L_connection + ESL) C))`, labelled with its frequency, and a tick on the
frequency axis at every switching harmonic, taller and in the warning colour where the harmonic lies within
`copper_report::emi_impedance::TOLERANCE` (5 %) of an anti-resonance, with a dot on the curve there (ringed when the report
finds it raised by 6 dB or more over the best single branch); the 5 % band is shaded. A second panel with the current each
branch carries per ampere the converter draws, in dB (0 dB is all of it), is drawn **only when the cell has more than one
capacitor branch** (with one it is a flat line at 0 dB). The legend is "total", "one branch", "anti-resonance"; one badge says
that the capacitor ESR and ESL are defaults, or that the extraction did not converge (the extraction wins).

**Extraction current map.** `copper emi-check BOARD --extract --plot-current OUT.svg` (an error without `--extract`)
draws where the hot loop's current flows in the copper, from the extraction of the same invocation; nothing of it is
stored in `route-report.json` and the `emi` JSON is unchanged (`HotLoopOptions::current_map` only arranges the branch
currents the loop analysis already solves; `copper_extract::current`, renderer `copper_report::emi_current`). The
solution is one frequency of the finest mesh, 10 MHz when it was solved (else the highest), for a unit current into the
port across the switch. Title "U1 hot loop: current density in the copper at 10 MHz"; subtitle "per ampere into the switch
port; quasi-static extraction (6787 filaments)"; the badge "extraction not converged: indicative only" when the extraction's
status is not `ok` (the reasons are in `measures.extract.status_reasons`).

* One map per copper layer whose peak is within 20 dB of the 0 dB level, titled with the layer and its peak ("F.Cu: peak
  8.49 A/mm"), over the window of the cells within the scale plus 1.5 mm. The copper of the layer is drawn white with a
  faint outline, one path per conductor shape.
* **Heat**: the sheet current `|K| = |I| / w` of every filament (`w` the cross-section width of the filament:
  the strip it stands for times the part of it that copper covers; the filaments across the thickness are summed),
  rasterised at 0.05 mm (coarser above about 90 000 cells), the x- and y-filaments of a cell combined as
  `sqrt(|K_x|^2 + |K_y|^2)`, in dB below the 0 dB level on a 30-step viridis scale (one dB a step, one compound path
  per colour); the colour bar has a tick every 10 dB. The 0 dB level is the 99th percentile of the cells of the layer with
  the largest peak: a few cells at a terminal or a corner, where the filament mesh crowds the current, would otherwise set
  the scale; the cells above it are drawn in the top colour. Copper more than 30 dB below is white.
* **Arrows**: on a grid of about a twentieth of the width, the strongest cell of each block, where it is above 15 % of
  the layer's 0 dB level and the real part of the current is at least half its magnitude: the direction at the
  instant the port current (the real unit current) peaks. In quadrature there is no arrow.
* **Vias** are circles whose area is the via's share of the current that changes layer there, with the percentage
  (labelled from 2 %), the share being `|I| / sum |I|` over the vias of the same net that join the same two layers. Only when a
  layer change carries current (0.1 A per ampere or more) a "division between the vias" lists it beside the maps with its
  strongest shares; with none there is no such heading and no sentence saying so.
* The loop's **pads** are labelled and the **port** is ringed (legend: "current direction", "port"); a three-row table
  "loop impedance" lists the inductance and resistance at the solved frequencies, from the same solution (the numbers of
  `measures.extract`).
* The per-capacitor current shares are not in this solution: the hot capacitor closes the loop as a copper bar and the
  other capacitors of the hot net are open ports of the multi-port matrix; their currents are the impedance picture's
  second panel.

**One-page summary.** `copper emi-check BOARD [--run RUN] [--extract] [--fullwave] --plot-summary OUT.svg` writes the
picture to open first: one page per switching cell (stacked when the board has several, as the other pictures do), 1400 px
wide and as tall as it needs (renderer `copper_report::emi_summary`; about 1400 x 1100 px for a cell with three or four
pictures). Every number on it is read from the report document or comes from the in-memory results the other pictures use;
nothing is computed for the page, and a value the report does not hold is left out (its tile is not drawn). It changes no
number of the report: the default `emi-check --json` is byte-identical with and without it.

1. **Header**: the title "U1: buck converter", one subtitle line ("500 kHz, 12 V in, 5 V out, 5 ns edge; worst case at the
   4.5 A current limit", or "declared operating point, 2 A load") and at most one badge line: an extraction or a full wave that
   failed or did not converge ("extraction not converged: indicative only") beats "default capacitor ESR/ESL (no datasheet
   values)".
2. **Up to five tiles**, each a small label, one big value and one small line, in neutral ink (the value that is an excess
   over a limit is in the warning colour; nothing is coloured as a pass or a fail): "Hot loop" (the inductance by the best
   method that ran, named: the full wave, else the extraction at the frequency nearest 10 MHz, else the closed form with the
   loop area and its basis); "Conducted, worst" ("+49.6 dB", "over average; 95.6 dBuV at 500 kHz", the worst harmonic's
   distance to the average limit; the level alone where there is no limit); "Harmonics over the limit" ("55 of 60", with the
   count over the quasi-peak limit); "Switch node" (copper area, capacitance to the plane); "Nearest sensitive net" (the
   smallest `EMI-PROXIMITY` distance, "L1 to FB").
3. **One line of chips** naming the cell's findings, `M<id> HOT-LOOP` with the id `copper issues` gives when `--run` is given
   (the code alone otherwise), worst severity first then by id, an accepted finding in grey; at most twelve, then "+N more".
4. **The pictures**: the near field and the conducted spectrum side by side at one size; under them the input-network
   impedance and, only when `--extract` ran, the current distribution (without it the impedance plot is centred and nothing
   stands in for the missing one). Each is drawn by the code of its full-size picture in a `compact` form (a short title of its
   own, no subtitle, no legend, a one-row colour bar, direct labels only) and embedded as a nested SVG at its natural size, or
   smaller by at most 10 % (the smallest text is then about 9 px). A picture that cannot be drawn is one line saying why.

The assumptions, what is not modelled and the commands that draw the full-size pictures are not on the page.

**No switching cell, and the figures of signals, pairs and connectors.** A board with no cell but with `EMI-RETURN-PATH`,
`EMI-MODE-CONVERSION` or cable findings (sections 15, 16 and 18, `--run` given for the routed ones) no longer refuses
`--plot-summary`. The pages follow the subject kinds of `copper issues`: the cell pages first; then, when return-path
findings exist, the return-path map (section 15.12); then the one page of the mode conversion of the pairs; then one page per
cable connector that has a finding; then, once, at the very end, one footer line. A board with none of these exits 2
("no switching cell and no return-path, mode-conversion or cable finding to summarise"). A document of cell pages only is
unchanged, byte for byte, except that the SVG's overall height covers the added pages. These figures answer one question
each with a chart. The text on them is a title, one subtitle line that says what the quantity is, the axis and bar labels
and the footer: "Screening evidence, never a pass or a fail. Voltages and ratios, not a field; no limit applies. Details:
`copper issues RUN --stage emi`; what is not modelled: `copper explain CODE`." The numbers a figure leaves out stay in
the JSON, in `copper issues --json` and in `copper explain`. No limit line, margin, field or current is drawn.

*Mode-conversion page* (`<g class="summary-pairs">`, about 1400 px wide): "Mode conversion of the differential pairs",
`|V_cm,out / V_diff,in|` in dB at the fundamental. One horizontal bar per pair that has an `EMI-MODE-CONVERSION` finding,
worst first (at most 16, then "+N more pairs"), on one axis in dB from the lowest value rounded down to 10 dB (not below
-80) to 0 dB with a gridline every 10 dB; the bar ends at the value at the fundamental (printed at the end, one decimal)
and a tick marks the worst value up to the edge corner. The pair is named by its positive net and the trailing letter of the
negative one (`LANE_D_P/N`). Beside each bar the channel is drawn as a strip of glyphs in the order of the cascade
(`elements_in_order`): a line for each routed section (thick and red with its skew in ps above it when the section has a
skew or an amplitude imbalance), a box for each part (R resistor, C coupling capacitor, CMC choke, FB ferrite, ? other; filled
when its transfer is in the cascade, hollow with a dashed outline when it is present and not evaluated), ending in a
connector (square) or an open end (circle); one line under the chart keys the glyphs. A pair that has only an
`EMI-RETURN-PATH` finding has no row (the map has it).

*Connector page* (`<g class="summary-connector">`): "J1: common-mode voltage at the connector, per driver", the cable in one
line (length, unshielded (assumed) or shielded with its bond vias, matched termination assumed or a declared impedance with
first-arrival voltages), and one bar chart in dBuV with three groups in this order, never sorted across groups because they
are different quantities: the pair common-mode voltage at the connector (the group says "(first-arrival voltage)" when the
cable's impedance is declared and not matched), the ground-noise voltage of the bundle against the board reference, and the
drivers that are ranking only (no bar: a single-ended signal's slew rate in V/ns, a switch node's coupling score in dB, a
ground-noise source or a pair with no voltage and the one short reason, for example "slot between the loops", "sense band
8.3 dB", "no routed copper"). The first two groups share one axis (60 dB below the strongest, rounded up to 10 dB, up to it) with a
gridline every 10 dB; each bar is the headline value as the finding ranks it, with its value and the frequency of that line
at its end ("84.9 dBuV @ 1.25 GHz") and, where the finding gives a band (a magnitude-only choke, the sense-height band
of a ground-noise source), a thin whisker. A group shows at most 20 rows, then "+N more".

**Common-mode voltages at a cable connector.** `copper emi-check BOARD --run RUN --plot-spectrum OUT.svg` also draws, when
the report has `EMI-CABLE-COMMON-MODE` findings with a published voltage (`copper_report::emi_cable_plot`), the same driver
chart as the connector page, one per connector, stacked, 900 px wide and ending in the footer (a connector with no published
voltage has no chart there). It prints "common-mode voltages at the connector of N drivers on K connectors (no limit line:
voltages, not a field) written to ...". The conducted spectrum of the cells, when the board has any, comes first in the same
file; a board with neither still reports "no EMI-CONDUCTED finding with harmonics to plot". The
pictures were checked structurally by the tests (bars, groups, no text outside its page, no two texts overlapping, the
words that must not appear) and not by eye.

For a cell with several hot loops the loop rows are repeated per loop, named by the loop; for a declared frequency
range the page reads the first conducted finding (the lower bound), the other scenarios are in `--plot-spectrum`. The
board overlay needs the board scene (the placed board in memory, or the routed run's board).


## 9. Quasi-static extraction (phase 2)

Module `copper_emi::inductance`: partial inductance of rectangular bars
(Ruehli/Hoer closed form for self terms, Hoer-Love or filament quadrature for
mutual terms), assembled into a loop inductance for an ordered list of
segments, with a ground-plane image option. Validation targets: closed forms
(Rosa/Grover straight conductor, rectangular loop), the Richtek AN045 figures
(about 4 nH for a 3 mm loop path, 6.1 nH with 73 pF ringing at 238 MHz), and,
where installed, FastHenry2 run as an external process on an exported deck
(never linked, never ported from source). Capacitance (switch node to plane)
starts as parallel-plate plus fringe and is replaced by a proper electrostatic
solve later.

The network extraction of the filled copper that goes beyond these closed forms (real fills, parallel paths,
finite and slotted planes, current sharing between vias) is section 13.

## 10. Conducted-emission prediction (phase 3)

Status: the linear-network prediction of `copper_emi::conducted` is wired into
`copper emi-check` as the finding `EMI-CONDUCTED` (module
`crates/copper-drc/src/emi/conducted.rs`), one per `buck` cell; other
topologies and cells whose inputs are missing get a `not_applicable` coverage
entry with the reason (a four-switch buck-boost needs the operating mode, which
the board does not declare; boost, buck-boost, inverting and flyback cells are
not predicted).

**The one supported operating mode** is a fixed-frequency buck (asynchronous or
synchronous) in continuous conduction. The converter is an ideal harmonic
current source drawing the input current of the high-side switch; the network
between it and the LISN is solved as one linear circuit
(`copper_emi::circuit`, modified nodal analysis by the simulator's solver
`copper-sim`, one solve per harmonic):

```text
  converter        L_loop      K (hot capacitor pads, feed)                LISN
  current   o-----[=====]-----+----------+------------+---[ L_f ]---+-----o  +
  source                      |          |            |            |
  (I_dm)                    === C_hot   [L_2]       [L_s1]       === C_s2 ...
                             |           |            |            |
                             |         === C_2     === C_s1        |
                             |           |            |            |
  return    o----------------+-----------+------------+------------+-----o  -
```

* Every capacitor is its own series R-L-C branch (`C` after its own DC-bias
  derating, `ESR`, `ESL`) with its own connection inductance `L_i`: the hot
  capacitor sits on node `K`, the other capacitors of the hot net (and those
  behind a fuse or other transparent feed) hang on `K` through `L_i`, and the
  capacitors on the supply side of the filter inductor hang on the supply-side
  node through their `L_i`. Nothing is lumped, so current sharing and the
  anti-resonances between unlike capacitors are in the result.
* The hot loop (capacitor to the switch pins and back, `EMI-HOT-LOOP`, traced or
  placement estimate) is the inductance `L_loop` between the converter's current
  source and `K`. It is in the converter's own branch, not in the path from the
  capacitor to the LISN: an ideal current source is not affected by a series
  inductance, so the level at the LISN does not depend on `L_loop`. A feed attached
  at the switch pins instead of the capacitor pads would put `L_loop` in series
  with the capacitor as seen from the feed; that raises the high harmonics with
  the loop inductance (and, near the capacitor's series resonance, lowers the
  fundamental), and is not modelled.
* The connection inductance of a capacitor other than the hot capacitor is an
  estimate, tag `estimated_connection_inductance`, stated in
  `measures.capacitor_branches[].connection_basis`:
  `2 x d x (L_hotloop / path length of the hot loop)`, with `d` the distance
  between the nearest pad centres of the capacitor and of its reference (the hot
  capacitor for a hot-net capacitor, the filter inductor's terminal on the upstream net
  for a supply-side capacitor). The per-length value is the board's own hot-loop
  inductance (which includes the plane under it) over its closed path length, so the
  go-and-return connection of length `d` is `2 d` of it. Mutual coupling between
  branches is not modelled in this default form.
* With `copper emi-check --extract` (section 13) the connection inductances of the other capacitors of the hot net
  come from the copper instead (tag `extracted_connection_inductance`, `measures.extracted_connections`,
  `connection_source` and, beside the extracted `connection_inductance_nh`, the estimate in
  `connection_inductance_estimate_nh`). The extraction is one inductance matrix `L` of ports across the hot loop
  (the switch, the hot capacitor and the catch diode closed by copper bars) and across each other capacitor of the
  hot net (its pads open), read at 10 MHz. Each port current leaves the hot node `K` along the V+ copper
  toward the switch or toward its capacitor, so `L_11` replaces the hot-loop inductance, `L_ii` the connection
  inductance of capacitor `i`, and a positive `L_1i` or `L_ij` is the mutual inductance between the corresponding
  inductors of the circuit, realised with `Circuit::mutual` (`ConductedInput::couplings`, `k = M / sqrt(L_a L_b)`; a
  matrix that is not positive definite is not used). With a coupling the hot-loop inductance is no longer without
  effect on the level at the LISN: the coupled inductors form a transformer between the source branch and the
  capacitor branches. The resistance of the interconnect is not in the circuit. Capacitors on the supply side of an
  input filter are not ports of the extraction and keep the estimate. So does a capacitor that no copper of the
  board joins to the hot node (a pad of it is not on the same piece of copper as the hot capacitor's pad on its net, decided
  before meshing from the copper's own connectivity): it gets **no port and no bar** in the extraction, so nothing is added
  to the board and the other capacitors' numbers are those of the board without it (to 1e-9 relative: `tests/connection_legs.rs`);
  its connection inductance stays the pad-distance estimate with its estimated provenance (`connection_source = estimated`,
  tag `estimated_connection_inductance`), the basis text and a note say "not extracted" with the pads that are not joined,
  and `measures.extracted_connections.estimated_connections_unconnected` and `measures.extract.unconnected_capacitors` name
  it. With the explicit option `BoardOptions::connection_legs` (off; the CLI does not set it) a straight bar is added for
  each pad that needs one, such a capacitor does get a port, and its row is still not used (`estimated_connections_synthesized_leg`).
  An extraction whose status is `error` (the mesh does not have the copper's connectivity) is not used at all, and the CLI and
  the extract text say why (count and first mismatch, `--extract-budget N`; `measures.extract.not_used_in_conducted`); the extraction's status (`ok` or
  `indicative`) is carried into the finding (tag `extraction_indicative`). Without `--extract` nothing here changes:
  `emi_check` and `emi_check_with_connections` with no matrix give identical reports. The coupled circuit is
  checked against a hand nodal solution of the branch current of a second capacitor with a mutual inductance to the
  hot-loop inductor (`copper-emi`, `coupled_connection_inductors_match_a_hand_nodal_solution`, to 1e-6) and the
  injection from `copper-drc` against the same hand solution at the fundamental (2 %).
* The result states which capacitor carries the largest share of the source
  current at the fundamental and at the worst harmonic
  (`measures.largest_current_share_fundamental`, `_worst_harmonic`, and `current_share_*` per branch). A share is
  `|I_branch| / |I_source|` at one harmonic, a ratio of the magnitudes of complex currents and not a partition of the
  source current: the branches can circulate current between themselves, so the shares can add to more than 100 % (the
  filter example reads CIN 100.4 %, CUP 0.42 %). The output says so wherever it prints them
  (`measures.current_share_definition`, an `input:` line, the headline). The anti-resonances of the impedance seen at `K`
  between 150 kHz and 30 MHz are all listed in `measures.anti_resonances`: local maxima of `|Z|` with a prominence of
  0.5 dB on both sides, each with its frequency and impedance magnitude, `envelope_impedance_ohm` (the smallest `|Z|` of a
  single capacitor branch on `K` at that frequency, each with its connection inductance), `rise_over_envelope_db`, the
  reported `harmonic` in its lobe (between the nearest local minima of `|Z|` on each side) with the largest
  `harmonic_rise_db`, `significant` and a `text`. **Significance criterion.** The LISN level of a harmonic is proportional
  to `|Z_K|` (the source drives `K`, and the path from `K` to the LISN does not depend on the branches on `K`), and
  parallel real branches that do not interact never exceed the best single branch, so
  `harmonic_rise_db = 20 log10(|Z_K(f_h)| / min_i |Z_i(f_h)|)` is by how much the interaction of the branches raises that
  harmonic's level over what `K` held by its best capacitor branch alone would give. A peak is **significant**
  when a reported harmonic in its lobe has `harmonic_rise_db >= 6.02 dB` (a factor 2, `ANTI_RESONANCE_SIGNIFICANT_DB`); only then
  is it tagged `anti_resonance_in_band` and named in the headline (the others are counted: "N further anti-resonance(s) not
  significant"). At the peak of two branches with reactances `+X` and `-X` the ratio to either branch is about
  `X / (R1 + R2)`, the quality factor of the loop they form, which is why unlike capacitors with low ESR pass and two identical
  capacitors a few millimetres apart (a 1.7 MHz peak of 18 mohm, 3.1 dB over the better capacitor, in the example test) do
  not. A sharp peak that falls between the harmonics of a fixed frequency is listed but not significant (it would matter for a
  tolerance, a range or a spread-spectrum clock, which are not modelled). A peak made by the filter inductor against the hot
  capacitor is judged against the hot capacitor.

**Scenarios.** `measures.scenario` names the one that is the headline.

* `declared_operating_point`: the converter's component declares
  `load_current` (section 4). With `D = Vout / Vin` (the ideal lossless buck),
  the power inductor `L` found on the switch net and the switching frequency
  `f`, the inductor current ripple is `dI = (Vin - Vout) D / (L f)` and the input
  current is zero in the off-time and a ramp from `I_out - dI/2` to
  `I_out + dI/2` during the on-time (`D T` between the 50 % points of the current
  edges), with linear current edges of finite 0-100 % time. Its harmonics are
  exact: the waveform is piecewise linear and each segment's Fourier integral is
  closed form (`PeriodicSpectrum::piecewise_linear`, `buck_input_current`). If the inductance cannot
  be resolved (the inductor's component value is not an inductance) the ripple is the
  rule-of-thumb ratio band of section 10.1, tag `rule_of_thumb_ripple`. The switch pin's `current_peak`, when declared, is
  reported as the secondary worst case, `measures.secondary_scenario`
  (`scenario: current_limit`, the same `harmonics`, `worst_harmonic`,
  `fundamental` keys).
* Two conditions are checked, in this order. Continuous conduction: `I_out >= dI/2` (the valley of the ramp is not
  negative). Otherwise (`measures.operating_point.status = outside_model`, `reason_code =
  discontinuous_conduction`, tag `operating_point_outside_model`; discontinuous conduction, pulse skipping and
  forced-PWM light-load operation) the model does not apply and says so. And the switch current limit: the peak
  `I_out + dI/2` (nominal ripple) must not exceed the switch pin's `current_peak`, because a converter whose current
  would pass the limit current-limits and the ramp-on-pedestal waveform does not exist (1e-9 relative slack). Otherwise
  the status is `outside_model` too, with `reason_code = peak_exceeds_current_limit`, a `reason` that gives the peak, the
  load, the ripple and the limit (for `load_current = 4A` on `examples/emi_buck`: dI = 1.2411 A, peak 4.6206 A against
  4.5 A; 3.8 A, peak 4.4206 A, stays supported), and the same tag. Either way the declared point is not evaluated, the
  detector comparison is not touched, and the current-limit scenario is the only one reported (without a
  `current_peak` the prediction is skipped, coverage `not_applicable` with the reason; the limit check cannot fail
  without a limit). `continuous_conduction` stays what it is (a point can conduct continuously and still exceed the
  limit). The one-page summary and the plots word the two reasons apart. With the rule-of-thumb ripple band (the
  inductor's value is not known) a point whose nominal peak is within the limit but whose band's high end is above it stays
  supported and carries the tag `peak_may_exceed_current_limit` and a sentence with the nominal peak, the limit and the
  high-end peak (`operating_point.peak_at_high_ripple_a`).
* `current_limit`: no operating point is declared (`operating_point.status =
  not_declared`, which names how to declare one), or it is outside the model. The
  switch pin's `current_peak`, the current LIMIT, is the amplitude of flat-top
  pulses of the same duty and edges. This is a worst case that bounds operation, not
  a prediction of it, and it carries no band for the operating current (how the
  converter is used is not a parameter of the design, so no rule of thumb stands in
  for it). Tag `worst_case_current`.

The radiated estimates (`EMI-HOT-LOOP` field, stored energy and overshoot, and the near-field map) follow the same scenario of the
cell (section 8, "Scenario of the hot-loop estimates"): the declared `load_current` drives them too, with the current limit
as the secondary worst case.

Inputs, each recorded with its source in `measures.inputs`, `measures.inputs_text`
and the coverage detail:

| Input | Source |
| --- | --- |
| Vin, Vout, duty | declared `supply` voltage of the hot-supply net and of the magnetic's output net; duty is Vout / Vin |
| Frequency | the fixed value; for a range the lower and the upper bound as two scenarios, tagged `frequency_range_bound` |
| Current edge time | the switch pin `current_rise_time`/`current_fall_time` (10-90 %, `measures.current_edge_source = declared`); else the voltage edge stands in for it (`current_edge_source = voltage_edge`, tag `current_edge_from_voltage_edge`, an assumption: the two are different quantities). The voltage edge is the switch pin `rise_time`/`fall_time`, else the edge of a declared `slew_rate` (`0.8 V_swing / slew`, tag `edge_from_slew_rate`), else the default slew band (section 10.1, tag `rule_of_thumb_edge`); when the current edge stands in for the voltage edge it takes its band, no separate number is invented. Divided by 0.8 for the 0-100 % ramp. Every harmonic to 30 MHz is reported: with a default edge the level is evaluated at the fast and the slow edge and the band collapses below the edge corner. The switch-node voltage edge keeps driving the common-mode score and the ringing estimate |
| Load current, ripple | the component's `load_current`; the inductor value from the magnetic found on the switch net; `measures.operating_point` (status, `load_current_a`, `current_limit_a`, `inductor`, `inductance_uh`, `ripple_peak_to_peak_a`, `ripple_basis`, `valley_a`, `peak_a`, `continuous_conduction`, `reason`, `how_to_declare`) |
| Current limit | the switch pin `current_peak` (the `current_limit` scenario and the secondary worst case) |
| Capacitors | each capacitor of the hot net (the hot-loop analysis' selected one is the hot capacitor, the others of `hot_capacitors`) and of the supply side of the filter is a branch: capacitance from the component value, or `capacitance_at_bias` when declared; `esr`, `esl`, `self_resonance` (ESL derived) from its part; otherwise 5 mohm and ESL by package from the footprint name (0402 0.4, 0603 0.5, 0805 0.6, 1206 0.8, 1210 0.9 nH), tag `default_capacitor_parasitics`; an unrecognised package without declared values leaves that capacitor out (hot capacitor: the prediction is skipped), listed in `measures.skipped_capacitors`, tag `capacitor_branch_skipped`. `measures.capacitor_branches[]` states per capacitor `declared` (`esr`, `esl`, `capacitance_at_bias`), `defaulted` (`esr`, `esl`, `dc_bias`), the bases, the values used, the connection inductance and the current shares |
| DC bias | a ceramic capacitor (a recognised SMD package, not an electrolytic, tantalum, polymer or film category) whose component declares no `capacitance_at_bias` gets the package rule of thumb of section 10.1 (a band, tag `rule_of_thumb_dc_bias`) whatever its ESR and ESL; `capacitance_at_bias` (the datasheet DC-bias curve at the board's voltage, at most the component value; equal to it for a C0G part) replaces the rule and its band (`dc_bias_basis = declared`); any other capacitor has no derating applied, with the reason in `dc_bias_note` |
| Hot-loop inductance | `EMI-HOT-LOOP` (traced or placement estimate), between the converter's current source and `K` |
| Input filter | walks from the hot-supply net toward the supply (at most 4 series parts) through the elements that FEED it. A filter inductor is a `passive.inductor*` or `passive.ferrite*` part, or an uncategorized two-pin part whose value is an inductance; any other two-pin `dc_power_path` part (fuse, jumper, 0-ohm link, load switch) is a transparent DC feed that is followed through with its resistance unknown and taken as 0 (tag `feed_resistance_not_modelled`; "through fuse F_MAIN, resistance not modelled"). Direction comes from the far net: a declared `external` or sourced supply (or, failing that, a connector with no other load) means upstream; other loads there mean the element feeds them downstream and it is ignored (listed in the `:filter` note); if neither decides, the filter is left out and the note says the direction cannot be decided. Exactly one filter inductor may be on the path; its value, `esr` (DCR) and `self_resonance` (winding capacitance) give the inductor, each capacitor to the hot return beyond it is a supply-side branch, and each capacitor between the hot net and the inductor (behind a transparent feed) is a branch on `K`. Capacitors to the hot return behind a fuse with no inductor are branches on `K` as well; resistors above 1 kohm and diodes to the hot return (TVS, enable divider) are ignored with a note; anything else on the path nets (a load, a resistor of 1 kohm or less, a connector or source before the end of the path) leaves the filter out with a `:filter` coverage note. A filter inductor without `self_resonance` is ideal (`filter_inductor_ideal`: attenuation above its real resonance is overstated) |
| Common mode | not modelled (`:common_mode` is `not_applicable`; the `EMI-SWITCH-NODE` common-mode coupling-risk score is a ranking and is not an input here). The island capacitance of `EMI-SWITCH-NODE` is to the board's own ground plane, which is the converter's return: its current closes without entering the LISN, so using it as the capacitance to the LISN reference would overstate common mode by the plane's shielding. The real path is board-to-reference capacitance (heat sink, chassis, cable), geometry outside the board model. Levels are differential mode only and tagged `differential_mode_only`; common mode often dominates above a few MHz |

Upstream filter capacitors that take the package rule of thumb are derated in the same corner as the hot capacitor.

Levels are RMS dBuV of each harmonic at the worst LISN port. **Detector applicability.**
Peak, quasi-peak and average coincide with that RMS level only for an isolated line, and
`copper_emi::conducted::DetectorApplicability` decides that explicitly, per CISPR band (A:
200 Hz, B: 9 kHz, C and D: 120 kHz receiver bandwidth), as `measures.detector_applicability`
(`isolated_lines`, `receiver_emulation` or `not_applicable`), `detector_reason` and `detector_bands`
(`band`, `rbw_hz`, `harmonic_spacing_hz`, `harmonics`, `status`, `reason`). The isolated-line
decision is `isolated_lines` when the switching frequency is one constant value and the harmonic spacing (the switching frequency)
exceeds twice the band's bandwidth. It fails when
(a) the spacing is at most twice the bandwidth (several lines inside the IF filter; 18 kHz in
Band B), or (b) the frequency is not known to be one constant value: the part declares
`switching_frequency_min`/`_max` and the component does not fix the frequency (whether the range
is an adjustable setting or a spread-spectrum or modulated sweep is not declared).

**Case (a), one constant frequency: `receiver_emulation`.** A strictly periodic waveform with closely spaced harmonics
is run through the time-domain receiver model (`copper_emi::receiver::scan_lines`, which reads a periodic signal given by
its harmonic lines and is equal to `scan` of the sampled record that holds them, and `conducted::receiver_emulation`, which forms the LISN
port voltages of both lines from every harmonic of the solution, so the neighbours inside the IF filter are present, and reads the larger port).
A reading is a pure function of the source spectrum and of its own tuning: the lines it uses are those inside the
filter's window `tuned +- 4 half-bandwidths` (`Band::skirt_half_width`, 2 x the 6 dB bandwidth, where the Gaussian is 96 dB
down: 18 kHz either side in Band B), whichever other tunings are requested and in whatever order. The solve therefore
reaches `max over the tunings of (n + skirt_lines)` harmonics (`receiver::skirt_lines`: 36 lines at a 500 Hz fundamental in
Band B, 1 at 500 kHz), and a request that would need more than `conducted::MAX_EMULATION_LINES` (1 000 000) is refused
as a whole with `ConductedError::NotEvaluated` and the reason (`measures.receiver_emulation_failure`), never read
with a truncated window. (Before, the solve stopped a fixed 8 lines above the highest tuning: at a 500 Hz fundamental a
tuning at 150 kHz read 62.35 dBuV peak alone and 63.45 dBuV with a second tuning requested, the window being cut
at +4 kHz instead of +18 kHz. Only a window wider than 8 lines was cut: a harmonic spacing below 2.25 kHz in Band B. No reading at 500 kHz, 15 kHz or 3 kHz moved (the window is 1, 2 and 6 lines); below 2.25 kHz the reading of a tuning is now the value it has in a request that asks for it alone.)
A full scan of the thousands of lines of a low-frequency converter is not a screening-cost computation, so the detectors are
read at a bounded selection of at most 48 harmonics (`measures.receiver_emulation.selection_rule`,
`selected_as`): the 8 with the least quasi-peak margin and the 8 with the least average margin (line level against the
limit, when a conducted table applies), the 16 highest line levels, and 8 spread evenly over the reported harmonics
(the first and last included), in that priority. Every case of the rule-of-thumb inputs (nominal and each corner) is
emulated at those harmonics: the nominal reading is the headline, the highest and lowest readings over the cases are the band, and the
margin is the limit against the highest (an offset of the nominal case does not carry the band: at a spectral null of the
trapezoid the detector-over-line offset moved by 7 dB between two edge times in a trial, which is why every case is run). Cost is stated
as counts, not times, so the JSON stays deterministic (`cases`, `harmonic_solves`, `receiver_evaluations`; a harmonic solve
is one MNA solve, a receiver evaluation one small inverse FFT and the detector run, for each port and case). Measured
(debug build, `copper emi-check examples/emi_buck/board.copper` with the frequency changed): 15 kHz, 1 991 reported
harmonics, 23 read, 3 cases: 0.33 s before the receiver path existed, 1.39 s with it; 3 kHz, 9 951 harmonics: 1.41 s and 2.23 s;
the detector run per read is the cost that grows with the number of reads, which the cap of 48 bounds. Status and margins: the finding stays
`conducted_emission`, tagged `detector_receiver_emulation` and `line_amplitudes_outside_the_evaluated_harmonics`;
`margins_available` is true and `margins_scope` is `evaluated_harmonics_only`; only the evaluated rows carry
`limit_qp`, `limit_avg`, the margins and `receiver` (`peak_dbuv`, `quasi_peak_dbuv`, `average_dbuv`, their high ends and the
quasi-peak and average low ends, `selected_as`); the other rows are line amplitudes with null limits and margins; the headline is
the least-margin evaluated harmonic and its message says "detector readings by receiver emulation at N of M harmonics (the
others are line amplitudes, NOT detector readings)". The secondary current-limit scenario of a declared cell stays line
amplitudes (the emulation runs on the headline scenario only). **Constants.** The IF bandwidths (A 200 Hz, B 9 kHz, C and D 120 kHz) and the
quasi-peak charge / discharge / meter time constants (A 45 / 500 / 160 ms, B 1 / 160 / 160 ms, C and D 1 / 550 / 100 ms) are
per band in `Band::rbw` and `Band::quasi_peak_constants`, used per tuned frequency through `band_of` (so a conducted harmonic of
150 kHz to 30 MHz uses Band B), and are written from memory of CISPR 16-1-1 Table 1: **unverified against the standard**, and
the Gaussian IF shape is a stand-in for the standard's selectivity mask. Validation: where the lines are isolated (500 kHz) the
three detectors read the line level within 0.5 dB; for 15 kHz and 3 kHz converters the readings equal an independent time-domain
reference (the port waveform mixed down and convolved with the Gaussian impulse response, then the detector arithmetic) within 0.01 dB,
and at 3 kHz peak, quasi-peak and average differ from the line level and from each other by up to 5 dB. At 15 kHz the neighbouring lines are
67 dB down in the Gaussian response, so the detectors read the line level: the 2 x bandwidth rule of the isolated-line path is conservative there.
Jitter, spread-spectrum clocking, frequency modulation and burst or skip modes are not modelled by anything: they need a
record long enough to contain the modulation, and the receiver's sweep dwell time is not modelled. **Case (b) stays `not_applicable`:** the levels are
**line amplitudes, not detector readings**: `limit_qp`, `limit_avg` and every margin are `null`,
`margins_available` is false, the finding is `conducted_line_amplitude` (tags `detector_not_applicable`,
`line_amplitudes_only`), its message says "NOT a detector reading" and states no margin, the coverage
detail says the same, and every consumer (text, JSON, plot, issues, the route-report `emi` section)
carries the status.
**A declared range is two fixed-frequency scenarios**: the lower and the upper bound each give one
`EMI-CONDUCTED` finding (coverage `EMI-CONDUCTED:<cell>@min` and `@max`, tags
`frequency_range_bound` and `frequency_scenario_min`/`_max`, `measures.frequency_scenario`), both
with the status `not_applicable`; the swept or modulated case between them is not modelled. A
declared `switching_frequency` on the component fixes the frequency and restores the detector
comparison. The CISPR 32 mains-port quasi-peak and average lines are the reference limit
(`cispr32_class_a`, `cispr32_class_b` only; other standards and no constraint give levels only).
Margins are never pass or fail. The conducted estimate lists its named omissions in
`estimate.not_modelled` (common mode, spread spectrum and jitter, source impedance and control
interaction, component models beyond ESR/ESL/self-resonance, the supply impedance behind the
V-network, mutual inductance between the capacitor branches and the hot loop, the load-side network,
inductor saturation and the switch's dead time, reverse recovery and ringing, operating modes other than a
fixed-frequency buck in continuous conduction, capacitors that could not be modelled, zone fills, and an
assumed stack-up).
`measures.harmonics` lists `harmonic`, `frequency_hz`, `source_current_a`,
`dm_dbuv`, `cm_dbuv` (null), `total_dbuv` (the nominal case), `low_dbuv`, `high_dbuv`, `limit_qp`, `limit_avg`, `margin_qp_db`,
`margin_avg_db`, `margin_db` (from the high end; limit minus level, so negative is over the line); `measures.assumptions`
holds the `predict` assumptions and the tool's. The finding's message states the margin of the worst harmonic in words, as the
spectrum picture and the summary page do ("49.6 dB over the average limit", "6.4 dB under the quasi-peak limit", "at the
average limit"), never as a signed number against a line; the numbers stay in the `margin_*_db` fields. Tests:
`crates/copper-emi/src/conducted.rs` (the circuit solution of a hot capacitor alone and of hot capacitor +
bulk electrolytic + LC filter + damping branch against an independent ladder reduction to 0.1 dB; the anti-resonance of
unlike capacitors against `1/(2 pi sqrt(L C))`; the independence of the LISN level from the hot-loop inductance; the
CCM boundary; the anti-resonance significance criterion on unlike capacitors, identical close capacitors and a peak between harmonics;
the receiver emulation against the isolated-line level and against an independent time-domain reference for 15 kHz and 3 kHz converters),
`crates/copper-emi/src/receiver.rs` (`scan_lines` against `scan` and against the time-domain reference), `crates/copper-emi/src/harmonic.rs` (the ramp-on-pedestal harmonics against an independent analytic
Fourier series to 0.05 dB for 20 harmonics, against numerical integration with finite edges, and the edge roll-off), and
`crates/copper-drc/tests/emi_check.rs` against hand calculations on `examples/emi_buck` and
`examples/emi_buck_filter` (the hot-loop field of a declared operating point against a numerical transform of the same waveform, the
ringing currents, the receiver-emulation selection and status, the anti-resonance significance on a board, the current-share wording).


### 10.1 Rule-of-thumb defaults and bands

Where the language leaves an input open, `copper emi-check` fills the gap with
a default from one table (`crates/copper-drc/src/emi/defaults.rs`, named
constants with doc comments). **Every entry is a rule of thumb; none is
verified against a datasheet or a published source**, and none is ever shown as
a datasheet value. The rules of the output:

1. A declared value always wins; a default only fills a gap.
2. A default is tagged (`rule_of_thumb_*`), and its value and rule are printed
   like every other input, as an `input:` line "... (rule of thumb: <rule>;
   declare <attribute> to replace it)" and in the coverage detail. The summary
   counts the findings that rest on a rule (`summary.rule_of_thumb`, a line in
   `limits.notes`, a clause in the text summary).
3. A result that rests on a default is a BAND with the nominal, not one number:
   `estimate.band_db = [low, high]` with `estimate.basis = "rule_of_thumb"`
   (radiated: slow to fast edge; conducted: `low_dbuv`/`high_dbuv` per
   harmonic). The unfavourable end drives the margin: the fast bound for the
   radiated estimates, the high end for the conducted headline and margin. The
   band spans the defaulted inputs only; it is a parameter sensitivity, not a
   confidence interval, and it does not cover the mechanisms listed in
   `estimate.not_modelled`.
4. The silicon or wide-bandgap distinction is not an enum: it is inferred from
   an optional declared `slew_rate` (20 V/ns or more is reported as
   wide-bandgap class in the `input:` lines, information only).
5. The operating current of a converter is not a rule of thumb: it is declared
   (`load_current`) or the scenario is the current limit, a worst case.

| # | Rule | Nominal | Band | Tag | Replaced by |
| --- | --- | --- | --- | --- | --- |
| 1 | Edge time of an integrated silicon switch node, t(10-90 %) = 0.8 V_swing / slew with V_swing the hot-supply voltage; resolution order: declared `rise_time`/`fall_time`, then declared `slew_rate` (exact, tag `edge_from_slew_rate`, no band), then the default slew. Skipped when the swing is unknown. The current commutation edge takes this edge (and its band) when `current_rise_time`/`current_fall_time` are not declared (tag `current_edge_from_voltage_edge`) | 3 V/ns (24 V: 6.4 ns) | 10 V/ns fast to 1 V/ns slow | `rule_of_thumb_edge` | `rise_time`, `fall_time` or `slew_rate` on the switch pin; `current_rise_time`/`current_fall_time` for the current edge |
| 2a | Switch-node capacitance of an integrated converter FET, for ringing, when the controller part declares no `output_capacitance` | 250 pF | 100 to 500 pF | `rule_of_thumb_capacitance` | `output_capacitance` on the controller part |
| 2b | Switch-node capacitance of a Schottky catch diode, when the diode part declares no `output_capacitance`; the cell's `current_peak` stands in for the diode rating; times 1/3 when the reverse voltage (hot-supply voltage) is 12 V or more, and the band keeps the un-derated value as its high end | 75 pF per ampere | 50 to 100 pF per ampere | `rule_of_thumb_capacitance` | `output_capacitance` on the diode part |
| 3 | Ceramic DC-bias loss of every capacitor in a ceramic SMD package (hot, hot-net and filter capacitors) whose component declares no `capacitance_at_bias`, whatever its declared ESR and ESL: 0402, 0603 and 0805 lose 50 %, 1206 35 %, 1210 and larger 25 % | the derated value when the hot-supply voltage is 5 V or more, else the un-derated value | no loss to the package loss | `rule_of_thumb_dc_bias` | `capacitance_at_bias` on the component (equal to its value for a C0G part) |
| 4 | Inductor current ripple when the power inductor's value cannot be read from the board: peak to peak, the usual 20 to 40 % design range | 0.3 x the load current | 0.2 to 0.4 x the load current | `rule_of_thumb_ripple` | an inductance as the value of the power inductor on the switch net |
| 5 | Capacitor ESR 5 mohm and ESL by package (0402 0.4, 0603 0.5, 0805 0.6, 1206 0.8, 1210 0.9 nH) | as listed | none | `default_capacitor_parasitics` | `esr`, `esl`, `self_resonance` on the capacitor's part |

Band semantics. The conducted band of each harmonic is the minimum and maximum
over the corner cases {current edge fast, slow} x {capacitance un-derated,
derated} x {ripple ratio nominal, low, high} (only the corners whose input is a
rule of thumb), plus the nominal case. `total_dbuv` is the nominal case. Below
the edge corner the edge and DC-bias bands collapse (the level hardly depends on the edge) and a board that
declares everything has a closed band. The radiated estimate of a declared operating point takes the same ripple corners (and the fast and slow edge) as
cases of its worst-line search: its band is the lowest and highest worst level over them and its margin the least over them; the
current-limit scenario keeps the current at the limit (a declared `current_peak`, no band for the current).

No default can make a headline number more favourable than the physics allows:
the high end includes the un-derated capacitance and the fast edge, and a
missing edge time no longer restricts the reported harmonics.


Module `copper_emi::spectrum`: trapezoid and sampled-waveform spectra;
`copper_emi::lisn`: 50 uH / 5 uH LISN transfer; `copper_emi::receiver`:
CISPR 16-1-1 style peak, quasi-peak and average detection of a sampled
waveform, or of a periodic signal given by its harmonic lines (`scan_lines`), by an
IF filter in the frequency domain (bands A-D, 200 Hz / 9 kHz / 120 kHz RBW; the detector
time constants are written from memory of CISPR 16-1-1 Table 1 and are
**unverified against the standard**). The record is taken as
exactly one period of a periodic signal and transformed at its own length
(`copper_emi::spectrum::dft`: the radix-2 FFT for a power of two, Bluestein's chirp-z
algorithm otherwise), so it is never zero-padded: a padded record would be a dead time that
lowers the average and quasi-peak readings. The caller supplies a whole number of periods;
the tests check a CW line and a gated carrier at 12 000, 20 000 and 24 000 samples
against their analytic peak (A/sqrt 2) and average (duty A/sqrt 2) and the quasi-peak against a
time-domain reference (direct convolution with the Gaussian IF response, no FFT). A linear MNA
solver for the extracted network plus LISN follows; nonlinear device models go
to ngspice as an optional external process.

## 11. Region-of-interest full wave (phase 4)

Status: fifth iteration implemented in `crates/copper-fullwave`. Screening
evidence only: nothing here qualifies a design. The tier stays optional (`--fullwave`, minutes per cell). Iteration 3 found where the cell dependence of the
loop inductance came from (the rule that moves copper edges, not the port), replaced the rule, completes
the records of loops that ring instead of waiting for them to decay, and states a mesh sensitivity next to
the inductance. Iteration 4 puts a floor under the cells that dense copper forces on the time step
(a third fewer steps and half the run time on the TPS62130A), runs the validation models on the default edge
rule, and found that the strap bias is not what its validation test measures (below). Iteration 5 answers an external
review of the tier: the ring-down fit is accepted only on evidence (a withheld part of its tail is predicted, its poles
decay, the completed response is passive; below, "Acceptance of the ring-down fit"), the edge rule is measured on
geometries it was not fitted on against the quasi-static extraction of section 13 ("The edge rule on other geometries"),
the pours are the native zone fill instead of the outline minus clearances ("Zone fills"), the strap bias is measured
("The strap bias, measured"), and the report sets the extraction's 100 MHz value beside the full wave's with both numerical
sensitivities ("Cross-check against the extraction").

**What it is.** `RegionModel::build(board, &RegionSpec)` translates the part of a
`PhysicalBoard` inside an axis-aligned window into a model for the pinned
`ems` FDTD solver, and `solve_port` / `solve_all` run it (one run per excited
port, the others terminated in their reference impedance) and return Z, S and
Touchstone text. `describe()` states the mesh, the cost estimate, the flags and
every approximation before anything runs. `hotloop::extract_hot_loop` builds
the region of a buck cell's hot loop from its `EMI-HOT-LOOP` finding and
returns the loop inductance, from two meshes when the budget allows;
`copper emi-check --fullwave` attaches it to the finding (see the language
reference).

**Model contents.**

- Dielectric slabs for every stack-up layer inside the window (relative
  permittivity, loss tangent as a conductivity at the band centre), air above
  and below, PML (8 cells) on all six sides. A board without a declared
  stack-up gets its copper layers spread over the board thickness with FR-4
  (er 4.5, loss tangent 0.02) between them, flag `assumed_stackup`.
- Copper as zero-thickness PEC sheets on mesh planes (optionally the solver's
  `ConductingSheet` per net, default PEC): placed pads (a rounded pad is the hull
  of 12-gons of equal area), tracks at any angle (a rectangle with square
  ends), arcs (chord chains), copper polygons, zones and the copper of hard macros. A zone is, by default
  since iteration 5, its native fill (`crate::fill`, `RegionSpec::zone_fills`, flag `zone_fill_used`: section "Zone fills"
  below); where no fill is available (`--fullwave-zone-outline`, or the export of the board failed) it is its
  outline minus its declared holes, an octagonal antipad where a via or plated hole of another net
  crosses it and, new in iteration 2, the zone clearance around the copper of other nets on the same
  layer (pads, tracks, bridges, legs: a polygon difference with `copper_cam::poly`, one keyhole sheet
  per piece that remains; flags `zone_outline_used`, `zone_cut_around_foreign_copper`): thermal reliefs, minimum widths and
  islands are then not applied. Without the cut, a
  pour on the layer of the loop's pads shorted the switch node to ground in the CopperOrgan model
  (flag `zone_overlaps_foreign_copper` now means a pour is still over foreign copper).
- Vias and plated holes as one-cell vertical PEC wires between the layers they span,
  on the mesh line the mesher puts at their centre (no barrel radius, no annular
  rings).
- Bridges (the hull of two pads as a PEC strap, for a capacitor or diode),
  added traces (the straight legs of a placed, unrouted loop), port straps (below)
  and lumped ports: in-plane between two pads or rectangles, or from a pad down to
  a plane layer. `solve_all` gives S; `PortMatrix::z_at` gives Z for real
  reference impedances; `touchstone()` writes `.sNp`.
- Copper is cut at the window edge (flag `copper_cut_at_window`) and planes end
  there (flag `planes_truncated`) unless `extend_planes` continues plane-like
  copper and the dielectric through the air into the PML. Keep the margin
  generous.

**Ports.** An in-plane port is a lumped element in a box between two terminals
on one layer.

- *Facing pads.* When the two terminals overlap in one axis and are separated
  in the other, with no copper of another net crossing the gap, the port spans the gap
  between the (inset) pad edges, as wide as the overlap. This is the
  asynchronous buck's port across the high-side switch. Copper of the
  terminals' own nets that reaches into the gap (the square end of a placed leg) does not
  block it.
- *Pads that do not face each other* (diagonal, or an obstructed gap: a
  QFN's pads on different sides, the common case on synchronous bucks): a strap port
  (flag `port_strap`). PEC straps of width equal to the narrowest pad dimension
  (0.2 to 1 mm, the closed form's bar width) run from the nearest edge of each pad, as one
  straight bar when the pads overlap in one axis and as an L (horizontal out of one pad, vertical
  into the other) when they overlap in neither, and the port sits at the middle of the longest leg
  with a gap of two cells (at least 0.4 mm). `PortSpec::with_strap()` forces this for
  facing pads. The strap is not in the real loop, so its inductance is a known bias that the
  result includes and does not subtract: `port_strap_nh` is the closed form of the
  strap bars over the nearest plane below (partial inductances with the image; free space when
  there is none), and it is stated in the notes. The straps cross whatever lies between the
  pads. Where foreign copper lies under that path (the exposed pad of a QFN or HSOIC between the pads
  of two sides: the strap would short it to the switch node, and the port box on it has no usable
  edge), the strap runs in the air, `AIR_BRIDGE_HEIGHT_MM` = 0.3 mm above the layer on two vertical
  PEC posts at the pads (flag `port_strap_in_air`): the package's lead frame, with the port in its middle.
  The bias is then larger and less certain; the closed form uses the bars at that height over the plane
  (the posts are not in the estimate).
- *Synchronous buck.* The finding's four-pad loop (capacitor, hot supply, hot return,
  capacitor) does not name the switch node; `copper emi-check` looks up the controller's pad on a
  switch net nearest the hot supply pad. With it, the port is across the high-side switch (hot supply
  pad to switch pad, a strap port when they do not face each other) and the low-side switch is a bridge
  from the switch pad to the hot return pad (the hull of the two pads), so the model carries the
  loop capacitor, high-side switch, low-side switch and capacitor in series. Without a switch pad the
  port is directly across the hot supply and hot return pads (the earlier model). The hull of two
  distant pads is a copper blob that covers whatever lies between them; the
  flag for that is not yet raised (see next steps).

**Mesher.** Every line is chosen by the crate. Lines at copper edges (the
axis-parallel edges and bounding box of each ring), ports, vias and the window;
lines within 0.4 of the local cell merge into the higher rank (port > copper > via >
window), lines of different nets never merge unless closer than a quarter of the minimum cell
(0.075 mm with smoothing: see below), and every move is reported. The general
copper cell is half (coarse) or a quarter (fine) of the narrowest modelled feature,
clamped to 0.15-0.5 mm (coarse) and 0.075-0.25 mm (fine); z has at least two (three fine) cells per
dielectric; cells grow by 1.3 outside the fine zone, at most 1 mm in the window and
3 mm in air.

- *Local refinement* (`RegionSpec::refine`, `RefineBox`). A rectangle of the
  board gets its own cell, `min(box cell, general cell) x refine_scale`; outside it
  cells grow by 0.3 per unit of distance, so neighbours stay within 1.3 to 1.5. The mesh is a
  tensor product, so the x range of a box is fine for every y. The hot loop refines the whole
  loop window (pads, legs, bridges, port, polygon, plus 0.3 mm) at a quarter of the narrowest
  loop pad, 0.2 to 0.25 mm (up to 200 mm2, else only around the port, the bridges and the
  pads); the rest stays at the general cell. The z cells follow the finest cell.
- *The edge rule* (`RegionSpec::edge_inset` and `edge_inset_z`): every outward-facing copper edge moves into
  the metal by 0.23 of the local lateral cell plus 0.11 of the vertical cell at its layer (the mean of the cells
  of the dielectric above and below the layer plane; at the top layer the cell below with the air cell above it grown
  by 1.3), because a PEC edge on a mesh line behaves as if the metal reached further: its singular field fills the
  dual cell. The lateral cell is that of the edge's normal axis at the edge, so a pad inside a refinement box is
  moved by the refined cell. **The rule is applied to the sheets of pads, tracks, arcs, polygons, bridges, added traces and
  the terminals of ports, and not to zones** (pours, planes and native fills keep the edges they have; measured in iteration 5
  on a slotted plane, below). The rule before iteration 3 was a third of the lateral cell and nothing vertical, fitted on
  one w/h = 2 microstrip at a 0.5 mm cell; the new one is fitted on the inductance of shorted lines (below), and the
  change is the answer to the open question of iteration 2. `describe()` states the rule.
- *Smooth meshes* (`RegionSpec::smooth_mesh`, always on for the hot loop and with refinement
  zones; off for the validation models, which keep the original cutting). In iteration 1
  the grading pass split the larger of two cells whose ratio exceeded 1.5 into equal pieces, up
  to 24 times. A jump of 1.55 at one kept line therefore cascaded outwards, halving every cell
  to the boundary: the coarse emi_buck mesh had 3 violations and many more cells than its features
  needed. Now a gap between kept lines narrower than 0.9 of the target becomes a refinement
  zone of its own (cells grow from its size by 1.3), a gap starts with at most 1.3 times the cell
  before it and ends in cells rounded to the target, free lines are relaxed where neighbours differ by
  more than 1.4, and the splitting pass never cuts a piece below 0.9 of the smallest cell the
  features need (the time step follows the smallest cell; 0.029 mm cells had cost the CopperOrgan
  model 1.5 times the steps). What remains is reported, with positions and where they are (window,
  air or PML) in `describe()`; the remaining pairs sit in crowded corners of dense copper
  or in the air.
- *Cell floor* (`CELL_FLOOR_OF_SMALLEST_GAP` in `mesh.rs`, smooth meshes only). The time step follows the
  smallest cell, and in dense copper the smallest cell was not a feature: a kept gap of 0.066 mm that followed a cell
  of 0.040 mm was cut into two cells of 0.033 mm, and the grading pass spread cells of 0.03 mm around it. Now no cell
  is cut below 0.92 of the smallest gap between two kept lines on the axis (nor below the merge distance of the
  features' own refinement), and the last cells of a gap cannot be thinner than that; the smallest cell of an axis is then the
  smallest real gap (at least 0.06 mm at the 0.15 mm cell, because lines closer than 0.4 of the cell merge, different nets
  excepted). The merge distance itself stays at 0.4 of the cell: 0.5 gave a 2.4 % lower L on the TPS62130A and 0.6 a
  5 % higher one on the placed emi_buck. The floor does nothing on boards without such gaps (emi_buck placed and routed, the
  shorted lines: identical meshes, steps and L to the bit); it is `mesh::tests::grading_in_dense_copper_never_cuts_cells_far_below_the_smallest_gap`.
  The factor 0.92 is a measurement, not a derivation (TPS62130A, one mesh of 0.15 mm, L against the run before the floor, 2.865 nH):

  | Floor | Cells | Steps | Run | L |
  | --- | --- | --- | --- | --- |
  | none (iteration 3) | 2.37 M | 45,751 | 397 s | 2.865 nH |
  | 0.85 | 1.80 M | 30,151 | 197 s | 2.838 nH (-0.9 %) |
  | 0.92 (shipped) | 1.77 M | 28,601 | 189 s | 2.823 nH (-1.5 %) |
  | 1.00 | 1.71 M | 27,751 | 177 s | 2.822 nH (-1.5 %) |
  | 1.00 with merge 0.5 of the cell | 1.31 M | 23,251 | 114 s | 2.796 nH (-2.4 %) |

  The mesh sensitivity of this loop (4 % floor) is larger than any of these moves. 0.92 was chosen as the smallest step
  count whose L stayed inside the 1.5 % set as the limit (-1.47 %: the margin is thin, and 1.00 sits at -1.50 %); a value of
  0.70 or less gives the sub-0.05 mm cells and most of the old step count back (estimated 45 k steps).
- Before the solver starts the model reports cells, the time step, the expected steps and the
  run time at an assumed 150 M cell updates per second (the measured rate is 250-300 M/s
  on the 6-core laptop), and a run above `Budget` (cell updates or seconds) is refused.

**Run control.** The excitation is a baseband Gaussian with 20 dB at the top of the band
(1 GHz); its support is 3.6 / 1 GHz = 3.6 ns, and the solver does not look at the energy
before the pulse has ended. On the validation loops over a plane the energy was 55 to 65 dB below
its maximum when the pulse ended, so the run ends at
the first energy check after 3.6 ns (13,451 steps of 0.268 ps on the 6 x 4 mm loop; 13,601 steps of 0.264 ps
on the placed emi_buck); the loop in free space, which radiates and rings longer, needed 4.14 ns (-34 dB at
4.05 ns), and the CopperOrgan loop, which resonates at 348 MHz, had not reached -35 dB when the cap ended
it (below).
The pre-run estimate is now `3.6 ns + 8 diagonals / c` in simulated time over the Courant step
(iteration 1 used 8 dielectric transits, which put it 24 to 39 % above the run on the four plane cases
measured here and 3 % above the free-space one; the one fine placed run of iteration 1 took twice the expected
steps, most likely because slivers of 0.04 mm in its mesh set a time step that the estimate did not see (the mesher no longer produces them; not re-run on the old code):
the fine placed run now takes 13,751 steps against 16,123 expected). It is 15 to 18 % above the run on all
seven decaying hot-loop runs of this section (placed, fine, routed and TPS62130A meshes), and 3.0 times below it
on the CopperOrgan loop, which rings: the solver's cap of three times the expected time ended that run with the
energy above -35 dB (`energy_not_converged`). **A finite record cannot replace the decayed one:** truncating the V and
I records of the loops at 80 % of their length (no window) changes L by -2.2 % (6 x 4 mm loop), -4.6 % (10 x 6 mm
over a plane) and -8.7 % (free space); a Hann taper over the last half of the record by -10 to -13 %
at full length and -26 to -29 % at 80 %, because the product of Z with the window is not Z times the
window's spectrum when the loop's memory is not short against the window; so the decay-based end
criterion stays for loops that decay, and so does the 35 dB threshold; a loop that rings is completed by a fit of
its ring-down instead (below, `Ring-down fit`). A shorter pulse is the other lever:
scaling its bandwidth by 1.5 / 2 / 3 shortens the run by 33 / 50 / 61 % over a plane
(19 / 29 / 38 % in free space) and changes L by -0.5 / -1.8 / -2.4 % (-0.5 / -0.85 / -1.3 %), so
`pulse_scale` exists (`RegionSpec`, default 1) and is not used. **Wall-clock guard:** a run whose time loop
takes longer than `Budget::max_seconds x wall_margin` (1.5) seconds is stopped from the
solver's progress callback and reported as `Error::BudgetExceeded` with the steps done
against the steps expected, the simulated time, the energy level and the seconds
(`measures.fullwave.status = "budget_exceeded"`, coverage `failed` with the same
text); the check on the estimate before the run is unchanged.

**Convergence estimate and mesh sensitivity.** `extract_hot_loop` runs the standard mesh and, unless
`convergence` is off or the budget is short, a second one with the refinement cells divided by
1.5 (or, if that does not fit what is left of the budget at the update rate measured by the first run,
multiplied by 1.5, which is cheaper; a mesh identical to the standard one is not run). `inductance_nh` is the
finer mesh's; `mesh_difference_pct` is |L2 - L1| / L(finer) in percent (a measured difference, not an error bound); `meshes` lists both runs; a single
mesh gives `mesh_difference_pct: null` and the flag `single_mesh`. Iteration 2 also reported a first-order
extrapolation; with the new edge rule the curve is no longer first order (below) and the field is removed. What is
reported instead is `mesh_sensitivity_pct`, the numerical sensitivity of the inductance to the mesh: the larger of the difference of the two meshes and a floor of 4 %
(`MESH_SENSITIVITY_FLOOR_PCT`, `mesh_sensitivity_floor_pct`): on the placed emi_buck the two meshes of the pair differ by 0.6 % while eleven meshes of
the same loop (cells of 0.10 to 0.25 mm, merge distances of 0.1 to 0.4 of the cell) span 4.62 to 4.90 nH, a half range of
3 %, so a difference of two meshes alone would understate how far the number moves with mesh decisions. The floor is that
observed scatter, reported as a sensitivity (`mesh_sensitivity_basis` says so); it is not an accuracy. The assumed stack-up,
the zone outlines, the bridged components and the strap bias are not in it: they are listed by name in
`measures.fullwave.not_modelled`. **When the record did not decay and the ring-down fit did not
stabilise** (`energy_not_converged` without `ring_fitted`), `status` is `indicative`,
`mesh_sensitivity_pct` is `null` and `mesh_sensitivity_unavailable` gives the reason, because a sensitivity of
a number that is not a converged result would read as a validity it does not have; the CopperOrgan run
below is that case. The fit's own spread of orders (`fit.rs`) is likewise a numerical sensitivity, not an error bound.

**Validation** (synthetic boards, `crates/copper-fullwave/tests/validation.rs` and `tests/shorted_line.rs`;
debug build with the optimised solver, 6 threads; every number measured). Until iteration 3 the meshes of cases 3a-3e
were pinned to the rule they were validated with (a third of the lateral cell, nothing for the vertical), so the
default rule had no always-run coverage. In iteration 4 the pin is gone: the tests run the default rule of the model
(`VAL_INSET` and `VAL_INSETZ`, in thousandths, override it for the studies), and the cases below were re-run on the final
code with it (the "default rule" column is the one that counts, and reproduces the column of iteration 3 exactly; the column
"iteration 2" is the third-of-a-cell rule they were pinned to).

| Case | Target | Iteration 2 (re-run, identical) | Default rule | Result |
| --- | --- | --- | --- | --- |
| 3a microstrip w/h = 2, h 0.5 mm, er 4.3: Z0 from S | Hammerstad-Jensen 49.40 ohm, 5 % | 50.53 ohm (+2.3 %) | 51.43 ohm (+4.1 %) | pass |
| 3a h 0.2 mm, w 0.4 mm | 5 % | 50.29 ohm (+1.8 %) | 51.22 ohm (+3.7 %) | pass |
| 3a h 1.5 mm, w 3.0 mm | 5 % | 49.09 ohm (-0.6 %) | 49.79 ohm (+0.8 %) | pass |
| 3a terminated in its Z0 | S11 < -15 dB, S21 near 0 dB | -33.6 dB, -0.01 dB | -32.6 dB, -0.01 dB | pass |
| 3b 10 x 6 mm loop of 0.5 mm trace, 0.2 mm over a plane | closed form 8.110 nH, 15 % | 8.255 nH (+1.8 %), flat to 0.23 %; 564 k cells, 34 s | 8.265 nH (+1.9 %); 1.31 M cells, 79 s | pass |
| 3b same, free space | closed form 21.701 nH, 15 % | 21.144 nH (-2.6 %); 318 k cells | 20.907 nH (-3.7 %); 440 k cells | pass |
| 3c bottom bar on In1.Cu through two vias | closed form 10.784 nH, direction and 15 % | 10.926 nH (+1.3 %); all on F.Cu 11.811 nH; change -0.885 nH against -1.003 nH | 10.893 nH (+1.0 %); all on F.Cu 11.775 nH; change -0.882 nH | pass |
| 3d 6 x 4 mm loop, coarse against fine | 5 % | 5.036 nH against 4.926 nH (+2.2 %); closed form 4.987 nH | 5.040 nH against 4.943 nH (+2.0 %) | pass |
| 3e reciprocity and passivity of 3a | 0.1 dB, a few degrees, \|S11\|^2+\|S21\|^2 <= 1 | 0.006 dB, 0.008 deg, 1.00007 | 0.0074 dB, 0.015 deg, 1.00017 | pass |

The default rule makes the S-parameter impedance of the coarse default-cell microstrips 1.4 to 1.8 points worse in 3a (still
inside 5 %) and leaves the inductance cases where they were. Why, measured (`z0_probe`, Z0 of the 20 mm line from S against
Hammerstad-Jensen, default rule against a third of the lateral cell; 'coarse' is the default cell, half the narrowest feature up
to 0.5 mm; the third is 333.3333 thousandths, and a third written as 0.333 moves some of these by 1.5 points, the size of the
mesh-quantisation noise of this measurement):

| Line (h, w) | 0.167 mm | 0.25 mm | 0.4 mm | coarse default |
| --- | --- | --- | --- | --- |
| 0.5, 1.0 (w/h 2) | -0.12 % / (-0.59 %) | -0.02 % / -2.03 % | -1.05 % / +0.09 % | +4.11 % / +2.30 % (0.5 mm) |
| 0.5, 0.5 (w/h 1) | +0.91 % / +0.14 % | +0.59 % / -2.51 % (coarse) | +5.50 % / +7.61 % | as 0.25 mm |
| 0.3, 1.5 (w/h 5) | +0.12 % / | +0.92 % / -0.15 % | | -2.29 % / +0.69 % (0.5 mm) |
| 0.2, 0.4 (w/h 2) | | -1.86 % / +1.94 % | | +3.68 % / +1.79 % |
| 1.5, 3.0 (w/h 2) | | | | +0.79 % / -0.62 % (0.5 mm) |

Each cell is default rule / third; values in brackets used 0.333. The default rule is within 1.9 % at cells of 0.167 to 0.25 mm on all
five lines (the range the rule was fitted in), and every error above 3 % is at a cell of 0.4 mm or more or at the coarse default
(the 1 mm strip at 0.5 mm, the 0.4 mm strip at its coarse default, the 0.5 mm strip at 0.4 mm), for either rule. At the coarse cell the sign of the error changes between geometries (+4.1, +3.7, -2.3, +0.8 %), and the effective
strip width after the rule and the merging (0.707 mm for the 1 mm strip against 0.667 mm for the third) does not order the
errors (the narrower strip reads the lower Z0), so the capacitance side does not need a different effective width and one
rule serves both within 2 % over the range it is fitted for; at the coarse cells the error looks like the quantisation of the strip's edge to the
mesh (the lines move by merging), not a bias that a different fraction would remove, and no rule
was changed (this is an interpretation: the cause was not isolated by a controlled mesh). The shorted line, whose inductance the rule was fitted on, at the
coarse 0.5 mm cell reads 12 % low with every rule. The unsmoothed meshes of 3b change a lot with a small move of the edges (564 k to
1.31 M cells for the plane case: the grading cascade of the original cutting, which the smooth mesher avoids).

Always run in `cargo test -p copper-fullwave` (58 s for `validation`, 14 s for `shorted_line`, 47 s for `ring_fit_acceptance`, under 1 s for the unit tests, `hotloop_spec` and `zone_fill`; `copper-extract`'s `edge_rule` 40 s; debug build with the optimised solver): 3a at the coarse cell (Z0 within 5 %,
measured +4.1 %), 3a at 0.25 mm (within 1.5 %, measured -0.02 %; it reads -11.7 % without the rule), the terminated line, 3e,
and `default_edge_rule_keeps_a_short_shorted_line_at_the_closed_form` (a 10 mm shorted line at 0.25 mm: +0.126 nH over the closed form,
allowed between +0.03 and +0.25 nH and 6 %; the edges as drawn read -0.187 nH, a third +0.101 nH). 3b-3d stay `#[ignore]` (35 to 190 s each) and were re-run on the final
code: 8.265, 20.907, 10.893 / 11.775 and 5.040 / 4.943 nH, as in the table.

New in iteration 3: `shorted_line_against_the_closed_form` (the ground truth below),
`ring_fit_against_decayed_run`, `fit::tests::a_truncated_ring_down_is_completed` and the unit tests of the edge
rule and of the slab cell counts. Kept from iteration 2: `a_run_that_outlasts_its_wall_clock_budget_is_stopped_with_partial_statistics`,
`strap_port_against_direct_port` (10 x 6 mm loop with a 3 mm gap: a direct port 8.319 nH, the same port at the
middle of straps 8.635 nH, a difference of +0.316 nH against the 0.834 nH closed form of the straps, with the default
rule; iteration 2: +0.559 nH against 0.870 nH), `extend_planes_against_truncated` (8.265, 8.266 and 8.265 nH),
`thread_determinism` (1, 2 and 6 threads: 16,451 steps each, impedance bit-identical, 5.040064 nH),
`termination_study`, `pulse_scale_study` (not re-run; their numbers above are those of iteration 2),
`convergence_order` and the mesher's and spec tests.

**Where the cell dependence of the loop inductance came from.** Iteration 2 left an open problem: the placed
emi_buck loop and a synthetic 6 x 4 mm loop fell at first order with the cell (3 nH per mm on emi_buck), and the
port gap, the pad edges and the bars were all suspects. The ground truth that settled it is a microstrip
line over a plane (er 4.3, zero thickness), shorted by a wall of vertical wires, driven by a lumped port, whose
input inductance is `Z0 tan(beta l) / omega` with Hammerstad-Jensen Z0 and eps_eff
(`tests/shorted_line.rs`; `L` read over 50-150 MHz, the reference evaluated per frequency). A line of 15 to 20 mm
gives 4 to 6 nH, so the line term dominates; one run costs 15 to 150 s. The port and the wall give end effects of a
few hundredths of a nanohenry that do not depend on the cell, and the line itself carries the cell dependence.
The attribution tables were made on the build of the day, before the time loop was split into chunks (decaying runs are
unchanged by that), with the edge rule varied as stated; the check on the final code is the table of the shorted lines further down.

*The port and the wall do not carry it.* Excess of L over the closed form (nH), h 0.5 mm, w 1.0 mm, the
default rule unless stated:

| Varied | Cases | Excess, nH | Cell dependence |
| --- | --- | --- | --- |
| Cell, vertical port of 1 cell, wall on every line, line 15 mm | 0.25 / 0.167 / 0.125 mm | +0.115 / +0.120 / +0.127 (of 4.4 nH) | +0.012 nH over 0.125 mm: opposite sign to the problem, -0.1 nH/mm |
| Same, earlier rule (a third of the lateral cell) | 0.25 / 0.167 / 0.125 mm | +0.079 / +0.094 / +0.096 | also flat |
| (a) vertical port length at 0.25 mm | 1 / 2 / 4 cells (0.25 / 0.5 / 1.0 mm) | +0.115 / +0.087 / +0.041 | -0.074 nH per mm of port: the port region carries less than line |
| (a) in-plane gap port in a line shorted at both ends (8 mm each side) at 0.25 mm | gap 0.25 / 0.5 / 1.0 mm | +0.117 / +0.127 / +0.145 | +0.03 nH over 0.75 mm of gap |
| (a) the same, gap 1.0 mm fixed | 0.25 / 0.167 / 0.125 mm | +0.145 / +0.146 / +0.152 (earlier rule +0.104 / +0.121 / +0.133) | +0.007 nH (earlier rule +0.029 nH) over 0.125 mm |
| (a) the placed loop's gap: 1.713 mm, w 0.827, h 0.533 (earlier rule, 8 mm each side) | 0.25 / 0.167 / 0.125 mm | +0.078 / +0.150 / +0.164 | +0.086 nH over 0.125 mm (-0.7 nH/mm) |
| (b) orientation, 1 cell | vertical / in-plane | +0.115 of 4.41 nH / +0.117 of 4.8 nH | the same |
| (c) wall of wires at every 1st / 2nd / 4th mesh line (0.25 mm cell) | across the 1 mm strip | +0.115 / +0.125 / +0.151 | a sparse fence adds 0.04 nH |
| (d) strip width at 0.25 mm: 2 / 4 / 6 cells | w 0.5 / 1.0 / 1.5 mm, h 0.5 | +0.183 / +0.115 / +0.093 | width in cells does not move the end effect |

Measured fact: no variation of the port gap length (1 to 4 cells, 0.25 to 1.7 mm), of its orientation, of the wall or
of the strip width gives an excess that falls as the cell shrinks. The excess is flat or rises, by 0.01 to 0.03 nH
from 0.25 to 0.125 mm, and by 0.09 nH (-0.7 nH per mm) for the 1.7 mm gap of the placed loop, whose trend is
the opposite of the loop's and partly hides the edge term; the lumped port and the shorting wall are not the origin of the first-order term. (The earlier
estimate that 1.4 nH of the 4.9 nH was the field in the port gap does not hold either: a 1.7 mm gap in a
0.83 mm strip adds about 0.1 nH to the closed form of the line that continues over it.)

*The strip edge does.* The line alone, with every end effect removed (the inductances of lines of 10 and 5 mm
subtracted and compared with the closed form's difference), error in percent:

| Line | Cell | Edge 0 | 0.1 | 0.2 | 1/3 of the cell (earlier rule) | Default rule |
| --- | --- | --- | --- | --- | --- | --- |
| h 0.2, w 0.5 mm (fitted) | 0.25 mm | -16.5 | -12.8 | -7.3 | +4.4 | **+1.8** |
| | 0.167 mm | -11.3 | -8.2 | -4.5 | +1.0 | **-0.3** |
| h 0.5, w 1.0 mm (fitted) | 0.25 mm | -10.2 | -8.1 | -5.6 | -1.3 | **-0.6** |
| | 0.167 mm | -6.9 | -5.4 | -3.8 | -0.8 | **-0.3** |
| h 0.5, w 0.5 mm (not fitted) | 0.25 / 0.167 mm | | | | not measured | **-0.7 / -0.6** |
| h 0.3, w 1.5 mm (not fitted) | 0.25 / 0.167 mm | | | | not measured | **+0.4 / 0.0** |
| h 1.0, w 1.0 mm (not fitted) | 0.25 / 0.167 mm | | | | not measured | **+0.7 / +0.5** |

(Edge columns: the fraction of the lateral cell by which the edges move into the metal, nothing vertically.)
The line's inductance is very sensitive to where the edge is: 2.3 % per hundredth of a millimetre of each edge on
the 0.5 mm strip. The inset that makes the line exact is not a fixed fraction of the cell: it is 0.28 to 0.31 of the
cell for the 0.2 mm dielectric and 0.37 to 0.40 for the 0.5 mm one. Written as `a * (lateral cell) + b * (vertical cell)`,
the four points (two heights, two cells) give a = 0.23 and b = 0.13 of the dielectric cell below the strip (0.11 of
the mean of the cell below and the air cell above it); they were fitted on h 0.2 / w 0.5 and h 0.5 / w 1.0 only. With the
earlier rule the 6 x 4 mm loop's 4.4 % line error at the 0.25 mm cell is the loop's first-order term: its slope of
about 2 nH per mm between 0.25 and 0.167 mm is the line error difference times 20 mm of bars. The rule is the
edge rule above. The reading (an interpretation: the rule was fitted, not derived): a PEC sheet edge on a mesh line has
the dual cell of its tangential field reaching half a lateral and half a vertical cell beyond it, and the singular edge field
loads that cell.

*The shorted line on the final code* (`shorted_line_against_the_closed_form`; 20 mm lines, vertical port of one
cell, wall on every line; the in-plane case is two 10 mm lines with a 2 mm gap port):

| Line | L at 0.25 mm | Against the closed form | At 0.167 mm | Cell dependence |
| --- | --- | --- | --- | --- |
| w/h 2 (h 0.5, w 1.0), the standard case | 6.0128 nH | +1.73 % of 5.9105 nH | 6.0435 nH (+1.88 %) | -0.37 nH/mm |
| w/h 2.5, h 0.2 < the cell | 5.3285 nH | +2.67 % of 5.1899 nH | 5.2377 nH (+0.61 %) | +1.09 nH/mm |
| w/h 1 (not fitted) | 8.5329 nH | +1.93 % of 8.3717 nH | 8.5625 nH (+1.91 %) | -0.36 nH/mm |
| w/h 5 (not fitted) | 3.3154 nH | +1.47 % of 3.2675 nH | 3.3158 nH (+1.17 %) | 0.00 nH/mm |
| in-plane port, 2 mm gap | 6.6794 nH | +2.61 % of 6.5098 nH | 6.6945 nH (+2.56 %) | -0.18 nH/mm |

The standard mesh is within 3 % of the closed form on all five and below 1 nH per mm of cell on four; the
line thinner than a cell (h 0.2 mm in a 0.25 mm cell) is at 1.09. The excess of 1.5 to 2.7 % is the end effect
of the port and of the wall (0.05 to 0.2 nH, of a physical origin: the current turns down a wall of height h), which
the closed form of a bare line does not contain.

**Convergence of the 6 x 4 mm loop** (`convergence_order`; 0.5 mm trace, 0.2 mm over a plane, 0.5 mm gap, smooth
meshes; closed form 4.987 nH):

| Cell | Earlier rule | Default rule | Cells (default) | Steps | Run |
| --- | --- | --- | --- | --- | --- |
| 0.25 mm | 5.073 nH | 4.903 nH | 251 k | 15,251 | 11 s |
| 0.167 mm | 4.925 nH | 4.859 nH | 426 k | 18,151 | 24 s |
| 0.125 mm | 4.847 nH | 4.843 nH | 650 k | 21,451 | 54 s |
| 0.083 mm | 4.748 nH | 4.797 nH | 1.24 M | 31,401 | 137 s |

The default rule moves 2.2 % over the series (the earlier one 6.4 %), with 0.53, 0.38 and 1.08 nH per mm between
consecutive cells (the earlier rule 1.8, 1.9 and 2.4). The continuum limit of this loop is not known better than
4.6 to 4.8 nH: the earlier first-order extrapolation to 4.55 nH was an artefact of the rule.

**Convergence of the placed hot loop** (`examples/emi_buck`, `--fullwave-single-mesh`, the loop window refined at the
cell in the first column, general cell 0.5 mm; one run each, 6 threads; the closed form is 4.973 nH). The hot-loop
code of the day was run with a temporary override of the refinement cell, removed afterwards; the rows
0.25 and 0.167 are also what the default two-mesh run gives. Earlier rule: 4.920 (0.25), 4.695 (0.167),
4.736 (0.20), 4.751 (0.15), 4.482 nH (0.10).

| Refinement cell | Cells | Steps | Run | L, default rule | Smallest cell |
| --- | --- | --- | --- | --- | --- |
| 0.25 mm (standard mesh) | 320 k | 12,801 | 12 s | 4.841 nH | 0.115 mm |
| 0.20 mm | 346 k | 12,851 | 13 s | 4.803 nH | |
| 0.167 mm (refined mesh) | 422 k | 15,901 | 23 s | 4.813 nH | 0.090 mm |
| 0.15 mm | 488 k | 16,751 | 28 s | 4.783 nH | 0.091 mm |
| 0.133 mm | 552 k | 31,951 | 66 s | 4.759 nH | 0.043 mm |
| 0.125 mm | 599 k | 39,251 | 94 s | 4.662 nH | |
| 0.10 mm | 887 k | 46,251 | 198 s | 4.618 nH | |

Measured: between 0.25 and 0.15 mm the curve is flat to 1.2 % (0.6 nH per mm, earlier rule 2.7), and the 0.25 mm
value is 4.8 % above the 0.10 mm one (earlier rule 9.8 %). Below 0.15 mm L steps down, by 2.5 % to 0.125 mm and 3.5 % to 0.10 mm, at meshes whose
number of steps rises 2 to 3 times because cells of 0.04 to 0.06 mm appear. With the cell floor of iteration 4 (above) the
same two runs read 4.741 nH at 0.133 mm (542,800 cells, 27,201 steps, 54 s, smallest cell 0.057 mm; before: 4.759 nH, 31,951 steps, 66 s)
and 4.621 nH at 0.10 mm (874,720 cells, 36,601 steps, 153 s, smallest 0.041 mm; before: 4.618 nH, 46,251 steps, 198 s): L within 0.4 % and
0.1 %, 15 % and 21 % fewer steps; the step count still rises with the refinement because the cells themselves are smaller. Variations of the
mesh decisions (merge distance of 0.4, 0.2 and 0.1 of the cell: 4.841 / 4.904 / 4.869 nH at 0.25 mm and
4.817 / 4.718 / 4.718 nH at 0.167 mm; one uniform cell and no refinement box with the earlier rule: 4.904, 4.843 and 4.746 nH at
0.25, 0.167 and 0.125 mm; a refinement margin of 1.5 mm instead of 0.3 mm: 4.903 and 4.695 nH with the earlier
rule) move L by 1 to 4 %, as much as the cell does. Interpretation: what remains on emi_buck is not
discretisation of smooth copper but small geometry (legs crossing foreign pads, clearances of 0.05 to 0.1 mm) that
a finer cell resolves differently and that the merging of lines within 0.4 of the cell moves by up to 0.057 mm;
the curve cannot be extrapolated, which is why the field `inductance_extrapolated_nh` is gone and a
mesh sensitivity with a floor replaced it (below).

**Hot loops** (`copper emi-check --fullwave`, default two meshes; the reported L is the finer mesh's, read at
50-150 MHz; every row of the first two made on the final code):

| Board | Meshes (cell in the refinement zones) | Cells | Steps (expected) | Run | L | Mesh difference; mesh sensitivity | Closed form | Ratio |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `examples/emi_buck` placed (straight legs) | standard 0.25 mm: 4.841 nH; refined 0.167 mm: 4.813 nH | 320 k; 422 k | 12,801 (15,009); 15,901 (18,685) | 12 s + 23 s | 4.813 nH, flat to 0.06 % | 0.6 %; 4 % | 4.973 nH | 0.97 |
| `emi_buck` routed (three 0.4 mm legs, `write_routed_run`) | standard 0.2 mm: 6.067 nH; refined 0.133 mm: 5.974 nH | 720 k; 1.02 M | 15,101 (17,711); 19,951 (23,473) | 59 s + 79 s | 5.974 nH, flat to 0.07 % | 1.55 %; 4 % | 6.712 nH | 0.89 |
| `examples/tps62130a_macro`, `--fab-profile jlcpcb-six-layer` (synchronous; strap port in the air), final code | standard 0.15 mm: 2.823 nH; refined 0.1 mm: 2.830 nH | 1.77 M; 2.63 M | 28,601 (32,983); 40,601 (46,822) | 185 s + 334 s | 2.830 nH, of which 0.948 nH is the strap | 0.27 %; 4 % | 1.69 nH | 1.67 (1.11 without the strap bias) |
| CopperOrgan routed board (default budget, `--fullwave` only; asynchronous, air-bridge port), final code | standard 0.15 mm, one mesh (the second did not fit the budget) | 5.19 M | 114,982 (38,328) = the 3x cap | 1879 s | 7.289 nH band mean, static 6.462 nH, **flat only to 51.7 %**, energy not converged, the ring fit did not stabilise | none (`single_mesh`) | 9.889 nH | 0.74 |

The TPS62130A row of the iteration before (2.817 / 2.832 nH, 1.78 M and 2.78 M cells, 39,101 and 50,201 steps, 234 s + 412 s) was on
the mesh of iteration 2; on the code of iteration 3 (before the cell floor) one mesh of 0.15 mm was 2.371 M cells, 45,751 steps, 397 s, 2.865 nH.

On the earlier rule the placed emi_buck read 4.920 / 4.695 nH (mesh error 4.78 %, extrapolation 4.245 nH) and the routed
one 6.049 / 5.969 nH (1.3 %); the standard-mesh values moved -1.6 % and +0.3 % and the finer-mesh values +2.5 % and +0.1 %. The mesh
sensitivity of the reported value is now 4 % on the three boards whose records decayed (the floor, see the
convergence estimate; the CopperOrgan row has none, being indicative). The TPS62130A and CopperOrgan rows are on the final code (the two meshes of the TPS62130A,
one mesh of CopperOrgan, below).

**The TPS62130A** loads (87 s) with its hard macro: 6 copper layers (an assumed stack-up, flag
`assumed_stackup`, the board declares none), 21 pads, 27 tracks, 2 polygons, 3 zones, 11 vias; its
four-pad loop gets a strap port in the air, 1.95 mm of 0.2 mm wide bar 0.3 mm above the top layer
from the VIN pad (B/U.12) to the nearest switch pad (B/U.1), because the exposed pad lies between them,
the low-side switch is a hull bridge from that pad to the hot return pad (B/U.15). The result of iteration 2, 2.83 nH,
is 1.14 nH over the 1.69 nH of the closed form, and 1.88 nH (1.11 times the closed form) without the strap
bias: the closed form is the pad-centre polygon with the loop closed through the component bodies, the
model closes it through a 0.95 nH air bridge (closed form) and a hull of two pads. Without the pour cuts (the first run, 1.59 nH) the
pours short the loop (flag `zone_overlaps_foreign_copper`): 1.59 nH is a short circuit, not an inductance.
Mesh (final code): 2.63 M cells for the refined mesh, 554 s for both (the log of the run with both meshes), inside the 30 minute budget. The
macro's copper has gaps down to 0.06 mm; before the cell floor of iteration 4 the grading around them cut cells of 0.030 to 0.040 mm and set
dt = 0.075 ps for the first mesh (45,751 steps, 397 s); now the smallest cell is 0.060 / 0.055 mm (the 0.06 mm gaps) and dt is about 0.13 ps (28,601 steps, 185 s).
The result moved from 2.865 to 2.823 nH (-1.5 %) on the first mesh.

**CopperOrgan** (`board.copper --run build/ci/emi-check-20261111T161807`; the closed form is 9.889 nH, a
54.4 mm path with 2 vias, a 0.20 mm neck and a plane shortcut; D_BUCK is in the loop). Iteration 2, at the default
budget (1800 s) the model is refused before the solver starts: 6.26 M cells x 50,513 steps = 316 G
cell updates, 2108 s at the assumed 150 M/s; the cells are dominated by the 0.15 mm cell the 0.2 mm
neck forces over the fine zone of a 36 x 19 mm window (the margin cannot go below ten dielectric heights,
5.33 mm), a 0.04 mm smallest cell sets dt. With `--fullwave-budget 3600` it ran 2801 s at 6.26 M cells x
151,539 steps (the measured rate is 340 M cell updates per second). **That run did not converge.** The loop
is the lossless PEC loop with the capacitor and diode bridged, and rings: first parallel resonance
348 MHz, the energy still above -35 dB when the 3x time cap ended the run (`energy_not_converged`); L(f)
rises across the reading band by 18.6 %, which is what a resonance at 348 MHz predicts (the factor
1/(1 - (f/348 MHz)^2) averages 1.10 over 50-150 MHz and spans 1.02 to 1.23, 18.8 %, against 18.6 % measured),
so the band mean 7.317 nH is 1.10 x the static inductance, about 6.6 nH. Of that 1.76 nH is the air bridge between the VIN pad (U_BUCK.2) and the SW pad
(U_BUCK.8) on the other side of the exposed pad, a bias the model includes: the loop without it is about
4.9 nH (static) to 5.6 nH (band mean), 50 to 57 % of the closed form. The direction is the one the
description of the closed form gives: it models the plane-shortcut leg as a 1-5 mm wide bar and ignores the
via-to-bar mutuals, and the model sees the real pour outlines (wide pours carry the current with less
inductance); but this reading is not isolated: the stack-up is assumed (flag `assumed_stackup`, so the height of
the planes under the loop is a guess), the record is not decayed, the strap bias is a closed-form estimate of an
arbitrary geometry (4.4 mm bar, 0.3 mm high), the pours are outlines minus clearances, not fills (thermal
reliefs, minimum widths), and 6 pairs of neighbouring cells exceed a ratio of 1.5 (up to 2.0, in the window
at the crowded corners and in the air). Treat 4.9-5.6 nH as the model's number and 9.9 nH as the
closed form's, not as a verdict on either. **Iteration 4 run** (final code, `emi-check board.copper --locked --run build/ci/emi-check-20261111T161807 --fullwave --json` from the
CopperOrgan checkout, nothing written; default budget). The cell floor takes the mesh from 6.26 M cells (smallest cell 0.040 mm, dt 0.075 ps)
to 5.19 M cells (0.052 / 0.056 mm, dt 0.124 ps) and the expected steps from 50,513 to 38,328. The run took 1879 s (31 minutes; the estimate
made from the mesh before the run had been 24.7) at 5.19 M cells x 114,982 steps, which is the 3x time cap, 14.2 ns, not a decayed record and not a stop of the
ring fit: the flags are `energy_not_converged`, `inductance_not_flat`, `band_near_resonance`, `single_mesh` and no `ring_fitted`. Result: **L 7.289 nH over 50-150 MHz, flat
only to 51.7 %** (the first resonance is now 314 MHz, was 348 MHz on the mesh of iteration 2, so a 10 % change of the
resonance from a change of the mesh), static inductance 6.462 nH (resonance factor removed; the earlier estimate
was about 6.6 nH from 7.317 nH), flatness of the static curve 46.5 %, 5.558 nH at 500 MHz, port strap 1.784 nH (closed form), ratio to the
closed form 0.74, no mesh sensitivity stated (the record is undecayed and the fit did not stabilise: status `indicative`; the earlier output reported the 4 % floor here). The band mean moved -0.4 %
against iteration 2's 7.317 nH and the static value -2 %, which is inside what the
mesh moves L on every other board, but a curve that is not flat to 51.7 % against a resonance factor that explains 27 % is not a
loop inductance: the fit did not accept the record (its orders disagree by more than 5 %), the cause was not diagnosed (a loop with several close modes is the case the fit was
not validated on). Treat 6.5-7.3 nH as the model's range and the record as undecayed, not as a result.
What the fit saw at each of its checks is not in the output; adding it (orders, spread, ring frequency and Q) is the first thing to
do before another 30 minute run. Of the 7.289 nH, 1.78 nH is the air bridge, as before, and the same caveats apply.
The first attempts of iteration 2 show what the checks catch: with the strap in the plane the port box sat on the
exposed pad and the solver ignored the port (29 minutes lost before the check for ports without usable edges was added to `solve_port`); with the
pours uncut the result was 1.76 nH, the strap alone, a short circuit.

**Ring-down fit** (`fit.rs`, `RegionSpec::ring_fit`, on by default). The time loop runs in chunks of one
energy-check interval with the solver's own schedule, so a loop that decays stops at the same step as before
(the decaying cases above reproduce their step counts exactly). While the energy has not fallen to the threshold, from 2.5 ns after the
pulse and every 1.5 ns, the voltage and current records so far are fitted: after the pulse the port is a free oscillation,
`x(t) = sum c_k exp(s_k t)`, so the tail of the current (from 0.3 ns after the pulse), block-averaged to about 240
samples, is fitted by linear prediction (Householder QR with a ridge, even orders 2 to 16; roots of the predictor by
Durand-Kerner, those outside the unit circle reflected into it), the amplitudes of the current and of the voltage
are found by complex least squares on the same poles, and the transform of the unrecorded future is added to the
transform of the record as a geometric series per pole. Z is the ratio. The inductance is the median over the orders and their spread is
reported. The run stops when, at two successive checks, the orders agree to 1 %, L has changed by less than 0.3 %
and the tail covers two periods of the lowest ring pole carrying amplitude (at least 2 ns); a run that reaches the
time cap undecayed is completed by the same fit when the orders agree to 5 %. The result carries the flag
`ring_fitted` and a note with the ring's frequency and Q, in place of `energy_not_converged`. The completed
spectra are the single source of Z and S: the voltage and current of every port are completed (the passive ports'
tails on the poles of the excited port's fit), so `S11 = (Z - Z0)/(Z + Z0)` of the returned Z, and `solve_all` and
Touchstone carry the completed responses. A passive port whose tail those poles do not describe (relative residual
above 0.2) keeps the plain transform of its truncated record; it is listed in `PortColumn::uncorrected_ports` and in
the run's warnings. Validation (`ring_fit_against_decayed_run`): a 10 x 6 mm loop 0.2 mm over a plane with
two 10 x 20 mm pads on both sides of a 1 mm gap, port across the gap; the pads are about 19 pF across the port by area and resonate
with the loop at 443 MHz. The Q of the ring is set by the port's series resistance (Z = V/I does not depend on the
port's reference impedance): 50 ohm gives a Q of 2.7 that decays in 11 ns (7.198 nH over 50-150 MHz), 500 ohm gives a Q of 27 that takes
299,501 steps and 86 ns to reach -35 dB (7.175 nH, 536 s). The record of the Q 27 run, cut at 5, 6, 8, 10, 12, 16, 24 and 40
ns and read plainly, gives L between -111 % and +112 % from the decayed run's (+1.0 % at 40 ns); the same cuts completed by the fit give
+0.16 to +0.17 % at every cut, 5 ns included (a tail of 1.1 ns, half a period), and the unit test of the fit
gives the transform of a truncated damped ring to 1 %. A live run with the fit stopped at 10.6 ns after 36,950 steps (the decayed run:
299,501) in 59 s (536 s), 8.1 times fewer steps, and read 7.187 nH, +0.17 % from the decayed run. Weak spots: the
validation ring is one dominant mode; a loop with several close modes
(CopperOrgan has a resonance at 348 MHz and its overtones) gets the orders' spread as the only warning, and a
ring far below the reading band's lowest frequency is not tested.

**The edge rule on other geometries (iteration 5).** The rule (0.23 of the lateral cell and 0.11 of the vertical cell) was fitted on two microstrip lines and checked on three; the review asked for a
validation on geometries not used to fit it, with a reference that does not come from the same solver. `crates/copper-extract/tests/edge_rule.rs` runs the default rule, without refitting, against
`copper-extract` (quasi-static, copper of 5 um read at 100 MHz, the full wave's copper being zero-thickness PEC; on the 12 mm shorted line below it is within 0.4 % of Hammerstad-Jensen at both pitches: 2.9996 and 2.9762 nH against 2.9873) and, where one exists, a closed form. Every case is a line of h 0.3 mm and w 0.8 mm (w/h 2.7: not one of the fitted pairs), driven by a vertical port at one end
and shorted by a wall of wires at the other, as the validation lines are; the straight line of the same run is the control, which carries what the port and the wall add. L is the mean of `Im Z / omega` over 50-150 MHz;
the extraction is read at two pitches of its fine mesh to show its own mesh dependence.

| Case | Reference | Full wave, cell 0.25 / 0.167 mm | Extraction, pitch 0.25 / 0.167 mm | Full wave against the extraction (finest) | Against the closed form |
| --- | --- | --- | --- | --- | --- |
| Straight 12 mm (control) | Hammerstad-Jensen, zero thickness: 2.987 nH | 3.0230 / 3.0435 nH (314 k / 581 k cells, 17 / 77 s) | 2.9996 / 2.9762 nH | +1.57 / +2.26 % | +1.2 % at 0.25 mm |
| Right-angle bend, 6 + 6 mm, corner filled | extraction | 2.8814 / 2.8986 nH (337 k / 629 k cells, 20 / 86 s) | 2.8541 / 2.8353 nH | +1.63 / +2.23 % | none |
| Corner excess (bend minus control) | extraction | -0.1416 / -0.1449 nH | -0.1455 / -0.1409 nH | within 0.004 nH | none |
| Loop 8 x 5 mm, trace 0.6 mm, 0.3 mm over a plane, 1 mm gap, in-plane port | loop over the image plane: 7.0705 nH | 7.0675 / 7.1090 nH (305 k / 603 k cells, 13 / 52 s) | 6.9891 / 6.9628 nH | +1.5 / +2.1 % | -0.04 / +0.54 % |
| Straight 12 mm with a 4 mm x 2 mm slot under its middle (plane 12 mm wide) | extraction | 3.9338 / 3.9312 nH (307 k / 579 k cells) | 4.0617 / 4.0617 nH | -3.15 / -3.21 % | none |
| Slot excess (slotted minus control) | extraction | +0.9108 / +0.8877 nH | +1.0621 nH | **-14.2 / -16.4 %** (-0.15 / -0.17 nH) | none |
| Via transition: F.Cu line over In1 (0.2 mm) to B.Cu through a via past two planes (0.4 mm between), B.Cu line over In2 (0.2 mm), four GND vias joining the planes, wall to In2 | extraction, the via tube at 0.23 of the cell outer radius | 2.9548 / 2.9154 nH (416 k / 707 k cells, 29 / 84 s) | 2.8302 / 2.9015 nH at pitch 250 um, 2.8142 / 2.8855 nH at 167 um (the tube follows the cell) | +5.0 / +1.0 % against the 167 um pitch (+4.4 / +0.5 % against 250 um) | none |

What these say, separately. **Bend and loop: the rule holds as well as it does on the straight line.** The full wave stands 1.6 to 2.3 % above the extraction on the straight control, 1.6 to 2.2 % on the bend and
1.5 to 2.1 % on the loop, so what a corner or a loop adds is as the extraction has it to within a few thousandths of a nanohenry (the corner excess, -0.14 nH in both, is the notch of the outer corner of a path drawn square);
the loop is within 0.6 % of its closed form at both cells, whose full-wave values rise 0.6 % over the cell step (the extraction falls 0.4 %: the two references move in opposite directions by about the offset). The offset of 1.6 to 2.3 % between the tools is
already there on the control; it is of the size of the end effect of the port and the wall (0.05 to 0.2 nH, above), of the extraction's thin copper, and of the cell dependence of the full wave (+0.02 nH from the 0.25 to the 0.167 mm cell on the straight line, +0.017 on the bend). **Slotted plane: the rule fails here, in the way it was not meant to cover.**
The slot's excess inductance is 14 % (0.25 mm) and 16 % (0.167 mm) below the extraction's, and the gap widens as the cell shrinks. The rule is not applied to zone edges (`push_zone_sheet` does not move rings; the slot is a hole in the plane's pour), so the slot reads narrower than drawn
and diverts the return current less: the direction (low) is the one the rule's own reading predicts for an edge that is not moved. This was **not refitted and the rule was not extended to zones**: whether moving plane edges by the same
fractions closes the gap was not tried, and it would be a fit to this case. The omission is stated in the result's `not_modelled` ("the edge rule is not applied to plane and pour edges") whenever a pour is in the model.
**Via transition: not conclusive.** The full wave stands 4.4 to 5.0 % above the extraction at the 0.25 mm cell and 0.5 to 1.0 % at 0.167 mm, but a via is a one-cell wire in the full wave and a tube in the extraction, so the reference has a
parameter that the board does not fix: the tube's outer radius was set to 0.23 of the cell by reasoning (the effective radius of a one-cell PEC wire on a Yee mesh), not measured, and it moves with the cell, which is why the extraction's value rises from 2.83 to 2.90 nH as the cell
shrinks. A via-only control, or a measurement of how L depends on the tube radius at fixed copper, would be needed to say how much of the 4.4 % is the vertical edge rule and how much the radius equivalence; it was attempted in the study (`study_via_transition` prints radius variants) but the variants did not print and were not run. The case therefore
does not validate or invalidate the rule: it shows the full wave and the extraction within 5 % on a transition at one cell and within about 1 % at the next.

Limits of this validation: one geometry per case, two cells, a reference with its own 1 to 2 % offset and mesh dependence (0.8 to 0.9 % from pitch 0.25 to 0.167 mm on the lines), no curved or slanted edges, no cell below 0.167 mm, no loop on a dielectric
other than er 4.3, and the straight control itself carries +1.6 to +2.3 % over the extraction, so a rule error below that is not resolved. Tests: `the_default_edge_rule_keeps_a_right_angle_bend_at_the_extraction_in_excess_of_the_straight_line` (default, 40 s: control and bend at 0.25 mm,
asserts each within 3 % of the extraction, the control within 3 % of the closed form and the two corner excesses within 0.05 nH), and the ignored `study_bend_and_slot_series` (7 min), `study_loop_over_a_plane` (1.6 min) and `study_via_transition` (2.5 min), run from `cargo test -p copper-extract --test edge_rule -- --ignored NAME --nocapture`.

**The strap bias, measured (iteration 5).** `tests/strap_bias.rs` (ignored, 3 min). A shorted microstrip line (12 mm, 1.0 mm wide over 0.5 mm, and 0.6 mm over 0.3 mm; wall of wires at its far end) has a pad `A` at its start and a grounded pad `B` (wall of wires to the plane) at a distance `d`
on its axis; the port is across `A` and `B` and carries a strap port (`with_strap`: bars 0.42 to 0.45 mm wide from the pad edges to a gap of two cells in the middle). The loop is the line, the strap and `B`'s wall, so `L(d)` is the same
line plus the strap that a strap port adds: the increase with `d` is the strap's own inductance, measured, and the extrapolation to `d = 0` is the same structure without a strap. Against it, `port_strap_nh`, the closed form of the strap bars over the plane
(or in the air at 0.3 mm over a foreign pad, on posts), at the same `d` (cell 0.25 mm; L in nH):

| Strap | d, mm | L | Closed-form strap | L(d) fit: `L(0)` + slope x d | Closed form: slope | Slope measured / closed form | Strap increase over L(0) at the largest d / closed form |
| --- | --- | --- | --- | --- | --- | --- | --- |
| In-plane, line h 0.5 / w 1.0 | 1.5, 2.5, 3.5 | 4.628, 5.084, 5.541 | 0.517, 0.949, 1.395 | 3.944 + 0.456 d | 0.439 nH/mm | 1.039 | 1.597 / 1.395 nH (+14 %) |
| In-plane, line h 0.3 / w 0.6 | 1.5, 2.5, 3.5 | 4.528, 4.897, 5.267 | 0.425, 0.765, 1.110 | 3.973 + 0.370 d | 0.343 nH/mm | 1.079 | 1.294 / 1.110 nH (+17 %) |
| In the air on posts, line h 0.5 / w 1.0, foreign pad between | 2.5, 3.5, 4.5 | 5.671, 6.198, 6.723 | 1.081, 1.608, 2.145 | 4.355 + 0.526 d | 0.532 nH/mm | 0.990 | 2.368 / 2.145 nH (+10 %) |

(The line alone is 3.580 nH by Hammerstad-Jensen for both in-plane lines, which have the same w/h; the extrapolated `L(0)` is 0.36, 0.39 and 0.77 nH above it, the end effects of the port, `B`'s wall and, in the air, the posts. Three points per series and a straight line through them: a
sketch of a slope, not a fit with an error bar.) **What was measured:** the closed form has the right growth with the strap's length, within 4 %, 8 % and 1 % of the measured slope on three geometries including the in-air construction that the TPS62130A and CopperOrgan loops use, and it
understates the strap's contribution at the lengths of a real strap (1.5 to 4.5 mm) by 0.2 nH (10 to 17 %), a constant that is the strap port's own junction and port gap rather than a bar's length. `strap_port_against_direct_port` of iteration 2 (+0.316 nH at a 3 mm gap against 0.834 nH) did not measure the strap
and this does. **What was decided: no correction is subtracted.** The 0.2 nH offset would be a correction derived from these three series, and the instruction was that none stands without a validation case that was not used to derive it; no fourth geometry (a strap that turns a corner, the TPS62130A's own, a strap of another width over another height) was
measured, and three series that differ by 0.18 to 0.22 nH are too few to tell a constant from a dependence on the geometry (the closed form's intercept is -0.14 to -0.25 nH where a bar of zero length would have none). The strap bias stays **uncorrected and named**: `port_strap_nh` is reported beside the inductance, as before, and its meaning is now stated by measurement: it is the strap's
growth with length to within about 10 %, and the strap contribution it names understates the measured one by 0.2 nH on three geometries. It was measured on 1 mm lines at a 0.25 mm cell, 12 mm long, not on the 0.2 to 0.45 mm
straps of the TPS62130A at its 0.15 mm cell, and the real package's current path through the die and bond wires is not what any strap represents.

**Acceptance of the ring-down fit (iteration 5).** Until iteration 4 a fit was accepted when its model orders agreed to
5 %: that says the orders are consistent with each other and nothing about the part of the record that the fit did not see.
A fit is now used only when all four checks below pass (`fit::check_fit`, `FitCheck`; the evidence is
`measures.fullwave.ring_fit`):

1. *Withheld tail.* The fit is made on the first 70 % of the tail and predicts the voltage and the current of the last
   30 % (`HOLDOUT_FRACTION`); the relative rms error of each over the withheld samples (`|predicted - recorded| / |recorded|`)
   must not exceed `HOLDOUT_ERROR_LIMIT` = 10 %, and the withheld part must cover at least one period of the lowest ring pole of
   the first-part fit (1 ns where it has none): a prediction over less says little about a ring.
2. *The orders agree* to 5 % (`SPREAD_LIMIT`, as before).
3. *The poles decay.* `poles_of` reflects every root outside the unit circle into it, which turns a growing fit into a
   decaying one without a word. The growth of each pole that carries amplitude (at least 1e-3 of the largest) over the fitted tail is now
   computed before the reflection, and a pole that would have grown by more than 5 % (`GROWTH_LIMIT`) is an unstable pole
   (`ring_fit.unstable_poles`, `max_pole_growth`).
4. *Passive at the port.* `Re Z / |Z|` of the completed response over the whole analysis grid (30 MHz to 1 GHz, 181 points) must not
   fall below -0.05 (`PASSIVITY_LIMIT`). The model is lossless PEC, so `Re Z` is small and of either sign by numerical noise
   (it reads -0.000 of `|Z|` on every FDTD record below) and only a clearly negative resistance is a violation.

A fit that fails is not used: the spectra are the plain transform of the truncated record, `status` is `indicative`,
`mesh_sensitivity_pct` is `null`, the flags are `energy_not_converged` and `ring_fit_rejected`, and `status_reasons`, the
notes and `ring_fit.violations` give every failed check with its numbers. The inductance stays in `inductance_nh` as a
marked number (status `indicative`); it is not withheld, because the report's ratios and the cross-check below read it. The run
no longer ends on a fit the checks would reject: the early stop (orders within 1 %, L moving by less than 0.3 % over two checks, a
tail of two ring periods) also needs the checks to pass, so a run that ends by the fit has a tail of at least 3.3 ring
periods, and a run whose fit is never acceptable goes on to the 3x time cap and is then declared indicative.

*What the limit rests on.* Two measurements, both in the tests. (a) Records whose exact response is known
(`fit::tests::the_withheld_tail_error_bounds_the_error_of_the_completion`: a passive 5 nH + 50 ohm port, a ring of 400 MHz with Q 25, cut at
10 ns, and a second ring of 233 MHz that starts at 7.5 ns, inside the withheld part, with growing amplitude; L over 50-150 MHz and
the largest relative error of the complex response over 25 MHz to 1 GHz of the completed fit against the exact one):

| Second ring, A (against 1 mA of the first) | Withheld-tail error | Completion: L error | Completion: response, largest | Orders' spread | Check |
| --- | --- | --- | --- | --- | --- |
| 1e-6 | 0.14 % | +0.12 % | 0.50 % | 0.11 % | accepted |
| 1e-5 | 1.39 % | +0.09 % | 0.57 % | 0.14 % | accepted |
| 1e-4 | 13.3 % | +0.00 % | 1.74 % | 0.09 % | rejected |
| 3e-4 | 35.2 % | +3.67 % | 4.22 % | 6.8 % | rejected |
| 1e-3 | 72.3 % | +23.7 % | 33.0 % | 36.5 % | rejected |
| 3e-3 | 91.7 % | +53.4 % | 77.3 % | 7.2 % | rejected |

The completion's response error is 0.5 to 0.6 % (the floor of this setup) up to a withheld-tail error of 1.4 %, then about an eighth of it (1.7 % at 13 %, 4.2 % at 35 %), until a mode that the fit
cannot see dominates (33 % at 72 %); L is wrong by more than 3 % first at a withheld-tail error of 35 %. A limit of 10 % sits between the last accepted row (1.4 %, response within 0.6 %) and the first rejected
one (13 %, response within 1.7 %). It is stricter than L needs (the 13 % row has an exact L), because the withheld error is the only warning of a mode that the fit did not see: the orders' spread stays under 0.15 % up to
that row and reaches 6.8 % only at 35 %. (b) FDTD
records (`tests/ring_fit_acceptance.rs`, a ringing loop of two 10 x 20 mm pads on both sides of a 1 mm gap in a 10 x 6 mm loop of 0.5 mm trace 0.2 mm over a plane,
the port across the gap; the port's resistance sets the Q of the ring): on every record cut at 5 to 40 ns
the withheld-tail error is 0.00 to 0.02 %, the orders agree to 0.03 % or better, no pole grows and `Re Z / |Z|` is -0.000, so the FDTD cases do not exercise the
limit; they are one dominant mode. **No FDTD record in the validation set has a withheld-tail error between 0.02 % and 10 %.** The limit
is therefore justified by (a) alone, on one family of perturbations (a late second mode), and by nothing measured on a real loop with several close modes.

*Agreement with a longer run.* `an_accepted_fit_agrees_with_the_long_run_over_the_whole_band` (default test, 47 s): the loop above on a
1.2 mm mesh (121 k cells, an unresolved geometry: the test is of the fit and says nothing of the loop's inductance), port 50 ohm (Q about 2.7), run to -70 dB of field energy (35.6 ns,
112,151 steps) as the long run, and the same record cut at 5 to 16 ns. The agreement is the complex response over the whole band
(50 MHz to 1 GHz, 181 points), `|Z_fit - Z_long| / |Z_long|` per frequency, its largest value and its rms:

| Cut | Held out | Plain transform: L against the long run | Plain: response, largest / rms | Fitted: L | Fitted: response, largest / rms | Check |
| --- | --- | --- | --- | --- | --- | --- |
| 5 ns | 0.3 ns | +39.6 % | 2.96 / 0.97 | +0.01 % | < 0.1 % / < 0.05 % | rejected (tail shorter than a ring period) |
| 6 ns | 0.6 ns | -18.2 % | 2.22 / 0.62 | +0.01 % | < 0.1 % / < 0.05 % | rejected (same) |
| 8 ns | 1.2 ns | +2.1 % | 0.92 / 0.27 | 0.00 % | < 0.1 % / < 0.05 % | rejected (same) |
| 10 ns | 1.8 ns | +2.0 % | 0.74 / 0.13 | 0.00 % | < 0.1 % / < 0.05 % | rejected (same) |
| 12 ns | 2.4 ns | +0.6 % | 0.45 / 0.064 | 0.00 % | < 0.1 % / < 0.05 % | accepted |
| 14 ns | 3.0 ns | +0.4 % | 0.23 / 0.031 | 0.00 % | < 0.1 % / < 0.05 % | accepted |
| 16 ns | 3.6 ns | +0.2 % | 0.11 / 0.015 | 0.00 % | < 0.1 % / < 0.05 % | accepted |

(The fitted response column is printed to three decimals by the test: 0.001 at most.) The test asserts, for every accepted cut, a largest response difference
under 3 % and an rms under 1 % and an L within 1 %; the assertion bounds are loose against what was measured. The same on the finer 0.8 mm mesh with port 150 ohm (Q about 8), run to -70 dB (58.8 ns, 188,051 steps, 98 s;
`study_reference_quality_and_where_the_response_disagrees`, ignored): the fit of the record cut at 10, 16 and 24 ns agrees with the long run to
0.59 / 0.59 / 0.58 % at the worst frequency (always the top of the band, 1 GHz) and 0.15 % rms. **The same run read at the `-35 dB` criterion of `solve_port` (30.1 ns) differs from the
-70 dB record by 26.7 % at the worst frequency and 7.2 % rms**: the criterion that decides that a record has decayed leaves an error of that size in the complex
response of a resonant loop (the effect on L over 50-150 MHz was not measured separately: the fits of `study_cuts_of_decayed_runs_against_the_decayed_run` read within 0.2 % of the
-35 dB record's L at port 50, 150 and 300 ohm, and that record is the reference of every earlier measurement of this section). A response read from a record that stopped at -35 dB is
not shown to be good to better than that, which the S-parameters of the Touchstone output inherit.

Tests: `fit::tests::{a_clean_ring_is_accepted_with_a_small_withheld_tail_error_and_a_passive_response,
a_mode_that_the_fitted_part_does_not_contain_fails_the_withheld_tail_test, a_growing_pole_is_counted_and_rejected,
a_negative_resistance_is_a_passivity_violation, a_tail_too_short_to_withhold_part_of_it_is_rejected_not_accepted_unchecked,
a_rejected_fit_leaves_the_plain_transform_and_says_why, the_withheld_tail_error_bounds_the_error_of_the_completion}`
(synthetic records with a known exact response: 1 s in all), `ring_fit_acceptance::an_accepted_fit_agrees_with_the_long_run_over_the_whole_band` (47 s), the
ignored studies `study_cuts_of_decayed_runs_against_the_decayed_run` (3 min) and `study_reference_quality_and_where_the_response_disagrees` (1.6 min), and
`emi_fullwave::tests::a_rejected_ring_down_fit_is_reported_with_its_evidence_and_its_violations` for the report. Weak spots: the
validated records have one dominant ring; a loop with several close modes (the CopperOrgan loop) is exactly the case that was not measured, and the
limit was set from a synthetic family of perturbations, not from FDTD records that fail; the early stop of a live run
now needs a longer tail, which was not re-measured on the 536 s case of iteration 3 (the ignored `ring_fit_against_decayed_run` and `a_fit_completed_run...` of `shorted_line.rs` were not re-run).

**Zone fills (iteration 5).** `copper_fullwave::fill` holds the code that `copper-extract` used alone: the board is exported in memory
(`copper_pcb::kicad_pcb::generate`), parsed with `copper_cam::board::parse_board`, and its zones are filled by `copper_cam::fill::fill_zones` under the rules
of the exported project (clearances, thermal reliefs, openings for the vias and pads of other nets, minimum width, removal of islands as each zone asks);
`copper-extract::board::board_model` now calls it (`BoardModel` is `FilledBoard`), so the two tiers see the same pours.
`RegionSpec::zone_fills` carries the result (`ZoneFillSet`: net, layer, nanometre polygons): where a net has a fill on a layer, the fill replaces the zone's outline
there (clipped to the window, and cut with the zone clearance around the copper that only the model has, the hull of a capacitor or diode and the legs of an
unrouted loop; flag `zone_fill_used`); a zone with no fill on a layer keeps the outline minus clearances (`zone_outline_used`). `extract_hot_loop` makes the fill once for both meshes
(`HotLoopOptions::native_fills`, default on; `--fullwave-zone-outline` switches it off) and falls back to outlines with a note when the export fails. The fill is cut to the model's copper but is not
re-made around it. Like every pour in the model, its edges are not moved by the edge rule.

*What it changed on the examples.* Nothing on the shipped examples, as measured: `examples/emi_buck` placed (standard mesh, `--fullwave-single-mesh`) reads 4.841 nH with the fill (flag
`zone_fill_used`) and 4.841 nH with the outlines, with the same 320,160 cells and 12,801 steps; the routed example (the three 0.4 mm legs of the CLI test, 720,000 cells, 15,101 steps)
reads 6.067 nH both ways. The only pour of the example is a floating ground on In1.Cu that nothing crosses, so the fill is the outline, which is what the identical meshes say.
`examples/tps62130a_macro` (the board whose pours are cut by 11 vias and a package): the native fill **cannot be made** there, the export refuses a hard macro without its
immutable copper (`cannot export a hard macro without its immutable copper`), so the run falls back to outlines minus clearances (note and flag `zone_outline_used`) and reproduces 2.823 nH, 28,601 steps,
as before. `--extract` calls the same export, so by the code it meets the same refusal on such a board (not run here). The difference that a fill makes is shown by the synthetic board of `tests/zone_fill.rs`
(`the_native_fill_replaces_the_outline_and_drops_the_island_the_outline_keeps`): a 20 x 10 mm pour on F.Cu that a foreign 0.4 mm track divides in two, a GND pad in the left piece. The outline minus
clearances is two pieces (x 40 to 49.75 and 50.25 to 60 mm: the cut is 0.2 mm clearance round the track as the model drew it, whose edges the edge rule had moved in); the native fill
is one (40 to 49.60 mm, the clearance taken from the track as drawn), because the right piece touches no pad of its net and the fill removes it as an island. `a_zone_without_a_fill_keeps_its_outline_and_says_so`
covers the fallback per layer. *What the fill does to the loop (a board that is not the shipped example).* `write_the_example_run_with_a_top_pour_for_manual_inspection` (ignored) writes the routed example with a
17 x 16 mm GND pour on F.Cu round the loop added (not part of `examples/emi_buck`; `--fullwave-single-mesh`, one run each). With the native fill (2 fill pieces, 998,325 cells, 19,351 steps, 121 s): 3.63 nH, flat to 0.04 %, status `ok`, closed form 5.10 nH.
With the outlines minus clearances (`--fullwave-zone-outline`; 1,231,200 cells, 69,961 steps, 367 s): the record did not decay, the ring-down fit was **rejected** (the first 70 % of the tail predicts the withheld
2.6 ns to 100 % in current and voltage, although the orders agree to 0.00 %, no pole grows and `Re Z / |Z|` stays above 0.60), status `indicative`, 6.93 nH as the plain transform of the truncated record, flat to 0.94 %. This is the first live
rejection by the new checks, and it is a case the old acceptance (orders within 5 %) would have taken by the code. The two numbers are not a comparison of two converged results, so the 3.2 nH between them cannot be attributed to the fill alone, and why the outline model rings
and the fill model does not was not diagnosed.


**Cross-check against the extraction (iteration 5).** When `--fullwave` and `--extract` both ran, `measures.extract.cross_check` and the `extract:` line state
the extraction's value at 100 MHz, the full wave's `Im Z / omega` at its lowest analysis frequency (30 MHz: `measures.fullwave.inductance_lowest_nh`, `lowest_frequency_mhz`),
the two numerical sensitivities (the extraction's difference of its two finest meshes at 100 MHz, the full wave's `mesh_sensitivity_pct`) and the difference of the extraction from the full
wave in percent of the full-wave value, then one of three statements: the values agree within the two sensitivities together, they disagree by more
than the two sensitivities together ("so the difference is not mesh noise; the models also omit different things"), or no statement is made (the full-wave value is indicative, or either side has one mesh and no sensitivity).
The words pass and fail do not occur (`a comparison of two screening numbers, not a pass or a fail`). It is `emi_extract::cross_check`; tests
`the_cross_check_states_agreement_or_disagreement_against_the_two_numerical_sensitivities` and `the_cross_check_makes_no_statement_without_a_sensitivity_on_either_side`.
The two values are at different frequencies (30 and 100 MHz); on the placed and routed examples and the TPS62130A L(f) is flat to 0.1 % between them, but a resonant loop's is not, which is a reason the full-wave value
of such a loop is usually indicative and gets no statement.

**Limitations.** Components are not modelled (the capacitor and diode are
copper; the switch is a port; their ESL, ESR, package and the controller's
bond wires are absent), copper is lossless, zone fills are the native filler's
where the board can be exported (otherwise the outline minus clearances), cut but not re-made around the model's own bridges and legs, vias are wires without radius, the window
cuts every net, there is no enclosure or cable, and no near-field export or radiated estimate. A
placed board's straight legs may cross foreign pads (the closed form has the
same blind spot); overlapping lines of different nets within 0.075 mm (a tenth of the cell without
smoothing) merge (flag `nets_merged_below_floor`). Round pads are chamfered. A strap port adds
a strap that the real loop does not have (`port_strap_nh`, the closed-form inductance of the strap bar alone over its plane; 0.95 nH of the
TPS62130A's 2.83 nH and 1.78 nH of the CopperOrgan loop). Iteration 5 measured it: the closed form has the strap's growth with length within 10 % and understates its contribution by 0.2 nH on three geometries ("The strap bias, measured"); no correction is subtracted. A package's exposed pad is bridged in the air at an assumed height of 0.3 mm. The hull
bridge of a low-side switch covers whatever lies between two distant pads. The stack-up of a board
that declares none is assumed. **Accuracy:** the edge rule is a fit on two geometries (0.2 and 0.5 mm of dielectric,
0.5 and 1.0 mm strips) checked on three others and on the 20 mm lines of the table above; it is not derived, it
is calibrated on lines at cells of 0.167 to 0.25 mm. Its S-parameter impedance is within 1.1 % of Hammerstad-Jensen at cells of 0.167 to 0.4 mm (the 1 mm
strip of 3a: -0.12, -0.02, -1.05 %; the geometries of the coarse table below, -1.9 to +0.9 %) and 4.1 % high at the coarse default cell of 0.5 mm, where the
strip is two cells wide (the rule of a third: 2.3 %). The shorted lines are within 3 % of the closed form at the standard mesh and
carry 0.05 to 0.2 nH of end effect, a stated 1.5 to 2.7 % excess that is real physics of the vertical
transition and not a model error, but it means that a 5 nH loop with a 4 mm port gap is not known better
than a few percent from this test alone. For a placed loop the mesh sensitivity is not below 4 %
(`mesh_sensitivity_floor_pct`) however well two meshes agree, because mesh decisions on small
geometry move L by as much as the cell does; the closed form of the placed example (4.973 nH) is within
3.2 % of the model (4.813 nH), but that is not independent evidence for either: the closed form of the 6 x 4 mm loop
(4.987 nH) is 1.7 to 4 % above the default-rule series and 4 to 10 % above the earlier rule's limit. FDTD is slow: the
standard mesh takes 12 s on the placed emi_buck, a minute on a routed loop with narrow traces, minutes on a macro board (9 minutes for two meshes of the TPS62130A) and about 31
minutes on the CopperOrgan board, which is single-mesh and did not decay; the two-mesh default two to three times the standard run.

**Next steps.** (1) Components as lumped R/L/C and a switch with an on-resistance and a
gate edge, to simulate the ringing the 143 MHz estimate predicts (the CopperOrgan loop rings at
348 MHz with the capacitor and diode shorted). (2) Zone fills: done in iteration 5 for boards the exporter can handle (not boards with an unmaterialised hard macro, which the TPS62130A example is); the edge rule for plane and pour edges, which the slot case of iteration 5 shows is missing.
(3) Near-field export and radiated estimates by per-port superposition. (4) The mesh decisions on small geometry:
a merge that does not move copper edges by up to 0.4 of the cell
(0.5 of the cell moved the TPS62130A by -2.4 % and 0.6 the placed emi_buck by +5 %: the merge distance is where the remaining
mesh sensitivity sits); the floor under the cells is done (above), the sub-0.05 mm cells of the 0.10 mm refinement of emi_buck remain
(0.041 mm: the gaps themselves). (5) The strap bias. `port_strap_nh` is the closed form of the strap bar alone over its plane
and it was not made trustworthy: `strap_port_against_direct_port` does not measure it. The strapped model of that test is the loop with
a continuous top bar (the strap fills the gap) and a port of 0.4 to 0.5 mm in its middle, so its inductance does not depend on the gap
(8.635 nH at gaps of 2 and 3 mm), while the direct port spans the gap and carries the inductance of the gap's own field; the measured
difference (+0.338 nH at 2 mm, +0.316 nH at 3 mm, against closed forms of 0.526 and 0.834 nH) is the strap bar, 0.36 mm wide, against the
0.5 mm track it replaces, plus the direct port's gap, not the inductance of a strap that the real loop lacks. The strap-length series with extrapolation to zero length was done in iteration 5 (above); a correction would need a geometry that is not one of the three series, and none was measured. A package
model for the switch (die, bond wires and lead frame, so that the strap port stops carrying a
bias of 1-2 nH) and a check of the hull bridge against foreign copper. (6) The edge rule for
curved and slanted edges, cells below 0.1 mm and the S-parameter impedance of coarse meshes, and the
fit on loops with several close modes; the lateral and vertical parts of the rule were fitted at
four points.

## 12. Test boards

- CopperOrgan: TPS54360 asynchronous buck without a macro. Expected to fire.
- CopperProbe / `examples/tps62130a_macro`: reviewed TPS62130A hard macro.
  Expected clean.
- CopperAssetTracker: two TPS63020 buck-boost macros beside three antennas.
  Aggressor-to-victim proximity.

Observed with `copper emi-check` (phase 1):

- `examples/emi_buck`: annotated asynchronous buck with fixed placements and
  proxy footprints; the reference board of the tests
  (`crates/copper-drc/tests/emi_check.rs`).
- `examples/emi_buck_boost`: a TPS63020-style four-switch buck-boost with
  `L1`/`L2`, `VIN`, `VOUT` and `PGND`, giving an input loop and an output loop.
- `examples/tps62130a_macro`: one inferred cell (the part declares no
  `emi_role`, frequency or edge data), so the radiated and ringing estimates
  are skipped with a reason; loop and switch-node geometry are measured, and
  the hot loop is `traced_copper` from the macro's copper (with a
  `plane_shortcut` on the ground leg).
- CopperOrgan: the pinned `TPS54360DDAR` declares no `category`, so the
  structural fallback (which needs `power.regulator.*`) finds no cell and the
  report is coverage only. Annotating the part or giving it the category is the
  fix; it is a library change, not a tool change.
- CopperAssetTracker: the `TPS63020` pins `L1`/`L2` are `passive`, so the
  fallback finds no switch pin; the cells need `emi_role` annotations in the
  library part.

Boards for the signal and cable checks (synthetic, no switching converter; each has a routed state checked in under
`routed/` because the language has no tracks, and is run with `copper emi-check BOARD --run BOARD_DIR/routed`):

- `examples/emi_return_path`: a pair over a whole pour, a pair crossing a cut between two pours, two clocks with a layer
  change (one with a ground via nearby, one with none) and a clock near the board and plane edge (section 15; `crates/copper/tests/emi_return_path_cli.rs`, and `emi_no_converter_cli.rs` for the report,
  the issues and the summary on a board with no cell).
- `examples/emi_mode_conversion`: four differential lanes: 10 mm of skew with no other part, no skew, 4 mm of skew
  before a common-mode choke behind two coupling capacitors, and 6 mm after the choke (section 16; `crates/copper/tests/emi_mode_conversion_cli.rs`).
- `examples/emi_cable_common_mode`: six lanes at an unshielded matched connector, a shielded connector with a 150 ohm
  cable (a first-arrival voltage) and a connector with single-ended signals, two clocks and a symmetric pair for the
  ground-noise driver (section 18; `crates/copper/tests/emi_cable_common_mode_cli.rs`, and
  `emi_no_converter_signals_cli.rs` for the report, the issues, the summary pages and the connector spectra of a board with
  clocks, pairs and connectors and no converter).

## 13. Quasi-static network extraction (phase 5)

Status: first cut, `crates/copper-extract`, reached by `copper emi-check --extract`. Screening evidence only: nothing
here qualifies a design, and no check turns a missing input into a pass.

**Why.** The closed-form hot loop (section 8) chooses one copper path geometrically (the shortest) and then evaluates it
electromagnetically as bars over an ideal plane. A shortest path does not establish how the current divides through wide
pours, parallel traces, several vias and return planes, and the plane shortcut (`plane_shortcut`) assumes an unbroken plane
of infinite extent, so openings and finite boundaries are missed. The region full-wave model (section 11) sees the
geometry but needs minutes per cell, takes zones as outlines minus clearances (no fill) and did not converge on the real
board. This tier sits between them: the impedance of the actual filled copper, quasi-statically, in seconds to a few
minutes.

### 13.1 Model

* **What is solved.** The magneto-quasi-static partial-element (PEEC, FastHenry-style) problem: the copper is cut into
  rectangular filaments, each with a resistance `rho l / (w t)` and a partial self inductance, every pair of parallel
  filaments has a partial mutual inductance, perpendicular filaments none. At angular frequency `w` the branch impedance
  matrix is `Z_b = R + j w L`. Output per frequency: the port impedance matrix `Z(f) = R(f) + j w L(f)` (one or more
  ports) and the complex current of every filament for a unit current into a port (current sharing).
* **Formulation: loop analysis.** A spanning forest of the filament graph (breadth first, branch order) fixes
  `n_b - n_n + n_c` independent loops. With `B` the loop-by-branch incidence matrix, the loop currents solve
  `B Z_b B^T m = -B Z_b b_p`, `b_p` being the unit current along the tree path between the two terminals of the port;
  the port voltage is the sum of `Z_b b` along that path. `B L B^T` and `B R B^T` do not depend on frequency and are built
  once; each frequency is one dense complex elimination of the `n_l x n_l` loop matrix. The loop form was chosen over the
  nodal form `A Z_b^-1 A^T` because the system is about half the size for a planar mesh and `Z_b` is never inverted.
  **Loops in conductors that touch no port are kept**: a floating plane piece or island carries induced (eddy) current
  loops, which is how a plane shields a loop (the image-plane effect) and how a finite or slotted plane differs from an
  ideal one. No plane is replaced by an image.
* **Neglected, stated in `not_modelled` and the flags.** Capacitance and displacement current (no resonance, no
  radiation, no dielectric: the stack-up's permittivity plays no part, only its heights), skin effect across the copper
  thickness (one filament across the thickness, so the current is uniform over it; `MeshSpec::thickness_filaments` adds
  parallel filaments across it and the resistance then rises with frequency, 1.53 times the DC value at 1 GHz for one
  filament and 3.9 times for four on a 10 x 1 mm bar of 35 um copper, a trend that is not validated against a closed
  form), proximity and edge crowding only as far as the lateral mesh resolves them, magnetic materials, copper
  temperature (resistivity 1.72e-8 ohm m), every lumped element (the capacitor and the diode are copper bars between
  their pads and the switch is the port: no ESL, ESR, package, die or bond wires), the copper of nets that are not
  modelled, and everything outside the window.

### 13.2 Geometry in

`copper_extract::board` exports the physical board in memory (`copper_pcb::kicad_pcb::generate`), parses it with
`copper_cam::board::parse_board` and fills its zones with `copper_cam::fill::fill_zones` under the rules of the exported
`.kicad_pro` and `.kicad_dru`: the native filler cannot be called on a `PhysicalBoard` directly (it works on its own
drawing model of a `.kicad_pcb`), so this export is the bridge, and nothing is written to disk. The fills therefore have
the zone clearances, thermal reliefs, openings for other nets' vias and pads, the minimum width and the removal of islands
the zone asks for. Tracks, arcs, pads (plated through-hole pads add a barrel) and vias come from the same parsed board, so
one net namespace holds everything.

* **Nets.** The nets of the loop's pads and of every zone fill that reaches the window; `other_nets` lists the latter.
  Copper of other nets is not modelled.
* **Window and corridor.** The loop polygon of the finding (the traced path through its pads and vias) and the pads, grown
  by the margin (`--extract-margin`, default 3 mm), bound the window; zone fills are cut to the polygon grown by the
  margin (flag `plane_corridor`), so a long loop does not mesh the rectangle of its bounding box. Planes and pours are
  therefore finite: a finite plane raises the loop inductance a little above the infinite-plane value (the validation
  below finds this smaller than the mesh sensitivity at a margin of 3 mm over 0.2 mm dielectric).
* **Stack-up.** Layer heights are those of `copper_fullwave::stack` (declared, else the board thickness spread over the
  copper layers, flag `assumed_stackup`); copper thickness is the stack-up's, else 35 um (`assumed_copper_thickness`); via
  plating is 25 um (`assumed_plating`: the export does not carry it). These are `parameter_assumptions`, separate from the
  numerical sensitivity and from the omissions.
* **Vias** are tube barrels along z joining the node nearest to the via (of the piece of copper it stands in, when it
  stands in one) on every layer it passes where its net has
  copper within twice the coarse pitch, as a chain of segments of the height difference of the layers. A via that joins
  fewer than two layers is listed (`via_unattached`) and left out.

### 13.3 Mesh and budget

Each layer is cut into rectangles (a k-d tree, `copper-extract/src/mesh.rs`) and the layers couple through the partial
inductance matrix and are joined only by vias. The nodes are the corners of the rectangles; a filament joins two
neighbouring nodes along a side of a rectangle (a rectangle next to two smaller ones has their corners as hanging
nodes on its side). Its cross-section is the strip between the half-way lines to the rectangles on either side times
the fraction of its body that the layer's copper of the net covers, so a bar whose edges lie on rectangle edges is
tiled exactly and has exactly `rho l / (w t)`; a body covered by less than 5 % is dropped and counted.
**The mesh has the connectivity of the copper polygons** (`copper-extract/src/mesh.rs`, tests `tests/topology.rs`). The
copper of a net on a layer is the union of its shapes, each polygon of it one *piece*; a filament is cut from one piece
only and its nodes carry the piece, so two pieces of copper that do not touch never share a node and are never joined
by a filament, whatever the pitch and however close their outlines (a first version of this mesher joined the two
halves of a switch bar across the port gap with a filament, and the port read 0.035 nH instead of 4 nH; a later one
kept a filament when the copper covered a quarter of its axis, measured on the hull of the covered part, so two
rectangles 0.4 mm apart in cells of 0.75 mm read 1.2 mOhm at default settings, and a plane with a slot was shortcut
across it by filaments that ended on a node in the slot). Now: the copper within 25 um of the filament's axis has to be
one chain (its pieces no more than 25 um apart) and cover a quarter of the length, else the filament is dropped
(`gap_bars_dropped` counts those); a node that no copper reaches (every filament on it stops short of it) is shared
only by filaments whose copper inside their bodies touches, as a diagonal trace needs, and is split otherwise, one
node for each side of the slot; filaments of different nets never share a node. A slot or gap narrower than a filament,
two pieces of copper in one filament body, and a piece of copper that the meshing cut apart are defects of the
partition, and each is **refined locally**: cells of half the body's extents round the defect, down to 50 um, in up to eight
rounds, **in order of priority while the filament budget holds**. First the repairs of a topology mismatch (the dropped
filaments next to the nodes of a part that was cut off), alone and repeated round by round until the mesh is whole:
they are required, and a pitch at which they do not fit is coarsened. Only then the gaps and shared cells, nearest to the
port's terminals first, as many as fit (the largest prefix of that ordered list that keeps the mesh within the budget,
found by bisection, deterministic). A mesh that was whole and whose next repair does not fit stays as it was; if no pitch
holds even the repairs, the best mesh that fitted (fewest mismatches) is kept, status `error`. What was left out is reported:
`unresolved_gaps` (at most 20 listed with layer, net and place, and their number), the flag `unresolved_gap` (the extraction is
`indicative`), the flag `refinement_stopped_by_budget`, and `refinements_applied` / `refinements_skipped` (counted over the
rounds). Nothing left out is bridged. **Terminals and vias attach to the copper they touch, and only to it.** A terminal takes the nodes inside its region
(grown by 25 um as a tolerance) **of the pieces of copper its own region overlaps with non-zero area**, never those of
another piece of its net however near, and when no node lies inside it snaps to the nearest node of those pieces only; a
terminal whose region touches no copper of its net is an error (`NoCopper`) that names it. A terminal whose region overlaps
several pieces is an equipotential that joins them, which is what a terminal is: it is kept, and recorded
(`measures.extract.topology.terminals_joining_pieces`, the flag `terminal_joins_pieces`, a note). A via that stands in copper of
its net on a layer joins the piece it stands in; one that does not attaches only to a piece its own circle (drill plus plating)
overlaps, else it joins nothing on that layer (before, it took the nearest node of any piece within twice the coarse pitch: a via
0.7 mm beside an island it did not touch joined it). Before, a terminal took every node of its net and layer inside the grown
region from any piece: two islands 10, 20 and 30 um apart with a terminal at the gap's edge read 0.60 mohm across the gap
instead of an open circuit, with no mismatch reported (`tests/topology.rs`, `a_terminal_at_a_gap_never_takes_the_copper_on_the_other_side`).
**A topology self-check** follows every build: the connected components of the copper
pieces (joined by the vias that stand in them, and by the terminals and shorts that contact them) are compared with those of the network the solver solves (filaments, barrels, and the merges of every terminal's nodes and every short, `Network::merge_pairs`).
A piece the mesh cut apart, or a network component that joins pieces that are not one piece of copper, is a mismatch,
listed in `measures.extract.topology.mismatches` with the flag `topology_mismatch` and the extraction status `error`
(the numbers are listed but are not the board's, and the multi-port matrix is not used in `EMI-CONDUCTED`).
Genuinely connected thin copper is kept connected by the same rules: a 0.15 mm and a 0.25 mm neck between two pads read 1.19 and 1.31
times the neck's own `rho l / (w t)` at default settings (the pads add the rest), unrefined, and a piece of copper that the mesh cuts is refined until it is whole or
reported.

A rectangle is cut in the middle (snapped to a copper edge within the middle half) while it is larger than the size
asked for at its place, which is the smallest of the requests round it, each `request + (growth - 1) distance` with
`growth` 2 (the size may grow by the distance from a fine place), and at most the coarse pitch (five cells, 1 mm
by default). The requests:

| Request | Size | Scales with the pitch |
| --- | --- | --- |
| narrow copper of the loop (a track, a pad, the bars of the capacitor, diode and switch), narrow meaning at most 8 nominal cells (1.6 mm) in an axis, or a thin shape that is not along an axis (a diagonal trace; its width taken as 2 area / perimeter) | at least two cells over the width (`MeshSpec::across`), at most one cell | only the cap at one cell |
| other narrow copper | one cell | yes |
| the hints of the board (strips along the loop's tracks on every layer, half the layer distance across, reaching four layer distances) | their pitch | yes |
| a terminal (contact, port end) | a third of its longer side, and at least 50 um | no |
| a via | its own diameter (drill plus plating) | no |

A terminal region and a via position are always cell edges where they reach a rectangle (two edges closer than 25 um
are one), so a terminal has nodes inside it and a contact is the nodes along one line; only a terminal with no node
inside (the layout gave it none) is joined by its nearest node and flagged `terminal_snapped`. Mirror-symmetric
layouts get meshes that are symmetric to rounding, not to the bit (a rectangle that is too large in both axes is cut in
both at once; half cells are rounded down). The earlier tensor grid put every fine line across the whole layer,
so a via or a pad anywhere made fine rows and columns everywhere; the filaments now go where the requests are.

**Budget.** The inductance matrix is dense (`n_b^2` doubles) and a frequency costs one complex elimination of about
`n_b / 2` loops, so the number of filaments is bounded: `--extract-budget` (default 9000, at most 20000). The assembly of
the matrix and the elimination share their rows between the machine's threads (`std::thread::scope`, at most 16;
every row is computed by the same code, so the result is the same to the bit whatever the thread count); on the
12-core machine of this work 6700 filaments took 25 s, 9400 took 56 s and 14100 took 203 s for one mesh and three
frequencies. A mesh over the budget is coarsened by 1.3 (the pitches `cell`, `coarse` and the hints; the resolution of
terminals, vias and the narrow copper of the loop is not a pitch and stays) up to ten times, flag `coarsened`; if it
still does not fit the extraction fails with the line and bar counts per layer. The default run solves a series of three
meshes: the finest that fits the budget, and two coarser ones with pitches 1.5 and 2.25 times larger.

### 13.4 Partial elements and thresholds

* Resistance: `rho l / (w t)` (a barrel: `rho l / (pi (r_o^2 - r_i^2))`, wall = the plating).
* Self inductance of a bar: the exact Hoer-Love integral (`copper_emi::inductance::self_inductance_bar`, Grover beyond
  `l^2 / (w t) = 1e5`). A barrel: Grover's tube `mu0 l / (2 pi) (ln(2 l / r) - 1 + r / l)`.
* Mutual inductance of parallel planar bars: the exact Hoer-Love integral of uniform current over both cross-sections
  (`copper_emi::inductance::mutual_parallel_bars`, a new public wrapper of the existing 64-term closed form) when the
  distance between the axes is below eight times the larger cross-section dimension of the pair
  (`partial::EXACT_RANGE`), centre filaments (Grover's closed form of two parallel filaments) beyond. The error of the
  filament form is about `s^2 / (24 d^2) / (ln(2 l / d) - 1)` and was measured against the exact form at the switching
  distance over cell-sized squares, long traces, side by side and stacked: 0.26 % at the worst, below 0.3 %
  (`partial::tests::the_centre_filament_switch_is_accurate_where_it_is_made`); at six sizes it was 0.46 %, which is why
  the range is eight. Perpendicular filaments: zero. Barrels: parallel filaments.
* Skin effect and the finite thickness of a filament are not in the partial elements; see 13.1.

### 13.5 The hot loop

The port, the pads and the closures are those of `copper-fullwave`'s hot-loop model (section 11), so the numbers can be
read beside the closed form and the full-wave value. **Port:** across the high-side switch (hot-supply pad to switch pad
for an asynchronous buck; for a synchronous one the hot-supply pad to the controller's switch pad, the low-side switch
being a closure). **Closures:** the capacitor and the catch diode (or low-side switch) are copper bars from pad edge to pad
edge along the line of the pad centres (width: the smaller pad dimension clamped to 0.2 to 1 mm, the bar the closed form
draws across the body), of the net of one pad and joined to the other pad by a contact 10 um deep. A contact is the equipotential
merge of the nodes of the bar's end face and of the pad's copper next to it (FastHenry's `.equiv`). **The switch** is
the same kind of bar with a gap of one cell in its middle, and the port is between the two ends at the gap, so the port sees
the flux of the gap as the closed form and the full-wave port do (`--extract-no-switch-bar` puts the port between the
facing sides of the two pads instead, and the gap is then not in the path). Copper legs without routed copper get the straight
bar of the closed form, as in the full-wave model.

Two design points that were found by measurement and are worth stating. (1) The terminals must be small. Merging a pad
(or any patch of area) into one node removes the inductance of the current's path across it and, worse, lets metal that
overlaps the merged region carry eddy loops through the merged nodes: a 0.5 mm deep contact lowered a 11 mm bar by 25 %, a
whole-pad terminal lowered the placed `emi_buck` by a third, and a bar overlapping its pad lowered the free-space loop by
22 %. Contacts are therefore thin and the bars are clipped to the space between the pads. (2) A Manhattan mesh meshes a
diagonal conductor as a staircase (measured with the earlier tensor mesher and not repeated; with the adaptive mesher rotating the whole loop by 20 and 45 degrees moves L by +1.5 % and +0.02 %, section 13.8): a 10 mm, 0.5 mm wide bar at 45 degrees reads 6.94 nH on a 62.5 um mesh against 7.26 nH
for the same bar along an axis (-4.4 %, converging slowly: -6.7 % at 250 um), and the loop of `emi_buck` has two diagonal legs.

### 13.6 Outputs

`measures.extract` of the `EMI-HOT-LOOP` finding (and the `extract:` line, and the coverage entry `EMI-EXTRACT:<cell>`):

| Key | Meaning |
| --- | --- |
| `inductance_nh`, `resistance_mohm`, `frequencies_mhz` | `Im Z / w` and `Re Z` of the finest mesh of the series at 1, 10 and 100 MHz |
| `closed_form_nh`, `ratio_to_closed_form`, `fullwave_nh`, `ratio_to_fullwave` | the traced closed form of the finding and, when `--fullwave` ran, its inductance, with the ratios |
| `status`, `status_reasons` | `ok`, `indicative` with the reasons, or `error` (the meshed network does not have the connectivity of the copper; the multi-port matrix is not used). Reasons: a gap in the copper narrower than the smallest cell that the mesh did not resolve (`unresolved_gap`, listed in `topology.unresolved_gaps`), a terminal joined by its nearest node because no node lies inside it (the mesh is coarser than the terminal), vias left out, the two finest meshes more than 3 % apart at 10 MHz (`convergence_threshold_pct`), a self inductance of the port matrix moving more than 3 % between them, a single mesh (no evidence of convergence). Coarsening to the budget is a flag and a note, not a reason: it cannot coarsen below the terminals, vias and narrow copper of the loop, and the series shows what it cost |
| `convergence` (13.10), `numerical_sensitivity_established` | the change of every entry of the L and R matrices between meshes, per frequency, the worst entry of each, the thresholds (3 % for L, 5 % for R), whether the two finest meshes are comparable and why not, the crowding length per frequency |
| `reference` (13.11, with `--extract-reference`) | the independent reference solver's R and L matrices beside the extraction's at 1 kHz, the difference of every entry, the topology (pieces, ports open), its own pitch sensitivity, the pitch and the cell count |
| `numerical_sensitivity_pct`, `numerical_sensitivity_earlier_pct`, `numerical_sensitivity_basis` | the difference between the two finest meshes of the series (pitch ratio 1.5) per frequency, and between the two coarser ones; `null` with `numerical_sensitivity_unavailable` for one mesh. Two meshes that agree do not bound the error: the series below still moves, and a falling step is evidence, not proof |
| `parameter_assumptions` | what the board does not fix: assumed stack-up, copper thickness, plating, window, resistivity |
| `not_modelled`, `flags`, `notes` | the omissions of 13.1; `assumed_stackup`, `assumed_copper_thickness`, `assumed_plating`, `coarsened`, `plane_corridor`, `merged_terminals`, `terminal_snapped`, `via_unattached`, `connection_legs`, `unresolved_gap`, `topology_mismatch`, `refinement_stopped_by_budget`; how each leg and component was drawn |
| `meshes` | per mesh: role (`standard` = the finest, then `coarser` and `coarsest`), pitches, filaments, nodes, loops, barrels, coarsening steps, `dropped_bars`, `gap_bars_dropped`, `refine_rounds`, `unresolved_gaps`, `copper_components`, `mesh_components`, L and R, seconds |
| `topology` (with `terminals_joining_pieces`), `unconnected_capacitors`, `not_used_in_conducted`, `synthesized_connection_legs` | the connected components of the copper polygons against those of the mesh (`copper_components`, `mesh_components`, `mismatches`, `unresolved_gap_count`, `unresolved_gaps`), and each straight bar added for a capacitor pad that no copper joins to the hot capacitor (`capacitor`, `pad`, `net`, `to`, `length_mm`) |
| `port_labels`, `inductance_matrix_nh` | the ports (`hot loop`, then the other capacitors of the hot net) and the inductance matrix of the finest mesh at each frequency (1 x 1 without other capacitors), the input of the multi-port use in `EMI-CONDUCTED` (section 10) |
| `via_currents` | per frequency, the vias that carry at least 1 % of the port current: current per ampere into the port and `share_of_net`, `|I| / sum |I|` over the vias of the same net (magnitudes, adding up to 1 whatever the phases) |
| `nets`, `other_nets`, `window_mm`, `layers`, `copper`, `seconds`, `filament_budget`, `method` | what was modelled, the fill (`zone_fills`, `islands_removed`, `fill_seconds`) and the cost |

Nothing is a pass or a fail and no accuracy is claimed. The series is the finest mesh that fits the budget and two coarser ones
(pitches times 1.5 and 2.25); the reported numbers are the finest mesh's. Without
`--extract` nothing changes, and everything stays in the one `emi` section of the route report.

### 13.7 Validation

Every case has a reference written in the test (`crates/copper-extract/tests/analytic.rs`, run by default unless marked
`ignored`) or a closed form of `copper_emi::inductance`, and a series over the mesh. The whole default suite of the crate
takes about 50 s in the debug build of this repository on a 12-core machine (18 s for `analytic`, 21 s for `boards`, 11 s for `attribution`; the `boards` tests use a budget of 3000).

Numbers below were measured in this repository (debug build with dependencies at `opt-level = 2`); "filaments" is
the size of the dense system. The tests assert the bounds given in the last column, not the measured values.

| Case (test) | Reference | Mesh series (filaments) | Result | Asserted |
| --- | --- | --- | --- | --- |
| Straight bar 10 x 1 x 0.035 mm (`straight_bar_matches_...`) | `self_inductance_bar` 6.9864 nH; `rho l / (w t)` 4.9143 mohm | pitch 250 / 125 / 62.5 um (112 / 314 / 1072; the test adds a uniform hint across the bar: next to the terminals of a bar that is wide against the pitch the adaptive partition changes cell size, which adds up to 0.9 % to R) | L(10 kHz) 6.9864 / 6.9864 / 6.9861 nH (0.000, 0.000, -0.005 %); R(DC) exact to 1e-6 | R within 1e-6, L within 2 % |
| Two parallel bars, 1.0 and 0.4 mm, driven in parallel (`two_parallel_bars_...`) | 2 x 2 solution with `self_inductance_bar` and the filament-averaged `mutual_bars` (L 6.986 and 8.679 nH, M 3.513 nH, R from `rho l / (w t)`) | 250 / 125 / 62.5 um (146 / 428 / 714) | DC share of the wide bar 0.7143 = conductance share (to 1e-3); shares at 0.1, 1, 10, 100 MHz and 1 GHz within 0.97 % of the hand solution (+0.2 to +0.6 % at 1 to 10 MHz on the two coarser meshes, -0.5 to -1.0 % at 10 MHz and above on the finest; the lateral mesh lets each bar's current crowd, the hand solution does not) | DC within 0.1 %, all within 1.5 % |
| Rectangular loop 10 x 6 mm in free space, trace 0.5 / 0.25 mm, 1 mm gap, port across the gap (`rectangular_loop_in_free_space_...`) | `loop_inductance` of the five centreline segments: 20.629 / 24.450 nH | 250 / 125 um (397 / 1062; 474 / 534) | L(10 kHz) -1.43 / -1.13 % (0.5 mm), -0.59 / -0.59 % (0.25 mm); L(100 MHz) at 125 um 19.68 and 23.73 nH | finest mesh within 5 % |
| The same loop over a floating plane 0.2 mm below, plane 16 and 32 mm (`rectangular_loop_over_a_plane_...`) | `loop_inductance_over_plane` (image plane at the plane layer's centre): 7.8037 nH | 0.4 mm pitch near the loop, 3 mm far (1864 / 2264) | L(1 MHz) 7.915 / 7.910 nH (+1.43 / +1.36 %); L(10 MHz) 7.390 / 7.384 (-5.30 / -5.38 %) | L(1 MHz) within 5 % |
| Plane growth and mesh series of that loop (`study_loop_over_a_plane_...`, ignored, 64 s; measured with the earlier tensor mesher, not re-run) | the same | 0.5 mm pitch, plane 16 / 24 / 32 / 48 / 64 mm (1058 ... 3132); 24 mm plane, pitch 800 / 500 / 350 / 250 um (909 / 1438 / 2485 / 3356) | L(1 MHz) 7.8660 / 7.8611 / 7.8596 / 7.8578 / 7.8571 nH: the finite plane lowers towards the infinite one as it grows, by 0.11 % from 16 to 64 mm, and the 64 mm value is +0.68 % over the image result. Mesh series, L(1 MHz): -1.95, +0.74, -0.63, -0.13 %; L(10 MHz): -6.0, -2.9, -4.9, -4.9 % | - |
| Shorted microstrip, w 1.0 mm, h 0.5 mm, 10 mm (`shorted_microstrip_matches_...`) | `Z0 sqrt(eps_eff) l / c` (Hammerstad-Jensen, the low-frequency limit of `Z0 tan(beta l) / omega`): 2.9814 nH zero thickness; 2.9019 nH with the classical effective width for 35 um copper | pitch 500 / 250 um (864 / 1456) | L(1 MHz) -0.76 / -1.14 %; L(10 MHz) -2.91 / -4.38 % of the zero-thickness value, -0.26 / -1.76 % of the thick one | L(10 MHz) within 5 % of zero thickness |
| Shorted microstrip, the four geometries of `copper-fullwave/tests/shorted_line.rs` at 20 mm (`study_shorted_microstrip`, ignored, 148 s; measured with the earlier tensor mesher, not re-run) | zero-thickness value / thick value | pitch 250 um (1792 to 2564) | L(10 MHz) of 35 um copper against zero thickness / thick: h 0.5 w 1.0: -3.2 / -0.6 %; h 0.2 w 0.5: -5.0 / -0.6 %; h 0.5 w 0.5: -2.5 / +1.9 %; h 0.3 w 1.5: -4.2 / -2.3 %. L(100 MHz) is 0.1 to 0.2 % lower. With 5 um copper at 100 MHz against zero thickness: -0.3, +1.4, +1.0, -0.3 %; with 1 um at 100 MHz: +0.8, +3.6, +2.3, +0.9 % (at 1 MHz the thin copper is still resistive and L is 15 to 115 % high) | - |
| Wall of vias instead of the ideal short (same study, h 0.5 w 1.0, 10 mm) | ideal short 2.8511 nH | 2 and 4 barrels (0.1 mm drill, 25 um plating) | 2.8865 / 2.8371 nH (+1.2 / -0.5 %) | - |
| Slot in the plane under a 10 x 0.5 mm trace, plane 12 mm wide, slot 2 mm wide (`a_slot_under_the_trace_...`) | unslotted: Hammerstad with the thick correction 2.4994 nH; limits: the unslotted and the fully slotted plane | pitch 0.5 mm (930 to 1722) | L(10 MHz) for slot 0 / 2 / 4 / 6 / 8 / 10 mm: 2.472 / 3.238 / 3.850 / 4.475 / 5.105 / 5.806 nH, 1.00 / 1.31 / 1.56 / 1.81 / 2.07 / 2.35 times the unslotted value, rising with every step and bounded by the intact plane below and the slot over the whole trace above (the "no plane" limit has no finite value for this port: its return would be the far edge) | monotone rise, last above 1.3 times the first, unslotted within 10 % of the reference |
| Four vias between two pours, symmetric (`parallel_vias_...`) | 1/N | 1 mm pitch with the cells round the vias (1868) | shares 0.25000 for all four at DC, 1 MHz and 100 MHz | within 1e-6 |
| The same with one via moved from 3 to 5 mm from the pads | the far via carries less; shares add to 1 | same | DC 0.2696 / 0.2677 / 0.2696 / 0.1930; 1 MHz 0.2725 / 0.2698 / 0.2725 / 0.1852; 100 MHz 0.2753 / 0.2716 / 0.2753 / 0.1779 | far via below the others and below 1/4, sum 1 |
| Two ports on two separate bars, 1.5 mm apart (`a_two_port_matrix_...`) | `mutual_bars` M 3.4875 nH; `self_inductance_bar` 8.2723 nH | 125 um | M12 = M21 = 3.4871 nH at 10 kHz (0.01 %), L11 = L22 = 8.2723 nH | reciprocity 1e-6, M and L within 2 % |
| A floating shorted ring beside a bar (`network::tests::...`) | the eddy current must lower the port inductance | hand-built network | lowered | yes |
| Partial elements (`partial::tests::...`) | exact mutual of a bar with itself equals the self inductance; centre filaments against the exact form at the switching distance | - | 1e-9; 0.26 % worst | 0.3 % |
| Hot-loop geometry of `emi_buck` placed against its closed form (`tests/attribution.rs`: `the_study_geometry_is_the_geometry_of_the_closed_form_of_the_finding`, `the_deficit_to_the_closed_form_falls_as_the_loop_grows_against_the_trace_width`, `a_quarter_turn_of_the_loop_moves_the_result_less_than_other_angles_do`) | `loop_inductance(_over_plane)` of the same bars | free space, 1 MHz, k = 1 / 2 / 4 | the study reproduces the finding's 4.9726 nH to 1e-3; deficit -13.0 / -3.2 % at k = 1 / 4 (-6.0 % at k = 2) in the test's run (section 13.8 for the table) | deficit of k = 1 above 5 %, k = 4 below 5 %, falling; a quarter turn within 3 %; 20 and 45 degrees within 10 % |
| Determinism (`the_extraction_is_bit_for_bit_repeatable`, `the_extraction_of_a_board_is_repeatable`) | two runs | - | identical to the bit | yes |

What the series say, separately from the errors: the straight bar and the two-port self term are exact because a uniform
current makes the sum of the mesh's partial inductances equal the bar's, so they test the assembly, the loop solve and the
ports and not the mesh; the shares, the loop in free space, the microstrip and the plane cases do test the mesh, and their
mesh dependence is 1 to 2 % at the pitches used (up to 6 % at the coarsest of the plane series); the 10 MHz values of every
case with a plane or a wide trace sit 2.5 to 5 % below the uniform-current closed forms because the mesh lets the current
crowd to the edges nearest the return, which is what the thick-copper formula also gives (the "thick" column) and which a
closed form with uniform bar currents cannot. FastHenry was not available: no comparison with it exists.


### 13.8 Board cases

`crates/copper-extract/tests/boards.rs` and `copper emi-check --extract`. `examples/emi_buck` is placed (proxy footprints,
four copper layers, a floating ground fill on In1.Cu 0.533 mm under the pads, stack-up undeclared) and routed with the
three 0.4 mm legs of the CLI tests. The full-wave values are the ones recorded in section 11 (the FDTD solver was not
run here). Extracted values are `L(1 MHz) / L(10 MHz) / L(100 MHz)` of the default run (budget 9000: the finest mesh
at the nominal pitch of 0.2 mm, no coarsening, with two coarser meshes), with the difference between its two finest meshes at 10 MHz.

| Board | Traced closed form | Full wave (section 11) | Extracted, default | Mesh difference at 10 MHz (finest two / earlier two) | Run time |
| --- | --- | --- | --- | --- | --- |
| `emi_buck` placed | 4.973 nH | 4.84 nH (standard 4.841, refined 4.813) | 4.140 / 3.901 / 3.883 nH, R 10.1 / 12.9 / 13.9 mohm (6816 + 2991 + 1673 filaments) | 1.4 % / 0.5 %, status `ok` | 40 s (whole `copper emi-check --extract` command 41 s) |
| `emi_buck` routed (test, budget 3000: 2204 + 1259 + 851 filaments, pitch coarsened) | 6.712 nH | 5.97 nH (standard 6.067, refined 5.974) | 5.674 / 5.390 / 5.353 nH, R 17.7 / 22.0 / 24.5 mohm | 0.7 % | 4.6 s |
| `emi_buck_filter` placed | 4.973 nH (the same hot loop: the filter is upstream of it) | 4.84 nH | identical to `emi_buck` placed | 1.4 % | 39 s |

Against the earlier tensor mesher (4.252 / 4.034 / 4.025 nH placed, 5.665 / 5.383 / 5.361 routed) the placed 10 MHz value
moved by -3.3 % and the routed one by +0.1 %. The earlier mesh series of these two boards (0.4 to 0.15 mm pitch, flat within
1.3 % and 2.2 %) was measured with the tensor mesher and has not been repeated; the series of the new mesher is the
three meshes of every run.

**Why the extraction reads below the closed form (attribution, measured).** Extracted over closed form is 0.78
(placed) and 0.80 (routed) at 10 MHz; over full wave 0.81 and 0.90. The extraction is not tuned to match and does not.
`tests/attribution.rs` rebuilds exactly the geometry the closed form assumes (six bars from pad centre to pad centre,
1 mm wide, 35 um thick, one net, ideal corners, a floating plane corridor 0.533 mm below), evaluates the closed form on
it, and then changes one thing at a time. The closed form of the finding is reproduced to 1e-3
(`the_study_geometry_is_the_geometry_of_the_closed_form_of_the_finding`, which also asserts that the pads are 1 mm, the
closed form's width clamp, so the bars have the width of the copper). Placed board, over the plane, nH:

| Step | L(1 MHz) | L(10 MHz) | Change | Of the gap to the closed form |
| --- | --- | --- | --- | --- |
| Closed form of the finding (`EMI-HOT-LOOP`) | 4.973 | 4.973 | | |
| The same bars with the port's 0.2 mm gap in the switch bar (closed form of the open path) | 4.873 | 4.873 | -0.100 (-2.0 %) | 9 % |
| Extractor on exactly those bars (corners overlap, one net) | 4.330 | | -0.543 (-11.1 %) | 51 % |
| 10 MHz instead of 1 MHz (current crowds to the edges nearest the return; the closed form has uniform current) | | 4.098 | -0.232 (-5.4 %) | 22 % |
| Square 1 mm pads added at the six corners | | 4.058 | -0.040 (-1.0 %) | 4 % |
| The real board (rounded pads, capsule ends, three nets joined by 10 um contacts, bars between pad edges, the port in the gap), default run | | 3.901 | -0.157 (-3.9 %) | 15 % |

The sum is -1.072 nH (4.973 to 3.901). The rows are single runs at a pitch of 0.2 mm; the extraction of the bars alone
is 4.299 at 1.5 times the pitch and 4.330 at the nominal one (0.7 %), and the real board's series moves 1.4 %, which is
the size of the last row: that row (-3.9 %) is not attributed to anything in particular and is within what the mesh
sensitivity and the nets-and-contacts details could account for, without being shown to be.

What the second row is, shown by changing the geometry and not the model: the closed form sums the partial inductances of
bars along the centrelines, and those bars overlap in the corner squares (the pad centres), so the corner is counted
twice, while the meshed current takes the inside of the corner. The effect should grow with the width against the length of
the legs, and does: the same six-bar path with the bars 1 / 0.5 / 0.25 mm wide gives -11.1 / -5.2 / -2.1 % over the plane
and -13.0 / -6.0 / -2.7 % in free space (each stable to 1 % over a mesh series of 1, 0.7 and 0.5 times the pitch in free
space); the whole path scaled by 1 / 2 / 4 at the width of 1 mm gives -13.0 / -6.0 / -3.2 % in free space. The legs of the
placed `emi_buck` are 2 to 5 mm long between pad centres and 1 mm wide: the thin-line closed form is outside its
validity there. Nothing in the closed form is wrong as an implementation (the thin rectangular loop of section 13.7 agrees
with it to 1 to 2 %); it is a model of thin traces applied to pads. No change to the closed form was made. The
other terms: the gap term is analytic (the same bars with and without the gap); the frequency term is the
current crowding the validation cases show (section 13.7); the pads add little. The staircase of the two legs that are
not along an axis does not show as a separate term: rotating the placed free-space loop by 20 and 45 degrees (the closed
form does not change) moves L(1 MHz) by +1.5 % and +0.02 %, a quarter turn by less than 1e-5
(`a_quarter_turn_of_the_loop_moves_the_result_less_than_other_angles_do`). The "port gap removes length" hypothesis is the
second row; the "assumed width differs from the copper" hypothesis is false here (the same 1 mm).

One extractor bug was found in the course of this and fixed: with the adaptive mesher a filament could cross the gap of the
port where a cell edge fell short of the end of the copper (14 % of the cell covered, kept as a narrow filament) and join
the two halves, so the port read 0.035 nH. A filament is now dropped unless the copper follows its axis along a quarter
of its length (`MIN_AXIS_COVERAGE`); `a_filament_does_not_bridge_the_port_gap_at_the_pitches_and_angles_of_the_study` covers
pitches of 0.1 and 0.2 mm at 0, 20 and 45 degrees. It does not cover a pitch of twice the gap with the gap in one net and
not along an axis (20 degrees, 0.4 mm: 0.106 nH). The board layouts put the two halves of the switch bar in different
nets (the nets of the two pads), which nothing can bridge. The tensor mesher's mesh of the real `emi_buck` was inspected at
three pitches and held no filament across the gap; the adaptive mesher's was not inspected for one.

**CopperOrgan routed board** (`copper emi-check board.copper --locked --run build/ci/emi-check-20261111T161807 --extract
--json`, read-only; traced path 54 mm through two vias and two layers, closed form 9.889 nH with `plane_shortcut`, full wave
indicative 6.5 to 7.3 nH, not converged; nets GND, SW, V24; eight zone fills of which 16 shapes reach the window, 291 islands
removed by the fill in about 25 s; the window grew from 31.4 x 14.3 to 31.4 x 19.0 mm to take in the pads of C_BIN2, the
other hot-net capacitor, which now has a port). The first three rows are the earlier tensor mesher:

| `--extract-budget` | Fine / coarse pitch | Filaments (standard + second) | L(1 / 10 / 100 MHz) | R(1 / 10 / 100 MHz) | Two meshes differ at 1 / 10 / 100 MHz | Time (extraction, whole command) |
| --- | --- | --- | --- | --- | --- | --- |
| before: 2400 (default) | 2.12 / 10.6 mm | 1992 + 1160 | 4.302 / 3.777 / 3.758 nH | 18.2 / 22.2 / 22.5 mohm | 1.4 / 7.1 / 7.4 % | 15 s, 34 s |
| before: 4500 | 1.25 / 6.27 mm | 3927 + 2389 | 4.928 / 4.272 / 4.234 nH | 19.8 / 26.6 / 27.5 mohm | 6.0 / 5.5 / 5.3 % | 46 s, 65 s |
| before: 7000 | 0.97 / 4.83 mm | 5439 + 3614 | 5.146 / 4.477 / 4.399 nH | 20.4 / 29.0 / 33.7 mohm | 4.1 / 5.7 / 5.2 % | 91 s, 108 s |
| after: 9000 (default) | 0.97 / 4.8 mm (6 coarsening steps) | 8158 + 7621 + 7365 | 6.506 / 6.039 / 6.006 nH | 23.8 / 27.6 / 30.4 mohm | 2.7 / 2.7 / 3.0 % (earlier step 0.7 / 0.8 / 0.8 %) | 106 s, 125 s |
| after: 14500 | 0.57 / 2.8 mm (4 coarsening steps) | 10627 + 8711 + 7730 | 6.285 / 5.767 / 5.734 nH | 23.3 / 27.5 / 30.3 mohm | 0.5 / 0.9 / 0.9 % (earlier step 0.1 / 0.6 / 0.3 %) | 159 s, 180 s |

The series of the last row is the acceptance series: the coarsest, the coarser and the finest mesh read L(10 MHz) 5.849, 5.817
and 5.767 nH, so the last two steps move it by 0.6 % and 0.9 %, below 3 %, with no terminal snapped (the status is `ok`), 10627
filaments at the most (ceiling 20000) and 3 minutes for the whole command on a 12-core machine. A development series of
the same board before the capacitor port and the last mesher changes (the layout without the window round C_BIN2), pitch
multipliers 3.5 / 2.4 / 1.8 of the nominal, gave 6.077 / 5.912 / 5.833 nH at 10 MHz (7077 / 9393 / 14141 filaments, 203 s for the
last).

What this does and does not say. The old numbers rose with every refinement (3.78, 4.27, 4.48 nH) because the pads and
the vias were smaller than the pitch: contacts and port ends were joined to the nearest node, which short-circuits up to a
cell of the path, and a via entered a plane at a node a cell away (by reasoning, not separated by a run: a current entering
a plane at a point has a spreading inductance that grows as the logarithm of the pitch until the pitch reaches the via's
radius; halving the size asked for round the vias moved the 3.5-times-pitch mesh by +0.4 % only). Both are resolved now,
independently of the pitch, and the values moved from 3.78 to 4.48 nH up to 5.77 to 6.04 nH and stopped moving much: the three meshes of the 14500
run agree within 1 %, and the default run's finest mesh (6.039 nH) is 4.7 % above the 14500 run's finest (5.767 nH),
more than its own two-mesh difference of 2.7 % says. A difference between two meshes is not an error bound. The default
run reports `ok` because its two finest meshes differ by 2.7 % (threshold 3 %); read it as the same tier of evidence
as the 14500 run, one step less converged. The converged value is not known; the series falls by about 1 % per step
in the last steps. The values are 0.58 to 0.61 of the closed form, which has the plane shortcut of its 54 mm path.
Which of the two changes (the terminals or the vias, or the finer narrow copper and the larger window) accounts for how much of the rise was not separated.

Via currents per ampere into the port are in `measures.extract.via_currents`.

**After the topology fix (external review, defects 1 and 2).** The mesher no longer bridges copper that does not touch
(section 13.3) and the capacitor legs are made per pad. What it moved, measured with the same commands:

* Closed cases (`tests/topology.rs`, default `MeshSpec`): two rectangles of one net 0.1, 0.2, 0.4 and 1.0 mm apart read
  1.186, 1.202, 1.243 and 1.565 mOhm before (a finite resistance across copper that does not touch) and are an open
  circuit (`PortDisconnected`) now; a 0.03 mm gap, below the 0.05 mm the mesher does not go under, is the same open
  circuit with `unresolved_gap` and a place in the statistics. A 0.8 mm trace 20 mm long over a plane 0.2 mm below,
  shorted to the plane at its far end, reads 3.548 nH at 100 MHz over the plain plane, 3.583 nH over a plane with a
  0.4 mm x 6 mm slot under it before (the slot was bridged) and 4.335 nH after (4.876 nH with a 1.0 mm slot): the return
  current goes round the slot. A 0.15 and a 0.25 mm neck of copper stay connected (1.19 and 1.31 times the neck's own
  `rho l / (w t)`). The slot case of `tests/edge_rule.rs` (`study_bend_and_slot_series`, ignored, 8.7 minutes, run once
  after the change) reads what it read before: slotted 4.0617 / 4.0617 nH, slot excess +1.0621 nH at both pitches (its
  slot is resolved by the pitch, so nothing was bridged); the analytic tests are unchanged.
* `emi_buck` placed (and `emi_buck_filter`, the same hot loop): L 4.140 / 3.901 / 3.883 nH at 1 / 10 / 100 MHz, as before to
  the last digit; R(10 MHz) 12.892 to 12.893 mohm; the numerical sensitivity 0.52 / 1.43 / 1.69 % and the status `ok`
  are unchanged. The finest mesh has 6787 filaments and 3731 nodes (6816 and 3751 before: one round of local refinement
  where two pieces of copper shared a cell); the coarser meshes are the same 2991 and 1673 filaments. Dropped filament
  bodies (the old mesher's own count): 192 / 140 / 107 before, 198 / 140 / 107 after, and none dropped for a gap in the
  copper along them (`gap_bars_dropped` 0): thin copper that is connected is not cut. Routed (the CLI test's tracks),
  the three meshes are identical before and after: 4855 / 2560 / 1398 filaments, 89 / 70 / 56 filament bodies dropped.
  The topology check reports 5 pieces of copper and 5 network components.
* CopperOrgan (`--run build/ci/emi-check-20261111T161807 --extract`, default budget 9000). Before the fix: L 6.506 / 6.039 /
  6.006 nH, R 23.8 / 27.6 / 30.4 mohm, matrix at 10 MHz [[6.039, 0.852], [0.852, 1.883]], filaments 8158 + 7621 + 7365, status
  `ok`. First version of the fix (all-or-nothing refinement): 6.599 / 6.142 / 6.109 nH, status `error` (5 pieces of copper cut
  apart, 23 gaps), the extraction not used in `EMI-CONDUCTED`. **With the prioritised refinement: L 6.632 / 6.175 / 6.139 nH,
  R 24.1 / 28.0 / 31.2 mohm, matrix at 10 MHz [[6.175, 0.851], [0.851, 1.908]], status `indicative`, no topology mismatch (16
  pieces of copper, 16 network components; 14 and 14 once the check also counts what the terminals and shorts join, below)**, filaments 8998 + 8999 + 7275, the finest mesh at the pitch multiplier 6.27
  (7 coarsening steps) after 4 refinement rounds with 73 refinements applied and 85 left out (counted over the rounds), 31 gaps
  reported unresolved (`unresolved_gap`, `refinement_stopped_by_budget`: cells of 0.25 to 1.9 mm, the nearest to the port
  first have been refined), numerical sensitivity 2.3 / 2.4 / 2.7 % (earlier step 5.3 / 6.1 / 6.4 %: the third mesh, with no
  refinement, is coarser than the budget needs). The direction against the old numbers is the physical one (filaments across
  antipads and gaps are gone; +2.1 % at 10 MHz); the matrix is used in `EMI-CONDUCTED` (C_BIN2 extracted 1.908 nH, mutual 0.851 nH,
  tag `extraction_indicative`) and the fundamental at 597 kHz reads 95.48 dBuV as before. The command takes 3 min 28 s. For
  comparison, `--extract-budget 14500 --extract-single-mesh` (finer pitch, multiplier 2.86, 12 361 filaments, no gap left, 16 / 16
  components) reads 6.278 / 5.767 / 5.733 nH: the default-budget value is 7 % above it, as the old default mesh was 4.7 % above the
  old 14 500 run; a difference between two meshes is not an error bound. The reason the topology could be repaired at 9000
  only at a coarser pitch: the repairs (about 50 to 90 dropped filaments next to the cut parts, refined to cells of 0.25 to 0.5 mm)
  cost about 900 to 1 600 filaments on top of the base mesh (8148 at multiplier 4.83), which is above 9000 there; the base mesh falls
  only to 7277 at multiplier 13.8 because terminals, vias and the loop's narrow copper keep their resolution.
* Connection legs (`tests/connection_legs.rs`): `emi_buck` with CIN2 routed to the hot capacitor's pads and CIN3 on the board
  without copper to the hot node. By default (no bars) asking for a port on CIN3 gives it none: the matrix has the ports
  `hot loop` and `CIN2` only, CIN3 is listed in `unconnected_capacitors` with its two pads, and the hot-loop self term, CIN2's self
  term and their mutual are equal to 1e-9 relative at 1, 10 and 100 MHz to the same board with no port requested on CIN3
  (the window and the filament count are the same too). Before this, asking for the port widened the window and moved the
  hot-loop/CIN2 mutual from 0.318 to 0.298 nH and the hot loop's own term from 5.48 to 5.04 nH. With the explicit option
  `connection_legs`, CIN2's self inductance at 10 MHz is 1.468 nH without CIN3 and 1.469 nH with it (3 % allowed), no bar is
  synthesized for CIN2 and one for each pad of CIN3 (5.3 mm each); forcing bars on every terminal, as the first retry did
  (measured with the first version of this change), gave CIN2 1.036 nH against 1.465 nH (-29 %).

### 13.9 What is integrated and what is not

Integrated: `copper emi-check --extract` (hot-loop interconnect impedance beside the closed-form and full-wave values,
with its own measure block, status, flags, numerical sensitivity, parameter assumptions and omissions), and its
use in `EMI-CONDUCTED`: the other capacitors of the hot net get a port each in the same extraction, and the matrix
becomes the connection inductances and mutual inductances of the conducted circuit (section 10, tag
`extracted_connection_inductance`). Realised by `copper_drc::emi::emi_check_with_connections`, which the `copper` crate
calls with the matrices and whose conducted findings and coverage entries replace the default ones of those cells.
Not integrated: (1) capacitors on the supply side of an input filter (`supply_capacitors`) keep the estimate: the node
they hang on has no copper closure in the extracted window (the hot capacitor closes the hot loop and the other
hot-net capacitors; nothing closes the filter's supply side), so a port across them has nothing to be measured
against; `examples/emi_buck_filter` therefore has an extracted hot-loop inductance and an estimated `CUP`. (2) A capacitor that no copper joins to the hot capacitor's pads (a placed board without tracks, or one pad
unrouted) gets no port and no bar, is named in `unconnected_capacitors` and keeps its pad-distance estimate with its
estimated provenance in `EMI-CONDUCTED` (section 10); it is decided before meshing from the copper's own connectivity (the
polygons of the net joined by vias), per capacitor. The explicit option `BoardOptions::connection_legs` (off; the CLI does not
set it) adds a straight bar from pad centre to pad centre for each pad that needs one, flag `connection_legs`, each bar in
`synthesized_connection_legs`: the connection of the estimate, not copper that exists, which can cross another net's pads, and the
capacitor's row is still not used. (3) The resistance of the interconnect and the
components' own parasitics (ESL, ESR) stay as they were: the matrix is read at 10 MHz and used as a constant inductance
over 150 kHz to 30 MHz. A FastHenry input deck of a mesh is written by `copper_extract::fasthenry::deck` and an ignored
test runs `fasthenry` when it is on the PATH; none is installed here, so no comparison with FastHenry exists.

### 13.10 Convergence across the whole matrix

The mesh series (the finest mesh and two coarser by 1.5 in pitch) is judged on **every entry of the L matrix and of the R matrix at
every frequency**, not on the hot loop's self inductance alone (`copper-extract/src/convergence.rs`,
`measures.extract.convergence`; the old keys `numerical_sensitivity_pct` and `_earlier_pct` stay). The change of an entry
between two meshes is, in percent, a diagonal entry relative to itself and an off-diagonal entry (a mutual can be near zero)
relative to the geometric mean of the two diagonal entries of the finer mesh. `finest_vs_next` is the two finest meshes,
`next_vs_earlier` the two coarser ones; `worst_l` and `worst_r` name the entry (matrix, ports by label, frequency, change) that
moved most between the two finest. `ok` needs every entry of L within 3 % and every entry of R within 5 % at every frequency;
otherwise the status is `indicative` and the reason names the worst entry ("R of hot loop at 100 MHz changes by 12.3 %").

**Why 5 % for R, and what it costs.** The extraction models each layer as one filament across the thickness, so the current
crowds laterally to the edges of the copper over the length `(rho / t) / (2 pi f mu0)` (the crowding length: 62 um at 1 MHz,
6 um at 10 MHz, 0.6 um at 100 MHz for 35 um copper; printed per frequency as `crowding_length_um`), and a pitch that is large
against it cannot resolve the crowding: R rises with every refinement. The analytic two coupled bars of
`tests/convergence.rs` (0.6 mm wide, 62.5 / 125 / 250 um) show it: between the two finest meshes R of a bar changes by 0.18 %
at 1 MHz, 2.4 % at 10 MHz and 20.6 % at 100 MHz, while L changes by 0.03, 0.03 and 0.27 % and the mutual L by 0.003 %. 5 % is
about twice the 10 MHz value of that case and is applied at every frequency; it was not chosen to make a board pass. On the
boards it means that R at 100 MHz (and at 10 MHz) is reported as not converged: `emi_buck`: L changes 0.5 / 1.4 / 1.7 % but R
0.4 / 8.0 / 12.3 % at 1 / 10 / 100 MHz between the two finest meshes (0.04 / 3.5 / 3.9 % between the two coarser), so its status is
`indicative` (it was `ok` while only L at 10 MHz was judged); the 4.140 / 3.901 / 3.883 nH are unchanged. R at 100 MHz of this
model is a lower bound that rises with refinement.

**A change that grows with refinement.** A small change between the two finest meshes is an error estimate only when the
changes shrink from step to step. Where both steps are comparable, the worst entry of each matrix whose change between the two
finest meshes is larger than between the two coarser ones (and above a third of its threshold, below which both are noise) is
named in `not_contracting` and is a status reason, even when it is under the threshold. On the analytic bars R contracts at
10 MHz (9.4 % then 2.4 %) and grows at 100 MHz (16.7 % then 20.6 %); L contracts at every frequency. On `emi_buck` both grow:
L of the hot loop 0.5 % then 1.7 % at 100 MHz (0.16 then 0.52 % at 1 MHz, 0.47 then 1.44 % at 10 MHz) and R 3.9 % then
12.3 %. Its L is therefore within 3 % between the two finest meshes but **not shown to be converging**; the finest mesh is the
only one of the three with a round of local refinement, which may be what moves it (not separated).

**Comparability.** A series whose meshes are not comparable does not report a clean figure: the sensitivity is `established:
false` with reasons when the filament ratio of the two meshes is below 1.10 (same density squeezed against one budget: the
8998 and 8999 filaments of CopperOrgan at the default budget), when their fine pitches differ by less than 1.2, when one
mesh's refinement was cut short by the budget and the other's was not, or when they leave different numbers of gaps unresolved or
see different numbers of copper pieces. Then the status is `indicative` with `mesh convergence is not established: ...`. A local
refinement that finished in one mesh and was not needed in the other is not a difference (emi_buck's finest mesh has one round).
CopperOrgan at the default budget: L changes 2.3 / 2.4 / 2.7 % and R 3.0 / 0.4 / 5.1 % at the hot loop between its two finest
meshes, but the series is **not established** (filament ratio 1.00, 31 against 23 unresolved gaps), and the second step is not
comparable either (refined against not refined, 14 against 11 copper pieces).

### 13.11 The independent reference solver (`--extract-reference`)

`copper-extract/src/reference.rs` is a second, independent opinion on the same physical geometry. It starts from the `Layout`
(the copper polygons per layer and net, heights and thicknesses, vias, terminals, shorts, ports) and shares nothing with
`mesh.rs`, `network.rs` or `partial.rs` (not the mesher, not the Hoer-Love partial elements): only the polygon library, the
radix-2 FFT of `copper-emi` and the `Layout` type. It exports no mesh, so it cannot validate a mesher by repeating its mistakes.

* **Raster and topology.** A uniform raster of every layer (pixel centres inside the net's polygons, the pitch from a budget of
  1.6 million pixels; two nets that overlap in a layer get a slot each at the same height); connectivity by flood fill on
  4-neighbours, vias joining between layers the pixels of their net inside the contact circle (drill radius plus plating, an
  equipotential: a single pixel would add a spreading resistance that grows with the logarithm of the pitch), terminals as
  the pixels under their region (an equipotential; a sliver thinner than a pixel takes the net's pixels within one to three
  pixels, of the one piece of copper nearest its centre only), shorts joining terminals. It
  counts the connected components and says for every port whether it is open. Components of four pixels or fewer and slivers in
  the outermost row or column of the window (which the mesher clips away) are not counted as pieces of copper in the comparison.
* **DC conduction.** Sheet conductance `sigma t` between neighbouring pixels, via barrels as tube conductances, terminals and shorts as
  one unknown, one conjugate-gradient solve per port (1 A in, 1 A out): `R_ij` and the current on every edge. A solve that
  does not reach a residual of 1e-10 fails the reference (`reference.status: failed`, a status reason) instead of returning
  a value.
* **DC inductance from the field energy.** `L_ij = (mu0 / 4 pi) sum I_a I_b (1 / h^2) integral integral 1 / |r - r'|` over the current
  cells (the edges between neighbouring pixels, each a cell `h x h` with the uniform sheet current `I / h`), evaluated per pair
  of layers by FFT convolution with the exact kernel (the same-cell self term `2.9732 h`, Gauss-Legendre for the next cells, the
  point formula beyond; the via barrels as filaments with the exact parallel-filament integral and the tube radius as the self
  distance). Each layer is a sheet at its height: the thickness enters the conductance only, which makes the inductance of a
  bar about `ln((w + t) / w)` of the logarithm higher than the bar's, 1 to 2 %.
* **It is the DC limit** (current distributed by resistance): it does not know skin effect, proximity effect or the inductive
  redistribution of current with frequency. It validates the copper (topology, vias, terminals, window clipping) and the mesh at
  low frequency; it does not validate the frequency dependence. The extraction at 1 kHz is the comparison (100 Hz gives the same
  to 1e-4: `w L` is far below `R`).

**Validation of the reference itself** (`tests/reference.rs`, `tests/reference_topology.rs`, all pass in under a second to a few
seconds):

| Case | Reference | Closed form or other | Difference |
| --- | --- | --- | --- |
| straight bar 10 x 1 x 0.035 mm, pitch 200 / 100 / 50 um, R | 4.4229 / 4.6686 / 4.7914 mohm (the length between the pixels of its terminals) | `rho l / (w t)` | < 1e-6 (exact) |
| the same, L | 6.1655 / 6.6059 / 6.8296 nH | `self_inductance_bar` of the same length | +1.00 / +0.96 / +0.97 % |
| rectangular loop 10 x 6 mm, trace 0.5 mm, pitch 250 / 125 / 62.5 um | 20.699 / 20.757 / 20.791 nH | partial-inductance closed form 20.629 nH | +0.34 / +0.62 / +0.79 %; the change per halving falls 0.058 to 0.034 nH |
| two such loops 2 mm apart (125 um), mutual L | -0.5468 nH | -0.5460 nH | +0.15 % |
| the same, self L; mutual R | 20.757 nH; 0 | 20.629 nH; no shared copper | +0.62 %; exact |
| two bars 10 x 0.6 mm, 1.5 mm apart (50 um), L matrix | [[7.107, 2.497], [2.497, 7.107]] nH | PEEC at 1 kHz [[6.957, 2.475], [2.475, 6.957]] nH | +2.15 % / +0.88 % |
| the same, R | 7.412 mohm | PEEC 7.371 mohm | +0.55 % |

The trace over a plane with its return (the loop over a plane of `tests/analytic.rs`) has no closed form here: at DC the return
current spreads through the plane by resistance instead of lying under the trace, so the image-plane result does not apply; the
DC comparison with the extraction is the check (the boards below).

**Topology cases** (the cases of `tests/topology.rs`, the reference's pitch half the gap so that a pixel centre falls in it):
slots of 0.03, 0.1, 0.2, 0.4 and 1.0 mm between two rectangles: open for both, two pieces for both; a 0.15 and a 0.25 mm neck:
connected for both, one piece, the reference R 8.389 / 5.612 mohm against the extraction's 7.816 / 5.161 mohm (the extraction
7 and 8 % lower at its default mesh); a terminal at a gap of 10, 20, 30 and 50 um: open across the gap, connected on its own island,
for both; a via 0.7 mm beside an island: joins nothing for both, and joins it when it stands on it. Every verdict agrees.

**Board comparison** (the hot-loop layout of the extraction, same window and ports; `measures.extract.reference`; differences are
reference minus extraction at 1 kHz, a diagonal entry of the extraction's value, an off-diagonal one of the geometric mean of the
two diagonals; the limit for a status reason, flag `reference_disagrees`, is 10 %):

| Board | Pitch, pixels, time | R (mohm), reference / extraction, difference | L (nH), reference / extraction, difference |
| --- | --- | --- | --- |
| `emi_buck` | 18.8 um, 381 000 copper pixels, 38 s | 7.510 / 7.586, -1.0 % | 6.905 / 6.763, +2.1 % |
| CopperOrgan, hot loop | 38.7 um, 826 000 copper pixels, 6 slots, 273 s | 16.86 / 18.61, -9.4 % | 10.32 / 10.45, -1.2 % |
| CopperOrgan, mutual hot loop - C_BIN2 | | 3.200 / 3.327, -1.2 % | 1.587 / 1.612, -0.5 % |
| CopperOrgan, C_BIN2 | | 5.451 / 5.906, -7.7 % | 2.823 / 2.864, -1.4 % |

What the comparison establishes, and what it does not:

* **Topology agrees** on both boards (no port open, the same number of pieces of copper after the stated exclusions: one
  sliver on `emi_buck`, one component of one pixel on CopperOrgan). The exclusion rule was written after seeing those two
  extra components; it has not been tried on other boards.
* **L agrees within 2.1 % in every entry**, self and mutual, on both boards. The reference's sheets make its L 1 to 2 % high
  by construction; the reference's own L moves 3.3 % (`emi_buck`) and at most 2.3 % (CopperOrgan) when its pitch is doubled.
* **R agrees only to about 10 %, and that figure is not established more tightly.** The reference's own R moves 8.2 % on
  `emi_buck` and 5.3 / 2.9 / 7.9 % on CopperOrgan when its pitch is doubled (the port terminals are slivers 10 um thick, and
  three of CopperOrgan's terminals are thinner than a pixel and are taken at the nearest copper), and R depends on how a via
  meets the copper: with the via on a single pixel instead of its contact circle the same runs read +0.2 % (`emi_buck`), -4.4 %
  (CopperOrgan hot loop) and +7.6 % (C_BIN2), a swing of 5 to 15 %. The contact circle is the model kept (a single pixel adds a
  spreading resistance that grows as the pitch falls). With it the extraction's R at 1 kHz is 8 to 9 % above the reference on
  CopperOrgan, just inside the 10 % limit; which of the two is closer to the truth is not decided here.
* The necks of the topology cases show the same sign: the extraction's R is 7 to 8 % below the reference there at its default
  mesh. Not investigated.
* The command also runs the reference at twice its pitch and prints the sensitivity per entry
  (`reference_sensitivity_*_pct`). The comparison is not established (flag `reference_not_converged`, a status reason
  naming each cause) when an entry moves by more than the limit, when an entry has no finite value at one of the two
  pitches (an open port gives no value, and a missing value is a failure, not a pass), when a port is open at one pitch
  and connected at the other, when the two pitches count different numbers of pieces of copper, or when the coarse run
  fails. A neck of 120 um is the test: a 100 um raster connects it and a 200 um raster cuts it. On `emi_buck` at a coarse pitch the reference's
  R is far off (+15.6 % at 61 um, the case of `tests/boards.rs`).
* A disagreement beyond 10 % in an entry is a status reason (flag `reference_disagrees`). A topology disagreement (a port open for the reference and solved by
the extraction, or different numbers of pieces of copper) makes the status `error` and the flag `reference_topology_disagrees`.

## 14. Signal sources, channels and spectra (P1 of the signal and cable plan)

Status: implemented. This section is the specification of the first phase of
[`emi-signal-cable-plan.md`](emi-signal-cable-plan.md); nothing here finds a return-path break, converts a mode or
estimates a cable current (phases P2 and later). It describes what `EmiModel::signals` holds, how a signal's channel to the
cable connector is walked, which defaults fill a missing edge or swing, and the three different spectrum quantities of
a data signal. Every number is screening evidence.

### 14.1 What is a signal source

`EmiModel::signals` (`crates/copper-lang/src/emi_signals.rs`, key `signals` of `copper query BOARD emi`, present only when
there are sources) has one entry per fast net, or per differential pair keyed by its P net (`partner` is the N net). A net is
a signal source when it has the `fast_edge` class (a driving pin with `rise_time` or `fall_time` below 2 ns, or a routing
constraint `kind = clock`) or when a driving pin on it declares `data_rate`, `clock_frequency`, `signal_swing` or
`spread_spectrum`. A *driving pin* is an output or bidirectional pin without an `emi_role` (the pins of switching
cells are not signal sources). The net classes themselves do not change: a net that only declares a rate is a source but
not a `fast_edge` net, and `fast_edge` in the entry says which.

A pair comes from `constraint routing(NET) { kind = differential; partner = OTHER; }` on either member. The pair is
dropped (both nets stay single-ended) when the partner names a different net or a third net names either member. The P
net is the one whose name has `P` or `+` where the two names first differ, against `N`, `M` or `-` (`USB_DP`/`USB_DM`,
`TX_P`/`TX_N`, `A+`/`A-`); when the names do not say, the first in name order.

Each number (`data_rate`, `clock_frequency`, `rise_time`, `fall_time`, `signal_swing`, `spread_spectrum`) is the first one
declared by a driving pin of the source, P net first, with that pin as its provenance (`declared_on`). `rise_time` and
`fall_time` default to each other as for any pin. Nothing is invented in the model: a missing number is `null`.

### 14.2 Defaults

`crates/copper-drc/src/emi/defaults.rs` and `crates/copper-drc/src/emi/signal.rs` fill a missing number when a phase needs it
(declared values always win; a default carries a `rule_of_thumb_*` tag, its band and its rule):

| Number | Default | Tag |
|---|---|---|
| Edge, source declares `data_rate` | 0.25 UI nominal, band 0.2 to 0.35 UI (10-90 %) | `rule_of_thumb_signal_edge` |
| Edge, no rate (a clock) | 1 ns nominal, band 0.3 to 3 ns | `rule_of_thumb_signal_edge` |
| Swing, differential | 0.2 V nominal, band 0.1 to 0.4 V (peak-to-peak) | `rule_of_thumb_signal_swing` |
| Swing, single-ended | 1.0 of the driver's supply, band 0.8 to 1.0; none without a supply voltage | `rule_of_thumb_signal_swing` |

The only published relation used is `B = 0.35 / t_r` between a 10-90 % rise time and a single-pole bandwidth (Johnson and Graham,
*High-Speed Digital Design*, 1993; Bogatin, *Signal and Power Integrity - Simplified*). The 0.2 to 0.35 UI band, the clock band and
the swing bands have no published source known to the author (the differential swing band is on the scale of the LVDS
and MIPI D-PHY output levels, as remembered and unverified), and the doc comments say so. In this phase no model code supplies the driver's supply
voltage, so a single-ended signal with no `signal_swing` stays without a swing.

### 14.3 The channel walk

A signal is one channel from its driver pin to the first cable connector, not one net. `external_connectors` (used for the
net classes and the existing checks) reaches a connector directly or through one resistor or ferrite and is unchanged. The
channel walk is a separate function used only by signals. Starting at each member net it follows, breadth first (the fewest
parts, nets and parts in board order):

* two-pin series parts, any number in a row: resistors, ferrites and `dc_power_path` parts as `is_series_element` recognises
  them (kind `series_part`), and capacitors in series with the signal (kind `coupling_capacitor`, AC coupling);
* parts with more than two pins across their declared `passes_to` pairs only (kind `declared_pair`); an entry pin of a pair
  passes to its partner whichever side declared the pair;
* never into a declared supply rail or a net with a ground pin. A capacitor to such a net is a shunt: it is not a hop and
  not a refusal. A resistor, ferrite or declared pair into one is refused and recorded;
* it stops at the first net that has a connector (a part with `external_cable = true`, or named by an `emission` constraint)
  and records the connector pin.

Two-pin parts that are none of these (inductors, diodes, ...) are not followed. One that is in series with the channel (its
other pin on a signal net) is recorded as a stop, `unsupported_series_part`; one to a supply or ground net is a shunt (a
protection diode) and is not recorded. Every entry of
`channel` is a hop (`component`, `kind`, the pins and nets crossed, `swaps_polarity`), in order from the driver. When no
connector is reached the member's `end` is `open` with the list of `stops`: each refusal with the pin, the net and the
reason (`no_passes_to`: a part with more than two pins and no declared pair for the pin the walk arrived on, which is
every receiver IC on the way; `supply_rail`; `reference_net`; `unsupported_series_part`). An open end with no stops means the nets simply end.
Stops are listed only when nothing was reached.

P and N identity: each member is walked on its own, so P stays P across every declared pair by construction. A part
swaps polarity when the net the P member leaves it on is, by name, the pair's N net (the same name rule as for the key,
applied to the two nets at the same hop). Both members' hops are flagged `swaps_polarity`. Pairs whose net names do not
say P from N are never flagged; a physical swap in the wiring is therefore reported only as far as the names reveal it. The
ordered hops are what the later phases use for coupling capacitors and chokes.

### 14.4 Spectrum of a signal

`crates/copper-emi/src/signal.rs`. Conventions: `swing` is the peak-to-peak value of a signal that sits at `+-swing/2`;
amplitudes are single-sided peaks (a sine of amplitude `a` has power `a^2/2`); an edge is a linear 0-100 % ramp centred on the bit
boundary. Declared `rise_time` and `fall_time` are 10-90 % times; the ramp passed to these functions is the 10-90 % time
divided by 0.8 (`ResolvedSignal::rise_ramp` / `fall_ramp` in `crates/copper-drc/src/emi/signal.rs`), the same conversion the
conducted model applies to a switch's edge times.

A **clock** has lines at multiples of `clock_frequency` from `trapezoid_harmonic` (`clock_lines`); `trapezoid_envelope` is a
continuous envelope over those lines, not a line spectrum. A **data signal** has three different quantities, each labelled;
none is a bound on another:

1. **`pattern_lines`**: the exact line spectrum of one named repeating pattern: alternating (`1010`, lines at odd multiples
   of `data_rate / 2`) and the square patterns with runs of 2, 4 and 5 bits. Each boundary with a change contributes a ramp of
   height `+-swing`, so `c_m = (1 / (j w P)) sum_k a_k sinc(pi f t_k) e^{-j w k T}` and the line is `2 |c_m|`. A longer pattern has
   lines where a shorter one has none: at 2.5 Gbps `11110000` has a line at 312.5 MHz where `1010` has none (its lowest is 1.25 GHz), so
   no pattern bounds the others and a finding lists which patterns were evaluated.
2. **`continuous_bound`**: `(2 swing / pi) min(1, 1 / (pi f t_e))`, an upper bound on any single line of any periodic
   two-level waveform with that swing and edge time. (a) With `|x| <= swing/2` a component along `cos(w t + phi)` is largest for
   `x = (swing/2) sign(cos(...))`, giving `swing / pi` and a line of `2 swing / pi`, reached by the square wave of that frequency. (b) A
   linear edge is a step smoothed by a rectangle of width `t_e`, so for equal rise and fall every line is multiplied by
   `sinc(pi f t_e)`, and `|sinc x| <= min(1, 1/|x|)`. The proof is for equal rise and fall times; for unequal edges the bound uses the
   shorter edge, which the tests check on random sequences but which is not proved. It is loose by construction: it is
   not reached by one pattern at every frequency and says nothing about how many lines there are. It holds for a
   periodic extension of any record, not for a spectral density.
3. **`random_data_in_rbw`**: random NRZ data has a continuous power spectrum, not lines. The two-sided density is
   `(swing/2)^2 T sinc^2(pi f T) sinc^2(pi f t_e)`; the power in a resolution bandwidth `B` expressed as the amplitude of a sine of equal power
   is `swing sqrt(B / data_rate) |sinc(pi f / data_rate)| |sinc(pi f t_e)|`. At 2.5 Gbps in 120 kHz this is `sqrt(120 kHz / 2.5 GHz)`, 43.2 dB
   below the alternating pattern's lines at any swing and edge time. `B` is a rectangular power bandwidth much narrower than the
   data rate; the Gaussian IF filter's noise bandwidth (about 1.06 times its 6 dB bandwidth) and the detector are not applied.

Checked in the tests against an independent numerical spectrum: a long sampled waveform written directly in the
time domain and transformed by the DFT of `copper_emi::spectrum`, for the named patterns (line by line), the clock lines, the bound (named patterns
and 30 random sequences of 16 to 512 bits at seven edge settings, equal and unequal) and random data (band power of a 65536-bit sequence).

### 14.5 Not modelled

Whether a link scrambles its data (unscrambled data repeat and put power into lines, scrambled data tend to the random case: not declared
in this phase); jitter, pre-emphasis, de-emphasis and multi-level signalling (PAM4); spread-spectrum clocking (`spread_spectrum` is recorded only); a
driver's source impedance and the loading that makes `signal_swing` a loaded voltage; any detector
weighting; reflections inside parts on the channel; and every consequence of the channel for the board (return-path breaks, mode
conversion and the cable are later phases).

## 15. Return paths: `EMI-RETURN-PATH` (P2 of the signal and cable plan)

Status: implemented. This section is the specification of the second phase of
[`emi-signal-cable-plan.md`](emi-signal-cable-plan.md); code in `crates/copper-drc/src/emi/return_path.rs`, example in
`examples/emi_return_path/`, tests in `crates/copper-drc/tests/emi_return_path.rs` and
`crates/copper/tests/emi_return_path_cli.rs`. It measures where the return current of a fast signal has no reference copper
under its trace. It reports distances, lengths, a half-wave resonance and a ranking score; it never reports a field, a
margin, a pass or a fail.

### 15.1 When it runs and what it needs

The check needs the routed board and the filled reference planes. `copper emi-check BOARD --run RUN` supplies the routed
`PhysicalBoard` (section 8); the fills come from the native zone fill of that board (`copper_fullwave::fill::zone_fill_set`,
the same export-parse-fill the extraction and the full-wave model use, so all three see the same copper) and are passed into
`copper_drc::emi::emi_check_with_return_path` as `ReturnPathInput::Fills`. `copper-drc` cannot fill zones itself: the fill code
lives in `copper-cam` and is reached through `copper-fullwave` and `copper-extract`, which depend on `copper-drc`. The fill is
made only when the board has a subject net (below), because a zone fill is not instant. Without `--run` the input is
`NotRequested` and nothing changes: the output of `emi-check` on a board without subject nets is byte-identical to before this
phase, and on a board with subject nets the coverage entry says so (below). `emi-check` takes no new option. The section that `route-board` embeds in its report (`emi_section::check`) prepares the same input from its final board, so `emi-check --run RUN --write` still reproduces it bit for bit; a board with a switching cell and a subject net therefore has its zones filled once more at the end of `route-board`, inside the guard that turns any failure of the check into the section's error record. On a board with a switching cell and no subject net the embedded section's `EMI-RETURN-PATH` coverage entry is now `not_applicable` instead of `unsupported`.

### 15.2 Subjects

Every net with a signal source (`EmiModel::signals`, section 14) and every net of a `routing` constraint of kind
`differential`, `clock` or `rf_feed` (the set `signal_integrity::needs_reference_plane` uses). The members of a differential
pair are one subject: one finding and one coverage entry, named after the P net, with the members listed in `nets`. A subject
that has only a routing rule and no signal source takes its edge time from the default band (section 14.2, tag
`rule_of_thumb_signal_edge`); with no swing it gets no ranking score.

### 15.3 Reference planes

A reference net is a declared supply (`supply NAME { net = ...; }`, ground included). Its filled copper on a layer is a
reference layer. For a signal layer the reference plane is the nearest reference layer above or below it, by the
centre-to-centre height `H` of `Stack::height` (the convention of the hot-loop estimates; the dielectric height alone is
smaller by half the two copper thicknesses). At equal height a layer with a ground net wins, then the layer higher in the
stack-up. The nearest reference layer on the other side of the signal layer is listed as `reference[].other_plane` with
`evaluated: false`: a stripline is evaluated against its nearer plane only. A layer with copper and no reference layer at all
is listed under `layers_without_reference_plane`, all of its copper counts as uncovered, and it is a finding. The reference
copper of a layer is the union of its reference nets' fills, so the gap between a ground and a supply pour on one layer is a
void like any other; a trace that passes from one pour to the other on one layer is not reported as a reference change (not
modelled).

### 15.4 Coverage

For each routed segment (tracks as drawn, arcs as chords with 5 um sagitta) the footprint is widened by `3 H` each side
(`defaults::RETURN_WINDOW_H`) into a strip with flush ends and swept against the reference copper with integer-nanometre
polygon operations. The uncovered length is the length of the centre line along which that cross-section is not entirely
covered: the stations of the strip minus the reference copper, projected on the segment. Per segment, per layer and per
member it is reported in millimetres (`uncovered_length_mm`) and together with the routed length. The cross-section is a strip,
not the trace alone: a trace running along the edge of a cut within `3 H` of it is uncovered for the whole run.

### 15.5 Slot crossing

The voids of a reference layer are the connected pieces of its bounding box with no reference copper (the window is that
bounding box: the connected hole of the plane is clipped to it, so a cut that runs to the plane's edge ends at the window).
Every void that the widened strip meets is a gap; both members of a pair that cross the same void are one gap with two
`crossings`. For each gap:

* the slot length `L` is the extent of the void's outer contour along the in-plane direction perpendicular to the first
  crossing segment (`slot_length_mm`): the plan's definition, not the principal axis of the void (a slanted slot is
  measured across the trace, not along itself);
* the resonance is `c / (2 L sqrt(eps_eff))` (`resonance_hz`) with `eps_eff` of the trace layer as the impedance screening
  computes it from the declared stack-up (`signal_integrity::effective_permittivity`: Hammerstad's effective permittivity of a
  microstrip, the plane-spacing weighted permittivity of a stripline; without a declared stack-up the assumed permittivity
  and the microstrip formula);
* `kind` is `cut_to_plane_edge` when the void reaches the window (a cut or a notch from the plane's edge) and `enclosed_void`
  otherwise; `via_antipads_in_void` counts the board's vias that pass through the layer inside the void. The antipads of a
  row of vias whose clearances merge are one void, hence one slot (tested with five vias 0.8 mm apart); a single antipad of
  another net is a gap too, with its diameter as length;
* the antipad of the subject's own via is not a gap: it is how a layer transition looks in a plane and is examined as a
  reference change (15.6). A void holds exactly one via, a via of the subject, and is at most three via diameters long.
  Such a void is closed for the coverage and the gap test.

### 15.6 Reference change at a via

For each via of a subject net the layers where a segment of that net ends within the via's radius are taken in stack-up order;
for each consecutive pair the reference (plane layer and, from the nearest reference-net fill at the via, its net) before and after
is compared. Different layer or net is a change. The nearest return path:

* same net on two plane layers: a via of that net whose span covers both reference layers (`return_span_covers`, the helper of
  the native rule `require_return_vias`, now shared), by centre distance (`return_path = stitching_via`);
* different nets: a capacitor with a pad on each net, by the distance from the via to the nearer of its pad centres
  (`decoupling_capacitor`; `return_element` is its reference);
* a side with no reference plane: `none`.

The threshold is the net's declared `maximum_return_via_distance` when a routing rule of the subject has one
(`threshold_source = declared maximum_return_via_distance`), else `lambda / 20` at the knee `0.35 / t_r` (`t_r` the
shorter of the 10-90 % rise and fall times, `lambda = c / (f sqrt(eps_eff))`, the larger `eps_eff` of the two layers; tag
`rule_of_thumb_return_via_distance`, `threshold_source = lambda_over_20_at_knee_frequency`). The native rule is
unchanged. It stays a pass-or-fail check for single-ended nets that declare `require_return_vias`; this check runs on every
subject, includes pairs, and reports the distance. For a via of such a net the same test (`return_via_covered`, shared) says
whether the DRC rule fails it (`drc_rule.satisfied`); a via that fails is listed with its distance and `reported_by_drc`, and the
message says that the rule reports it. The rule also fails vias that change no reference; this check says nothing about those.

### 15.7 Plane edge and board edge

Each segment is sampled every 0.25 mm at most. The distance to the outer contours of the reference copper (holes are gaps, not
edges) and to the board outline is compared with `20 H` (`defaults::EDGE_DISTANCE_H`): the smallest distance with its position
(`nearest_mm`, `at_mm`), the length within the limit and `within_limit`. On an outer layer, the length within `20 H` of the board
edge with no stitched ground via on the edge side is `outer_layer_unguarded_edge_length_mm`: a guard is a via of a reference net
whose span covers the signal layer and the reference plane, lies within `20 H` of the sample and is closer to the board edge than
the sample is. One via guards the samples within `20 H` of it, so this is a coarse test of a ground fence, not of its pitch.

### 15.8 The finding

`EMI-RETURN-PATH`, warning severity, no cell, one per subject, present only when there is something to report: copper on a layer
with no plane, uncovered length, a gap, a reference change without a return path within its threshold, or a trace within `20 H` of a
plane or board edge. A subject with none of these has only its coverage entry, which says so ("not a pass"). `nets` lists the
members, `subjects` the driving pins and components for `emi_accept`. `measures`:

| Key | Content |
|---|---|
| `subject`, `members[]` | the P net; per member: routed and uncovered length, per layer the segment count, `plane_edge` and `board_edge` (`nearest_mm`, `at_mm`, `limit_mm`, `within_limit`, `length_within_limit_mm`), `outer_layer_unguarded_edge_length_mm` |
| `routed_length_mm`, `uncovered_length_mm` | totals over the members |
| `reference[]` | per signal layer: reference layer and nets, `height_mm`, `structure`, `effective_permittivity`, `other_plane` |
| `gaps[]` | `nets`, `signal_layer`, `reference_layer`, `position_mm`, `kind`, `via_antipads_in_void`, `crossings[]` (net, segments, `length_mm`), `slot_length_mm`, `effective_permittivity`, `resonance_hz`, `quarter_wave_resonance_hz` (null for an `enclosed_void`) |
| `reference_changes[]` | `net`, `via`, `position_mm`, `layers`, `reference_before`/`reference_after` (layer, net), `return_path`, `return_element`, `return_distance_mm`, `threshold_mm`, `threshold_source`, `within_threshold`, `drc_rule` (null, or rule, `satisfied`, `reported_by_drc`) |
| `via_transitions_examined`, `drc_return_via_reported`, `thresholds` | the count of layer transitions looked at; the vias the DRC rule reports; the three factors in force |
| `ranking` | `slot_mm`, `worst_gap_position_mm` and `spectrum` of the worst gap, or `score: null` with the reason |
| `inputs_text` | the `input:` lines of the defaults used |

Positions are `[x, y]` in millimetres in board coordinates, so CopperReview and the plots can mark them.

**Ranking score.** Where a gap exists and the signal has a swing: `20 log10(V(f_res) L / (1 V * 1 mm))` for the worst gap
(`estimate.quantity = slot_ranking_score`, unit dB, `kind = ranking_score`, tag `ranking_only`; no margin, never compared with a
limit), with `f_res` the slot's resonance, `L` its length in mm and `V` the continuous bound of the signal's spectrum at `f_res`
(`copper_emi::signal::continuous_bound` at the swing and the shorter edge ramp, the 10-90 % time divided by 0.8). The continuous
bound is the headline because it is the one of the three spectrum quantities of section 14.4 that needs no assumption about
the data pattern; `measures.ranking.spectrum` carries all three by name beside it, each with its own score: `continuous_bound`,
`pattern_lines` (the strongest line of each named pattern within one octave around the resonance, null where a pattern has no
line there) and `random_data_in_rbw` (120 kHz, CISPR 16-1-1 above 30 MHz) for a signal with a `data_rate`, and `clock_lines` for one with a
`clock_frequency`. A single-ended signal with no `signal_swing` has no swing (section 14.2) and so no score. With a default edge
or swing the estimate carries `band_db` (the score at the unfavourable and the favourable end of the bands) and `basis =
rule_of_thumb`. `not_modelled` names slot radiation efficiency, plane-pair cavity modes, enclosure and cable coupling, the
common-mode part of a differential pair (the score takes the whole signal, so it overstates a balanced pair; mode conversion is
a later phase), return current outside the window and the other plane of a stripline.

Tags: `rule_of_thumb_return_window` and `rule_of_thumb_edge_distance` on every finding (the `3 H` and `20 H` are used),
`rule_of_thumb_return_via_distance` when a threshold is `lambda / 20`, `rule_of_thumb_signal_edge`/`_swing` when a default edge
or swing feeds a number, `ranking_only` with a score. `rank` (the order among findings with no estimate) is the number of gaps
plus the number of changes without a return path plus the uncovered length in metres.

### 15.9 Coverage

`EMI-RETURN-PATH` is `executed` when at least one subject was examined, with an entry `EMI-RETURN-PATH:<net>` per subject
(`executed` with the numbers, or `unsupported` when the net has no routed copper); `not_applicable` when the board has no subject
net and a run was given; `unsupported` otherwise, with the reason: not requested (a board with no subject net keeps the entry of
before this phase word for word; one with subject nets says that a routed run is needed), the fills could not be made (the
fill's own error, for example a rule the native evaluator does not support), or no subject net has routed copper.

### 15.10 Defaults and their sources

All in `crates/copper-drc/src/emi/defaults.rs`, section 7, each with its tag, band and a statement of its source.

| Default | Value (band) | Source |
|---|---|---|
| Coverage window | `3 H` each side (2 H to 5 H) | derived: the plane current density of a thin line at height `H` over an infinite plane is `I H / (pi (x^2 + H^2))` (method of images), so the share within `k H` is `(2/pi) atan(k)`: 50 % at 1 H, 70.5 % at 2 H, 79.5 % at 3 H, 87.4 % at 5 H. No published citation is given because none was checked |
| Plane and board edge | `20 H` (10 H to 100 H) | the "20 H rule" of EMC design guides (Montrose, *Printed Circuit Board Design Techniques for EMC Compliance*), from memory and **not checked against the text: no verified citation** |
| Return via distance | `lambda / 20` (lambda / 10 to lambda / 40) at the knee `0.35 / t_r` | `0.35 / t_r` as in section 14.2; the `lambda / 20` fraction **has no published source**: a screening choice |
| Half-wave resonance | `c / (2 L sqrt(eps_eff))` | the half-wave resonance of a slot, by definition; `eps_eff` as the impedance screening computes it |
| Quarter-wave resonance | `c / (4 L sqrt(eps_eff))` | reported as `quarter_wave_resonance_hz` for a `cut_to_plane_edge` gap only: a notch open at the plane edge and closed at its other end resonates there, a cut through the plane does not. The void's shape does not tell the two apart, so the score keeps the half wave and the lower frequency is carried beside it |

### 15.11 Validation

Against hand calculations in the tests (`crates/copper-drc/tests/emi_return_path.rs`): the stack-up of the example gives `H = 0.235 mm`,
`3 H = 0.705 mm`, `20 H = 4.7 mm` and `eps_eff = 3.0844` for the 0.12 mm lanes; a pair over the solid pour has no gap and no finding; the
same pair across the 2 mm cut has one gap with `L = 76 mm` (the pour is 76 mm across), `f_res = c / (2 L sqrt(3.0844)) = 1.1230 GHz`, 2 mm
uncovered per trace and the score `20 log10(0.25465 * 76) = 25.735 dB` (swing 0.4 V, ramp 0.125 ns, so the bound is
`2 * 0.4 / pi`); an enclosed 30 mm slot gives `L = 30 mm`, `f_res = 2.845 GHz`; a row of five antipads is one void of `3.2 mm +
one antipad diameter`; a via beside which a ground via sits 1.5 mm away passes a 2 mm threshold and one with the nearest
stitching via at `sqrt(1.5^2 + 7^2) = 7.159 mm` does not, which is also what the native DRC rule reports; the default threshold is
`c / (0.35 GHz sqrt(eps) 20) = 24.0 mm` for a 1 ns edge and a tenth of that for 0.1 ns; a capacitor between two supply planes is a return
path at the distance to its nearer pad; a trace 3 mm from the board edge and 1 mm from the plane edge reports both distances against 4.7 mm and
20 mm unguarded, and none unguarded with a fence of ground vias. The defaults module tests the derived window fraction and the threshold.
None of this is correlated with a measurement.

### 15.12 Not modelled

Radiation efficiency of a slot and what feeds it; plane-pair cavity modes and the resonance of the board as a whole; the enclosure; the
inductance of the detour around a gap and current crowding at its ends; return current beyond the window; the second plane of a
stripline and the split of its return current; a reference net that changes inside one plane layer; vias in pads and transitions where the net's
copper does not end on the via; the reference plane's own gaps from other layers (a plane is judged by the single reference layer chosen); the common-mode
part of a differential pair (section 16); the effect of nearby ground copper on the signal layer (a coplanar return); the effect of a
thermal relief or a fill island of a reference net that the fill removed; and nets that are not declared supplies as references.

### 15.12 Reporting without a switching cell

The reporting chain no longer assumes a converter. The `emi` section of `route-report.json` (written by `route-board`, or by
`emi-check --run RUN --write`; section 8) is kept when the report has a cell, a finding, or an `executed` coverage entry of
`EMI-RETURN-PATH` or `EMI-RETURN-PATH:<net>`; a board with none of these gets no section, byte-identical to before. A
board with no cell has `cells: []`. `route-board` fills the zones for the check only when a net is a subject, as in 15.1.

`copper issues RUN --stage emi` lists each finding as one issue, grouped by subject kind (cells, then signals and pairs, then cable connectors; section 8). The window and
the points of an `EMI-RETURN-PATH` issue are its located causes, read from the `measures` and listed as `details.causes`
(`kind`, `at_mm`, `text`): `gap` (position, slot length, resonance), `reference_change_without_return_path` (via, nearest
return path against the threshold, or "no return path found"), and `plane_edge` / `board_edge` (the point of the trace
nearest to the edge, where it is within the limit). A finding with no located cause falls back to the pads of its nets.
`copper explain EMI-RETURN-PATH` says what was measured, that it is screening evidence and never a pass or a fail, and what is
not modelled (the list of 15.8).

`--plot-summary` draws the return-path map (`copper_report::emi_return_path`): the board in its coordinates (millimetres, y
down as the layout), the outline and the traces of the subject nets when the board scene exists, and a numbered marker at each
cause listed above: a circle at each gap, a diamond at each reference change without a return path within the threshold, a
square at each plane-edge point and a triangle at each board-edge point within `20 H`. A compact legend in a column right of
the map (below a wide board) gives one short line per number, gaps first, then vias, plane edges and board edges, at most 24
lines and then "+N more": `1  gap  LANE_B_P/N  slot 76 mm, 1.12 GHz`, `2  via  CLK_U  return 7.2 mm > 2.0 mm`,
`3  plane edge  CLK_E  1.0 mm < 4.7 mm`, `4  board edge  CLK_E  3.0 mm < 4.7 mm`. Marks of one kind within 1 mm of each other
are one mark (an edge keeps the smaller distance, both nets are listed) and a plane-edge mark within 1 mm of a gap of the
same subject is dropped, because the gap already says that the trace leaves the copper there; a gap is also drawn as a short
thick line across its trace when the traces of the subject say which way the trace runs. The mark groups keep `data-kind`,
`data-subject`, `data-x-mm` and `data-y-mm` and add `data-number`; numbers that would sit on each other are moved apart with
a thin leader to the true position. Only the `measures` of the findings are used. A finding with no located cause has no
mark: nothing was located at these thresholds, which is not a pass. The footer line of the document says what the figures
are, and each finding lists what is not modelled. The `copper emi-check --help` text names the map.

## 16. Mode conversion: `EMI-MODE-CONVERSION` (P3 of the signal and cable plan)

Status: implemented. This section is the specification of the third phase of
[`emi-signal-cable-plan.md`](emi-signal-cable-plan.md); closed forms in `crates/copper-emi/src/mode_conversion.rs`, the check in
`crates/copper-drc/src/emi/mode_conversion.rs`, example in `examples/emi_mode_conversion/`, tests in
`crates/copper-drc/tests/emi_mode_conversion.rs` and `crates/copper/tests/emi_mode_conversion_cli.rs`. It estimates how much of the
differential voltage a driver launches appears as common mode at the end of its channel. The result is a ratio of two voltages
inside the channel; it never reports a field, a margin, a pass or a fail, and it is not correlated with a measurement.

**The choke.** A four-pin common-mode choke is crossed by the channel walk along its declared `passes_to` pairs and appears in the finding
as a part, with its position, and as the boundary of an observation plane. With P4 a choke or ferrite can declare its
`common_mode_impedance` (section 3): it is then **evaluated** with the transmission `T_cc = 2 Z_c,ref / (2 Z_c,ref + Z_cmc(f))` at the frequency
it is evaluated at, inside its declared range only (log-axis interpolation, nothing extrapolated); with magnitudes only the result is a band
(`band_db`, `basis` naming it), the upper end of the transmission limited to 1 because a passive series impedance does not amplify. One that
declares no impedance, or is evaluated outside its declared range, is **present and not evaluated**: its transfer is taken as exactly 1, so the
common mode made upstream of it is not reduced, and the finding says so in words (the same words as before P4 for an undeclared part). The
numbers of `examples/emi_mode_conversion` changed with this: its chokes now declare 681.5 + j0 ohm at 1.25 GHz (18 `Z_c,ref`, `T_cc = 0.1`),
so lane C reads -46.76 dB (it read -26.76 dB while the choke was not evaluated) and lane D, with its skew after the choke, is unchanged at
-23.25 dB; the tests keep the undeclared case with the declaration edited out.

### 16.1 When it runs and what it needs

It needs the routed board, like section 15, and uses the same input (`ReturnPathInput`, now documented as the input of every check that
needs routed copper; `ReturnPathInput::routed()`): `copper emi-check BOARD --run RUN`, and the section `route-board` embeds. Without
`--run`, a board with a differential pair gets the coverage entry `EMI-MODE-CONVERSION` `unsupported` ("needs routed copper: run `copper
emi-check BOARD --run RUN`"); a board with no differential pair gets no entry at all, so the default output of a board with no differential
signal is byte-identical to before this phase. With `--run`, a board with no pair gets `not_applicable`. The zone fills are the ones of
section 15 and are used only for the amplitude imbalance; if they could not be made the skew is still estimated and the coverage entry says
the imbalance was not evaluated. `emi-check` takes no new option.

### 16.2 Subjects and the channel

The subjects are the differential pairs of section 15.2 (a signal source with a partner net, or a `differential` routing rule): one
finding and one coverage entry per pair, named after the P net. Single-ended signals are not subjects. The channel of a pair is the
ordered list the channel walk of section 14.3 gives, from the driver to the observation plane, as a cascade of elements:

* a **routed section** for each net pair the walk crosses (the pair's own nets, then the nets after each part), with, for each leg, the
  routed length of tracks and arcs and the vias' vertical path;
* between two sections the **part** the walk crossed: a series resistor and a coupling capacitor (per-leg elements from their
  declared `value`; a part with no value is present and transparent, and says so), a ferrite, a common-mode choke along its `passes_to`
  pairs (evaluated when it declares `common_mode_impedance`, else present with transfer 1 and not evaluated, see above) and any other
  series part (transparent, said so).

The observation plane is the connector pins when the walk reaches a connector, else the end of the routed pair; when the walk stops
before a connector the channel is the pair's own nets (the hops before the stop are not kept by the walk) and the finding lists where
and why it stopped. A section with routed copper on only one leg, or none, is not examined (its skew is unknown, not zero); a pair with
no examined section gets `unsupported`.

### 16.3 The cascade and its formulas

Names: `V_diff,in` is the differential voltage the driver launches (`V_p - V_n` at its pins, the loaded value `signal_swing` declares);
`V_p,out`, `V_n,out` the leg voltages at an observation plane, `V_diff,out = V_p,out - V_n,out`, `V_cm,out = (V_p,out + V_n,out) / 2`.
The reported ratio is `|V_cm,out / V_diff,in|`, not `V_cm,out / V_diff,out`; for pure skew `dt` they are `(1/2)|sin(pi f dt)|` and
`(1/2)|tan(pi f dt)|` (0.354 and 0.5 at `f dt = 0.25`, 3.01 dB apart). Both are voltage ratios, not power-normalised mixed-mode
S-parameters. The finding gives the ratio against `V_diff,out` as well, at each plane, named apart.

The state is the pair of forward (incident) leg waves `(V_p, V_n)`, starting at `(V_diff,in / 2, -V_diff,in / 2)`, in one differential
and one common-mode reference impedance for the whole cascade; every element is a transmission of incident to transmitted wave, so an
element that does nothing has a transfer of exactly 1.

* A routed section multiplies the legs by `g_p = (1 + a) exp(-j 2 pi f t_p)` and `g_n = (1 - a) exp(-j 2 pi f t_n)`. For skew `dt` and
  imbalance `a` of one section the result is `(1/2) sqrt(sin^2(pi f dt) + a^2 cos^2(pi f dt))`; skew alone `(1/2)|sin|`, imbalance alone
  `a / 2`. The larger of the two terms is not used: at `a = sin = 0.1` it reads 3 dB low.
* A series impedance `Z` in each leg multiplies it by `2 Z_leg / (2 Z_leg + Z)`.
* A series common-mode impedance `Z_cmc` converts to `(V_diff, V_cm)`, leaves the differential part alone and multiplies the common part
  by `T_cc = 2 Z_c,ref / (2 Z_c,ref + Z_cmc)`: 1 for `Z_cmc = 0`, 1/2 for `2 Z_c,ref`, 0.1 for `18 Z_c,ref`. It is not the open-circuit
  divider `Z / (Z + Z_cmc + Z)`, which belongs to a Thevenin source driving a load; the two agree for a matched source (`V_inc =
  V_oc / 2`, tested).

Order matters: common mode made before a choke goes through it, common mode made after it does not. With `T_cc = 0.1` and the same
total skew (`f dt = 0.25`, unfiltered `0.35355`): the choke after the skewed section gives `0.035355` (20 dB below), the choke before it
gives `0.35355`, and the skew split evenly on both sides gives `0.19445` (-5.19 dB), by hand `V_cm = 0.1375 + j 0.1375`.

Assumption, stated in the finding: a matched cascade without reflections between elements; reflections and mode conversion inside
parts are not modelled.

### 16.4 Reading the board

* **Skew.** Each leg's delay is `sum(length_layer * sqrt(eps_eff,layer)) / c` over its tracks and arcs (`eps_eff` as the impedance screening
  takes it from the declared stack-up, `signal_integrity::effective_permittivity`; without a stack-up `(4.5 + 1) / 2` on an outer layer and
  4.5 inside, as for the other checks) plus the vias' vertical path (`via_path_length_nm`) over `sqrt(eps_r)` of the dielectrics between the
  outer copper layers. A section's skew is `t_p - t_n`. Lane-to-lane skew of a match group is **not** a contributor, and the finding says so.
* **Reference impedances.** The section with the longest segment gives the layer and width; `signal_integrity::modal_impedances` (the
  same estimate `si-check` prints) gives the single-ended and, with a declared pair gap, the differential impedance. The common-mode
  reference is half a leg's single-ended impedance (the legs are taken as uncoupled in the common mode: the stack-up screening has no
  even-mode estimate). With no declared stack-up: 50 ohm single-ended (tag `rule_of_thumb_pair_impedance`, no checked source). These only
  matter to parts that act on the legs or on the common mode.
* **Amplitude imbalance.** `a = k (e_n - e_p)`, `e` the fraction of a leg's routed length in the section that has no reference copper
  within the coverage window of section 15.4 or lies within the edge limit of the reference copper's edge or the board edge (the longer of
  the two per layer, at most the routed length), measured by the same code as section 15 on the section's net pair; `k = 0.1`
  (band 0 to 0.2, tag `rule_of_thumb_leg_imbalance`). The leg with more exposure has the lower gain.
* **Coupling capacitor.** `C (1 + tol)` in one leg and `C (1 - tol)` in the other, `tol = 0.1` (band 0 to 0.2, tag
  `rule_of_thumb_capacitor_tolerance`); at gigahertz frequencies a 100 nF part is a gain of 1 to within 1e-8.

### 16.5 The finding

`EMI-MODE-CONVERSION`, warning severity, no cell, one per pair, present only when the worst ratio exceeds `1e-9` (-180 dB): a pair with no
skew and no imbalance has only its coverage entry, which says "no conversion at these inputs (not a pass: ...)". `estimate`: quantity
`mode_conversion_ratio`, unit dB, `kind = voltage_ratio` (a ratio of two voltages of one network; it is not an absolute field and gets
no margin), at the fundamental (half the `data_rate`, or the `clock_frequency`; with neither, at the worst frequency, said so), with
`band_db` and `basis = rule_of_thumb` when the imbalance or capacitor tolerance defaults shape it. `measures`:

| Key | Content |
|---|---|
| `ratio` | the quantity named, its definition, input and output plane, `headline_db` and frequency, `at_fundamental_db`, `worst` (frequency and dB of the largest value of the nominal cascade on a logarithmic scan from 1 MHz to the edge corner `1 / (pi t_ramp)` of the shorter ramp, and at the fundamental) and `edge_corner_hz` |
| `references` | single-ended, differential and common-mode reference impedance and where they come from |
| `sections[]` | per section: nets, `examined`, lengths per leg, via counts and paths, delays, `skew_ps`, `amplitude_imbalance`, `return_path_exposure`, layers |
| `parts[]` | per part: component, kind, what it acts on, per-leg values, `transfer_basis` (`not_declared_not_modelled` for a choke or ferrite that declares nothing; `declared_complex_inside_declared_range` or `declared_magnitude_band_inside_declared_range` for one that does), and `declared_common_mode_impedance` (the points, the range, the applicability) only where declared |
| `elements_in_order[]` | per element in order from the driver: what it adds or removes of `V_cm` (`change_in_v_cm_db`) and the `V_cm` before and after, at the fundamental and at the worst frequency, so a reviewer sees which section converts and whether a choke is upstream or downstream of it |
| `observation_planes[]` | before and after each modal part (choke, ferrite) and at the output: the ratio, the ratio against `V_diff,out`, and `V_cm,out` for each of the spectrum quantities of section 14.4 by name (`continuous_bound`, `pattern_lines`, `random_data_in_rbw`, `clock_lines`) at the fundamental and at the worst frequency |
| `lane_to_lane_skew`, `inputs_text`, `channel_notes` | the note that a match group's lane-to-lane skew is not a contributor; the defaults used; where the walk stopped and what was not examined |

`not_modelled` names: reflections and mode conversion inside parts, the attenuation of a choke or ferrite that declares no impedance or is outside its declared range, the coupling of the legs in the
common-mode reference, loss and dispersion of the sections, lane-to-lane skew, the geometry before the driver pins and after the
connector, the observation plane's own return path (no field, no radiation efficiency, no margin) and imbalance from other causes.

### 16.6 Coverage

`EMI-MODE-CONVERSION` is `executed` when at least one pair was examined, with an entry `EMI-MODE-CONVERSION:<P net>` per pair
(`executed` with the numbers, or `unsupported` when no section has routed copper on both legs); `not_applicable` when a run was given and
the board has no differential pair; `unsupported` without a run when a pair exists (it needs routed copper); no entry without a run and
without a pair. The route report's `emi` section is kept when an `EMI-MODE-CONVERSION` entry is `executed`, like section 15.12. `copper issues` lists
the finding in the signal group and the converting sections as `details.causes`, and `--plot-summary` draws a signal page
(section 8); neither shows a limit, a margin or a field.

### 16.7 Defaults and their sources

In `crates/copper-drc/src/emi/defaults.rs`, section 8, each with its tag, band and a statement of its source.

| Default | Value (band) | Source |
|---|---|---|
| Imbalance per unit exposure `k` | 0.1 (0 to 0.2) | **rule of thumb with no published source**: a screening choice |
| Coupling capacitor tolerance | 0.1 (0 to 0.2), opposite signs on the two legs | ceramic tolerance codes K (10 %) and M (20 %), from memory and **not checked against the standard** |
| Single-ended impedance without a stack-up | 50 ohm | the usual high-speed serial target, **not checked against a standard** |

The edge and swing defaults are those of section 14.2 and are tagged when used.

### 16.8 Validation

Against hand calculations in the tests. Closed forms (`copper-emi`): `(1/2) sin(pi/4) = 0.35355` against `(1/2) tan(pi/4) = 0.5`; `a / 2 = 0.05`;
`(1/2) sqrt(0.0199) = 0.07053` for `a = sin = 0.1`; a numerical check with two sampled trapezoids (one scaled and delayed) against the
cascade at four harmonics for delay only, amplitude only and both; `T_cc = 1, 1/2, 0.1, 0.4472` for `Z_cmc = 0, 2, 18` times
`Z_c,ref` and `j 100` ohm against 25 ohm; the Thevenin and incident-wave forms agreeing; the order fixtures (0.035355, 0.35355, 0.19445) and the
identity fixture (zero-impedance elements and zero-length sections anywhere, several at once, change no state by more than 1e-14).
Board (`crates/copper-drc/tests/emi_mode_conversion.rs`): with `eps_eff = 3.0844` a leg delay is 5.858 ps per mm; the 10 mm skew of lane A at
1.25 GHz is `(1/2) sin(0.2300) = 0.1140`, -18.86 dB; the 4 mm skew of lane C (before a choke and two capacitors that evaluate to
nothing) -26.76 dB before the choke, and after it, with the choke declared at 681.5 + j0 ohm at 1.25 GHz (`T_cc = 75.72 / 757.22 = 0.09999`,
-20.00 dB), -46.76 dB; the 6 mm skew of lane D (after the choke) -23.25 dB; lane B, with equal legs, has no conversion and no finding; one leg
moved to 1 mm from the plane edge and 3 mm from the board edge, with the same length, gives `a = 0.1`, `a / 2 = 0.05`, -26.02 dB with a band
from none to -20.00 dB. None of this is correlated with a measurement.

### 16.9 Not modelled

Reflections between elements and mode conversion inside parts; the attenuation of a choke or ferrite that declares no `common_mode_impedance` and outside its declared frequency range; the
coupling of the legs in the common-mode reference; loss and dispersion; lane-to-lane skew of a match group; the package and connector
breakouts; the return path of the observation plane, radiation efficiency and any field or margin; imbalance from the driver, the
receiver termination or asymmetric loading.

## 17. The cable's circuit: definitions and fixtures (P4a of the signal and cable plan)

Status: **definition and test fixtures**, used unchanged by the cable check of section 18: no number in any output comes from a network
other than these, and none of it is correlated with a measurement. It is the specification that the
cable check of the fourth phase of [`emi-signal-cable-plan.md`](emi-signal-cable-plan.md) (`EMI-CABLE-COMMON-MODE`, voltage and ranking)
uses: every quantity of that check is a quantity of one of the networks below, and the fixtures check the closed forms
against an independent circuit solution instead of against themselves. The closed forms are in `crates/copper-emi/src/cable_circuit.rs`
(and `mode_conversion::common_mode_transfer`); the independent nodal solver is in `crates/copper-emi/tests/support/nodal.rs`, shares no
code with the estimators and has its own checks against hand calculations (a resistive divider, a series RLC at resonance, coupled
branches, a quarter-wave line); the fixtures are in `crates/copper-emi/tests/cable_circuit_fixtures.rs`.

### 17.1 Conductors and the bundle

The **signal conductors** of a channel are one conductor (single-ended) or a P/N pair. The **return or ground conductor** is the
conductor inside the same cable that carries the return current, when there is one. The **shield** is the shield, when there is one. The
**bundle** is all conductors of the cable taken together: the cut of the cable that a current clamp around the whole cable encloses.

### 17.2 Two different common modes

1. **The pair's common mode** against the board reference at the connector, `V_cm` of section 16. Its current can return through the
   ground conductor inside the same cable; in the ideal case the net current of the bundle is then zero.
2. **The bundle's common mode**: the net current of all conductors together. It returns outside the cable, through the environment, and
   it is what a clamp around the whole cable reads and what radiates. The published cable-radiation models drive it from a voltage
   between the cable and the antenna's *other* conductor (the board, its ground plane, other cables) through a stated coupling path
   (Hockanson et al.); the board model does not contain that other conductor.

The two are not converted into each other here: dividing the pair's `V_cm` by a cable impedance does not give the bundle current,
because what is missing is the circuit (the in-cable return, the antenna's other conductor), not a parameter value. The net current of
the bundle is a quantity of a stated termination and environment scenario only, and nothing is reported for it yet.

### 17.3 Reference and observation point

For the pair's common mode (1): the board reference at the connector's ground pins. For the bundle's common mode (2): the clamp
position on the cable at the connector, and the other half of the antenna.

### 17.4 Source and load

* **Source impedance** per driver, and the **series parts** from the channel walk (coupling capacitors, chokes), are elements of the
  network between the driver and the observation point.
* **The load each quantity sees** is stated with the quantity: the cascade's reference impedance, a cable load, a declared load.
* **Incident wave and Thevenin source are two representations of one network** and are kept apart. The cascade of section 16 carries
  incident (forward) voltage waves, and a series common-mode impedance `Z_cmc` in the common-mode reference `Z_ref` transmits
  `T = 2 Z_ref / (2 Z_ref + Z_cmc)`: 1 for no choke, exactly; 1/2 for `Z_cmc = 2 Z_ref`; tending to zero for a choke far larger than
  `Z_ref`. A Thevenin source `V_oc` with `Z_source` into `Z_load` through `Z_cmc` has the current `I = V_oc / (Z_source + Z_cmc +
  Z_load)` and the load voltage `V_oc Z_load / (Z_source + Z_cmc + Z_load)`. For `Z_source = Z_load = Z_ref` the load voltage is
  `(V_oc / 2) T` for every `Z_cmc`, zero included: `V_oc / 2` is the incident wave of a matched source. Zero-impedance elements in
  series change nothing in either representation.

### 17.5 The cable termination, and what the cascade's output means

The cascade is forward only and has no reflections. Its output at the connector is the incident common-mode wave, and that equals the
steady-state voltage there **only for a matched termination**, a cable whose common-mode impedance at the connector is `Z_ref`. A final
multiplier `2 Z_cable / (Z_ref + Z_cable)` on the forward wave gives the **first-arrival voltage** (the first wavefront before any
reflection returns) and does not recover the steady state: the complete relation has the load-feedback denominator the cascade does not
carry. For 1 V incident in 50 ohm, a 900 ohm choke and a 900 ohm cable load, the multiplier gives 0.189 V and the series circuit
0.973 V (14.2 dB apart); for an open cable the circuit gives the whole open-circuit voltage, twice the incident wave, whatever the choke
(the multiplier 0.2 V against 2 V). The three cases are matched (the two agree), high impedance and open.

The consequence for P4 is binding: an absolute connector voltage is published only for a matched cable and says so; for any other
termination only a value labelled `first_arrival_voltage`, never a steady-state figure and never compared with one. Solving the
terminated network with its reflections belongs to a later, scenario-defined step.

### 17.6 The ground-noise network

`omega L_gnd I_return` is not a calculation until the following four are stated. The corridor rule of the cable check only selects
which sources are considered; it does not give their coupling.

**Driven loop A.** One per source conductor: from the driver pin along the routed copper to the load (or the connector pin), back
through the reference plane under the route to the driver's reference pin. `Z_A` is what the loop presents to the pin: the line's
characteristic impedance (from the stack-up, as `si-check` computes it) while the edge travels on a terminated or long line, or the
declared load. The current follows from **one** of two source representations, never a mixture:

* *Loaded pin voltage* (the default, and what `signal_swing` and `V_diff,in` of section 16 mean: the voltage at the pin while it drives
  the line): `I_A = V_pin / Z_A`. The source impedance does not appear, because its loading is already in `V_pin`. A 1 V launched signal
  on a matched 50 ohm line is 20 mA.
* *Thevenin source* (only when a part declares an open-circuit swing and an `output_impedance`): `V_pin = V_oc Z_A / (Z_source + Z_A)`
  and `I_A = V_oc / (Z_source + Z_A)`.

Dividing a loaded pin voltage by `Z_source + Z_A` applies the source loading twice (10 mA instead of 20 mA in the example, 6 dB low).
It is the error this definition excludes. A source declared both ways (`V_oc`, `Z_source` and the `V_pin` they imply) gives identical
`V_pin`, `I_A` and `V_B`, and a test checks it. Without a characteristic impedance or a declared load for the route, `I_A` is not
estimated (see the fallback).

**Observation loop B.** The loop whose voltage is the ground-noise voltage at the connector: from the connector's ground pins through
the reference plane to the source's reference pin, closed by a stated sense path. The sense path is part of the definition because the
answer depends on it (it stands for the cable leaving the board and the antenna's other conductor). Version 1 uses a straight return at
a stated height above the reference plane, a scenario parameter with a band; the finding reports how much the result moves across that
band.

**Transfer, per conductor.** Each driven conductor `k` has its own loop and its own signed transfer to loop B,
`Z_Bk = j omega M_Bk` plus the resistance of the copper the two loops share, with `M_Bk` the mutual inductance of that conductor's loop
and loop B, sign included. "`L_gnd`" in the plan means such an `M_Bk` and nothing else.

**Signed contributions: couple first, then sum.** `V_B = sum over k of Z_Bk I_k`. For a differential pair
`V_B = j omega (M_Bp I_p + M_Bn I_n)`. The leg currents are **not** added before the coupling is applied: with `I_p = -I_n` the summed
current is zero, but the voltage is `j omega (M_Bp - M_Bn) I_p`, which is not zero whenever the two legs couple differently to loop B
(one leg nearer the connector's ground path, or over a gap). In modal terms
`V_B = j omega [ (M_Bp + M_Bn) I_cm + (M_Bp - M_Bn) I_dm ]` with `I_cm = (I_p + I_n) / 2` and `I_dm = (I_p - I_n) / 2`: the common-mode
current couples through the sum of the mutuals and the differential current through their difference, and both terms are kept. Example:
`+1 mA` and `-1 mA` with `M_Bp = 2 nH` and `M_Bn = 1 nH` give `omega 1 nH 1 mA`, about 628 uV at 100 MHz, with zero summed current;
with `I_cm = 0.5 mA` added the extra voltage is `omega (M_Bp + M_Bn) I_cm` and the total 1.571 mV. Several independent sources are
summed as phasors only when they are harmonics of one clock, otherwise as powers.

A choke that carries the signal conductors of the pair but not the ground conductor attenuates the pair's common mode (1) and leaves
the ground-noise voltage unchanged: the ground-noise driver acts on the bundle and does not pass through it.

**Fallback, binding for P4.** Where the route's characteristic impedance (or declared load) or the geometry of any of the loops is not
available, or where the result moves by more than 6 dB across the sense-path band, no ground-noise voltage is published for that source.
It is ranked by geometry instead: corridor overlap with the connector's ground path, distance, and any slot of section 15 between them.
Extraction of the `M_Bk` from the real copper stays deferred; the definition does not.

### 17.7 The fixtures

All in `crates/copper-emi/tests/cable_circuit_fixtures.rs`, each solved by the independent nodal solver and compared with the hand
calculation written in the test, and with the closed form where there is one. Numbered as in the plan.

| # | Fixture | Hand numbers |
|---|---|---|
| 1 | Return-conductor cancellation: a pair with common mode and an in-cable return | 1 V, 10 ohm source, two 50 ohm conductors, 5 ohm return: 12.5 mA on each, 25 mA back, net 0; return removed, 500 ohm environment: net 1.869 mA, the pair's current; both: net 0.2478 mA |
| 2 | Source, series and load impedance, complex, across frequency | 3.3 V, 22 / 10 / 50 ohm: 2.0122 V; 20 ohm, `+j100`, 50 ohm: 1.3517 V at -55 degrees |
| 3 | Choke traversal in both representations: incident wave `T` (no choke, `2 Z_ref`, a large choke), Thevenin current and voltage, the two agreeing for every `Z_cmc`, identity under zero-impedance elements | `T = 1, 1/2, 1e-7`; `(V_oc / 2) T` against the divider |
| 4 | Simultaneous gain and delay mismatch of a two-leg network against the expression of section 16 | `(1/2) sin(pi/4) = 0.35355`, `a/2 = 0.05`, `(1/2) sqrt(0.0199) = 0.07053` |
| 5 | A choke in the signal lines only | pair 1 V to 0.1 V (-20 dB); ground conductor 0.909 mV either way (21.7 uV if the choke carried it too) |
| 6 | Cable termination behind a 900 ohm choke, 1 V incident in 50 ohm | matched 0.1 V and 0.1 V; 900 ohm 0.189 V against 0.973 V (14.2 dB); open 0.2 V against 2 V (20 dB) |
| 7 | Choke position: the order fixtures of section 16 as a network of two legs and a common-mode series impedance | 0.035355, 0.35355 and 0.19445 (`0.1375 + j 0.1375`) |
| 8 | Ground-noise transfer: numerical Neumann integral of loops against the filament closed form; sign and cancellation cases | single loop, loop moved away, symmetric cancellation 0, asymmetric residual 628.3 uV, common mode added 1.571 mV, a source declared both ways 20 mA, the sense height band |

The assertions of fixture 6 state the size of the disagreement in the high-impedance and open cases, so the forward-only figure cannot be
relabelled as a steady-state voltage without a test failing.

### 17.8 Not modelled

Reflections between the cascade's elements and the solution of the terminated network (a scenario step); the bundle's net current of
a real cable, the antenna's other conductor, chassis and enclosure; the shield's transfer impedance; the extraction of the `M_Bk` from
routed copper (the fixtures use rectangular filament loops in free space for the geometry cases, and the `Z_Bk` from them are not a
board estimate); a slot between the loops; the coupling between the driven loops themselves.

## 18. Cable common mode: `EMI-CABLE-COMMON-MODE` (P4 of the signal and cable plan)

Status: implemented. This section is the specification of the fourth phase of [`emi-signal-cable-plan.md`](emi-signal-cable-plan.md); the
check is in `crates/copper-drc/src/emi/cable.rs`, the closed forms in `crates/copper-emi/src/cable_circuit.rs` (section 17) and
`mode_conversion.rs` (section 16), the example in `examples/emi_cable_common_mode/`, the tests in
`crates/copper-drc/tests/emi_cable_common_mode.rs`, `crates/copper/tests/emi_cable_common_mode_cli.rs`,
`crates/copper-lang/tests/emi_cable.rs` and `crates/copper-emi/tests/cable_circuit_fixtures.rs`. It reports, for each cable connector and
each driver, a common-mode **voltage** at the connector against the board reference there, and a **ranking** of the drivers.

**It reports no current, no radiated field, no `margin_db` and no pass or fail**, and nothing in its output is correlated with a
measurement. A field would need the net current of the whole cable (the bundle's common mode, section 17.2), and that needs what the board
model does not have: the cable's in-cable return conductor, its far-end termination and the antenna's other conductor. Dividing the pair's
`V_cm` by a cable impedance, however wide its band, would be a different quantity; that conversion is the planned P4b and is not here. The
function `copper_emi::estimate::cable_common_mode_field` stays unused by the report. (Its constant `mu0 f l I / r` was checked in this
phase: the broadside far field of a short current element is `E = eta0 I l / (2 lambda r) = mu0 f l I / (2 r)` in free space, so the
function's value is twice the free-space field, the factor 2 being the ground reflection of a test site; no book was checked, the
derivation is in the function's documentation and a unit test.)

### 18.1 When it runs and what it needs

One finding and one coverage entry per cable connector: a part with `external_cable = true` or a component named by an `emission`
constraint. It runs on every `emi-check`: the default run lists what needs no routed copper (the ground-noise selection, the single-ended
signals, the switch-node scores, the shield) and marks each differential pair `unsupported` ("needs routed copper: run `copper emi-check
BOARD --run RUN`"); `--run RUN` adds the pair voltages and the ground-noise networks. A board with no cable connector keeps its previous
`EMI-CABLE-COMMON-MODE` entry (`unsupported`, byte for byte), so its default output is unchanged. A connector with nothing to report (no pair
or single-ended signal reaches it by the channel walk, no fast net or cell is near its ground pins, no switch-node score, no shield pin) gets
`not_applicable` and no finding; when every connector is like that the overall entry is `not_applicable` too.

### 18.2 The drivers

Each is a voltage between named conductors and the board reference at the connector (the connector's ground pins: pads of the connector on a
declared 0 V supply net), with its network from section 17.

1. **Differential pairs that leave on the cable** (found by the channel walk of section 14.3, not by the net classes). The voltage is
   `V_cm,out` of the section 16 cascade at the connector pins, for each spectrum quantity of section 14.4 by name, in dBuV (single-sided peak
   amplitude of the line), with the band the rule-of-thumb inputs and a magnitude-only choke open. **No divider of its own is applied**: a
   choke is an element of the cascade at its position, so common mode generated upstream of it is attenuated by `T_cc(f) = 2 Z_c,ref / (2
   Z_c,ref + Z_cmc(f))` and common mode generated after it is not. The finding shows the two parts separately for every quantity
   (`from_upstream_of_last_choke_dbuv`, `generated_after_last_choke_dbuv`; the cascade is linear, so the two add as phasors to the total).
   A choke of zero impedance changes nothing (transmission exactly 1, tested through the real code path).
   *The choke's declared impedance.* Inside the declared range the impedance is the declared one (log-axis interpolation); with a phase the
   transmission is complex and one value; with magnitudes only it is a band, `2 Z_c,ref / (|Z| + 2 Z_c,ref)` to `2 Z_c,ref / | |Z| - 2 Z_c,ref |`
   (the plan's band, reported as `transmission_low` and `transmission_high`, the latter unbounded for `|Z| = 2 Z_c,ref`), of which the voltage
   uses the upper end limited to 1 (`transmission_high_used`: a passive series impedance has a real part of at least zero, which keeps the
   transmission at or below 1). Outside the declared range, and at any frequency other than the one point a single declaration gives, the
   choke is **present with its attenuation not modelled** (`declared_outside_range_not_modelled`, transmission null, listed in
   `choke_not_modelled`); nothing is extrapolated.
   *What the connector voltage means.* The cascade is forward only, so its output at the connector is the incident common-mode wave. That is the
   steady-state voltage **only for a matched cable**, one whose common-mode impedance at the connector is `Z_c,ref`. That is the single
   case for which an absolute connector voltage is published, as `common_mode_voltage_at_connector` with the assumption stated and the tag
   `matched_cable_termination`: it is the case whenever the connector part declares no `cable_common_mode_impedance`, or one within 1 % of
   `Z_c,ref`. For a declared, unmatched cable only `first_arrival_voltage` is given, the first wavefront at the cable before any reflection
   returns (`2 Z_cable / (Z_c,ref + Z_cable)` times the incident wave), labelled as not a steady-state value, with `steady_state_voltage`
   null, and never ranked against a steady-state quantity (the 50 / 900 / 900 ohm example of section 17.5: 0.189 V against 0.973 V, 14.2 dB).
   The terminated network with reflections is the scenario step of P4b.
2. **Single-ended signals that leave on the cable.** Their swing is **not** taken as common-mode voltage: a signal whose return is in the same
   cable is not common mode by itself. A ranking-only entry lists each with its edge time, swing and slew rate; they contribute through the
   ground-noise driver, driven by their own return current. Signal-to-return imbalance inside the cable is not modelled.
3. **Ground noise (current driven).** The voltage `V_B = sum over the driven conductors of Z_Bk I_k` of the network of section 17.6: each
   conductor has its own loop A and its own signed mutual `M_Bk` to the observation loop B, **coupled first and then summed** (for a pair
   `V_B = j omega (M_Bp I_p + M_Bn I_n)`, never one value for the pair); this is the driver that acts on the bundle, and a choke in the signal
   lines does not attenuate it (fixture 5).
   * *The source current* from exactly one representation (section 2): `I_A = V_pin / Z_A` for the loaded pin voltage (a pair: `V_diff` over the
     differential impedance of the stack-up, `I_n = -I_p`), or `I_A = V_oc / (Z_source + Z_A)` for a declared Thevenin source; `Z_A` is the
     route's characteristic impedance from the stack-up as `si-check` takes it.
   * *Loop A* of each conductor is its routed copper (chained from the end nearest the driver, per layer, each run at the height of its layer
     over the reference plane) closed through the plane directly beneath it; *loop B* is a straight sense path from the reference pin of the
     driving component to the connector's ground pin nearest the driver, at a stated height over the plane, closed through the plane. The
     sense height is a scenario parameter, a band (3, 5 and 8 mm, tag `rule_of_thumb_sense_height`, no published source), and the finding gives
     the result for all three.
   * *The published form of `M_Bk`* is `copper_emi::cable_circuit::loop_mutual`: the Neumann mutual inductance of two closed polylines as the
     sum over their edges of the finite-filament mutuals of `copper_emi::inductance::mutual_filaments` (Grover's closed form for parallel
     filaments, adaptive quadrature for skew ones). Validity: thin conductors (filament separation large against width), free space with the
     reference plane represented by the return edge of each loop directly beneath its conductor (no current spreading, no finite plane, no
     slot). It agrees with an independent midpoint Neumann integral written in the tests (`cable_circuit_fixtures.rs` fixture 8 and
     `emi_cable_common_mode.rs`, to 0.3 %). Positions of the loops are in `measures` (`network.loop_b`, `network.conductor_loops`).
   * *When a voltage is published.* Only when all of these exist for the source: the characteristic impedance of the route (a routed layer
     with a declared stack-up), the routed copper of every conductor, a swing, a reference pin and a connector ground pin, a reference plane
     layer, **no slot** of the reference plane between the loops (the voids of the zone fills on the straight path of loop B, widened by `3 H` as
     in section 15, named in `slots_between_the_loops`) and the result within 6 dB across the sense-height band. Where the result is more than 30 dB
     below the larger of the legs' own terms (the legs' signed mutuals cancel) it is published as it is and `cancellation` is set: the band test
     would only measure the rounding of a difference. Otherwise the source is ranked by geometry and `no_voltage_because` names what was missing
     or how far the result moved. This is expected to be the common case on a board that is not laid out for it. There is no declared load in the
     language yet, so a route without a characteristic impedance has no current.
   * *Which sources are considered.* The signal that leaves on this connector, always. Another fast net or a switching cell is considered when
     the straight corridor between its source and load pins, widened by `3 H`, overlaps the corridor of the connector's ground pins: the corridor
     from the ground pins to the nearest point of that net's own return path ends on the net's corridor, so the test as implemented is the
     distance from the connector's ground pin to the net's corridor against `2 x 3 H` (each corridor widened by `3 H`). Otherwise the source is
     listed in `ground_noise_not_counted` with its distance and the threshold. A switching cell is selected the same way on the corridor between
     its switch pin and its reference pin, and is ranking only (its pulsed current is the hot loop's, section 8).
4. **Switch-node coupling (voltage driven).** The `cm_coupling_score` of `EMI-SWITCH-NODE` with the distance of that net to this connector,
   ranking only, no voltage. The island capacitance to the connector's shell and pins themselves is not computed (the score uses the
   capacitance to the nearest ground plane).

### 18.3 Shielded cables

`cable_shielded = true` with pins of `emi_role = shield` gives the **shield bond** report: the pins and their nets, the vias on the pin's net
within 5 mm of it (tag `rule_of_thumb_shield_bond_radius`, no published source) and an estimated bond inductance, those vias in parallel, each
`mu0 h / (2 pi) (ln(4 h / d) + 1)` over the height `h` to the nearest ground plane (the via estimate of the conducted check, expected within
30 %; a band is given, the pin's own inductance is not included). The shield's transfer impedance is not modelled. `cable_shielded = true`
without any shield pin is its own finding, `EMI-CABLE-SHIELD` ("no declared bond"). A connector with neither the attribute nor a shield pin is
treated as unshielded and tagged `assumed_unshielded_cable`; one with shield pins and no attribute reports the bond and is not tagged.

### 18.4 The finding

`EMI-CABLE-COMMON-MODE`, warning severity, no cell, one per connector. `estimate`: the strongest pair voltage (quantity
`common_mode_voltage_at_connector` or `first_arrival_voltage`, unit dBuV, `kind = common_mode_voltage`, the definition stated, with `band_db`
when the rule-of-thumb inputs or a magnitude-only choke open one); none when no pair has a voltage. `measures`:

| Key | Content |
|---|---|
| `connector`, `part` | the connector and its part |
| `cable` | `shielded`, `shield_pins_declared`, `assumed_unshielded`, `cable_length_m` (the `emission` constraint's `cable_length`, default 1 m), `quarter_wave_frequency_hz` (above it the cable is resonant, not modelled), the declared cable impedance |
| `shield` | the bond report of section 18.3, or null |
| `differential_pairs[]` | per pair: `conductors`, `reference`, `voltage` (`quantity`, `headline_dbuv`, `band_dbuv`, `spectrum[]` per quantity with the two parts at the last choke), `termination`, `choke_transfer[]`, `channel_provenance[]` (the parts crossed), `choke_not_modelled`, `rank` |
| `ground_noise[]` | per considered source: `status` (`evaluated` or `ranking_only`), `voltage` (headline, the sense band and its spread, the spectrum with `line_amplitude_v`) or null with `no_voltage_because`, `network` (impedance, mutuals for the three sense heights, the loops), `slots_between_the_loops`, `distance_to_connector_ground_pins_mm`, `rank` |
| `ground_noise_not_counted[]` | the sources the corridor rule did not select, with the distance and the threshold |
| `single_ended_signals[]`, `switch_node_coupling[]` | the ranking-only drivers |
| `ranking` | the drivers ordered within their own kind: pairs by the continuous bound (or the clock line) at the headline frequency, ground noise by voltage and, beside it, by geometry (a source with a slot first, then the nearer), single-ended signals by slew rate, switch nodes by score; a note that the kinds are shown side by side and **not summed** (they act on different conductors, the pair against the reference and the bundle against the environment) |
| `not_modelled`, `inputs_text` | what is left out (below); the defaults used |

The headline frequency is the fundamental (half the `data_rate`, or the `clock_frequency`); with neither it is the frequency of the largest value
over a logarithmic scan of 30 MHz to 1 GHz (and below the edge corner), said so. The `EMI-MARGIN` coverage entry says cable common mode is
reported as voltage and ranking only, and why. `has_content` of the route-report section counts an executed `EMI-CABLE-COMMON-MODE` entry (overall
or per connector), so a board with a connector and something to report keeps its `emi` section; one with a connector and nothing to report keeps
the bytes it had. `copper issues` lists the finding in the connector group with one cause per driver (its quantity label as above),
`--plot-summary` draws a connector page and `--plot-spectrum` the same ranked bar chart of the drivers' voltages at the
connector, without a limit line (section 8).

### 18.5 Defaults and their sources

In `crates/copper-drc/src/emi/defaults.rs`, section 9.

| Default | Value (band) | Source |
|---|---|---|
| Sense-path height of loop B | 5 mm (3 to 8 mm), tag `rule_of_thumb_sense_height` | **rule of thumb with no published source**; the heights of the independent fixture 8 |
| Sense-band limit for publishing a voltage | 6 dB | the plan's decision, not a rule of thumb |
| Cancellation threshold | 30 dB below the larger leg term | a choice of this implementation |
| Radius of a shield bond via | 5 mm (2 to 10 mm), tag `rule_of_thumb_shield_bond_radius` | **rule of thumb with no published source** |
| Corridor widening | `3 H` each side (section 15's `RETURN_WINDOW_H`) | derived there |

The edge, swing and reference-impedance defaults are those of sections 14.2 and 16.7 and are tagged when used. `matched_cable_termination` and
`assumed_unshielded_cable` are tags of the plan, not rules of thumb.

### 18.6 Validation

Against hand calculations in the tests, with `Z_c,ref = 37.86 ohm` and the lanes of section 16.8. Through the real code path
(`emi_cable_common_mode.rs`): a 6 mm skew with no choke, after a choke, or after a zero-impedance choke reads `0.254648 x 0.068789 = 17.517 mV
= 84.87 dBuV` (the identity); a 4 mm skew before ChokeA (681.5 + j0 ohm, `T_cc = 0.1000`) reads 61.36 dBuV against 81.36 unfiltered; a choke
of `2 Z_c,ref` reads 6.02 dB lower, one of 10 kohm 42.48 dB lower; a magnitude of 450 ohm at 1.25 GHz gives the band 64.53 to 67.48 dBuV
(`|T|` 0.14403 to 0.20231), and the 100 MHz lane through the same part is unattenuated (59.44 dBuV, outside the declared point); the same total
skew with the choke before and after, and split, agree with a small cascade written in the test; the 150 ohm cable of the shielded connector
gives the first arrival `17.517 mV x 2 (150) / (37.86 + 150) = 27.97 mV = 88.94 dBuV` and no steady-state value; the three vias of the shield pin
give 0.0336 nH. Ground noise: the three mutuals of a 60 mm route 2 mm beside the sense path agree with an independent Neumann integral to
0.3 %, the voltage is `omega M A / Z_A`, the legs of a symmetric pair cancel to more than 100 dB below one leg's term and a pair with one leg
nearer gives `omega (M_p - M_n) I`, the Thevenin and loaded declarations of the same source give the same voltage, a cut across the sense path
makes the source ranking only with the slot named, and the corridor distances of the clocks (1.115 mm and 44.008 mm) are the geometry by hand.
Language: `crates/copper-lang/tests/emi_cable.rs`. None of this is correlated with a measurement.

### 18.7 Not modelled

The bundle's net current, the cable's routing and height, chassis and enclosure; the far-end termination (an absolute connector voltage assumes
a matched cable); cable resonance above the quarter-wave frequency; reflections between a choke and the cable; the shield's transfer impedance;
signal-to-return imbalance inside the cable; the resistance of the copper two loops share and current spreading in the plane; mode conversion
inside parts and the connector's breakout; a choke or ferrite with no declared impedance, or outside its declared range; radiation, a field and a
margin. The extraction of the `M_Bk` from filled copper stays deferred (P5).
