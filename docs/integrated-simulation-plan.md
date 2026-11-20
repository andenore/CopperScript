# Integrated simulation plan

Status, 20 November 2026: the simulator exists and is on branch
`integrated-simulation-plan` (not merged to main). This document is the plan
for what comes next. What is built is summarised under [Built](#built); its
syntax is in the [language reference](language-reference.md#simulation). The
detailed build notes of the first phases (P0 to P3) are in the history of this
file at commit `2361c46d`.

Goal: circuit simulation as a normal part of a CopperScript project, at the
level people use LTspice for: put sources on a circuit, run AC, transient,
noise and DC analyses, step parameters, read measurements, and fail the build
when a measurement leaves its limits. The board source stays the single
description of the circuit; simulation never needs a second schematic.

The first consumer is the
[impedance analyzer example](../examples/simulation/impedance_analyzer/SPEC.md).

Earlier research, written against the removed Python implementation, is in
[integrated simulation options](integrated-simulation-research.md) and
[the analog example note](simulation-analog-research.md).

## Decisions

1. **One solver, written in Rust, inside `copper`.** No ngspice, no other
   external tool, nothing to install.
2. **The board is not rewritten.** Sources, loads and the device under test
   live in a `bench`. A bench may replace a component with a stand-in but must
   say so.
3. **Model fidelity is always visible.** Every result names the model each
   part ran with: `ideal`, `macro` or `vendor` (file and hash). A missing
   model is an error. A vendor file that uses something the reader does not
   support is rejected with the line and the construct named; it is never
   approximated.
4. **Vendor files are fetched, not committed.** A model is a URL plus SHA-256
   on the part's `sim spice` block, fetched by `copper models fetch` into a
   gitignored cache. `copper sim` never downloads.
5. **Limits are assertions.** `copper sim` exits non-zero when a `measure`
   with a limit fails.
6. **Existing results do not move.** Every phase records the measure values
   and CSVs of all examples before it starts and compares them at the end.
   Differences are either zero or explained.

## Built

| Area | What works | Where |
| --- | --- | --- |
| Analyses | Operating point, AC, noise, transient, DC sweep, loop gain, poles and zeros; `step` (cartesian and zipped), `reference` runs, `ic` and `uic` | `crates/copper-sim`, `crates/copper-simulate` |
| Statistics | `tolerance`, matched groups, `montecarlo`, `worstcase` | `copper-lang/src/tolerance.rs`, `copper-simulate/src/variation.rs` |
| Built-in models | `sim opamp`, `diffamp`, `source`, `load`, `diode`, `switch`, `regulator`, `reference`; supply current and PSRR; op-amp clipping, slew and current limit in transient and DC sweep | `copper-sim/src/models.rs`, `devices.rs` |
| Vendor models | `sim spice`: SPICE3, PSpice and LTspice netlists; behavioural sources; Gummel-Poon BJT, JFET, MOSFET levels 1 and 3 with Meyer gate charge; ideal delays and lossless transmission lines | `crates/copper-spice`, `copper-sim/src/semiconductors.rs`, `behavioral.rs` |
| Bench language | `bench`, `fixture`/`using`, `supply`, `model`, `replace`, `ignore`, measures with limits and `where`, `upto`/`downto` (with a "never" result), `if()`, windowed reductions, waveforms; several analyses per bench, several noise outputs and `noise()` of a derived quantity, parameter sets per supply, matched groups on model parameters, fixtures using fixtures | `copper-lang/src/bench.rs`, `expression.rs` |
| Output | Text table, `sim-results.json` (`copperscript-sim/v0.1`), CSV and an HTML page with plots per bench | `copper-simulate/src/results.rs`, `report.rs` |
| Examples | `rc_filter`, `howland_pump`, `preamp`, `precision_rectifier`, `spice_amplifier`, `vendor_models`, `impedance_analyzer` (14 benches) | `examples/simulation/` |

Checked by closed-form tests (RC, RLC, Howland output impedance, kT/C noise,
device equations, self-written op-amp subcircuits against their design) and
by loading real Texas Instruments models (eleven amplifiers give a gain
bandwidth within the spread expected of a macro model; the table is in
`docs/language-reference.md`). The OPA810 bias point matches the one PSpice ships with the model
within 24 uV on all 90 nodes.

Known weak spots of what is built:

- Transient results at the default tolerances carry about 0.2 % integration
  error on the analyzer's start-up bench: a change of rounding in the solver
  moves its measures by that much. Harmonics below about 1e-5 % are inside
  that error and are not meaningful.
- The TL431 vendor model loads but does not regulate at its operating point
  (100 V at 10 mA; a current ramp in transient settles at 2.0 V against
  2.495 V). The cause is not established.
- Nothing has been compared against a PSpice or LTspice run of the same
  netlist beyond the OPA810 bias point. Vendor-model transients are slow in
  the debug build (an OPA2277 slew test did not finish in 40 minutes), so the
  slew checks are opt-in and were not rerun.
- No Analog Devices model has been loaded: analog.com refuses automated
  downloads.
- Everything runs at 27 degC.
- The HTML reports were looked at once; nobody has used them for real work.

## Forward plan

Phases are ordered by what blocks real use today. Size is a rough guide: S is
one focused working session, M two to three, L more than that or work with
open design questions. Each phase ends with its acceptance checks passing and
decision 6 holding.

| Phase | What | Size | Needs |
| --- | --- | --- | --- |
| F1 | Speed | M | none |
| F2 | Bench language gaps | M | none |
| F3 | Stability analysis | M | none |
| F4 | Supply current and rails | M | none |
| F5 | Layout parasitics | L | a routed analyzer board |
| F6 | Model library | M | permission to edit CopperLib |
| F7 | Device and netlist coverage | L | F1 |
| F8 | Temperature | M | F7 for device scaling |
| F9 | Spectrum and long-run analyses | M | F1 |
| F10 | Viewing and editor support | L | stable JSON |
| F11 | One solver for all checks | M | F1 |

### F1. Speed

Built on 20 November 2026, except device-form allocation (below).

Result, debug build, 12 threads: the analyzer's `Distortion` bench went from
about 19 minutes to 19 s, the whole analyzer example from about 31 minutes to
41 s. A release build runs the whole example in about 10 s (reported by the
implementing agent, not re-measured).

- `copper sim --timing` prints wall time, step counts and seconds per phase
  for each bench; `--jobs N` sets the thread count (default one per core).
- `copper-sim/src/sparse_lu.rs`: Markowitz ordering with threshold pivoting,
  refactorisation in the same pivot order, real and complex. Used by
  transient, DC, AC and noise. A matrix it refuses goes to the dense LU, so
  singular circuits keep their diagnostics.
- The fixed stamps are made once per step size; each Newton iteration
  restamps only the elements that follow the iterate.
- Step control and stored traces were left alone: the profile showed neither
  costs time.
- Result differences against the run before: at most 1e-11 relative on
  linear, noise and operating-point measures; up to 3e-7 relative on error
  measures of the AC benches whose conductances span twelve decades (about
  1e-11 absolute); values that are pure rounding (around 1e-11 %) change
  freely. Transient measures move by up to 0.2 % (start-up) and harmonics
  near the 1e-5 % floor by more; see the weak spots above. Every bench keeps
  its verdict.

Open: device evaluation allocates a form per element per iteration and is
now the largest share of transient time in a release build.

Scope as planned:

- A timing breakdown per bench (assembly, factorisation, solve, device
  evaluation, step control, storage, measures).
- A sparse LU with a fill-reducing ordering and a symbolic factorisation
  reused across Newton iterations and time steps, replacing the dense LU for
  transient and, where it falls out, for AC and noise.
- Stamp the linear part once per step size; re-evaluate only the nonlinear
  devices per iteration.
- Step control reviewed against rejected steps and restarts, without loosening
  default tolerances.
- Store only probed traces.
- Run independent steps, Monte Carlo runs and worst-case corners on threads,
  with `--jobs N` and results identical to the serial run.

Acceptance:

- The analyzer's `Distortion` and `Startup` benches run at least 10 times
  faster, and the whole example in under 5 minutes.
- Sparse LU tests against the dense LU on random sparse systems; singular
  systems keep their diagnostics.
- Result differences against the baseline at round-off level for linear
  analyses and inside the integration tolerance for transient.

### F2. Bench language gaps

Built on 20 November 2026: all eleven items, each with a test. Diagnostics
SIM095 (noise quantities), SIM096 (supply sets), SIM097 (several analyses).

- Several analyses are positional sections (`analysis NAME: ac { ... }`
  followed by its measures and plots); each section runs as bench
  `BENCH.SECTION`. One run holding several analyses was not built.
- A part's `supply 3.3V { ... }` set is chosen automatically, the one nearest
  the instance's rail span. An explicit choice per instance was not built.
- A "never" from `upto`/`downto` fails every limit that covers its step.
- The analyzer's `StrayCapacitance` uses `SoilModel`, its four noise benches
  are one (`MeasurementNoise`), both with unchanged values. The impedance
  noise is a measure, `noise(z1)`: 0.12 % and 0.070 degrees at 5 ohm m and
  1 kHz. The hand-combined value in the README (0.17 %, 0.10 degrees) was
  too high by the square root of two and is corrected.
- Results that moved: only `at(x, f)` between sweep points, now solved at
  `f` (largest change 0.014 degrees, the analyzer's `SoilSweep` at 300 kHz).

Open: no supply sets on the ADA4807 part yet; `MatchingTolerances` still
rebuilds the LT5400 from bench resistors instead of using `match` on the
part; `CalibrationCard` and `ContactImbalance` have no `usable_to_magnitude`
measure; supply-set selection with ramping rails (as in `Startup`) is
untested; the HTML pages of multi-section and multi-output benches were not
looked at.

Scope as planned:

Every item was hit while writing the analyzer's benches.

- `upto` and `downto` return a defined "never" result instead of stopping the
  bench when the condition fails at the first point.
- Optional components in a fixture: a bench can add a pin to a fixture's
  bench net, a zero-value capacitor is allowed, and a fixture can use and
  override another fixture.
- Conditionals in expressions (`if(cond, a, b)`), and a `let` that is not
  used by a measure or plot of the running analysis is not evaluated.
- Several analyses in one bench, each with its own measures and plots.
- Several outputs in one noise analysis, and noise of a derived quantity
  (the impedance result `V/I`) without combining by hand.
- Matched tolerance groups on model parameters and on parts with a `sim`
  block (a resistor network as one part).
- Model parameter sets per supply voltage, so a part used at 3.3 V and at
  +/-5 V carries the right slew and swing for each.
- `at(x, f)` solves at `f` instead of interpolating between sweep points.
- `I()` of a supply.
- The text header and the JSON `analysis` field show the stepped analysis
  bounds, not the param defaults.
- A `Gohm` unit.

Acceptance: a test per item; the analyzer's `StrayCapacitance` uses the soil
fixture like the others; its four noise benches become one; the hand-combined
impedance noise in its README is replaced by a measure.

### F3. Stability analysis

Built on 20 November 2026. Diagnostics SIM098 to SIM101.

- `analysis loop { from; to; points_per_decade; probe = COMPONENT.PIN; }`:
  loop gain by Tian's method with a 0 V probe in series with the pin, so the
  operating point does not change. Measures `loop_gain()`, `phase_margin`,
  `gain_margin`, `crossover`, `phase_crossover` ("never" without a crossing);
  a Bode plot by default.
- `analysis pz;` gives the poles of the linearised circuit, the nets each
  dominates, a built-in `stable` measure and `damping()`; with `input` and
  `output` also the zeros. Dense: infinite eigenvalues deflated, QR, then
  each pole refined on the circuit's own matrices. No dependencies
  (`copper-sim/src/eigen.rs`).
- Howland pump: phase margin 20 to 30 degrees across loads (limit 15),
  pole pair near 69 MHz with damping 0.18 to 0.26; the two agree through
  the second-order relation and with the step overshoot.
- Analyzer: every amplifier loop has at least 49 degrees (limit 45); no
  right-half-plane pole in soil or with the electrodes in air. The guard
  loop question is settled: stable, least-damped mode 4.06 kHz with damping
  0.27. `LoopPeaking` under-reports the gain-1 driver's loop (its own closed
  loop peaks 23 % at 135 MHz).

Open: zeros are not refined (up to 7e-4 error rebuilding a stiff transfer
function); at most 1500 unknowns; a margin at a two-way break (a resistor
pin, not an amplifier output) is not a stability margin; Monte Carlo and
worst case with loop or pz sections are untested; the HTML pole map and
Bode pages were not looked at.

Scope as planned:

AC analysis cannot tell whether a loop oscillates. The analyzer's guard loop
and the Howland pump's 22 % step overshoot are judged by peaking today.

Scope:

- Loop gain with a probe inserted in a loop from the bench, without breaking
  the DC operating point (Tian's method; Middlebrook as the simpler special
  case). Measures: phase margin, gain margin, crossover frequency.
- Poles and zeros of the linearised circuit (generalised eigenvalues of the
  MNA pencil), with any pole in the right half-plane reported as a failure
  that names the nodes it dominates.

Syntax is a proposal: `analysis loop { probe = R_G.1; from; to; }` with
`phase_margin()`, `gain_margin()`, `crossover()`; `analysis pz;` with
`poles()` and a built-in `stable` measure.

Acceptance: closed-form tests (two-pole amplifier phase margin, an RLC
network's poles, a deliberately unstable loop detected); the analyzer's guard
loop and the Howland pump get a phase-margin measure with a limit; the
analyzer README's "negative real part that AC analysis cannot classify" is
settled either way.

### F4. Supply current and rails

Built on 20 November 2026. Diagnostics SIM102 to SIM104.

- `quiescent_current` on `sim opamp` and `diffamp`, opt-in: the supply pins
  carry the quiescent current and the output current comes from the
  sourcing rail, in every analysis, so `I()` of a rail is real. A rail
  without a DC path is SIM102. The quiescent current does not depend on the
  rail voltage: an unpowered amplifier still draws it.
- `psrr` and `psrr_corner`, one figure for both rails.
- A rail with impedance is a `source` with `output_resistance` in place of
  the supply; the board's decoupling capacitors then act.
- AC analysis of a circuit with nodes floating at DC still works: those
  nodes are held only while the bias point is solved.
- Analyzer bench `SupplyCurrent`: +/-4.5 V rails 51.8 mA idle and at most
  52.3 mA driving at 1 MHz, confirming the hand estimate. The 3.3 V analog
  rail is 3.66 mA idle but 5.2 to 6.1 mA at 1 MHz (the ADC drivers into
  470 pF); the README said 3.7 mA and is corrected.
- Results that moved: transient measures only, by up to 0.22 % (`Startup`)
  and more for harmonics near the floor (`Distortion`), within the
  integration error noted under the weak spots. Every bench keeps its
  verdict.

- `sim regulator` (roles `in`, `out`, `gnd`, optional `en`): set point,
  dropout, output impedance with a corner, line rejection with a corner,
  current limit, enable with soft-start, quiescent current, output noise.
  With `efficiency` it is an averaged DC-DC converter: input power is
  output power over efficiency, no switching ripple. `sim reference` is the
  same model with `startup_time`. An adjustable regulator is modelled at
  its divider's set point, not through its feedback pin.
- The analyzer's `Startup` runs on models of the TPS65130, TPS7A4901,
  TPS7A3001 and REF2033 instead of estimated ramps. The negative rail
  regulates at 9.3 ms, the positive at 14.3 ms, the mid reference at
  10.7 ms. The drive buffer input is still 0.18 V over its unpowered rail
  (limit 0.2 V): the peak is at 50 us, before any rail moves, and still
  rests on the estimated AD9833 power-up level. New: the ADC drivers'
  inputs now reach 0.165 V under their rail (limit 0.2 V), up from 0.140 V.
  All 23 limits pass.

Open: no PSRR or output impedance figures on the analyzer parts (none
cited), so its rails are stiff; rail noise in the noise benches; converter
ripple and loop dynamics; the boost's inductor and diode path before it
switches; quiescent current that falls with the rail (no datasheet figure
to base it on); small-signal output current at exactly 0 A bias reaches
neither rail; loop and pz analyses with rail models are untested;
`copper power-check` models no current, so it neither confirms nor
contradicts the simulated figures.

Scope as planned:

The op-amp models draw nothing from their supply pins, and benches drive
rails from ideal sources. Supply current, rail sag and power-supply rejection
are therefore not simulated.

Scope:

- Op-amp and diffamp supply pins carry quiescent current and the output
  current.
- `psrr` and its corner on `sim opamp`.
- A `sim regulator` macro model (dropout, output impedance against
  frequency, rejection, current limit, enable with soft-start) and `sim
  reference`.
- Rail impedance: a bench can run with the board's real regulators and
  decoupling instead of ideal supplies.

Acceptance: closed-form tests for supply current into a resistive load and
for rejection of a rail tone; the analyzer's analog current per rail
(51.8 mA by hand) is a measure; its `Startup` bench runs with the regulator
models instead of estimated ramps.

### F5. Layout parasitics

The one thing a schematic simulator cannot do. The analyzer's rating rests on
stray capacitances that are estimates until the board is routed.

Scope:

- Net-to-net and net-to-plane capacitance from the routed board in
  `copper-extract`, which today extracts only inductance and resistance.
- `use layout;` in a bench inserts the extracted capacitances, trace
  resistance and inductance between the pins they connect.
- Each bench that uses layout reports both results, schematic only and with
  layout, and names the largest extracted contributors.
- Extraction accuracy stated against closed forms (parallel plate, microstrip,
  two coplanar traces) and against the full-wave crate where it applies.

Needs the analyzer placed and routed first (stake shape, guard layers), which
is its own task. A smaller board (the Howland pump) can prove the mechanism
earlier.

Acceptance: closed-form extraction tests; the analyzer's `SoilSweep` and
`StrayCapacitance` run with layout, and SPEC.md's layout requirements
(1 pF residual per guarded node, 0.2 pF mask coupling) are checked against
extracted values instead of assumed.

### F6. Model library

Scope:

- `sim` blocks for the amplifiers, references and regulators already in
  CopperLib, each from data-sheet values with the table or figure cited.
- `copper sim --check-model PART`: a standard bench set (open-loop gain and
  phase, follower step, noise, supply current) that compares the macro model
  with the vendor model where both exist.
- `copper sim --models macro` to force built-in models.
- Vendor models that cannot be fetched automatically: a documented way to
  register a file the user downloaded by hand, by hash, so a board still
  builds the same everywhere.
- The Analog Devices models of the analyzer (ADA4817, ADA4807, AD8130),
  loaded from hand-downloaded files; the reader fixed where they fail.

Needs the owner's permission to change CopperLib; until then the models stay
example-local.

Acceptance: `--check-model` passes for every CopperLib part with a `sim`
block; the analyzer's `SoilSweep` runs with vendor amplifier models and the
difference from the macro models is reported.

Built on 20 November 2026. Diagnostics SIM106 to SIM109.

- A unit can carry a built-in block and a `sim spice` block: its macro and
  vendor models. `copper sim --models default|macro|vendor`; the default is
  the macro model, which needs no download.
- `copper sim --check-model PART BOARD`: supply current, open-loop gain, gain
  bandwidth, unity-gain frequency, phase margin, follower bandwidth and
  peaking, noise and slew rate for amplifiers; quiescent current, output
  voltage, output resistance, line rejection, dropout, efficiency, noise and
  start-up for regulators and references. It passes when the models load,
  the operating points converge and every declared figure is reproduced
  within the printed tolerance. This checks the macro model against its own
  block, not against the part. Vendor figures are printed beside it and
  marked where they differ by more than 25 %, 3 dB or 30 degrees.
- Vendor files downloaded by hand: `download = "..."` beside `sha256` in the
  block, `copper models add BOARD FILE`, SIM108 when the file is missing.
  Vendor caches now belong to the board's project, not the part's directory.
- Reader: files that are not UTF-8 are read as Latin-1; braces nested in an
  expression group like parentheses.
- CopperLib branch `sim-models` (local, needs this branch of CopperScript):
  blocks for OPA2277, OPA2182, OPA2189, OPA547, OPA548 (macro and TI vendor
  model), TLV75718P, TPS7A2018P, TPS7A2025P, AP2112K-3.3 and the RECOM
  R-78B, R-78E and R-78K converters, each with data-sheet citations. All pass
  `--check-model`.

Open:

- No Analog Devices model is loaded, so `SoilSweep` with vendor amplifier
  models has not run. The ADA4817-1, ADA4807-1 and AD8130 files have to be
  downloaded by hand; the analyzer parts carry the steps.
- TI's TLV757 model has `.IC` inside its subcircuit and the TPS7A20 model
  reads a global parameter; neither loads.
- LTC6268 has no block (its data sheet could not be fetched). Adjustable and
  isolated regulators, fully differential amplifiers, current-sense
  amplifiers and comparators have no model kind.
- A single `cmrr_corner` far below the gain bandwidth lets the common-mode
  term feed forward near the gain bandwidth (OPA2189 showed a 33 MHz follower
  until the corner was removed); a regulator's `psrr_corner` passes a fast
  input edge to the output.
- Vendor transients in the debug build: 100 s for the OPA2277 follower step;
  the OPA2182 step fails with a collapsing time step.
- Vendor models differ from the data sheets in places (OPA548 supply current
  4.3 mA against 17 mA; OPA2189 slew 11.6 V/us against 20 V/us).

### F7. Device and netlist coverage

Scope, in order of how often vendor files need it:

- MOSFET intrinsic (Meyer) gate capacitance, then level 3, then the LTspice
  `VDMOS` power model. BSIM stays rejected unless a part the examples use
  needs it.
- Netlist constructs now rejected: `DDT`, `SDT`, delay and `exp(-sT)` in
  LAPLACE, frequency tables, `.LIB file section`, `.OPTIONS` (read and
  ignored with a note where harmless), parameter overrides from the `sim
  spice` block, switches that start on.
- Diode and BJT parameters now rejected (`ISR`, `IKF`, `IRB`, quasi-saturation).
- Transmission lines: lossless first, then lossy.
- Saturating and hysteretic inductors and transformers.
- The OPA1612 discrepancy resolved.

Acceptance: each device against its equations evaluated independently in the
test; a self-written subcircuit per new construct; the compatibility table of
real models rerun and extended to at least ten parts.

Built on 20 November 2026. Diagnostic SIM105.

- MOSFET: Meyer gate charge as three charge branches (charge conserving; in
  the linear region the source-drain split adds a transcapacitance of at most
  C_ox/6 that Meyer's model does not have) and level 3 after SPICE3's
  `mos3load`. The drain-source Newton limit now acts on |v_ds|, so a MOSFET
  converges below -0.5 V.
- Netlist: `params = values { ... };` on `sim spice`, `.OPTIONS` (solver and
  output controls read and named, others rejected), `.LIB file SECTION`,
  switches that start `ON`, `DDT`, an ideal delay element (LAPLACE
  `exp(-sT)`, `DELAY(x, T)`), PSpice `VALUE {expr}` without `=`.
- Diode `ISR`, `NR`, `IKF` (pspice dialect only) and BJT `IRB`.
- Lossless transmission lines (Branin). `analysis pz` refuses a circuit with
  a delay.
- OPA1612 resolved: the reader matches a hand calculation from the model's
  element values within 2.5 %. The data sheet's 40 MHz is the follower
  bandwidth and its gain bandwidth is 80 MHz at a gain of 100; the model gives
  50.4 MHz and 91.7 MHz. The earlier 63 MHz was read off a 20 points per
  decade sweep without interpolation.
- Compatibility table: 13 Texas Instruments parts, 12 load, 11 give a gain
  bandwidth (0.95 to 1.38 of the data-sheet figure, LM358 the furthest).

Open: `VDMOS`, `SDT`, `FREQ` tables, BJT quasi-saturation and `PTF`, lossy
lines, saturating and hysteretic inductors, BSIM (the TI CSD MOSFET models
are BSIM3 and do not load), the level-3 `KAPPA` difference between SPICE3 and
PSpice, and the TL431 result above.

### F8. Temperature

Scope:

- A bench temperature that can be stepped, applied to noise, resistor and
  capacitor temperature coefficients (`tempco`) and device models (`TNOM`,
  `EG`, `XTI`, `XTB`, mobility and threshold scaling).
- `esr` and `esl` on capacitors and `dcr` on inductors from the part
  definitions.

Acceptance: diode forward voltage and bandgap-style closed forms over
temperature; a vendor model with `TNOM` other than 27 degC loads.

### F9. Spectrum and long-run analyses

Scope:

- A spectrum plot of a transient result (windowed FFT), beside the existing
  `thd` and `harmonic` measures.
- Periodic steady state by shooting, so switching and driven circuits do not
  need a long run to settle.
- Transient noise.
- Nested DC sweeps and a DC sweep as the outer loop of an AC analysis.

Acceptance: spectrum of known waveforms; shooting against a long transient
on the precision rectifier; the analyzer's `Distortion` bench uses steady
state instead of a settling run.

### F10. Viewing and editor support

Scope:

- The HTML report becomes interactive: zoom, cursors with differences,
  curves toggled per step, linear or log axes.
- A simulation view in CopperReview fed by `sim-results.json`.
- Language server: completion and hover inside `sim` blocks and benches
  (parameters with units and meaning), and a "run this bench" action.
- `copper sim --watch` reruns the benches a changed file affects.
- `copper sim` as a step of the board Makefiles.

Needs the JSON schema frozen as `copperscript-sim/v1`.

Acceptance: the views are looked at on the analyzer's results before the
phase is called done (charts first, per the EMI figures rule).

### F11. One solver for all checks

Scope:

- `copper-emi` uses `copper-sim` instead of its own one-frequency solver.
- Simulation results feed other checks where they are stronger than a rule:
  pin voltages against absolute maximum ratings in ERC, rail currents in
  the power-state check.

Acceptance: `emi-check` outputs unchanged on every board that has them;
a board whose simulated pin voltage exceeds a rating fails ERC with the
bench named.

## Not planned

- BSIM transistor models and encrypted vendor files.
- Mixed-signal or digital simulation.
- A schematic editor; the board source is the schematic.

## Open questions

- Should a bench be allowed to change board component values directly, or
  only parameters the board declares? Today: parameters, plus `replace`.
- Does sparse elimination order make results differ between machines? If so,
  the ordering must be deterministic and independent of thread count.
- Loop-gain syntax (F3) and `use layout` (F5) are proposals, not accepted
  language.
- How the analyzer board gets its layout: by the normal routing pipeline, or
  with hand-placed guard copper first.
