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
| `current_peak` | current | Peak pulsed current through the pin (switch current limit or rated peak) |

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
`fall_time` not a positive time; `current_peak` not a positive current;
`rise_time`/`fall_time`/`current_peak` given without `emi_role` being one of
the aggressor roles is allowed (a clock output may declare an edge rate).

IR: `profile.emi = { "role", "rise_time", "fall_time", "current_peak" }`, each
key present only when declared; the whole object absent when nothing is. The
`fall_time` default is applied by readers (`EmiProfile::fall`), not stored.
The `emi_role` value is a bare name or a string; time attributes accept `s`,
`ms`, `us` and `ns`. Only the EMI attributes take time units: the general
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
diode and capacitor, plus the first declared `rise_time`, `fall_time` and
`current_peak` of the switch pins, so a layout checker needs no second lookup.
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
routed board places a reference the design does not have. A run directory that
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
`{ cell, provenance, tags, estimate: { quantity, value, unit, method, uncertainty_db, frequency_hz } | null, margin_db?, accepted?: { reason }, measures }`.
`measures` holds the further numbers of the finding by name. `tags` carry
`placement_estimate`, `traced_copper`, `partially_traced`, `unrouted_leg`, `plane_shortcut`, `pads_only`, `inferred` (the cell comes from the
structural fallback), `provisional_placement` (a position involved is the
prototype placement, not fixed by a `fixed_placement` constraint or a fixed
macro anchor) and `macro_relative_placement` (the loop lies in one rigid macro
cluster: its geometry is fixed, the cluster's position is not).

| Code | Status | What is measured | Ranked by |
| --- | --- | --- | --- |
| `EMI-HOT-LOOP` | implemented; placement estimate, traced where copper joins the pads | Per cell (per `input loop` and `output loop` for a four-switch buck-boost), the loop capacitor -> hot-supply pad -> switch pad -> catch diode (switch side -> rail side) or, without a diode, hot-return pad -> capacitor return. The legs between pads of one net follow the shortest copper path when copper exists, else the straight pad-to-pad segment. Enclosed area (shoelace), path length, partial inductance over the nearest inner ground plane (free space without one), differential-mode field, ringing, ring energy; traced length, vias, layer changes, narrowest width | differential-mode field in dBuV/m, then loop area |
| `EMI-SWITCH-NODE` | implemented, pads only unless copper exists | Per switch net: copper area of the pads (and of tracks, polygons, zones and macro copper when present), island capacitance to the nearest ground plane, distance to the board edge and to each cable connector, voltage-driven common-mode field | voltage-driven common-mode field in dBuV/m, then copper area |
| `EMI-PROXIMITY` | implemented | Inductor and switch-node pads against pads of `sensitive` nets and of cable connectors, closer than a threshold in units of the inductor size | threshold / distance |
| `EMI-CONDUCTED` | implemented for `buck` cells (section 10) | Per buck cell: the conducted level at the input, 150 kHz to 30 MHz, dBuV at a 50 uH V-network, differential mode only. Worst harmonic (frequency, level, quasi-peak and average margin with a `cispr32_class_a`/`cispr32_class_b` `emission` constraint), the fundamental, harmonics above each line, and every harmonic in `measures.harmonics` | margin when margins exist, else level; after the radiated findings (a different quantity, dBuV against dBuV/m) |
| `EMI-ACCEPT-UNUSED` | implemented | An `emi_accept` that matched no finding | listed last |
| `EMI-RETURN-PATH` | coverage `unsupported` | needs filled copper and traced routed copper | - |
| `EMI-DECOUPLING-PATH` | coverage `unsupported` | needs routed copper | - |

Every check that cannot run on the given input gets a coverage entry with
status (`executed`, `not_applicable`, `unsupported`) and reason; nothing is
reported as a pass for what was not measured. Standing entries:
`EMI-RETURN-PATH` and `EMI-DECOUPLING-PATH` (unsupported),
`EMI-COPPER-TRACE` (traced copper path: `executed` when at least one copper leg of a hot loop is traced, else `unsupported` with the reason), `EMI-CABLE-COMMON-MODE`
(current-driven common mode on cables, unsupported), `EMI-MARGIN` (whether an
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
  `inductance_method`); via barrels are added in series with
  `via_inductance(height, drill)` (no mutual terms with the bars; a barrel with
  height under a quarter of its diameter is omitted and counted). Plane
  shortcuts get a bar width of the straight length clamped to 1 to 5 mm. An
  arc contributes its chord chain to the polygon and its arc length to the
  path length. The pad chosen at each loop position is the one with the
  shortest pad-centre cycle, not re-optimised for the traced path.
- **Radiated estimate.** Frequency: the fixed value, else the upper bound of
  a declared range; edge: the faster of `rise_time`/`fall_time`; amplitude:
  `current_peak` (an upper bound for the pulsed current); duty: Vout/Vin when
  the hot-supply net and the magnetic's output net both have a declared supply,
  else 0.5. The envelope `trapezoid_envelope` is swept on a log grid over
  30 MHz to the lower of 1 GHz and `c / (10 * path length)`; the reported level
  is the least-margin point against the `emission` line, or the maximum level
  without one, at the standard's distance (10 m for CISPR 32, 3 m or 10 m for
  FCC 15 per the table; 3 m without a standard). Free-space value; the +6 dB
  ground-plane factor is not applied. Peak envelope against quasi-peak limits.
  Skipped with a reason when the frequency, an edge time or `current_peak` is
  missing.
- **Ringing.** With `output_capacitance` declared on the controller part
  and/or the catch diode part and a computed inductance: `ring_frequency`,
  `ring_overshoot` (needs `current_peak`); ring energy `0.5 L I^2` needs only
  the inductance and `current_peak`.
- **Switch node.** Per switch net: pads, tracks (arcs by chord), polygons and
  zones summed; capacitance is the sum of `island_capacitance` of each element
  over its own layer's nearest ground plane (dielectric height from the
  stack-up, relative permittivity 4.5 when undeclared). The common-mode field is
  `voltage_driven_cm_field` (constant 115, unverified) with the envelope of
  the hot supply's declared voltage as swing, scaled from 3 m to the standard's
  distance, swept like the loop. `pads_only` is set when the net has no tracks,
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
  frequency. Every absolute level carries `uncertainty_db = 15`. Nothing is
  printed as pass or fail. The `cable_length` of the constraint is read but no
  cable estimate uses it yet.

Estimators (module `copper_emi::estimate`, all SI, all documented with their
source):

- Trapezoid envelope: corners at `1/(pi*t_on)` and `1/(pi*t_r)`; harmonic
  amplitude bound `2*A*d*|sinc(n*pi*d)|*|sinc(n*pi*t_r/T)|`.
- Differential-mode loop at distance r: `E = 1.316e-14 * I * f^2 * A / r`.
- Voltage-driven common mode at 3 m: `E = 115 * f * C * V` (V/m, f in Hz, C in F) per the
  Clemson voltage-driven summary; verify the constant against the source
  before relying on absolute values.
- Current-driven common mode: `V_cm = 2*pi*f*L_return*I`, `E ~= 0.365 * V_cm` at 3 m
  (not used by `emi-check` yet).
- Ringing: `f_ring = 1/(2*pi*sqrt(L_loop*C_oss))`.
- Limit lines for the `emission` standards, as tables.

Stated uncertainty of every absolute level: 10 to 20 dB (15 dB is used).
Margins are reported with that figure and never as pass/fail.

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
  existing run. `RUN` is the run directory (or a state file in it) and must hold
  the `route-report.json` of a finished run next to `final-board.state.json`
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
  uncertainty, margin, tags and the scalar measures, and the text output ends
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
gridline every 10 dB, one stem and dot per harmonic at its total level (the
differential-mode series is the total while common mode is not modelled; with
common mode present DM and CM are added as markers), the quasi-peak and average
lines as polylines through the per-harmonic `limit_qp`/`limit_avg` when an
`emission` standard with a conducted table is declared, harmonics above the
average line in the warning colour, a band and whisker for the stated
uncertainty of the worst harmonic, and the finding's tags (`worst_case_current`,
`assumed_edge_time`, `differential_mode_only`, ...) under the title. A board
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
- The amplitude is the cell's `current_peak` (the same worst case as
  `EMI-CONDUCTED`, tag `worst_case_current`); without one the map is
  normalised to 1 A and says "per ampere".
- Biot-Savart for each straight filament, `H = I/(4 pi d) (sin a2 - sin a1)`
  around the segment axis (zero on the axis, linear inside a 50 um core), summed
  over the loop; an ideal infinite ground plane at the reference plane's z by
  image currents (mirrored segments, reversed current: zero normal field at the
  plane, zero field behind it).

Limits, which the legend and footer repeat: a ranking and placement aid, not a
field prediction. The model is quasi-static (valid where the region is much
smaller than a wavelength: fine up to a few hundred MHz over a few cm), uses the
peak switch current as the amplitude of a single current, and only the hot loop
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

## 10. Conducted-emission prediction (phase 3)

Status: the linear-network prediction of `copper_emi::conducted` is wired into
`copper emi-check` as the finding `EMI-CONDUCTED` (module
`crates/copper-drc/src/emi/conducted.rs`), one per `buck` cell; other
topologies and cells whose inputs are missing get a `not_applicable` coverage
entry with the reason (a four-switch buck-boost needs the operating mode, which
the board does not declare). Inputs, each recorded with its source in
`measures.inputs` and in the coverage detail:

| Input | Source |
| --- | --- |
| Vin, Vout, duty | declared `supply` voltage of the hot-supply net and of the magnetic's output net; duty is Vout / Vin |
| Frequency | the fixed value; for a range the upper bound, tagged `frequency_range_bound` |
| Edge time | the switch pin `rise_time`/`fall_time` (10-90 %), divided by 0.8 for the 0-100 % ramp of the trapezoid; if undeclared, an assumed 10 ns (10-90 %) with only harmonics below `1 / (10 pi 10 ns)` = 3.18 MHz reported, tag `assumed_edge_time` (below that frequency the level changes by under 1 dB between 5 ns and 20 ns edges: tested) |
| Current | the switch pin `current_peak`, the current LIMIT used as the on-time amplitude (no load current exists in the language): a worst case, tag `worst_case_current`, level change `20 log10(I / current_peak)` dB at another current |
| Hot capacitor | the hot-loop analysis' selected capacitor: value from the component; `esr`, `esl`, `self_resonance` (ESL derived) from its part; otherwise 5 mohm and ESL by package from the footprint name (0402 0.4, 0603 0.5, 0805 0.6, 1206 0.8, 1210 0.9 nH), tag `default_capacitor_parasitics`; an unrecognised package without declared values skips the prediction. No DC-bias derating. Parallel hot capacitors are not modelled (`parallel_capacitors_not_modelled`, listed in `measures.ignored_parallel_capacitors`) |
| Hot-loop inductance | `EMI-HOT-LOOP` (traced or placement estimate), in series with the capacitor with the supply feed at the switch end (pessimistic) |
| Input filter | walks from the hot-supply net toward the supply (at most 4 series parts) through the elements that FEED it. A filter inductor is a `passive.inductor*` or `passive.ferrite*` part, or an uncategorized two-pin part whose value is an inductance; any other two-pin `dc_power_path` part (fuse, jumper, 0-ohm link, load switch) is a transparent DC feed that is followed through with its resistance unknown and taken as 0 (tag `feed_resistance_not_modelled`; "through fuse F_MAIN, resistance not modelled"). Direction comes from the far net: a declared `external` or sourced supply (or, failing that, a connector with no other load) means upstream; other loads there mean the element feeds them downstream and it is ignored (listed in the `:filter` note); if neither decides, the filter is left out and the note says the direction cannot be decided. Exactly one filter inductor may be on the path; its value, `esr` (DCR) and `self_resonance` (winding capacitance) plus the capacitors to the hot return beyond it are lumped in parallel. A path with fuses but no inductor reads "fed through fuse F1 from VIN_IN; no filter inductor on the feed path". Capacitors to the hot return on the hot-supply net count as hot capacitors; resistors above 1 kohm and diodes to the hot return (TVS, enable divider) are ignored with a note; anything else on the path nets (a load, a resistor of 1 kohm or less, a connector or source before the end of the path) leaves the filter out with a `:filter` coverage note. A filter inductor without `self_resonance` is ideal (`filter_inductor_ideal`: attenuation above its real resonance is overstated) |
| Common mode | not modelled (`:common_mode` is `not_applicable`). The island capacitance of `EMI-SWITCH-NODE` is to the board's own ground plane, which is the converter's return: its current closes without entering the LISN, so using it as the capacitance to the LISN reference would overstate common mode by the plane's shielding. The real path is board-to-reference capacitance (heat sink, chassis, cable), geometry outside the board model. Levels are differential mode only and tagged `differential_mode_only`; common mode often dominates above a few MHz |

Levels are RMS dBuV of each harmonic at the worst LISN port; for a fixed
frequency above twice the 9 kHz RBW peak, quasi-peak and average coincide. The
CISPR 32 mains-port quasi-peak and average lines are the reference limit
(`cispr32_class_a`, `cispr32_class_b` only; other standards and no constraint
give levels only). Uncertainty: 6 dB below 5 MHz, 10 dB above (literature
envelope 5 dB at low MHz, 7 to 12 dB above); margins are never pass or fail.
`measures.harmonics` lists `harmonic`, `frequency_hz`, `source_current_a`,
`dm_dbuv`, `cm_dbuv` (null), `total_dbuv`, `limit_qp`, `limit_avg`,
`margin_qp_db`, `margin_avg_db`, `margin_db`, `uncertainty_db`; `measures.assumptions`
holds the `predict` assumptions and the tool's. Tests:
`crates/copper-drc/tests/emi_check.rs` against a hand calculation on
`examples/emi_buck` and `examples/emi_buck_filter`.


Module `copper_emi::spectrum`: trapezoid and sampled-waveform spectra;
`copper_emi::lisn`: 50 uH / 5 uH LISN transfer; `copper_emi::receiver`:
CISPR 16-1-1 style peak, quasi-peak and average detection of a sampled
waveform by short-time FFT (bands A-D, 200 Hz / 9 kHz / 120 kHz RBW; detector
time constants are taken from the standard and cited in code). A linear MNA
solver for the extracted network plus LISN follows; nonlinear device models go
to ngspice as an optional external process.

## 11. Region-of-interest full wave (phase 4)

A general `PhysicalBoard`/filled-board to `ems` translator with multiple ports,
used for edge-rate-band questions only (ringing band, plane cavity, slot and
cable radiation, near-field maps by per-port superposition). Not started.

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
