# Simulation results: `copperscript-sim/v1`

`copper sim BOARD` writes `sim-results.json` beside the per-bench pages and
CSVs (`--out`, default `build/sim`). This page describes every field of that
document. The shape is frozen: `crates/copper-simulate/tests/results_schema.rs`
checks every path and its JSON types against
`crates/copper-simulate/tests/results_schema.shape` and checks that each path is
named on this page. A change of shape is a new schema version.

Rules that hold everywhere:

- Every field listed here is present in every document `copper sim` writes. A
  field that does not apply is `null` (or an empty array), never left out. The
  fields of [Added within v1](#added-within-v1) are new optional fields: a
  document written before them lacks them, and a reader must accept that.
- Numbers are SI base units: seconds, hertz, volts, amperes, ohms. `units` is
  the base-unit text of a value (`V`, `A`, `ohm`, `Hz`, `dB`, `deg`, `V/rtHz`;
  `1` for a dimensionless ratio, which the text table and pages show in `%`).
  `dimension` is the quantity's dimension name (`voltage`, `frequency`, ...),
  or `null` for a combination without a name.
- A value that cannot be computed (a "never" measure, an undefined statistic)
  is `null`.
- Indices count from 0. A step index refers to `benches[].steps[]`.
- Object keys are in a fixed order; arrays are in step, measure or plot order
  as declared in the bench.

Code: `crates/copper-simulate/src/results.rs` (the document, `bench_value`),
`noise.rs` (`noise`), `stability.rs` (`pole_zero`), `variation.rs`
(`variation`), `spice.rs` (the model text of a vendor model).

## Document

| Path | Type | Meaning |
| --- | --- | --- |
| `schema` | string | `copperscript-sim/v1`. |
| `solver` | object | The solver that produced the results. |
| `solver.name` | string | `copper-sim`. |
| `solver.version` | string | The `copper-sim` crate version. |
| `solver.method` | string | The numerical method in words. |
| `source` | object | Added within v1. The sources the results were made from, so a check that reads them can refuse results of another version of the board or benches. |
| `source.board_file` | string | The board file as `copper sim` was given it. |
| `source.design_sha256` | string | SHA-256 of the compiled design (the JSON `copper compile` writes): any change to the board, its parts, packages, ratings or sim blocks changes it; a comment does not. |
| `source.benches_sha256` | string | SHA-256 over the board's bench sources: each file of `benches/` in load order, its name relative to `benches/`, a NUL, its bytes and a NUL. |
| `source.models` | array | Added within v1. Every local SPICE model file the board's simulation reads: each `sim spice { file = ...; }` of the parts of the board, its modules and its benches, and every file it includes (`.INCLUDE`, `.LIB FILE`), sorted by file. Vendor models are not listed (the hash their part declares pins them, and `design_sha256` covers it). A document without the field is accepted only for a board that reads no local model file. |
| `source.models[].file` | string | The file, relative to the board's directory, with `/`. |
| `source.models[].sha256` | string | SHA-256 of its bytes; empty when it could not be read. |
| `board` | string | The board's name. |
| `status` | string | `pass`, `fail` (a measure outside its limits) or `error` (a bench could not be solved); the worst of the benches. |
| `benches` | array | One entry per bench run, in file and declaration order. A bench with several analyses gives one entry per analysis (a section). With `--bench NAME` only that bench. |
| `benches[]` | object | One bench (or one section of a bench). |

## Bench

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].name` | string | The run's name: the bench name, or `BENCH.SECTION` for one analysis of a bench with several. Unique in the document; file names derive from it. |
| `benches[].bench` | string | The bench's declared name (without the section). |
| `benches[].section` | string or null | The analysis name of a section, `null` for a bench with one analysis. Entries with the same `bench` are the sections of one bench. |
| `benches[].board` | string | The board the bench is for. |
| `benches[].module` | string or null | The qualified module a bench simulates on its own (`bench B for module M of Board`), else `null`. |
| `benches[].file` | string | The bench source file. |
| `benches[].status` | string | `pass`, `fail` or `error`. |
| `benches[].error` | string or null | Why the bench could not be solved, in source terms; `null` when it ran. A bench with an error has no step results. |
| `benches[].files` | object | The files `copper sim` wrote for this bench in the output directory. |
| `benches[].files.html` | string | The report page, `NAME.html` (name characters outside `[A-Za-z0-9_.-]` replaced by `_`). |
| `benches[].files.csv` | string or null | The plotted vectors, `NAME.csv`; `null` without plots or on an error. |
| `benches[].files.spectrum_csv` | string or null | The plotted spectra, `NAME.spectrum.csv`; `null` without `plot spectrum(...)`. |

### Analysis

`analysis` describes the bench's analysis with the fields of its kind; the
fields of other kinds are absent. A field whose value differs between steps
(a bound that names a stepped param) is `null` here, named in `stepped`, and
given per step in `benches[].steps[].analysis`.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].analysis` | object | The analysis. |
| `benches[].analysis.kind` | string | `op`, `ac`, `noise`, `tran`, `dc`, `loop` or `pz`. |
| `benches[].analysis.stepped` | array | Names of the fields that differ between steps. |
| `benches[].analysis.stepped[]` | string | One field name. |
| `benches[].analysis.from_hz` | float or null | `ac`, `noise`, `loop`: the first frequency. |
| `benches[].analysis.to_hz` | float or null | `ac`, `noise`, `loop`: the last frequency. |
| `benches[].analysis.points_per_decade` | int or null | `ac`, `noise`, `loop`: sweep density. |
| `benches[].analysis.outputs` | array or null | `noise`: the outputs, `V(OUT)` or `V(P, N)`, one per output of the analysis. |
| `benches[].analysis.outputs[]` | string | One output. |
| `benches[].analysis.input` | string or null | `noise`: the source the noise is referred to; `pz`: the source of the transfer function whose zeros are found; else `null`. |
| `benches[].analysis.output` | string or null | `pz`: the output of the transfer function (`null` without one). |
| `benches[].analysis.temperature_k` | float or null | `noise`: the noise temperature, K. |
| `benches[].analysis.stop_s` | float or null | `tran`: the end of the run. |
| `benches[].analysis.start_s` | float or null | `tran`: the start of the recorded part (0 records everything). |
| `benches[].analysis.max_step_s` | float or null | `tran`: the largest time step, `null` when the solver chooses. |
| `benches[].analysis.reltol` | float or null | `tran`: the relative tolerance of the step control. |
| `benches[].analysis.steady_period_s` | float or null | `tran`: the period of a periodic steady state (`steady = ...`), `null` without one. |
| `benches[].analysis.noise_seed` | int or null | `tran`: the seed of transient noise (`noise = true`), `null` without noise. |
| `benches[].analysis.sweep` | string or null | `dc`: the swept param, source or supply. |
| `benches[].analysis.units` | string or null | `dc`: the base unit of the swept value. |
| `benches[].analysis.dimension` | string or null | `dc`: the dimension of the swept value. |
| `benches[].analysis.from` | float or null | `dc`: the first swept value. |
| `benches[].analysis.to` | float or null | `dc`: the last swept value. |
| `benches[].analysis.points` | int or null | `dc`: the number of swept values. |
| `benches[].analysis.probe` | string or null | `loop`: the probed pin, `COMPONENT.PIN`. |
| `benches[].analysis.net` | string or null | `loop`: the net the probe breaks. |

### Axis

The common x axis of the bench's plotted vectors. Its values per step are
`benches[].steps[].x`.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].axis` | object or null | `null` for `op` and `pz` (nothing is swept). |
| `benches[].axis.quantity` | string | `frequency` (`ac`, `noise`, `loop`), `time` (`tran`) or `sweep` (`dc`). |
| `benches[].axis.name` | string | `frequency`, `time`, or the swept name of a dc sweep. |
| `benches[].axis.units` | string | `Hz`, `s`, or the swept value's base unit. |
| `benches[].axis.dimension` | string or null | The axis dimension. |
| `benches[].axis.scale` | string | `log` for frequency, `linear` for time and dc sweeps: how the pages draw it. |

### Models

Decision 3 of the simulation plan: every result names the model each part ran with.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].models` | array | Every component, supply and bench model of the circuit. |
| `benches[].models[]` | object | One of them. |
| `benches[].models[].name` | string | Reference (`R1`, `U3`) or supply name. |
| `benches[].models[].kind` | string | `component`, `supply` or `model` (a bench `model` instance). |
| `benches[].models[].part` | string or null | The part it instantiates, `null` for passives without one and supplies. |
| `benches[].models[].bench` | bool | Declared by the bench (not the board). |
| `benches[].models[].fidelity` | string | `ideal`, `macro`, `vendor`, `replaced`, `ignored`; supplies also `ground` and `component`. |
| `benches[].models[].model` | string | The model in words: `resistor`, `opamp (parameter set for a 10 V supply)`, `spice OPA810 (file, dialect, sha256 ...)`, `dc source 5 V`. |
| `benches[].models[].inactive_parameters` | array | Declared model parameters this analysis does not use (noise parameters in an ac run, slew in a small-signal run). |
| `benches[].models[].inactive_parameters[]` | string | One parameter name. |

### Steps

A bench runs once per combination of its `step` values; a bench without
steps has one step labelled `nominal`.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].steps` | array | The steps in run order (the first `step` statement outermost). Empty on an error. |
| `benches[].steps[]` | object | One step. |
| `benches[].steps[].index` | int | Its index. |
| `benches[].steps[].label` | string | `R_LOAD = 1 kohm, TEMP = 85 degC`, or `nominal`. |
| `benches[].steps[].stepped` | object | The stepped params and their values, keyed by param name (empty without steps). |
| `benches[].steps[].stepped.*` | object | A value as written, in the quantity form of all CopperScript JSON. |
| `benches[].steps[].stepped.*.value` | string | The value as text, `1 kohm`. |
| `benches[].steps[].stepped.*.unit` | string | Its unit, `kohm`. |
| `benches[].steps[].stepped.*.base_value` | string | The exact value in base units as a decimal string, `1000`. |
| `benches[].steps[].params` | object | Every effective param at this step, keyed by name. |
| `benches[].steps[].params.*` | float | Its value in base units. |
| `benches[].steps[].analysis` | object | The analysis fields named in `benches[].analysis.stepped`, with this step's values; empty when none. |
| `benches[].steps[].analysis.*` | float, int, string or null | One field's value at this step (the types of the field in `benches[].analysis`). |
| `benches[].steps[].x` | array | The axis values of this step (`benches[].axis`): frequencies, recorded times or swept values; empty for `op` and `pz`. Every plot of the step has one value per entry. |
| `benches[].steps[].x[]` | float | One axis value. |
| `benches[].steps[].transient` | object or null | The transient solver's work at this step; `null` outside `tran`. |
| `benches[].steps[].transient.accepted_steps` | int | Accepted time steps. |
| `benches[].steps[].transient.rejected_steps` | int | Rejected time steps. |
| `benches[].steps[].transient.newton_iterations` | int | Newton iterations. |
| `benches[].steps[].transient.factorizations` | int | Matrix factorisations. |
| `benches[].steps[].transient.breakpoints` | int | Source breakpoints the step control landed on. |
| `benches[].steps[].transient.smallest_step_s` | float | The smallest accepted step. |
| `benches[].steps[].transient.largest_step_s` | float | The largest accepted step. |
| `benches[].steps[].transient.steady_iterations` | int | Newton iterations of the periodic steady state (0 without one). |
| `benches[].steps[].transient.steady_periods` | int | Periods integrated to find it, one per Jacobian product included (0 without one). |
| `benches[].steps[].transient.steady_residual` | float or null | Its final residual, `null` without a steady state. |
| `benches[].steps[].transient.steady_fallback` | string or null | Why shooting gave up and the run settled period by period instead, `null` when shooting converged or without a steady state. |
| `benches[].steps[].transient.noise_generators` | int | Noise generators injected (transient noise; 0 without). |
| `benches[].steps[].pins` | array | Added within v1. Every pin of the target's components that has an electrical profile and whose node the bench solved, for the checks of `copper check --sim-results`: over the step of an `op`, `dc` or `tran` bench, and at the bias point of an `ac`, `noise`, `loop` or `pz` bench (the operating point of its circuit; a node with no DC path is held to ground for it, as the small-signal analyses hold it, and nothing on such a node is recorded). Empty at a step whose `bias_step` names another. |
| `benches[].steps[].pins[]` | object | One pin. |
| `benches[].steps[].pins[].pin` | string | `COMPONENT.PIN` (the package pin) in the bench's names: a module bench's are the module's own. |
| `benches[].steps[].pins[].net` | string | The node it is on. |
| `benches[].steps[].pins[].fidelity` | string | The component's fidelity in this bench (as in `models[]`). |
| `benches[].steps[].pins[].min_v` | float | The lowest voltage against the bench's ground over the step: the operating point (a bias point included), the swept values or the recorded span of a transient run. |
| `benches[].steps[].pins[].max_v` | float | The highest. |
| `benches[].steps[].pins[].min_at` | float or null | The axis value where the lowest was reached (time, swept value); `null` for `op` and a bias point. |
| `benches[].steps[].pins[].max_at` | float or null | Where the highest was reached. |
| `benches[].steps[].pins[].relative` | array | For each pin an absolute rating of this pin is relative to (`absolute_max = "VP+0.2V"`): the extremes of `V(pin) - V(that pin)`, taken at the same instants. Empty without relative ratings. |
| `benches[].steps[].pins[].relative[]` | object | One reference pin. |
| `benches[].steps[].pins[].relative[].reference` | string | `COMPONENT.PIN` of the reference. |
| `benches[].steps[].pins[].relative[].min_v` | float | The lowest difference. |
| `benches[].steps[].pins[].relative[].max_v` | float | The highest difference. |
| `benches[].steps[].pins[].relative[].min_at` | float or null | Where the lowest was reached. |
| `benches[].steps[].pins[].relative[].max_at` | float or null | Where the highest was reached. |
| `benches[].steps[].supplies` | array | Added within v1. For the steps and bias points `pins` covers: every supply the bench drives with a source or a replacement model, and every supply driven by a component the bench simulates with its own model (the current out of the supply's `source` pin); not the ground or an ignored supply. |
| `benches[].steps[].supplies[]` | object | One supply. |
| `benches[].steps[].supplies[].supply` | string | Its name. |
| `benches[].steps[].supplies[].net` | string | Its net, in the bench's names. |
| `benches[].steps[].supplies[].voltage_v` | float | Its declared voltage. |
| `benches[].steps[].supplies[].min_a` | float | The lowest `I(SUPPLY)` (the current it delivers into its net) over the step. |
| `benches[].steps[].supplies[].max_a` | float | The highest. |
| `benches[].steps[].supplies[].min_at` | float or null | Where the lowest was reached; `null` for `op`. |
| `benches[].steps[].supplies[].max_at` | float or null | Where the highest was reached. |
| `benches[].steps[].supplies[].mean_a` | float or null | The operating point's current (a bias point's included), or the time-weighted mean over the recorded span of a transient run; `null` for `dc`. |
| `benches[].steps[].supplies[].driven_by` | string | Added within v1 (I4). What drives the supply: `source` (an ideal DC source), `replacement` (the bench's model in its place) or `COMPONENT.PIN`, the source pin of the component that drives it. A document without the field recorded only the first two. |
| `benches[].steps[].stress` | array | Added within v1 (I4). For the steps and bias points `pins` covers: for each component the bench simulates with its own model whose part declares a stress rating, the quantity the rating limits, for the checks of `copper check --sim-results`. Components without a rating are not recorded (the design digest covers the ratings). |
| `benches[].steps[].stress[]` | object | One quantity of one component. |
| `benches[].steps[].stress[].component` | string | The component, in the bench's names. |
| `benches[].steps[].stress[].quantity` | string | `power`: `V(a, b) x I(a)` of a two-terminal part with `rated_power` (a and b its first and second pin, `I(a)` the current into a); `voltage`: `V(a, b)` of one with `rated_voltage`; `current`: the current into a pin with `current_max`. |
| `benches[].steps[].stress[].pin` | string or null | The package pin of a `current`; `null` for `power` and `voltage`. |
| `benches[].steps[].stress[].units` | string | `W`, `V` or `A`. |
| `benches[].steps[].stress[].min` | float | The lowest value over the step (as `pins[].min_v`). |
| `benches[].steps[].stress[].max` | float | The highest. |
| `benches[].steps[].stress[].min_at` | float or null | Where the lowest was reached; `null` for `op` and a bias point. |
| `benches[].steps[].stress[].max_at` | float or null | Where the highest was reached. |
| `benches[].steps[].stress[].mean` | float or null | The operating point's value, or the time-weighted mean of a transient run; `null` for `dc`. |
| `benches[].steps[].bias_step` | int or null | Added within v1 (I4). At a step of an `ac`, `noise`, `loop` or `pz` bench whose bias point is the same as an earlier step's (every recorded value within `1e-9` relative), that step's index: its `pins`, `supplies` and `stress` apply, and this step's are empty. `null` otherwise. |

### Measures

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].measures` | array | The measures in declaration order. |
| `benches[].measures[]` | object | One measure over every step. |
| `benches[].measures[].name` | string | Its name. |
| `benches[].measures[].expression` | string | Its expression as written. |
| `benches[].measures[].units` | string | Base unit of its values. |
| `benches[].measures[].dimension` | string or null | Their dimension. |
| `benches[].measures[].limits` | string | The limits as written (`min 15 deg, max 1 %`), empty without limits. |
| `benches[].measures[].where` | string or null | The condition that scopes the limits to some steps (`R_TRANSFER <= 800ohm`), `null` when they apply at every step. |
| `benches[].measures[].covered_steps` | array | The steps the limits apply at (every step without `where`). |
| `benches[].measures[].covered_steps[]` | int | A step index. |
| `benches[].measures[].mark` | object or null | How the limit is drawn on a plot of this bench, `null` when it is not drawn. |
| `benches[].measures[].mark.kind` | string | `lines`: `min(P)` or `max(P)` of plot `P`, horizontal lines at the limits across the axis; `window`: `at(P, x)` (or `db(at(X, x))` beside a plotted `db(X)`, likewise `mag`, `phase`, `re`, `im`), a window at `x` (per step in `benches[].measures[].steps[].at`) between the limits. |
| `benches[].measures[].mark.plot` | int | Index into `benches[].plots`. |
| `benches[].measures[].worst_step` | int or null | The step with the least margin to the limits, `null` without limits. |
| `benches[].measures[].status` | string | `pass`, `fail`, `error` or `no_limit`. |
| `benches[].measures[].steps` | array | One entry per step. |
| `benches[].measures[].steps[]` | object | The measure at one step. |
| `benches[].measures[].steps[].value` | float or null | The value, `null` when it is "never". |
| `benches[].measures[].steps[].never` | bool | An `upto` or `downto` whose condition fails at the first sample: it has no value and fails every limit that covers its step. |
| `benches[].measures[].steps[].min` | float or null | The lower limit at this step (limits may name stepped params), `null` without one. |
| `benches[].measures[].steps[].max` | float or null | The upper limit at this step. |
| `benches[].measures[].steps[].pass` | bool or null | Within the limits; `null` without limits or where `where` does not cover the step. |
| `benches[].measures[].steps[].covered` | bool | The limits apply at this step (`where` holds; always true without `where`). |
| `benches[].measures[].steps[].at` | float or null | The axis value of the window a `window` mark draws (in `benches[].axis.units`), else `null`. |

### Plots

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].plots` | array | The plotted expressions in declaration order (`plot spectrum(...)` is in `spectra` instead). |
| `benches[].plots[]` | object | One plotted expression. |
| `benches[].plots[].expression` | string | As written, `db(h)`. |
| `benches[].plots[].units` | string | Base unit of its values. |
| `benches[].plots[].dimension` | string or null | Their dimension. |
| `benches[].plots[].steps` | array | One entry per step. |
| `benches[].plots[].steps[]` | object | The vector at one step. |
| `benches[].plots[].steps[].values` | array | One value per entry of `benches[].steps[].x` of that step. A transient record keeps every time point; the pages draw a decimated copy. |
| `benches[].plots[].steps[].values[]` | float or null | One value, `null` where the expression is not finite there. |

### Spectra

`plot spectrum(x, f0)` (coherent over whole periods of `f0`) and
`plot spectrum(x)` (windowed over the record) of a `tran` bench.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].spectra` | array | The plotted spectra in declaration order; empty without. |
| `benches[].spectra[]` | object | One spectrum. |
| `benches[].spectra[].expression` | string | As written, `spectrum(V(OUT), 1kHz)`. |
| `benches[].spectra[].units` | string | Base unit of the amplitudes. |
| `benches[].spectra[].dimension` | string or null | Their dimension. |
| `benches[].spectra[].coherent` | bool | With a fundamental: bins on its harmonics, levels drawn relative to it (dBc). |
| `benches[].spectra[].steps` | array | One entry per step. |
| `benches[].spectra[].steps[]` | object | The spectrum at one step. |
| `benches[].spectra[].steps[].fundamental_hz` | float or null | `f0` of a coherent spectrum, else `null`. |
| `benches[].spectra[].steps[].periods` | int | Whole periods of `f0` analysed (0 for a windowed spectrum). |
| `benches[].spectra[].steps[].window` | string | `rectangular` (coherent), `blackman_harris`, `hann`, `flat_top`. |
| `benches[].spectra[].steps[].samples` | int | Uniform samples the record was resampled to. |
| `benches[].spectra[].steps[].start_s` | float | Start of the analysed span. |
| `benches[].spectra[].steps[].span_s` | float | Its length. |
| `benches[].spectra[].steps[].uniform_steps` | bool | The solver's steps were uniform over the span (a `max_step` held them), which lowers the integration floor. |
| `benches[].spectra[].steps[].integration_floor` | float | The integration floor relative to the largest component: components below it are inside the integration error. |
| `benches[].spectra[].steps[].frequency_hz` | array | Bin frequencies, from 0 Hz. |
| `benches[].spectra[].steps[].frequency_hz[]` | float | One bin. |
| `benches[].spectra[].steps[].amplitude` | array | Peak amplitude per bin. |
| `benches[].spectra[].steps[].amplitude[]` | float | One amplitude. |

### Noise

`analysis noise`: per step and output, the integrated output noise and its contributors.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].noise` | array | One entry per step and output (steps outermost); empty outside `noise`. |
| `benches[].noise[]` | object | One output at one step. |
| `benches[].noise[].step` | int | The step. |
| `benches[].noise[].output` | string | The output, `V(OUT)`. |
| `benches[].noise[].from_hz` | float | The integration band's lower end (the analysis band). |
| `benches[].noise[].to_hz` | float | Its upper end. |
| `benches[].noise[].total_rms_v` | float | RMS output noise over the band, V. |
| `benches[].noise[].contributors` | array | Every noise generator, largest share first. |
| `benches[].noise[].contributors[]` | object | One generator. |
| `benches[].noise[].contributors[].contributor` | string | `R1 thermal`, `U1 voltage noise`. |
| `benches[].noise[].contributors[].owner` | string | The component it belongs to. |
| `benches[].noise[].contributors[].rms_v` | float | Its RMS share at the output, V. |
| `benches[].noise[].contributors[].share` | float | Its fraction of the total noise power (0 to 1). |

### Poles and zeros

`analysis pz`: per step, the poles of the linearised circuit and, with an
input and output, the zeros of that transfer function. Frequencies are
`s / (2 pi)`.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].pole_zero` | array | One entry per step; empty outside `pz`. |
| `benches[].pole_zero[]` | object | The roots at one step. |
| `benches[].pole_zero[].step` | int | The step. |
| `benches[].pole_zero[].stable` | bool | No pole in the right half-plane. |
| `benches[].pole_zero[].unknowns` | int | Unknowns of the circuit's equations. |
| `benches[].pole_zero[].infinite_eigenvalues` | int | Unknowns without dynamics of their own (deflated). |
| `benches[].pole_zero[].transfer` | string or null | `V(OUT) / SRC`, the transfer function whose zeros are listed; `null` without an input and output. |
| `benches[].pole_zero[].poles` | array | The finite poles, both members of each complex pair. |
| `benches[].pole_zero[].poles[]` | object | One pole. |
| `benches[].pole_zero[].poles[].re_hz` | float | Real part. |
| `benches[].pole_zero[].poles[].im_hz` | float | Imaginary part. |
| `benches[].pole_zero[].poles[].frequency_hz` | float | Natural frequency `abs(s) / (2 pi)`. |
| `benches[].pole_zero[].poles[].damping` | float or null | Damping ratio `-Re(s) / abs(s)`: 1 for a decaying real pole, 0 to 1 for a decaying pair, negative when it grows; `null` at the origin. |
| `benches[].pole_zero[].poles[].right_half_plane` | bool | The pole grows: the circuit oscillates or latches. |
| `benches[].pole_zero[].poles[].refined` | bool | Refined on the circuit's own matrices (false: as the QR iteration gave it). |
| `benches[].pole_zero[].poles[].real` | bool | A real pole. |
| `benches[].pole_zero[].poles[].origin` | bool | A pole at the origin. |
| `benches[].pole_zero[].poles[].nodes` | array | The nets (or `U1 (internal)`) the pole dominates, largest share first. |
| `benches[].pole_zero[].poles[].nodes[]` | string | `net 'OUT'` or `U1 (internal)`. |
| `benches[].pole_zero[].zeros` | array | The zeros of `transfer`, with the fields of a pole (`nodes` empty, `refined` false); empty without a transfer function. |

### Variation

`montecarlo` or `worstcase`: the statistics over the runs of each step. A
measure's `steps[].value` is then the worst run's.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].variation` | object or null | `null` without `montecarlo` or `worstcase`. |
| `benches[].variation.method` | object | The method. |
| `benches[].variation.method.kind` | string | `montecarlo` or `worstcase`. |
| `benches[].variation.method.runs` | int or null | Monte Carlo runs per step; `null` for worst case. |
| `benches[].variation.method.seed` | int or null | The Monte Carlo seed; `null` for worst case. |
| `benches[].variation.method.distribution` | string or null | `uniform` or `gaussian`; `null` for worst case. |
| `benches[].variation.variables` | array | The varied quantities: component values, matched groups (`GAIN common`, `GAIN R_G`), params and model parameters (`U1.input_offset_voltage`). |
| `benches[].variation.variables[]` | string | One variable. |
| `benches[].variation.yield` | float or null | Fraction of runs within every limit at every step, `null` without limits. |
| `benches[].variation.steps` | array | One entry per step. |
| `benches[].variation.steps[]` | object | One step. |
| `benches[].variation.steps[].index` | int | The step. |
| `benches[].variation.steps[].runs` | int | Runs (or corners) at this step. |
| `benches[].variation.steps[].yield` | float or null | Fraction of runs within every limit at this step. |
| `benches[].variation.steps[].measures` | array | One entry per measure, in measure order. |
| `benches[].variation.steps[].measures[]` | object | One measure over the runs. |
| `benches[].variation.steps[].measures[].name` | string | The measure. |
| `benches[].variation.steps[].measures[].nominal` | float or null | Its value without variation. |
| `benches[].variation.steps[].measures[].min` | float or null | Smallest over the runs. |
| `benches[].variation.steps[].measures[].max` | float or null | Largest over the runs. |
| `benches[].variation.steps[].measures[].yield` | float or null | Fraction of runs within this measure's limits, `null` without limits. |
| `benches[].variation.steps[].measures[].worst_run` | int or null | The run with the least margin, `null` without limits. |
| `benches[].variation.steps[].measures[].mean` | float or null | Monte Carlo mean, `null` for worst case. |
| `benches[].variation.steps[].measures[].std` | float or null | Monte Carlo standard deviation, `null` for worst case. |
| `benches[].variation.steps[].measures[].values` | array | The value of every run in run order (Monte Carlo; worst case: the corners'). |
| `benches[].variation.steps[].measures[].values[]` | float or null | One run's value. |
| `benches[].variation.steps[].measures[].low_corner` | object or null | Worst case: the corner of the lowest value; `null` for Monte Carlo. |
| `benches[].variation.steps[].measures[].low_corner.value` | float | The value there. |
| `benches[].variation.steps[].measures[].low_corner.x` | object | Each variable's position at that corner, keyed by variable. |
| `benches[].variation.steps[].measures[].low_corner.x.*` | int | -1 (low end of its tolerance), 0 (nominal) or 1 (high end). |
| `benches[].variation.steps[].measures[].high_corner` | object or null | Worst case: the corner of the highest value. |
| `benches[].variation.steps[].measures[].high_corner.value` | float | The value there. |
| `benches[].variation.steps[].measures[].high_corner.x` | object | Each variable's position. |
| `benches[].variation.steps[].measures[].high_corner.x.*` | int | -1, 0 or 1. |
| `benches[].variation.steps[].measures[].sensitivity` | object or null | Worst case: the change of the measure per variable over its tolerance, keyed by variable; `null` for Monte Carlo. |
| `benches[].variation.steps[].measures[].sensitivity.*` | float | One variable's sensitivity, in the measure's units. |

### Timing

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].timing` | object or null | With `copper sim --timing` (or `COPPER_SIM_TIMING=1`), else `null`. |
| `benches[].timing.wall_s` | float | Wall time of the bench. |
| `benches[].timing.steps_s` | array | Wall time of each step with its variation runs. |
| `benches[].timing.steps_s[]` | float | One step. |
| `benches[].timing.phases` | object | Solver time per phase, summed over threads. |
| `benches[].timing.phases.assembly_s` | float | Stamping the matrices. |
| `benches[].timing.phases.devices_s` | float | Evaluating nonlinear devices. |
| `benches[].timing.phases.factor_s` | float | Factorising. |
| `benches[].timing.phases.solve_s` | float | Solving with the factors. |
| `benches[].timing.phases.step_control_s` | float | Transient step control. |
| `benches[].timing.phases.storage_s` | float | Storing traces. |
| `benches[].timing.phases.evaluation_s` | float | Evaluating measures and plots. |
| `benches[].timing.factorizations` | object | Factorisations by kind. |
| `benches[].timing.factorizations.ordered` | int | Sparse with a new ordering. |
| `benches[].timing.factorizations.refactored` | int | Sparse in a kept ordering. |
| `benches[].timing.factorizations.dense` | int | Dense (singular or nearly singular systems). |

### Layout

Only on a bench that says `use layout;` and ran with `copper sim --layout RUN`
([language reference](language-reference.md#layout-parasitics)); every other
bench has no `layout` key. The bench's own `steps`, `measures` and `plots` are
the run with the layout.

| Path | Type | Meaning |
| --- | --- | --- |
| `benches[].layout` | object | The parasitics the bench ran with, and its schematic-only run. |
| `benches[].layout.source` | string | The routed board file (`RUN/final-board.state.json`). |
| `benches[].layout.elements` | object | What the extraction holds. |
| `benches[].layout.elements.resistors` | int | Track resistances (one per merged branch). |
| `benches[].layout.elements.inductors` | int | Track inductances (one per merged branch). |
| `benches[].layout.elements.capacitors` | int | Capacitances between nodes of two nets. Elements on nets the bench does not simulate are left out of its circuit. |
| `benches[].layout.contributors` | array | The largest extracted values: up to five net-to-net capacitances, largest first, then up to five nets' series copper, largest inductance first. |
| `benches[].layout.contributors[].kind` | string | `capacitance` or `series`. |
| `benches[].layout.contributors[].nets` | array | The two nets (`capacitance`) or the net (`series`). |
| `benches[].layout.contributors[].nets[]` | string | A net. |
| `benches[].layout.contributors[].value` | float | F, or H (the net's total track inductance) for `series`. |
| `benches[].layout.contributors[].unit` | string | `F` or `H`. |
| `benches[].layout.contributors[].text` | string | The same in words, with the track length and resistance for `series`. |
| `benches[].layout.flags` | array | `assumed_stackup`, `assumed_copper_thickness`, `inductance_without_return`. |
| `benches[].layout.flags[]` | string | One flag. |
| `benches[].layout.notes` | array | What the extraction left out or assumed, and its size and time. |
| `benches[].layout.notes[]` | string | One note. |
| `benches[].layout.schematic` | object | The bench on the schematic alone. |
| `benches[].layout.schematic.error` | string or null | Why it could not be solved. |
| `benches[].layout.schematic.measures` | array | One entry per measure, in measure order. |
| `benches[].layout.schematic.measures[].name` | string | The measure. |
| `benches[].layout.schematic.measures[].steps` | array | One entry per step. |
| `benches[].layout.schematic.measures[].steps[].value` | float or null | Its value without the layout. |
| `benches[].layout.schematic.measures[].steps[].pass` | bool or null | Whether that value is within the limits. |

## Added within v1

New optional fields, for the checks that take simulation results as
evidence (plan F11; code: `crates/copper-simulate/src/evidence.rs`):

- `source`: the board file and the digests of the design and the bench
  sources. `copper check --sim-results FILE` and `copper power-check
  --sim-results FILE` recompute both from the board they are given and refuse
  (`SIM_RESULTS_STALE`) a document whose board, design digest or bench digest
  differs, or that has no `source`. `source.models` (added later in v1, plan
  I5 fixes): the local SPICE model files and the files they include, with
  their digests, recomputed and compared the same way; a document without it
  is refused only when the board reads a local model file, since it cannot
  vouch for that file's contents.
- `benches[].steps[].pins` and `benches[].steps[].supplies`: what those checks
  read. `copper check --sim-results` judges each pin against the absolute
  rating the board declares now (a relative rating against its `relative`
  entry); a module bench's pins are the pins of every instance of the module.
  `copper power-check --sim-results` lists the supplies' currents against the
  board supplies on the same nets (a module bench's through the instance's
  port connections).

Added in plan I4 (simulation improvement plan), still under v1:

- The bias point of an `ac`, `noise`, `loop` or `pz` bench is recorded in
  `pins` and `supplies` (and `stress`) like an operating point, and a step
  whose bias point repeats an earlier one's refers to it (`bias_step`). The
  operating point is solved from the bench's own circuit at each step; nodes
  with no DC path are held to ground for it and nothing on them is recorded.
- `supplies[]` also covers a supply driven by a component the bench simulates
  (`driven_by` names its source pin), so a board bench that runs a regulator's
  model records the rail it makes.
- `steps[].stress`: the quantities the parts' stress ratings limit, which
  `copper check --sim-results` judges (`SIM_STRESS_*`); `copper power-check
  --sim-results` compares the supplies' currents with the rails' budgets
  (`SIM_RAIL_*`; [language reference](language-reference.md#supplies)).

Nothing else in the document changed; the measure values of the examples are
identical.

Added in plan I5, still under v1: `benches[].layout` ([Layout](#layout)), on
benches that use layout only. Nothing else changed.

## Changes from `copperscript-sim/v0.1`

Made when the schema was frozen, so that every field has one place and one type:

- `schema` is `copperscript-sim/v1`.
- The plots' x values moved from every plot (`frequency_hz`, `time_s` or
  `sweep` in each `plots[].steps[]`) to `steps[].x`, once per step; the new
  `axis` says what they are. `plots[].steps[]` holds only `values`.
- `analysis`: a field that differs between steps was an array of per-step
  values; it is now `null` with the values in `steps[].analysis`.
- `analysis.output` of a noise analysis (outputs joined by commas) is the
  array `analysis.outputs`.
- `analysis` of `tran`: `steady_period_s` and `noise_seed` are always present
  (`null` without). The `uic` field the v0.1 description named was never
  written; it is not part of v1.
- Always present: `bench`, `section`, `module` (were only on sections and
  module benches), `spectra`, `noise`, `pole_zero` (empty arrays), `variation`
  and `timing` (`null`), a measure's `where` and `covered_steps`, each
  measure step's `covered` and `never`, a pole's `real`, `origin` and `nodes`,
  `pole_zero[].transfer` and `zeros`, the steady-state and noise fields of
  `steps[].transient`, and every field of both variation methods.
- New: `files` (the bench's file names), `axis`, `measures[].mark` and
  `measures[].steps[].at` (how the pages draw a limit), `spectra[].coherent`.
- `variation.method.seed` was a string when above 2^63 - 1; it is an integer.

## Using the document in a viewer

The CopperReview simulation view (planned in the
[integrated simulation plan](integrated-simulation-plan.md), F10) reads only
this document. What it needs, and where it is:

| Need | Fields |
| --- | --- |
| List of benches with verdicts, grouped by bench and section | `benches[].bench`, `section`, `status`, `error` |
| The source to open | `benches[].file` |
| Link to the static page and CSVs | `benches[].files` |
| What was run, in words | `analysis`, `steps[].analysis`, `axis` |
| Curves: x per step, y per plot and step, axis scale | `steps[].x`, `plots[].steps[].values`, `axis.scale`, `plots[].units` |
| Step legend and toggles; grouping of steps by stepped param | `steps[].label`, `steps[].stepped` |
| Limits drawn on the curves | `measures[].mark`, `measures[].steps[].min`, `max`, `at`, `where`, `covered` |
| Measures table with worst step and pass/fail | `measures[]` |
| Spectra with the integration floor | `spectra[]` |
| Noise contributors | `noise[]` |
| Pole map | `pole_zero[]` |
| Monte Carlo histograms, worst-case corners | `variation` |
| Model fidelity per component (decision 3) | `models[]` |

A viewer should decimate long transient records for drawing (the
`Startup` bench of the impedance analyzer has 3052 points per step and twelve
plots) and keep the values for its cursor readout.
