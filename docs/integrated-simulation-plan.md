# Integrated simulation plan

Status: phase P0 implemented, 19 November 2026 (branch
`integrated-simulation-plan`, not yet reviewed for merge); see
[what of P0 is still missing](#p0-status). Phase P1 implemented the same day,
see [P1 status](#p1-status), and phase P2 too, see [P2 status](#p2-status);
the loose ends closed after them are in [closing round](#closing-round).
Phase P3 (vendor SPICE subcircuits read natively) was implemented on the same
branch, see [P3 status](#p3-status). Phases P4 and P5 are plans. The P0 to P3
syntax as built is in the [language reference](language-reference.md#simulation);
the syntax of later phases below is a proposal to review, not accepted language.

Goal: circuit simulation as a normal part of a CopperScript project, at the
level people use LTspice for: put sources on a circuit, run AC, transient,
noise and DC analyses, step parameters, read measurements, and fail the build
when a measurement leaves its limits. The board source stays the single
description of the circuit; simulation never needs a second schematic.

The first consumer is the [impedance analyzer example](../examples/simulation/impedance_analyzer/SPEC.md).
Every phase below is done when that board's corresponding bench passes.

Earlier research, written against the removed Python implementation, is in
[integrated simulation options](integrated-simulation-research.md) and
[the analog example note](simulation-analog-research.md). This plan replaces
their implementation sequences.

## What exists today

| Piece | State |
| --- | --- |
| `copper sim` | `copper sim BOARD.copper [--bench NAME] [--out DIR]` (`crates/copper-simulate`): lowers board plus bench, runs the bench's analysis (`ac`, `op`, `noise`, `tran` or `dc`) per step, evaluates measures and plots, prints measures against limits, writes `sim-results.json` (`copperscript-sim/v0.1`), `BENCH.csv` and `BENCH.html`; exit status 1 on a failing measure or an unsolvable bench. |
| Language | `param` (boards and modules), part `sim opamp`/`source`/`load` blocks, `bench` documents in `benches/` for a board or for one of its modules on its own, with `component`, `net`, `supply`, `model`, `replace`, `ignore`, `analysis ac`/`op`, `step`, typed `let`/`measure`/`plot` expressions (`copper_lang::simulation`, `bench`, `expression`). |
| Solver | `crates/copper-sim`: named nodes, R, C, L, coupling, independent and the four controlled sources, `opamp`/`source`/`load` macro models, linear operating point and AC sweeps on a dense complex LU, singular systems explained (no path to ground, voltage-source loops). Linear in P0; P1 and P2 add noise and transient analysis with diodes and switches (see their status notes). `copper-emi/src/circuit.rs` is still a separate one-frequency solver. |
| Extraction | `copper-extract` computes inductance and resistance of copper. It does not extract net capacitance. |
| Examples | `examples/simulation/rc_filter.copper` with `benches/rc_filter_ac.copper`, `howland_pump/` (board, parts, `benches/flatness.copper`), `inrush.copper` and `analog_frontend/` (board source only). |
| Vendor models | `sim spice` (P3): local files and URL-plus-SHA-256 vendor files read natively by `copper-spice`; see [P3 status](#p3-status). (Before P3, one attempt: TI OPA2197 on ngspice 46, which parsed only with `analog.cm` and `xtradev.cm` loaded and whose full-board bias and cold start did not converge.) |

## Decisions

1. **One solver, written in Rust, inside `copper`.** No ngspice, no other
   external tool, nothing to install. The solver handles operating point, AC,
   noise and transient, for the built-in models and for vendor SPICE
   subcircuits that `copper` reads itself.
2. **The board is not rewritten.** Sources, loads, cables and the device under
   test live in a `bench`, attached at connector pins or nets. A bench may
   replace a component with a stand-in (a DDS becomes a sine source) but must
   say so.
3. **Model fidelity is always visible.** Every result names the model each
   active part ran with: `ideal`, `macro` (built-in parameter model) or
   `vendor` (subcircuit file and hash). A missing model is an error, never a
   silent ideal part. A vendor file that uses something the reader does not
   support is rejected with the line and the construct named; it is never
   approximated.
4. **Vendor files are fetched, not committed.** A model is a URL plus hash in
   `copper.lock`, cached locally. Vendor licences generally forbid
   redistribution, so neither CopperScript nor CopperLib carries the files.
5. **Limits are assertions.** `copper sim` exits non-zero when a `measure`
   with a limit fails, like `emi-check` and DRC.

## Language additions

### 1. Model binding on parts

A part declares how it simulates. Two forms, both optional, both may be present:

```copper
part ADA4817_1 {
    category = "amplifier.opamp";
    pin INP { number = "3"; ... }

    // Built-in parameter model. No external file.
    sim opamp {
        inp = INP; inn = INN; out = OUT; vp = VP; vn = VN;
        gbw = 410MHz;
        open_loop_gain = 65dB;
        input_capacitance_common = 1.3pF;
        input_capacitance_differential = 0.1pF;
        input_bias_current = 2pA;
        voltage_noise = 4nV/rtHz;
        current_noise = 2.5fA/rtHz;
        slew_rate = 870V/us;
        output_resistance = 8ohm;
        output_swing_from_rail = 1.4V;
    }

    // Vendor subcircuit. Terminal order is the subcircuit's.
    sim spice {
        source = "https://www.analog.com/media/en/simulation-models/spice-models/ada4817.cir";
        subckt = "ADA4817";
        terminals = INP, INN, VP, VN, OUT;
        dialect = "spice3";          // or "pspice"
    }
}
```

Multi-unit packages name the unit: `sim opamp A { ... }`, `sim opamp B { ... }`,
or one `sim spice` per unit with shared supply pins. Pins the model does not
cover are listed in `unmodelled = NC, EPAD;` so omissions are explicit.

Built-in model kinds for the first release: `opamp`, `diode`, `switch`
(on-resistance, off-capacitance, charge injection left out), `source`
(voltage or current output with impedance, used for regulators and DAC/DDS
outputs), `load` (R parallel C, used for ADC inputs). Discrete transistors use
`sim spice` with a model card.

### 2. Passive non-idealities

```copper
component R_SET: RESISTOR { value = 10kohm; tolerance = 0.1%; tempco = 10ppm/K; }
component C_F: CAPACITOR { value = 2.2pF; tolerance = 5%; esr = 50mohm; }
```

`tolerance` drives Monte Carlo and worst-case runs. `match = "HOWLAND"` with
`match_tolerance = 0.01%` on several resistors declares a ratio-matched group
(a resistor network), which sampling must respect.

### 3. Parameters

```copper
board ImpedanceAnalyzer {
    param R_SET = 10kohm;
    component R1: RESISTOR { value = R_SET; }
}
```

A typed named value usable wherever a quantity is. Benches override and step
parameters. This is useful outside simulation too.

### 4. Bench

A bench sits beside the board, in the same file or `benches/*.copper`.

```copper
bench SoilSweep for ImpedanceAnalyzer {
    // Simulation-only circuit: device under test and stray capacitance.
    param Z_DUT = 10kohm;
    component DUT: RESISTOR { value = Z_DUT; }
    component C_STRAY_HC: CAPACITOR { value = 2pF; }
    net E_HC.1 { DUT.1; C_STRAY_HC.1; }      // joins the board's net at that pad
    net E_LC.1 { DUT.2; }
    net GND { C_STRAY_HC.2; }

    // Stand-in: the DDS output pin becomes a source.
    replace U_DDS with source {
        out = U_DDS.VOUT; ref = GND;
        waveform = sine { offset = 0.3V; amplitude = 0.3V; frequency = FREQ; }
        ac = 0.3V;
        output_resistance = 200ohm;
    }
    ignore U_MCU;                             // digital, no analog effect in this bench

    analysis ac { from = 1kHz; to = 1MHz; points_per_decade = 20; }
    step Z_DUT in 100ohm, 1kohm, 10kohm, 50kohm;

    let i_dut = I(DUT);
    let z = (V(SENSE_P) - V(SENSE_N)) / V(I_MON) * R_TIA;

    measure current_flatness = max(abs(mag(i_dut) / 15uA - 1)) { max = 1%; }
    measure z_error = max(abs(mag(z) / Z_DUT - 1)) { max = 0.5%; }
    measure z_phase = max(abs(phase(z))) { max = 1deg; }
    plot mag(z), phase(z);
}
```

Elements:

| Element | Meaning |
| --- | --- |
| `component`, `net` | Simulation-only circuit. A net named by a board pin or net joins it. |
| `replace X with ...` | Swap a component for a built-in model for this bench. |
| `ignore X` | Remove a component; its pins become open. Required for every part with no model. |
| `source` waveforms | `dc`, `sine`, `pulse`, `pwl`, plus `ac` magnitude/phase for small-signal runs. |
| `analysis` | `op`, `ac`, `tran`, `noise`, `dc` (sweep a source or parameter). |
| `step` | Repeat over a list or range of a parameter. Several `step` lines nest. |
| `montecarlo { runs = 200; seed = 1; }` | Sample tolerances; `worstcase` runs the corners. |
| `use layout` | Add extracted trace and pad parasitics (see phase 5). |
| `let`, `measure`, `plot` | Expressions, asserted results, figures. |

### 5. Expressions

The largest new piece of the language. Needed in benches only at first.

- Typed arithmetic on quantities: `V / A` is `ohm`; mismatched units are a
  compile error, as elsewhere in CopperScript.
- Complex values in AC and noise results: `mag`, `phase`, `re`, `im`, `db`.
- Probes: `V(net)`, `V(a, b)`, `I(component)` or `I(component.pin)`.
- Reductions over the sweep or time axis: `min`, `max`, `mean`, `rms`, `pp`,
  `at(x, f)`, `cross(x, level)`, `settle(x, band)`, `integrate(x, from, to)`.
- Fourier: `thd(x, f0)`, `harmonic(x, f0, n)`.

### 6. Units

`dB`, `deg`, `%`, `ppm/K`, `mohm`, `fA`, `pA`, `nA`, `nV/rtHz`, `fA/rtHz`,
`pA/rtHz`. Slash units already exist for slew rate.

## Commands and output

```
copper sim board.copper                    # all benches
copper sim board.copper --bench SoilSweep
copper sim board.copper --models macro     # built-in models even where a vendor file is bound
copper sim --check-model ADA4817_1         # can the reader load this part's vendor file?
```

Output per bench: a compact text table of measurements against limits, an HTML
page with the plots (magnitude and phase on a shared frequency axis, one curve
per step), CSV of the raw vectors, and the model-fidelity list. Following the
EMI figures rule: charts first, text only for the numbers that decide pass or
fail. CopperReview gets a simulation view once the JSON is stable.

## Phases

Each phase ends with the named bench of the impedance analyzer running.

**P0. Simulation IR and AC with built-in models.**
New crate `copper-sim`. Lower board plus bench to a flat element list. Move the
nodal solver out of `copper-emi` (which then depends on it) and add: operating
point for linear circuits, the `opamp`, `source` and `load` macro models,
`analysis ac`, probes, `let`/`measure`/`plot`, `step`. Language: `sim opamp`,
`bench`, expressions, units, `param`.
Check: analytic RC and RLC responses; ideal Howland output current and output
impedance against the closed-form result. Bench: `SoilSweep`, `CalibrationCard`.

<a id="p0-status"></a>**P0 status** (19 November 2026). Implemented: `copper-sim`
(solver, macro models, operating point, AC, measurement helpers);
the language (`param`, `sim` blocks, benches, typed expressions);
`copper-simulate` and `copper sim` (lowering, stepping, expression evaluation,
measures with limits and worst step, solver errors in net and component terms,
results JSON, CSV and HTML with plots). Checked: RC and RLC closed forms and the
buffered and plain Howland output current and output impedance in
`copper-sim/tests/closed_forms.rs`; the RC corner and the Howland load current
through the whole chain (board, bench, lowering, solver, expressions) in
`copper-simulate/tests/examples.rs`. Still missing from P0:

- The impedance analyzer board (`examples/simulation/impedance_analyzer/`) and
  its benches exist: one battery-powered board whose analog front end is a
  module, simulated on its own by every bench (`bench NAME for
  AnalogFrontEnd`, with `supply` and `model` statements for its ports, so no
  bench ignores the digital and power section). All fourteen benches pass and
  `copper sim` exits 0: the SPEC rates 1 % and 1 degree over a stated usable
  range (soil up to 100 ohm m, matched sense contacts, bounded layout strays),
  the benches with accuracy limits step only inside it, and study benches
  without accuracy limits report the conditions outside it (the example's
  SPEC, "Usable range", and README, "Results"). Added for
  it: derived bench params, zipped steps, `reference` runs (calibration),
  `second_pole`, `cmrr` and `cmrr_corner` on `sim opamp`, the `sim diffamp`
  kind, pin sharing between `load` blocks, benches for modules, bench
  supplies and models, and params in modules.
- The nodal solver of `copper-emi` was not moved onto `copper-sim`; the two
  solvers coexist.
- `I(...)` of a supply is not supported (`SIM045`); the op-amp model's
  supply pins carry no current. (`I(X)` and `I(X.ROLE)` of a replaced
  component or bench model read the model's terminals since the
  [closing round](#closing-round).)
- The impedance analyzer's benches still repeat their setup and still split
  rated and `*_study` benches; fixtures and scoped limits now allow one of
  each, and the conversion is a separate step.
- `at(x, f)` interpolates between sweep samples instead of solving at `f`.
- The op-amp model needs `gbw` and `open_loop_gain` (an error otherwise).
- AC analysis cannot find unstable poles; benches use closed-loop peaking as the
  warning sign (no pole-zero or loop-gain analysis).
- `copper sim --models` and `--check-model` belong to P3 and P4 and do not exist.
- CopperReview has no simulation view.

**P1. Noise and tolerance.**
`analysis noise` with resistor thermal noise and op-amp voltage/current noise,
integrated over a band. `tolerance`, matched groups, `montecarlo`, `worstcase`.
Check: Johnson noise of a resistor; noise gain of an inverting stage.
Bench: `MeasurementNoise`, `ResistorMatching`.

<a id="p1-status"></a>**P1 status** (19 November 2026, not yet reviewed).
Implemented: `copper_sim::noise` (adjoint noise analysis: resistor thermal
noise, op-amp and differential-amplifier voltage and current noise with 1/f
corners, per-contributor output densities, input-referred density,
power-law band integration) and `copper_sim::rng` (SplitMix64); the language
(`analysis noise { ...; output = V(...); input; temperature; }`, `noise()`,
`noise(X)`, `input_noise()`, `noise_rms`, `noise_share`, the `sim opamp`
parameters `voltage_noise_corner` and `current_noise_corner`, component
`tolerance`, `match` and `match_tolerance`, bench `tolerance` statements for
params, passives and model parameters, `montecarlo { runs; seed;
distribution; }` and `worstcase;`, codes `SIM085`-`SIM094`); the runner
(Monte Carlo runs as the same boards at every step, worst case by
sensitivity corners, limits on the worst run, yield) and the results
(optional `noise` and `variation` fields in `copperscript-sim/v0.1`, the
text table's statistics lines, the contributor table, histograms and the
sensitivity table in the HTML page). Checked: Johnson noise, a divider's
density and shares, `sqrt(kT/C)` of an RC low-pass, the inverting stage's
noise gain with `e_n`, `i_n` and every resistor, and 1/f corner integration in
`copper-sim/tests/noise_closed_forms.rs`, with sampling statistics and a
Monte Carlo divider; through the whole chain the RC noise, a divider's worst
case against its analytic corners, a matched pair's ratio corners, Monte
Carlo mean, standard deviation, yield and determinism, and the diagnostics in
`copper-simulate/tests/noise_and_tolerance.rs`. Benches:
`examples/simulation/preamp/` (`MeasurementNoise`, `GainMonteCarlo`,
`OffsetWorstCase`, each agreeing with the hand calculation in its comment) and
`examples/simulation/howland_pump/benches/` (`ResistorMatching`,
`MeasurementNoise`). Still missing from P1:

- The impedance analyzer's own `MeasurementNoise` and `ResistorMatching`
  benches: its parts carry no noise data yet and its board no tolerances.
- `tempco` and `esr` (the passive non-idealities of the language additions)
  are not read; temperature is a noise-analysis property only, not a param
  that can be stepped.
- Noise sources are resistors, amplifiers, diode junctions (shot noise) and
  switches (thermal noise of their state's resistance): no noise of sources
  or supplies, the `e_n` times input-capacitance current is not modelled and
  a differential amplifier's feedback pair is noiseless. The output is a
  voltage (`V(OUT)` or `V(P, N)`); a current output is read as a voltage
  across a known resistance.
- Worst case assumes monotonic measures (see the language reference); there
  is no corner enumeration or optimisation for non-monotonic ones.
- Monte Carlo plots show the nominal run only (no envelope); the HTML has
  histograms per measure at its worst step.

**P2. Transient.**
Trapezoidal integration with step control, source breakpoints, `sine`/`pulse`/
`pwl`. Op-amp macro gains slew limiting and output clipping, which needs a
Newton loop; `diode` and `switch` arrive with it. Fourier measurements.
Check: RC step, RLC ringing, slew-limited sine against the closed form.
Bench: `Startup`, `Distortion`.

<a id="p2-status"></a>**P2 status** (19 November 2026, not yet reviewed).
Implemented in Rust with no new dependency: in `copper-sim`, a real-valued
assembly with backward-Euler and trapezoidal companion models and Newton
iteration (`newton.rs`; junction-voltage limiting, gmin stepping, source
stepping, a reused factorisation when the matrix is unchanged), the
nonlinear operating point (the operating point of any circuit with a diode
or switch; AC analysis then uses their small-signal admittance at it),
`transient` (`transient.rs`: trapezoidal with a backward-Euler start and
restart after breakpoints, divided-difference truncation-error control,
`max_step`, breakpoints at every waveform corner, switch events located to
1e-6 of the run, errors that name the nodes and elements), the `diode`
(Shockley, series resistance, depletion and diffusion charge, breakdown)
and `switch` (on/off resistance, threshold, hysteresis) models
(`devices.rs`), the op-amp's transient limits (clipping at the rails less
per-side headroom, slew rate, output current limit; inactive in op, AC and
noise, so every earlier result is unchanged), the waveforms `sine` (with
`delay`), `pulse`, `pwl` and `step` (`waveform.rs`) and the time-domain
measurements (`time_measure.rs`). The language has `analysis tran { stop;
start; max_step; reltol; }`, `sim diode`, `sim switch`, the waveforms, the
windowed reductions `min max mean rms pp (x, from, to)`, `at(x, t)`,
`cross`, `settle`, `overshoot`, `harmonic` and `thd` (`copper_lang::transient`,
SIM069-SIM071); `copper sim` runs it and writes the traces on `time_s` in
JSON and CSV and a linear time axis in HTML. Checked in
`copper-sim/tests/transient_closed_forms.rs`: RC step (pointwise within
2.1e-4 of 1 V at the default tolerance, 77 steps), series RLC ringing frequency
(within 0.15 %, and 1e-4 at `reltol = 1e-6`) and decay per period (within
1 %), charge conserved to 1e-9 and energy balanced to 1e-6 in charge
sharing, the diode operating point (1e-6) and small-signal AC, the
half-wave rectifier peak, junction charge (1e-3 measured, 2e-3 asserted), op-amp slew rate on a step
(2e-3) and on a slew-limited sine (triangle amplitude SR / 4f within 1 %),
clipping at the rails less headroom (within 20 mV) and the output current
limit (1e-3), and the switch threshold with hysteresis. Example:
`examples/simulation/precision_rectifier/` (a superdiode showing the
diode, clipping, slew delay and THD against the ideal rectifier), and
`howland_pump/benches/step_response.copper`. Still missing from P2:

- The impedance analyzer's `Startup` and `Distortion` benches are not
  written; its board and benches were out of this step's scope.
- `copper-emi`'s circuit solver is still separate.
- The op-amp supply pins still carry no current, so supply current and
  rail sag are not modelled; the op-amp output clamp references the rails
  at the supply pins' nodes.
- The dense LU limits transient runs to circuits of tens to a few hundred
  unknowns; a transient run stores every time point.

<a id="closing-round"></a>**Closing round** (19 November 2026, not yet
reviewed). Loose ends of P0 to P2, as built (syntax in the
[language reference](language-reference.md#simulation)):

- **Limits per step.** `measure NAME = expr { max = 1%; where R_SOIL <=
  100ohm and ...; }`: comparisons of params scope a measure's limits to the
  steps where they hold; uncovered steps are reported without limits, the
  worst step and Monte Carlo yield count covered steps only, a `where` that
  holds nowhere is an error, and text, JSON (`where`, `covered_steps`,
  per-step `covered`) and HTML show the coverage. Limits that name params
  were already possible; the page now draws one limit line or window per
  distinct limit instead of the first step's only. `SIM072`.
- **Fixtures.** `fixture NAME for TARGET { ... }` in `benches/` holds shared
  setup; `bench NAME for TARGET using NAME, NAME { ... }` puts its
  declarations in front of the bench's own, and a bench param, supply,
  model, replacement or bench component of the same name takes the
  fixture's place. The Howland example's four loaded benches share
  `benches/loaded_pump.copper` with identical results. `SIM073`.
- **Reductions.** `upto(a < b)` and `downto(a < b)`: the axis value up to (or
  down to) which a comparison holds from the start (or end) of the axis,
  the zero of the margin interpolated in log frequency (linear on time and
  dc axes); `integrate(x)` and `integrate(x, from, to)`; the windowed
  `min max mean rms pp (x, from, to)` on the ac axis are documented as
  sub-band reductions; `thd(x, f0, n)` sums harmonics 2 to `n`. `SIM072`.
  `harmonic(x, f0, n)` read its last two arguments as a time window before
  (an error at run time); only the windowed reductions take a window now.
- **Noise through nonlinear parts.** `copper_sim::noise` solves the Newton
  operating point and uses the diodes' and switches' small-signal admittance
  as AC does, and adds a diode junction's shot noise `2 q |I|` and a
  switch's thermal noise in its state (a diode's series resistance already
  was a noisy resistor).
- **DC sweep.** `analysis dc { sweep = SOURCE_OR_PARAM_OR_SUPPLY; from; to;
  step | points; }` (`copper_sim::dc_sweep`): Newton operating points with
  continuation, the op-amp clipping and current limit active, probes real
  over the swept value. `SIM074`.
- **Initial conditions.** `ic V(NODE) = q;`, `ic C1 = q;`, `ic L1 = q;`
  held during the initial operating point, and `analysis tran { uic = true;
  }` to start from the capacitor voltages and inductor currents without one.
  `SIM075`.
- **Currents of placed models.** `I(X)` and `I(X.ROLE)` of a replaced
  component or a bench model; a part pin of a replaced component is a
  `SIM045` that names the roles to write.
- **Formatting.** `uV` and `nV` voltage units (in the shared unit table, so
  benches may write `12uV`); values of 1 mV and above print as before.
  Noise densities plot log-log.

Checked: `copper-sim/tests/sweep_and_initial_closed_forms.rs` (diode shot
and series-resistance noise and switch noise in both states to 1e-6 and
1e-9, the diode I-V of a dc sweep to 1e-9, an amplifier transfer curve
linear to 1e-6 and clipped within 20 mV, RC discharge from a capacitor, a
node and with `uic` within 1e-3, an LC tank from a capacitor or an inductor
within 2e-3) and `copper-simulate/tests/scopes_sweeps_and_fixtures.rs`
(scoped limits and their outputs, `upto`/`downto` at an RC's -3 dB point
within 2e-4, sub-band windows exact on sweep points, fixtures against the
written-out bench exactly, dc sweeps of a source, a param and a downward
sweep, `ic` through the chain, `I(...)` of a replaced source and a bench
load to 1e-9, a diode's noise through the chain to 1e-5, the log noise
axis, and a square wave's harmonics and `thd` to 2e-3); diagnostics in `copper-lang/tests/simulation.rs`. Still open after
this round: `I(...)` of a supply, noise of sources and supplies,
temperature as a stepped param, Monte Carlo envelopes, non-monotonic worst
case, and converting the impedance
analyzer benches to fixtures and scoped limits.

**P3. Vendor SPICE subcircuits, read natively.**
A SPICE netlist reader in Rust that lowers a `.subckt` into the same element
list the solver already runs. In order of need:

| Step | Adds | Unlocks |
| --- | --- | --- |
| P3a | `.subckt`, `.model`, `.param`, R/C/L/K, independent sources, linear controlled sources (E, F, G, H), diode (`D`) with the standard model card | Boyle-style op-amp macromodels, the form most Analog Devices `.cir` files take |
| P3b | Bipolar (Gummel-Poon) and junction FET models | Macromodels with a real input pair |
| P3c | Behavioural sources: `VALUE = { }`, `TABLE`, polynomial sources, with the PSpice function set (`LIMIT`, `IF`, `MIN`, `MAX`, `TANH`, ...) and voltage-controlled switches | Texas Instruments PSpice op-amp models |
| P3d | MOSFET level 1 and 3; BSIM only if a real part needs it | Discrete power stages, analog switches at transistor level |

Each step needs the Newton solver from P2 plus the usual convergence aids
(source stepping, a minimum conductance to ground, junction voltage limiting).
Model fetch and hash pinning in `copper.lock` arrive with P3a.

This is the largest phase and the price of having no external tool. Its limits
are stated up front: encrypted models (most of the LTspice library, some
PSpice files) cannot be read by any open reader; a file that needs a construct
outside the implemented steps is rejected until that step exists.

Check: per step, a set of small circuits with closed-form or published
answers (diode I-V, transistor bias point, a Boyle macromodel's open-loop gain
and phase against its datasheet curve). While P3 is developed, results are
also compared against ngspice on a developer machine; that comparison is a
development aid and never part of `copper` or its tests.
Bench: `VendorModels`, the `SoilSweep` with the vendor amplifier
subcircuits, with a report of the difference from the built-in models.

<a id="p3-status"></a>**P3 status** (19 November 2026, not yet reviewed).
Implemented with no external tool and no new dependency: the netlist reader
`crates/copper-spice` (depends on `copper-sim` only), the `copper-sim` law
elements, devices and convergence aids it needs, the `sim spice` part block,
its lowering in `copper-simulate`, and `copper models fetch` for pinned
vendor files. Syntax, the accepted constructs and the rejected ones are in
the [language reference](language-reference.md#spice-subcircuits). By step:

- **P3a, done.** `.SUBCKT`/`.ENDS` (nested, `PARAMS:`), `.MODEL` (`AKO:`),
  `.PARAM` with expressions, `{}` and `''` values and bare parameter names,
  `.FUNC`, continuations, `*` `;` and (SPICE3) `$` comments, engineering
  suffixes with `MEG`/`MIL` and unit letters, case-insensitivity,
  `.INCLUDE`/`.INC`/`.LIB file` and `.LIB`/`.ENDL` sections for local files,
  node 0; R (models with `T_ABS`, LTspice `noiseless`, negative values), C,
  L, K, V and I with `DC`, `AC`, `SIN`, `PULSE`, `PWL`, `EXP` (a new
  waveform), linear E/G/F/H, X; the diode card (`IS N RS CJO VJ M TT BV IBV
  FC KF AF`; the device gained `FC` and flicker noise). Encrypted (Cadence)
  files are recognised and rejected.
- **P3c, done.** E/G `POLY(n)` (F/H too), `VALUE`, `TABLE`, `LAPLACE`
  (rational `H(s)` only, realised exactly as a frequency-scaled state-space
  network; delays and other non-rational forms rejected), LTspice/PSpice `B`
  sources, the function set (`LIMIT MIN MAX ABS SGN IF TANH EXP LOG SQRT
  PWR PWRS TABLE` and more), `V(a, b)`, `I(Vx)`, ternaries, PSpice `|` and
  `&`, LTspice `^` as exclusive or; S/W switches with `SW`/`CSW`
  (hysteresis) and PSpice `VSWITCH`/`ISWITCH` (the smooth switch, with a
  window step limit). Derivatives are analytic: forward-mode automatic
  differentiation (`copper_sim::behavioral::Dual`) through every operation,
  one-sided at kinks. Convergence aids added for circuits with law elements
  (existing circuits run exactly as before): a singular Jacobian at an
  iterate falls back to the aids, pseudo-transient continuation (node
  conductances anchored to the previous iterate) after gmin stepping,
  truncation error controlled on stateful nodes only, and a step rejected
  at a billionth of the run accepted as a discontinuity.
- **P3b, done.** Gummel–Poon BJT (the listed parameters plus `RBM`,
  `XCJC`, `CJS`, `XTF`/`VTF`/`ITF`, area, substrate node; `IRB`, `PTF` and
  quasi-saturation rejected) and the SPICE3 JFET (`B` other than 1,
  `ALPHA`, `VK`, `ISR` rejected), with `pnjlim` junction limiting, exact
  small-signal admittances, shot noise of junction and transport currents,
  flicker noise, channel noise `4 k T (2/3) g_m`.
- **P3d, partly.** MOSFET level 1 (`VTO KP GAMMA PHI LAMBDA`, `W L LD`, `RD
  RS`, body diodes with `IS`/`JS`, junction capacitances `CBD CBS CJ CJSW
  MJ MJSW PB`, overlap capacitances `CGSO CGDO CGBO`, `fetlim`/`limvds`,
  channel and flicker noise). Not done: the intrinsic (Meyer) gate
  capacitances, so a level-1 card with `TOX` is rejected rather than run
  without them; level 3; process parameters (`U0`, `NSUB`, `TPG`). Other
  levels are rejected with the level named (BSIM by name).
- **Not done anywhere:** temperature scaling (a model measured at a `TNOM`
  other than 27 degC is rejected; `EG`, `XTI` have no effect at 27 degC),
  `.LIB file section`, `.OPTIONS`, parameter overrides from the `sim spice`
  block (`PARAMS:` defaults apply), `copper sim --models` and
  `--check-model`.

**Fetching and pinning.** The plan's `copper.lock` entry is replaced by the
precedent of URL 3D models (`model_sha256` on the part): the pin travels with
the part as `source` plus `sha256` in the `sim spice` block (with `member`
for a model inside a ZIP archive, as Texas Instruments ships them; a
dependency-free inflater reads it). `copper models fetch BOARD` downloads
with curl (HTTPS only, the same code as 3D models) into
`.copper-cache/spice/<sha256>/` beside the nearest `copper.mod` (else beside
the part's file), refusing other bytes; `copper sim` never downloads and
reports `SIM077` with the command to run. No vendor file is in the
repository: tests and examples use subcircuits written for them.

Checked: `copper-sim/tests/semiconductor_closed_forms.rs` (Gummel–Poon DC
currents in forward, saturated and reverse operation and a PNP against the
equations written out in the test, to 1e-9; internal nodes with `RB RE RC`
to 1e-7; common-emitter gain, input admittance with depletion and diffusion
charge, and collector shot noise to 1e-6/1e-5; JFET curves in all regions,
P-channel and channel noise to 1e-9/1e-6; MOSFET level-1 curves with body
effect, reverse mode, body diodes, P-channel and overlap capacitance to
1e-9; diode `FC` and flicker noise to 1e-5; behavioural laws: a quadratic
current, a `tanh` source and its AC slope, a current-controlled law and a
`TIME` law to 1e-9 to 1e-12), `copper-spice/tests/spice_elements.rs` (every
card against hand formulas, `LAPLACE` responses to 1e-9 and a step to 2e-3,
waveforms, switches, resistor noise temperature, device cards with area,
the diagnostics of thirteen unsupported constructs, includes),
`copper-spice/tests/spice_opamps.rs` (four op-amp subcircuits written for
the tests, one per dialect style: a Boyle model with `B` sources, gain to
1e-6, unity-gain frequency to 1e-3, slew rate to 1 %; a two-stage BJT
amplifier, GBW to 5 %, slew to 5 %, DC gain to 30 % of the hand estimate; a
JFET-input stage, stage gain to 1e-4, unity gain to 2e-3; a PSpice-style
`VALUE`/`TABLE`/`LIMIT` model, gain, GBW, slew, current limit, swing and
supply current), `copper-simulate/tests/spice_runs.rs` (the example through
the whole chain, the offline vendor failure, file and netlist diagnostics)
and `copper-lang/tests/spice_blocks.rs`. Example:
`examples/simulation/spice_amplifier/` (op, AC, tran and noise benches on a
local, self-written model). Vendor comparison:
`examples/simulation/vendor_models/` (TI's OPA810 model by URL and hash
beside the built-in macro model at the product page's figures; it needs
`copper models fetch` and fails clearly without it).

Compatibility with real vendor models (19 November 2026, files fetched into
a cache outside the repository, `copper-spice/tests/vendor_models.rs`
`--ignored`): Analog Devices refused automated downloads (connection reset
on every request), so the ADA4817, ADA4807 and AD8130 models of the
impedance analyzer were not tried. Texas Instruments' PSpice models load and
run in every analysis: OPA810 (unity gain near 71 MHz, product page GBW
70 MHz; slew 225 V/us at gain 10, page 200 V/us; 13.2 nV/rtHz at 1 kHz and
5.9 at 100 kHz, page 9.5 at 1 kHz), OPA1612, OPA2134, TL072 (a Boyle model
with JFETs and `POLY`) and LM358. TI's archives carry PSpice's own bias
points for two test circuits: every node agrees (OPA810: 88 of 90 nodes
within 1 uV, all within 24 uV; LM358: 84 of 106 within 1 uV, all within
0.9 mV; the outputs to nine digits). The 24 uV are the noise-generator
diodes: PSpice's thermal voltage is 2.8e-5 lower (older physical constants).
The `SoilSweep` with vendor amplifiers is not done (no ADI model could be
fetched).

**P4. Model library.**
`sim` blocks for the amplifiers already in CopperLib (OPA2182, OPA2189,
OPA2277, OPA547, OPA548) and the new parts of the example. Each from datasheet
values, with the datasheet section cited. A `copper sim --check-model PART`
bench set (open-loop gain and phase, follower step, noise) compares macro
against vendor model where both exist.

**P5. Layout parasitics.**
Net-to-net and net-to-plane capacitance from the routed board, added to
`copper-extract`, which today only extracts inductance and resistance.
`use layout` in a bench inserts them. For the impedance analyzer this decides
whether the board meets its 1 MHz target, so the bench runs twice: schematic
only and with layout.

## Open questions

- Should a bench be allowed to change board component values (`step R_SET`),
  or only parameters the board declares? The plan assumes parameters only.
- How far to take P3. P3a and P3b cover most Analog Devices amplifier models;
  P3c is needed for Texas Instruments ones and is the most open-ended step.
  Decide after P3a against the parts the example and CopperLib actually use.
- Loop-gain measurement needs a loop-break probe (Middlebrook or Tian). Not in
  any phase yet; the Howland pump's stability is checked by transient step and
  output-impedance peaking until it is.
- Encrypted models cannot be read. Parts whose only model is encrypted get a
  built-in macro model and say so.
