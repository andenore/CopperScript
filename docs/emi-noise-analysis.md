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

Validation (`CMP130`): wrong dimension, non-positive value, `min > max`, fixed
and range frequency together, unknown `topology` or `shielding` name,
`external_cable` that is not `true` or `false`. A range may give only one
bound. The units are the ones of `copper-units`, so an ESR below 1 ohm is
written as a decimal (`esr = 0.01ohm`).

IR: `part.emi = { ... }` with only declared keys.

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
hard macro contributes copper.

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
| `EMI-RETURN-PATH` | coverage `unsupported` | needs filled copper and traced routed copper | - |
| `EMI-DECOUPLING-PATH` | coverage `unsupported` | needs routed copper | - |

Every check that cannot run on the given input gets a coverage entry with
status (`executed`, `not_applicable`, `unsupported`) and reason; nothing is
reported as a pass for what was not measured. Standing entries:
`EMI-RETURN-PATH` and `EMI-DECOUPLING-PATH` (unsupported),
`EMI-COPPER-TRACE` (traced copper path: `executed` when at least one copper leg of a hot loop is traced, else `unsupported` with the reason), `EMI-CABLE-COMMON-MODE`
(common-mode radiation from cables and attached conductors, unsupported: the board model has no cable, chassis or return structure), `EMI-MARGIN` (whether an
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
  voltages of the hot-supply and output nets are declared with `Vout < Vin`, and the inductor current stays positive
  (`I_out >= dI/2`). The high-side switch current is the hot-loop current: the ramp from `I_out - dI/2` to `I_out + dI/2`
  with the declared current edges. The current limit `current_peak`, when declared, is the secondary worst case
  (`measures.secondary_scenario`: level, frequency, margin, band of the same estimate with flat-top pulses of the limit's
  amplitude, as a trapezoid envelope). `current_limit`: no operating point is declared, or it is outside the model
  (`operating_point.status` `not_declared` / `outside_model` with the `reason`; the tag `operating_point_outside_model`), or
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
  Nothing is printed as pass or fail. The `cable_length` of the constraint is read but no
  cable estimate uses it yet.

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
node copper is counted, not traced), `EMI-RETURN-PATH`, `EMI-DECOUPLING-PATH`
and current-driven common mode on cables.

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
  switching cell gets no `emi` key, so its report is byte-identical to what it
  was before.
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
  findings grouped by cell and in the report's ranking inside a cell. The title
  starts with `screening estimate`, the details carry the estimate with its
  `numerical_sensitivity`, parameter band, `not_modelled` list, margin, tags and the scalar measures, and the text output ends
  with a line saying the issues are screening estimates, not EMC qualification.
  An accepted finding (`emi_accept`) is listed as `ACCEPTED (reason)` with
  count 0, so it does not add to `finding_counts`. Windows: `EMI-HOT-LOOP` the
  bounding box of its `loop_polygon_mm`; every other finding the pad centres of
  its objects as found on the run's board (a proximity finding: the closest pair
  of source and victim pads, the cell's own controller aside; a switch-node
  finding: every pad of the net). `--stage emi` filters; an `error` section or
  an empty one lists nothing.
- *`copper view RUN --issue M2`* frames the window, highlights the issue's nets
  and draws the hot loop as a red outline over the copper (`SceneArea` of kind
  `emi_loop`, SVG class `emi_loop`, on the layers of the finding); the legend
  names it as a screening estimate. Other emi issues are marked at their pad
  centres like the other stages. `view` has no stage filter of its own.

### Visualization

Two hand-written, deterministic SVGs (no dependencies, explicit white
background, fixed palette), both labelled as screening estimates and both
ending in "screening estimate, not EMC qualification" or "not a field
prediction".

**Conducted spectrum.** `copper emi-check BOARD [--run RUN] --plot-spectrum OUT.svg`
writes one panel per cell with an `EMI-CONDUCTED` finding (renderer:
`copper_report::emi_plot`, data read from `measures.harmonics`): a log
frequency axis from 150 kHz to 30 MHz (decade and 2/5 gridlines), dBuV with a
gridline every 10 dB, one stem and dot per harmonic at the high end of its band (its total level
when it has none; the differential-mode series is the total while common mode
is not modelled; with common mode present DM and CM are added as markers), a
translucent whisker from the low to the high end behind each stem for a banded
harmonic (legend `band: rule-of-thumb inputs`), the quasi-peak and average
lines as polylines through the per-harmonic `limit_qp`/`limit_avg` when an
`emission` standard with a conducted table is declared **and the finding's levels are detector
readings** (`measures.detector_applicability = isolated_lines`; for `receiver_emulation` the lines run through the harmonics the receiver model read, which are marked by diamonds at their high-end quasi-peak reading, in the warning colour above a limit, while the stems stay line amplitudes and the panel is headed "NOT detector readings; receiver emulation at the diamonds"), harmonics above the
average line in the warning colour, and the finding's tags (`scenario_declared_operating_point` or `scenario_current_limit` with
`worst_case_current`, `rule_of_thumb_edge`, `differential_mode_only`, ...) under the title. A panel whose detector
assumption fails (harmonic spacing inside the IF bandwidth, or a declared frequency range) is headed
"conducted line amplitudes, NOT a detector reading", draws no limit line, no warning colour and no
margin, and prints the reason; the panels of a frequency range are one per fixed-frequency
scenario, titled with the scenario. A report that does not state the status is drawn the same way.
There is no accuracy whisker. The common-mode coupling-risk score of `EMI-SWITCH-NODE` is a
relative ranking and is never drawn here. A board
without an `EMI-CONDUCTED` finding writes nothing and exits 2 with the reason.

**Magnetic near field.** `copper emi-check BOARD [--run RUN] --plot-field OUT.svg
[--field-height 1mm]` and `copper view RUN --issue M<n> --emi-field
[--field-height 1mm]` (an `EMI-HOT-LOOP` issue) draw `|H|` in a plane
`--field-height` above the top copper (default 1 mm) as a heat map in
dB(uA/m) over the loop window, with the loop outline, the pads of the cell's
loop components, the top-layer copper of the board scene under the map, a colour
bar and, for every pad of a `sensitive` net outside the cell and of every cable
connector, a marker with the field level at the pad centre (the useful number:
the field at the victim), also listed beside the map with the distance from the
loop. The colour scale is a fixed 16-step viridis-like table over 40 dB below
the map maximum; cells below it are transparent. Grid cells are 0.25 mm, with a
larger step when the window would exceed 200 cells a side; runs of equal colour
in a row are one rect.

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

Limits, which the legend and footer repeat: a ranking and placement aid, not a
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
(`copper_emi::circuit`, modified nodal analysis, one solve per harmonic):

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
  branches is not modelled.
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
* Continuous conduction is checked: `I_out >= dI/2` (the valley of the ramp is not
  negative). Otherwise (`measures.operating_point.status = outside_model`, tag
  `operating_point_outside_model`; discontinuous conduction, pulse skipping and
  forced-PWM light-load operation) the model does not apply and says so: the
  declared point is not evaluated, the detector comparison is not touched, and the
  current-limit scenario is the only one reported (without a `current_peak` the
  prediction is skipped, coverage `not_applicable` with the reason).
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
`margin_avg_db`, `margin_db` (from the high end); `measures.assumptions`
holds the `predict` assumptions and the tool's. Tests:
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

Status: fourth iteration implemented in `crates/copper-fullwave`. Screening
evidence only: nothing here qualifies a design. Iteration 3 found where the cell dependence of the
loop inductance came from (the rule that moves copper edges, not the port), replaced the rule, completes
the records of loops that ring instead of waiting for them to decay, and states a mesh sensitivity next to
the inductance. Iteration 4 puts a floor under the cells that dense copper forces on the time step
(a third fewer steps and half the run time on the TPS62130A), runs the validation models on the default edge
rule, and found that the strap bias is not what its validation test measures (below).

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
  ends), arcs (chord chains), copper polygons, zones and the copper of hard macros. A zone is its
  outline minus its declared holes, an octagonal antipad where a via or plated hole of another net
  crosses it and, new in iteration 2, the zone clearance around the copper of other nets on the same
  layer (pads, tracks, bridges, legs: a polygon difference with `copper_cam::poly`, one keyhole sheet
  per piece that remains; flag `zone_cut_around_foreign_copper`). Fills are unknown (flag
  `zone_outline_used`): thermal reliefs, minimum widths and islands are not applied. Without the cut, a
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
  moved by the refined cell. The rule before iteration 3 was a third of the lateral cell and nothing vertical, fitted on
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

Always run in `cargo test -p copper-fullwave` (59 s for `validation`, 13 s for `shorted_line`, debug build with the optimised solver): 3a at the coarse cell (Z0 within 5 %,
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

**Limitations.** Components are not modelled (the capacitor and diode are
copper; the switch is a port; their ESL, ESR, package and the controller's
bond wires are absent), copper is lossless, zone fills are unknown (the
outline minus clearances, no thermal reliefs or minimum widths), vias are wires without radius, the window
cuts every net, there is no enclosure or cable, and no near-field export or radiated estimate. A
placed board's straight legs may cross foreign pads (the closed form has the
same blind spot); overlapping lines of different nets within 0.075 mm (a tenth of the cell without
smoothing) merge (flag `nets_merged_below_floor`). Round pads are chamfered. A strap port adds
a strap that the real loop does not have (`port_strap_nh`, the closed-form inductance of the strap bar alone over its plane; 0.95 nH of the
TPS62130A's 2.83 nH and 1.78 nH of the CopperOrgan loop). That closed form was not made trustworthy in iteration 4 (next steps). A package's exposed pad is bridged in the air at an assumed height of 0.3 mm. The hull
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
348 MHz with the capacitor and diode shorted). (2) Zone fills from the CAM fill (`copper_cam::fill`)
instead of the outline minus clearances, and per-layer antipads, so planes carry their real slots.
(3) Near-field export and radiated estimates by per-port superposition. (4) The mesh decisions on small geometry:
a merge that does not move copper edges by up to 0.4 of the cell
(0.5 of the cell moved the TPS62130A by -2.4 % and 0.6 the placed emi_buck by +5 %: the merge distance is where the remaining
mesh sensitivity sits); the floor under the cells is done (above), the sub-0.05 mm cells of the 0.10 mm refinement of emi_buck remain
(0.041 mm: the gaps themselves). (5) The strap bias. `port_strap_nh` is the closed form of the strap bar alone over its plane
and it was not made trustworthy: `strap_port_against_direct_port` does not measure it. The strapped model of that test is the loop with
a continuous top bar (the strap fills the gap) and a port of 0.4 to 0.5 mm in its middle, so its inductance does not depend on the gap
(8.635 nH at gaps of 2 and 3 mm), while the direct port spans the gap and carries the inductance of the gap's own field; the measured
difference (+0.338 nH at 2 mm, +0.316 nH at 3 mm, against closed forms of 0.526 and 0.834 nH) is the strap bar, 0.36 mm wide, against the
0.5 mm track it replaces, plus the direct port's gap, not the inductance of a strap that the real loop lacks. A bias that can be
validated needs the reference of the real loop (a die path of known inductance, or a bridge shortened by moving its posts: the
inductance extrapolated to a bridge of zero length, two more solves), and was not done. A package
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
* **Vias** are tube barrels along z joining the node nearest to the via on every layer it passes where its net has
  copper within twice the coarse pitch, as a chain of segments of the height difference of the layers. A via that joins
  fewer than two layers is listed (`via_unattached`) and left out.

### 13.3 Mesh and budget

Each layer has its own tensor grid of lines; layers couple through the partial inductance matrix and are joined only by
vias. Nodes sit on line crossings; an x (y) filament joins two neighbouring nodes of a row (column) with the strip between
the half-way lines to the neighbouring rows (columns) as its width times the fraction of its body that the layer's copper of
the net covers (so a bar whose edges lie on lines is tiled exactly and has exactly `rho l / (w t)`; a bar between lines still
carries the right amount of copper; a body covered by less than 5 % is dropped and counted). Filaments of different nets never
share a node. Lines are placed on the bounding box edges of every outline and hole of the copper, of every terminal and hint,
and at every via; between them at the fine pitch (`cell`, default 0.2 mm) where a shape is narrow (at most eight cells) in
that axis, near vias and terminals, and in hints (strips along the loop's tracks on every layer, reaching four layer
distances to either side with a pitch of half the layer distance); the pitch grows by 1.4 per unit distance to the coarse
pitch (five cells) elsewhere. Holes and islands no larger than three cells ask for no lines of their own. Lines closer than
a third of the pitch merge. Mirror-symmetric layouts get mirror-symmetric lines (the lines are marched from both ends of a
span towards its middle), which the equal via shares below rely on.

**Budget.** The inductance matrix is dense (`n_b^2` doubles) and a frequency costs one complex elimination of about
`n_b / 2` loops, so the number of filaments is bounded: `--extract-budget` (default 2400, at most 20000). At 2400
filaments the assembly takes seconds and each frequency a second or two in the optimised-dependency debug build used for the
tests (`copper-extract` and `copper-emi` are built with `opt-level = 2` in `[profile.dev.package.*]` of the workspace
`Cargo.toml`; there was no per-crate precedent there, only the rule for third-party crates). A mesh over the budget is coarsened by
1.3 (all pitches, hints included) up to ten times, flag `coarsened`, status `indicative`; if it still does not fit the
extraction fails with the line and bar counts per layer. For the 3 mm margin around a hot loop that fits the default
budget (the placed `emi_buck`) the fine pitch ends at 0.34 mm; the real board of section 13.7 needs 5000 to 7000
filaments to reach a pitch near one millimetre, which is still too coarse for its pads.

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
draws across the body), of the net of one pad and joined to the other pad by a contact one sixth of a cell deep. A contact is the equipotential
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
diagonal conductor as a staircase: a 10 mm, 0.5 mm wide bar at 45 degrees reads 6.94 nH on a 62.5 um mesh against 7.26 nH
for the same bar along an axis (-4.4 %, converging slowly: -6.7 % at 250 um), and the loop of `emi_buck` has two diagonal legs.

### 13.6 Outputs

`measures.extract` of the `EMI-HOT-LOOP` finding (and the `extract:` line, and the coverage entry `EMI-EXTRACT:<cell>`):

| Key | Meaning |
| --- | --- |
| `inductance_nh`, `resistance_mohm`, `frequencies_mhz` | `Im Z / w` and `Re Z` of the finer of the two meshes at 1, 10 and 100 MHz |
| `closed_form_nh`, `ratio_to_closed_form`, `fullwave_nh`, `ratio_to_fullwave` | the traced closed form of the finding and, when `--fullwave` ran, its inductance, with the ratios |
| `status`, `status_reasons` | `ok`, or `indicative` with the reasons: coarsened to the budget, a terminal joined by its nearest node because no node lies inside it, vias left out, two meshes more than 10 % apart at 10 MHz, a single mesh |
| `numerical_sensitivity_pct`, `numerical_sensitivity_basis` | the difference between the two meshes (pitch ratio 1.5) per frequency; `null` with `numerical_sensitivity_unavailable` for one mesh. Two meshes that agree do not bound the error: the mesh series below still moves |
| `parameter_assumptions` | what the board does not fix: assumed stack-up, copper thickness, plating, window, resistivity |
| `not_modelled`, `flags`, `notes` | the omissions of 13.1; `assumed_stackup`, `assumed_copper_thickness`, `assumed_plating`, `coarsened`, `plane_corridor`, `merged_terminals`, `terminal_snapped`, `via_unattached`; how each leg and component was drawn |
| `meshes` | per mesh: role (`standard`, `refined` or `coarser`), pitches, filaments, nodes, loops, barrels, coarsening steps, L and R, seconds |
| `via_currents` | per frequency, the vias that carry at least 1 % of the port current: current per ampere into the port and `share_of_net`, `|I| / sum |I|` over the vias of the same net (magnitudes, adding up to 1 whatever the phases) |
| `nets`, `other_nets`, `window_mm`, `layers`, `copper`, `seconds`, `filament_budget`, `method` | what was modelled, the fill (`zone_fills`, `islands_removed`, `fill_seconds`) and the cost |

Nothing is a pass or a fail and no accuracy is claimed. The second mesh is a finer one (the standard pitch divided by 1.5)
when it fits the budget, else a coarser one (multiplied by 1.5); the reported numbers are the finer mesh's. Without
`--extract` nothing changes, and everything stays in the one `emi` section of the route report.

### 13.7 Validation

Every case has a reference written in the test (`crates/copper-extract/tests/analytic.rs`, run by default unless marked
`ignored`) or a closed form of `copper_emi::inductance`, and a series over the mesh. The whole default suite of the crate
takes about 35 s in the debug build of this repository (12.7 s for `analytic`, 12.6 s for `boards`).

Numbers below were measured in this repository (debug build with dependencies at `opt-level = 2`); "filaments" is
the size of the dense system. The tests assert the bounds given in the last column, not the measured values.

| Case (test) | Reference | Mesh series (filaments) | Result | Asserted |
| --- | --- | --- | --- | --- |
| Straight bar 10 x 1 x 0.035 mm (`straight_bar_matches_...`) | `self_inductance_bar` 6.9864 nH; `rho l / (w t)` 4.9143 mohm | pitch 250 / 125 / 62.5 um (139 / 348 / 1006) | L(10 kHz) 6.9864 / 6.9864 / 6.9861 nH (0.000, 0.000, -0.003 %); R(DC) exact to 1e-6 | R within 1e-6, L within 2 % |
| Two parallel bars, 1.0 and 0.4 mm, driven in parallel (`two_parallel_bars_...`) | 2 x 2 solution with `self_inductance_bar` and the filament-averaged `mutual_bars` (L 6.986 and 8.679 nH, M 3.513 nH, R from `rho l / (w t)`) | 250 / 125 / 62.5 um (216 / 491 / 1402) | DC share of the wide bar 0.7143 = conductance share (to 1e-3); shares at 0.1, 1, 10, 100 MHz and 1 GHz within 0.61 % of the hand solution (+0.2 to +0.6 % at 1 to 10 MHz, the lateral mesh lets each bar's current crowd, the hand solution does not) | DC within 0.1 %, all within 1.5 % |
| Rectangular loop 10 x 6 mm in free space, trace 0.5 / 0.25 mm, 1 mm gap, port across the gap (`rectangular_loop_in_free_space_...`) | `loop_inductance` of the five centreline segments: 20.629 / 24.450 nH | 250 / 125 um (157 / 358; 123 / 210) | L(10 kHz) -1.65 / -2.71 % (0.5 mm), -5.30 / -0.44 % (0.25 mm); L(100 MHz) at 125 um 19.39 and 24.18 nH | finest mesh within 5 % |
| The same loop over a floating plane 0.2 mm below, plane 16 and 32 mm (`rectangular_loop_over_a_plane_...`) | `loop_inductance_over_plane` (image plane at the plane layer's centre): 7.8037 nH | 0.4 mm pitch near the loop, 3 mm far (1454 / 2276) | L(1 MHz) 7.769 / 7.762 nH (-0.45 / -0.53 %); L(10 MHz) 7.435 / 7.428 (-4.7 / -4.8 %) | L(1 MHz) within 5 % |
| Plane growth and mesh series of that loop (`study_loop_over_a_plane_...`, ignored, 64 s) | the same | 0.5 mm pitch, plane 16 / 24 / 32 / 48 / 64 mm (1058 ... 3132); 24 mm plane, pitch 800 / 500 / 350 / 250 um (909 / 1438 / 2485 / 3356) | L(1 MHz) 7.8660 / 7.8611 / 7.8596 / 7.8578 / 7.8571 nH: the finite plane lowers towards the infinite one as it grows, by 0.11 % from 16 to 64 mm, and the 64 mm value is +0.68 % over the image result. Mesh series, L(1 MHz): -1.95, +0.74, -0.63, -0.13 %; L(10 MHz): -6.0, -2.9, -4.9, -4.9 % | - |
| Shorted microstrip, w 1.0 mm, h 0.5 mm, 10 mm (`shorted_microstrip_matches_...`) | `Z0 sqrt(eps_eff) l / c` (Hammerstad-Jensen, the low-frequency limit of `Z0 tan(beta l) / omega`): 2.9814 nH zero thickness; 2.9019 nH with the classical effective width for 35 um copper | pitch 500 / 250 um (938 / 2174) | L(1 MHz) -0.85 / -1.15 %; L(10 MHz) -2.95 / -4.37 % of the zero-thickness value, -0.29 / -1.75 % of the thick one | L(10 MHz) within 5 % of zero thickness |
| Shorted microstrip, the four geometries of `copper-fullwave/tests/shorted_line.rs` at 20 mm (`study_shorted_microstrip`, ignored, 148 s) | zero-thickness value / thick value | pitch 250 um (1792 to 2564) | L(10 MHz) of 35 um copper against zero thickness / thick: h 0.5 w 1.0: -3.2 / -0.6 %; h 0.2 w 0.5: -5.0 / -0.6 %; h 0.5 w 0.5: -2.5 / +1.9 %; h 0.3 w 1.5: -4.2 / -2.3 %. L(100 MHz) is 0.1 to 0.2 % lower. With 5 um copper at 100 MHz against zero thickness: -0.3, +1.4, +1.0, -0.3 %; with 1 um at 100 MHz: +0.8, +3.6, +2.3, +0.9 % (at 1 MHz the thin copper is still resistive and L is 15 to 115 % high) | - |
| Wall of vias instead of the ideal short (same study, h 0.5 w 1.0, 10 mm) | ideal short 2.8511 nH | 2 and 4 barrels (0.1 mm drill, 25 um plating) | 2.8865 / 2.8371 nH (+1.2 / -0.5 %) | - |
| Slot in the plane under a 10 x 0.5 mm trace, plane 12 mm wide, slot 2 mm wide (`a_slot_under_the_trace_...`) | unslotted: Hammerstad with the thick correction 2.4994 nH; limits: the unslotted and the fully slotted plane | pitch 0.5 mm (700 to 1317) | L(10 MHz) for slot 0 / 2 / 4 / 6 / 8 / 10 mm: 2.583 / 3.330 / 3.961 / 4.589 / 5.222 / 5.887 nH, 1.00 / 1.29 / 1.53 / 1.78 / 2.02 / 2.28 times the unslotted value, rising with every step and bounded by the intact plane below and the slot over the whole trace above (the "no plane" limit has no finite value for this port: its return would be the far edge) | monotone rise, last above 1.3 times the first, unslotted within 10 % of the reference |
| Four vias between two pours, symmetric (`parallel_vias_...`) | 1/N | uniform 1 mm mesh (628) | shares 0.25000 for all four at DC, 1 MHz and 100 MHz | within 1e-6 |
| The same with one via moved from 3 to 5 mm from the pads | the far via carries less; shares add to 1 | same | DC 0.2761 / 0.2723 / 0.2761 / 0.1756; 1 MHz 0.2782 / 0.2736 / 0.2782 / 0.1699; 100 MHz 0.2796 / 0.2745 / 0.2796 / 0.1663 | far via below the others and below 1/4, sum 1 |
| Two ports on two separate bars, 1.5 mm apart (`a_two_port_matrix_...`) | `mutual_bars` M 3.4875 nH; `self_inductance_bar` 8.2723 nH | 125 um | M12 = M21 = 3.4871 nH at 10 kHz (0.01 %), L11 = L22 = 8.2723 nH | reciprocity 1e-6, M and L within 2 % |
| A floating shorted ring beside a bar (`network::tests::...`) | the eddy current must lower the port inductance | hand-built network | lowered | yes |
| Partial elements (`partial::tests::...`) | exact mutual of a bar with itself equals the self inductance; centre filaments against the exact form at the switching distance | - | 1e-9; 0.26 % worst | 0.3 % |
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
four copper layers, a floating ground fill on In1.Cu, stack-up undeclared) and routed with the three 0.4 mm legs of the CLI
tests. The full-wave values are the ones recorded in section 11 (the FDTD solver was not run here). Extracted values are
`L(1 MHz) / L(10 MHz) / L(100 MHz)` of the default run (budget 2400, two meshes, fine pitch 0.34 mm after two coarsening
steps), with the difference between its two meshes at 10 MHz.

| Board | Traced closed form | Full wave (section 11) | Extracted, default | Mesh difference at 10 MHz | Run time |
| --- | --- | --- | --- | --- | --- |
| `emi_buck` placed | 4.973 nH | 4.84 nH (standard 4.841, refined 4.813) | 4.252 / 4.034 / 4.025 nH, R 10.4 / 12.3 / 12.4 mohm | 3.2 % | 5.6 s (whole `copper emi-check --extract` command) |
| `emi_buck` routed | 6.712 nH | 5.97 nH (standard 6.067, refined 5.974) | 5.665 / 5.383 / 5.361 nH, R 17.8 / 21.8 / 22.3 mohm | 3.1 % | about 6 s (inside the test) |
| `emi_buck_filter` placed | 4.973 nH (the same hot loop: the filter is upstream of it) | 4.84 nH | identical to `emi_buck` placed | 3.2 % | 5.7 s (whole command) |

Mesh series with the budget lifted (`study_emi_buck_mesh_series`, ignored, 315 s; the fine pitch is the default times the
scale, and the mesh is not coarsened):

| Scale | Fine pitch | Placed: filaments, L(1 / 10 / 100 MHz), R(10 MHz), time | Routed: filaments, L(1 / 10 / 100 MHz), R(10 MHz), time |
| --- | --- | --- | --- |
| 2 | 0.40 mm | 1347, 4.234 / 3.991 / 3.978 nH, 13.0 mohm, 3 s | 1307, 5.670 / 5.418 / 5.400 nH, 21.1 mohm, 3 s |
| 1.5 | 0.30 mm | 2095, 4.237 / 4.007 / 3.995, 12.6 mohm, 6 s | 1943, 5.655 / 5.320 / 5.264, 23.8 mohm, 5 s |
| 1 | 0.20 mm | 4403, 4.251 / 4.002 / 3.982, 13.4 mohm, 33 s | 4169, 5.681 / 5.407 / 5.381, 21.4 mohm, 25 s |
| 0.75 | 0.15 mm | 7510, 4.321 / 4.042 / 4.010, 14.8 mohm, 110 s | 7598, 5.746 / 5.439 / 5.396, 23.0 mohm, 130 s |

The series is flat within 1.3 % (placed) and 2.2 % (routed) at 10 MHz from a pitch of 0.4 to 0.15 mm, so the default
run's number is a property of the model and not of the mesh, and the default's 3 % two-mesh difference is of the size of
the series' scatter.

**Reading the difference to the closed form and the full wave.** Extracted over full wave is 0.83 (placed) and 0.90
(routed) at 10 MHz; over the closed form 0.81 and 0.80. The extraction is not tuned to match and does not. What was
measured about the gap: (1) the in-plane bar model alone, with the plane removed, reads 6.42 nH against 7.77 nH for the
closed form of the same loop in free space (-17 %), so the deficit is not in the treatment of the plane; (2) drawing only
the six bars (pad centre to pad centre, 1 mm wide) over the plane, with a 1.4 mm gap at the port, gives 4.10 nH against
4.21 nH for the closed form of the same open path (-2.6 %) and the pads' copper changes it by 0.01 nH; (3) a 45 degree
bar reads 4.4 % below the same bar along an axis (13.5), and two of the six legs are diagonal; (4) the full-wave model
closes the capacitor and the diode with hull bridges of copper (3 mm long) where the extraction uses 1 mm bars, and its
port straps add a bias that section 11 states. These do not add up to the whole difference and the remaining part is
not attributed; nothing was fitted. The extraction also keeps the die out and puts the port in a one-cell gap, as the
closed form and the full wave do not exactly.

**CopperOrgan routed board** (`copper emi-check board.copper --locked --run build/ci/emi-check-20261111T161807 --extract
--json`, read-only; traced path 54 mm through two vias and two layers, closed form 9.889 nH with `plane_shortcut`, full wave
indicative 6.5 to 7.3 nH, not converged; window 31.4 x 14.3 mm; nets GND, SW, V24; eight zone fills of which 16 shapes
reach the window, 291 islands removed by the fill in 14.7 s):

| `--extract-budget` | Fine / coarse pitch | Filaments (standard + second) | L(1 / 10 / 100 MHz) | R(1 / 10 / 100 MHz) | Two meshes differ at 1 / 10 / 100 MHz | Time (extraction, whole command) |
| --- | --- | --- | --- | --- | --- | --- |
| 2400 (default) | 2.12 / 10.6 mm | 1992 + 1160 | 4.302 / 3.777 / 3.758 nH | 18.2 / 22.2 / 22.5 mohm | 1.4 / 7.1 / 7.4 % | 15 s, 34 s |
| 4500 | 1.25 / 6.27 mm | 3927 + 2389 | 4.928 / 4.272 / 4.234 nH | 19.8 / 26.6 / 27.5 mohm | 6.0 / 5.5 / 5.3 % | 46 s, 65 s |
| 7000 | 0.97 / 4.83 mm | 5439 + 3614 | 5.146 / 4.477 / 4.399 nH | 20.4 / 29.0 / 33.7 mohm | 4.1 / 5.7 / 5.2 % | 91 s, 108 s (a compile ran meanwhile) |

Via currents per ampere into the port (budget 7000): the two ground vias at (64.47, 53.50) and (85.40, 48.50) mm carry 1.00
each at every frequency (a third of the magnitudes of the net's barrels each; the loop changes layers through them),
the ground pad barrels at (70.30, 54.50) and (71.70, 54.50) mm carry 0.23 to 0.25, a ground via at (74.50, 54.36) mm 0.17 and
the two V24 vias at (59.52, 53.50) and (67.50, 54.36) mm 0.15 to 0.17 each (half of the net's via magnitudes each).

**Is it trustworthy? No.** The status is `indicative` at every budget, for two reasons the mesh series itself shows: the
inductance has not stopped moving (3.78, 4.27, 4.48 nH at 10 MHz for pitches 2.1, 1.25 and 0.97 mm: +13 % and +5 % per
step, while the two-mesh difference of each run reads only 5 to 7 %), and the pads (about 0.6 mm) are smaller than the
pitch, so every contact and the port terminals are joined by their nearest node (`terminal_snapped`). The values are
well under the closed form (0.38 to 0.52) and under the indicative full-wave figure, and the converged value is not
known; a mesh with a pitch of 0.2 mm would need tens of thousands of filaments over this window (the 0.97 mm mesh already has
5439, and the count grows about as the square of the inverse pitch), beyond what the dense solve holds (a smaller window,
or a sparse or accelerated solver, would be needed). Do not read the 4.4 nH as the loop's
inductance of this board; read it as a lower figure that rises as the mesh is refined.


### 13.9 What is integrated and what is not

Integrated: `copper emi-check --extract` (hot-loop interconnect impedance beside the closed form and the full-wave value,
with its own measure block, status, flags, numerical sensitivity, parameter assumptions and omissions). Not integrated:
the multi-port extraction of the non-hot capacitors' connections into `EMI-CONDUCTED` (the connection inductance there
is still the estimate of section 10). The extraction itself takes any number of ports (the two-port test above reads the
mutual inductance of two bars to 0.01 % of the closed form) and `copper_emi::circuit` takes coupled inductors
(`Circuit::mutual`), so the matrix could be realised as inductors with mutual couplings, but the conducted model lives in
`copper-drc`, which `copper-extract` depends on, so the connection would have to be supplied to it from the `copper` crate
and the cell's `EMI-CONDUCTED` finding recomputed; that is not done. A FastHenry input deck of a mesh is written by
`copper_extract::fasthenry::deck` and an ignored test runs `fasthenry` when it is on the PATH; none is installed here, so
no comparison with FastHenry exists.
