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
copper emi-check BOARD --run RUN [--json]
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
`EMI-SWITCH-NODE:<net>` (and `:common_mode`) and `EMI-PROXIMITY:<cell>`, each
with the missing input when skipped.

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
node copper is counted, not traced), `EMI-RETURN-PATH`, `EMI-DECOUPLING-PATH`,
current-driven common mode on cables, and the `copper issues` stage and
`copper view --issue` highlighting.

**Extension points for `copper issues` / `copper view`.** The findings'
`measures.loop_polygon_mm` and `nets` map directly to a `RunIssue`
(`crates/copper-report/src/issues.rs`: `RunIssue::new(id, "emi", code, title)`
with `.nets()`, `.layers()`, `.objects()` and `.points()`, whose bounding box
`RunIssue::window` frames). It was not wired in because the run report is the
source of the issues: `run_issues` reads only `route-report.json`, whose
digest the route summary records, so adding a stage means either storing the
EMI document inside the report (changing the digest contract) or having
`load_run` read a sidecar `emi-check.json` next to it; plus a new `emi` entry
in the fixed `STAGES` array of `crates/copper/src/issues.rs` (and the stage
list of its `USAGE` and help text, which tests compare) and in the stage
filters of `view.rs`. A sidecar written by `emi-check --run` is the smaller
change, about 60 lines and tests; it needs a decision on where the file lives.

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
