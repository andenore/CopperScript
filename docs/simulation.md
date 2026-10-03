# Analog and power simulation

CopperScript can export or run an analog subset through ngspice using a separate JSON simulation plan. The compiled board supplies connectivity, part identities and nominal values. The plan supplies sources, loads, analyses, model bindings, cases, plots and limits. Simulation does not modify the authoritative board IR.

## Install and run

Install the optional reporting dependencies and an ngspice console executable:

```text
python -m pip install -e ".[simulation]"
copper sim run examples/simulation/rc_filter.copper --plan examples/simulation/rc_filter.json
copper sim run examples/simulation/inrush.copper --plan examples/simulation/inrush.json
```

Use `--ngspice /path/to/ngspice` or the `NGSPICE` environment variable to select a runtime. Windows installations that provide a DLL, including KiCad, can use:

```text
copper sim run examples/simulation/inrush.copper --plan examples/simulation/inrush.json --ngspice "C:\Program Files\KiCad\10.0\bin\ngspice.dll"
```

DLL runs use a disposable Python child process; native simulator state never enters the compiler process. Every run uses controlled startup files and records the engine version, binary hash and mode. Models in this release use native ngspice syntax; PSpice/LTspice compatibility profiles and external code models are not yet supported.

The default output is a new directory under `build/sim/<plan-name>/`. Use `-o <empty-directory>` for an explicit location. Existing nonempty directories are rejected to prevent stale waveforms from being mistaken for a successful new run. `--timeout` bounds each engine process, including each analysis/case. `--locked` and `--offline` retain their normal package-resolution behavior.

Export without installing a simulator or plotting libraries:

```text
copper sim export examples/simulation/rc_filter.copper --plan examples/simulation/rc_filter.json -o build/sim/rc-decks
```

Exit codes: `0` for a completed run with all requested checks/outputs satisfied, or an export; `1` for failed/incomplete simulation evidence; `2` for invalid input, unavailable tools or preparation errors. A completed solve without checks is labeled `completed`, not a design pass.

## Plan structure

Plans use `schema_version: 1`. Unknown fields, duplicate JSON keys, invalid units and unresolved references are errors. See the two committed examples for complete runnable files.

| Field | Meaning |
| --- | --- |
| `name` | Stable identifier used in output/report labels |
| `circuit` | `all: true`, or explicit `components` and/or `modules`; one `reference_node` |
| `sources` | Named voltage/current sources, DC bias, AC excitation, finite ramp or PWL waveform |
| `loads` | Named current sinks, with explicit waveform or current/enable/rise-time shorthand |
| `models` | Local registry path and component-reference-to-model-ID `bindings` |
| `values` | Simulation-only component value overrides |
| `analysis` / `analyses` | One analysis, or an object mapping analysis names to definitions |
| `probes` | Differential voltages and voltage-source branch currents |
| `derived_signals` | Typed ratio, difference or product operations |
| `initial_state` | Operating point by default; optional node voltages and explicitly requested UIC |
| `cases` | Named `values`, `sources` and/or `loads` overrides; nominal case by default |
| `checks` | Named measurements and dimension-checked minimum/maximum limits |
| `plots` | Bode plots or aligned time-series panels; sensible defaults if omitted |
| `outputs` | HTML report, SVG/PNG plots and CSV/JSON data, all enabled by default |
| `assumptions` | Explicit scope and approximations shown in reports |

Names for plans, analyses, cases, signals, checks and plots begin with a letter and contain letters, digits, underscores or hyphens. Component/net references retain the elaborator's actual identities: `PWR/C_IN`, `PWR/VIN`. Module ports are also accepted as aliases, such as `PWR.VIN`. Connected ports may alias a parent net. A connected component pin can identify its node, e.g. `PWR/C_IN.1`.

Values include units: `1 ms`, `100 uF`, `0.25 ohm`, `100 mA`, `10 kHz`. Compact forms such as `100uF` also work. AC phase uses degrees (`0 deg`). Gain thresholds can use dB (`-3 dB`); a dimensionless linear gain threshold is a JSON number. Native positive R/C/L values come from explicit passive categories and typed design values, never reference designator guesses. SPICE emission uses scientific SI notation to avoid suffix ambiguity.

## AC frequency sweeps and Bode plots

Supported AC grids are:

```json
{ "kind": "ac", "sweep": "decade", "start": "10 Hz", "stop": "1 MHz", "points_per_decade": 100 }
```

Use `octave` with `points_per_octave`, or `linear` with the total `points`. Logarithmic sweeps use ngspice's native grid: if the stop frequency is not aligned with a grid point, the last sample is the final grid point below it. Checks outside the available samples are incomplete; choose aligned endpoints or a linear grid when an exact endpoint is required.

AC excitation belongs to a source separately from its DC bias and transient waveform:

```json
{ "kind": "voltage", "positive": "VIN", "negative": "GND", "dc": "0 V", "ac": { "magnitude": "1 V", "phase": "0 deg" } }
```

Define gain from measured differential voltages, and optionally restrict it to one analysis:

```json
{ "gain": { "kind": "ratio", "analysis": "frequency", "numerator": "output_voltage", "denominator": "input_voltage" } }
```

The Bode plot uses `20 log10(abs(gain))` and phase in degrees on aligned frequency axes. Wrapped or unwrapped phase is explicit. Complex real/imaginary samples are retained in exports. Zero denominators produce invalid samples, diagnostic gaps and incomplete requested evidence; zero magnitude has undefined phase/dB and is recorded explicitly. Graphs never substitute finite values for these states.

AC is a small-signal analysis about the DC bias point. Its normalized excitation does not model large-signal clipping, slew-rate behavior or cold startup. A converter's AC response needs a model suitable for that operating point. Closed-loop voltage gain alone is not a loop stability measurement.

## Transient/inrush scenarios

Transient analysis specifies `stop` and the maximum internal `max_step`. Sources accept a finite `ramp` with `from`, `to`, `duration` and optional `delay`, or `pwl` with strictly increasing `[time, value]` points starting at zero. Sources hold their final waveform value after the last point. If DC and a waveform are both specified, their initial values must agree; use separate plans for AC bias and cold-start scenarios that need different operating points.

Voltage sources may include `series_resistance`; the generated series element and internal node are recorded in the deck/provenance. Model capacitor ESR as an explicit resistor in the circuit. Loads accept `current_sink`, with `current`, `enable_at` and finite `rise_time`, or explicit DC/PWL settings. Supplies declared in the board are not automatically turned into ideal voltage sources.

Source-current probes default to positive **delivered** current. Voltage, current and derived power are plotted in separate panels with a shared time axis. A `product` of voltage and current produces watts; integrating current or power on the actual nonuniform solver grid gives charge or energy. Initial node voltages are explicit; `method: "uic"` skips the initial operating point only when the plan requests it. Initial-state settings are restricted to transient-only plans.

The inrush example models a capacitor, ESR, a finite input ramp, entered source resistance and a timed load. Its results are evidence for that lumped scenario, not a prediction for the full tracker board or a current-limiting regulator.

## Subsets and model coverage

Explicitly select the complete charging and return path, including downstream capacitance. Module selection does not automatically include external decoupling or firmware-driven loads.

Every excluded endpoint connected to the subset needs a replacement or an acknowledged open boundary. Sources/loads use `covers: ["J1.VBUS", "J1.GND"]` to name the excluded endpoints they replace. Their nodes must match the covered connections. `circuit.open_boundaries` maps intentionally open endpoint names to nonempty reasons. Unexplained, repeated or mismatched coverage is rejected before simulation.

Resistors, capacitors and inductors use native primitives. Other selected devices require a registry binding. The registry supports native `.subckt` bundles and diode/BJT/MOSFET `.model` cards. There is no automatic manufacturer model lookup.

A registry file has `schema_version: 1` and a `models` object. Each model records `kind`, SPICE `name`, ordered `terminals`, `pin_map`, `supported_parts`, supported `analyses`, qualified `ngspice_versions`, `license`, an `entrypoint`, and all `files` with their byte SHA-256 hashes. Optional `source_url` and `assumptions` record provenance and validity. `ignored_pins` must explicitly account for any package terminals omitted by the model. Pin order is checked against the subcircuit declaration. Package pin numbers, logical names and unambiguous functional terminal aliases resolve through the electrical mapping helper.

Native card terminal names and order are `A, K` for a diode, `C, B, E` for a BJT, and `D, G, S, B` for a MOSFET. The `pin_map` binds those terminals to the actual package pins.

All includes must stay inside the explicitly hashed bundle. Files are copied into the output with deterministic names, and checked includes are rewritten to those copies. Control scripts and unsupported directives are rejected as model data. Encrypted models and binary code models are outside this release. A vendor model must be qualified for its engine version and intended analysis; merely converging does not establish startup, protection or thermal validity.

## Measurements, graphs and exports

Checks support `min`, `max`, `at`, `integral`, `mean`, `rms`, `peak_to_peak` and `crossing`, with optional axis `window`. AC representations include `real`, `magnitude`, `dB` and `phase`; unwrapping is explicit. Crossing checks specify a `level`, `direction` and `first`, `last` or default `unique` crossing selection. Missing or ambiguous crossings are incomplete. Use crossing of a declared dB level for bandwidth checks. For multiple analyses, checks and plots name their `analysis`.

Transient means/RMS/integrals use trapezoidal integration on the solver's nonuniform time grid; AC means/RMS use the sampled frequency points. `at` and crossing operations interpolate between available points, using log-frequency coordinates for logarithmic AC sweeps. Limits and measurements retain SI units. The full source samples are used for checks and plotting; no display decimation hides narrow peaks.

Open `report.html` for zoom/pan, hover values, legend toggles, case overlays, limit lines, measurement markers and source/load event markers. The report bundles its graph library and works offline. SVG/PNG graphs are rendered headlessly for CI and sharing. Separate CSV/JSON downloads preserve solver times/frequencies, units, complex samples and validity flags. The output also includes resolved inputs, model manifests, engine identity, decks, raw files, logs, normalized measurements, requirement status and SHA-256 artifact inventory.

Solver errors, timeout, missing vectors, invalid values, incomplete grids and requirement failures remain distinct from graph rendering failures. A completed but out-of-spec solve still produces its graphs and data. Failed runs produce a diagnostic report; partial/truncated traces are not presented as valid measurement evidence.

## CI and Python API

Commit CI builds the source/hash-pinned ngspice release in `.github/simulation-toolchain.json`, installs pinned reporting tools, runs the complete suite with simulator integration required, and uploads the bounded example reports. Long example routing remains restricted to release tags.

```python
from pathlib import Path
from pcbir.loader import load_board
from pcbir.simulation import load_plan, run_simulation

result = run_simulation(
    load_board("examples/simulation/inrush.copper"),
    load_plan("examples/simulation/inrush.json"),
    Path("build/sim/my-inrush-run"),
    ngspice="/usr/bin/ngspice",
    timeout=30,
)
```

The `NgspiceBackend` in `pcbir.backends.ngspice` also implements the existing text-artifact generation protocol. Numerical fixtures cover analytic RC gain/phase and charging, nonuniform inrush integration, source-current polarity, all AC grids, model pin mapping and negative evidence gates. Manufacturer-specific qualification, nonlinear load profiles, loop-injection stability fixtures, device-internal probes and parasitic extraction remain future extensions.
