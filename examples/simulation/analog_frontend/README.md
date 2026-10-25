# Analog front end example

The electrical board and simulation plans are implemented. The biased step and
timestep-refinement checks pass with the off-the-shelf TI model. Bias/AC and
cold-start convergence remain unresolved, so the complete example currently
reports **failed** and is not promoted into the passing CI examples.

## Circuit

`board.copper` uses two OPA2197 dual amplifiers: U1A buffers a divider reference,
U1B provides gain four, and U2A/U2B form a low-pass filter near 2 kHz. The source
declares a 60 mm by 40 mm outline, pin-associated supply bypasses, power/input/output
headers and paired signal/ground measurement headers. TP_REF measures the
reference, TP_GAIN the gain stage, and TP_FILTER the first filter stage; J_OUT
also provides access to the final output.

R_TERM and C_TERM are populated bench termination, rather than fictitious external
components. Simulation stress cases change their total equivalent termination.
All measurement/output headers are explicitly open in the plans. The power and
sensor sources explicitly replace their excluded connector endpoints.

Parts come from the existing versioned dependencies and bundled passive fixtures.
CopperLib is unchanged. The OPA2197 example definition exposes pin/unit mapping
but no reviewed operating-range or layout requirements. The board's 5 V operating
choice and bypass intent follow TI SBOS737C sections 6, 9 and 10. Passive SKUs,
orderable headers, mounting, routed geometry and assembly qualification remain open.

## Prepare the vendor model

Install CopperScript's simulation extra and an ngspice 46 or 47 runtime built
with XSPICE. The model needs the **matching engine installation's** `analog.cm`
and `xtradev.cm`. The runner records their paths and hashes and copies them into
controlled per-run initialization. Model data cannot supply a binary path or a
startup command.

```console
python examples/simulation/analog_frontend/prepare_model.py
```

This downloads the byte-pinned SBOMA34D archive from TI and checks both archive
and model hashes before installing `models/vendor/OPAx197.LIB`. It never changes
the vendor syntax or overwrites changed model bytes. Use `--archive PATH.zip` for
an already downloaded copy. The original TI model is ignored by Git and fetched
separately; its usage/copyright notices remain in the original file. The authored
dual-package wrapper is MIT and maps both channels and all eight package pins.

## Run and inspect

Select the engine with `NGSPICE` or `--ngspice`. For KiCad on Windows, its
`C:/Program Files/KiCad/10.0/bin/ngspice.dll` is supported. A standard install
locates code-model libraries under `../lib/ngspice` relative to the engine's bin
directory; use `--code-model-root PATH` for another installation layout.

```console
python -m copperscript check examples/simulation/analog_frontend/board.copper --locked --offline
python -m copperscript sim run examples/simulation/analog_frontend/board.copper --plan examples/simulation/analog_frontend/step.json
python examples/simulation/analog_frontend/run.py -o build/sim/analog-review
```

For an unrouted two-layer placement draft with the selected real footprints:

```console
python -m copperscript plan-layout examples/simulation/analog_frontend/board.copper --layers 2 --candidates 1 --locked --offline -o build/analog-layout/board.kicad_pcb --report build/analog-layout/layout-report.json
```

This generates a legal placement candidate and reports routing/native signoff
as pending. It does not establish the requested analog feedback or ground geometry.

The last command runs frequency/bias, biased step, step with half the maximum
timestep, and startup in fresh directories. It produces `index.html` and
`example-requirements.json`, alongside the normal plots, raw files, logs, model
manifests and provenance. It exits nonzero for failed or incomplete requirements.
It does not fetch models implicitly. Choose a new output directory for each run.

`run.py` evaluates bandwidth relative to each case's measured gain at 100 Hz,
nominal stopband rejection, overshoot, error throughout the complete settling
window and peak/excursion changes under timestep refinement. Checks use full
solver data; a narrow late excursion cannot be hidden by the final sample.

## Current evidence

Local ngspice 47 console evidence on 13 October 2026:

| Check | Result |
| --- | --- |
| Electrical source | ERC passed |
| Selected footprint geometry | All five footprint types resolved from locked dependencies |
| Nominal biased step | Gain 3.982; overshoot 10.78 percent; maximum settling-window error 0.0032 percent of step |
| Heavy-load biased step | Gain 3.909; overshoot 10.76 percent; settling-window error 0.0100 percent |
| Half-timestep comparison | Peak excursion changed below 0.002 percent for both cases |
| Bias and AC cases | Failed/incomplete solves or incorrect bias; requirements reject them |
| Cold startup | Solver failure; no accepted waveform evidence |

An independent four-follower fixture validates bias, AC, a small biased transient,
input bias current, supply current and eight-pin mapping with the original model.
That fixture passes; it does not establish convergence for the complete board's
resistive bias/feedback networks. Solver convergence messages are exposed in the
HTML reports and retained logs. No generic model, relaxed requirement or fabricated
waveform substitutes for failed board evidence.

The TI model is typical. Channel interaction, temperature corners, thermal,
noise, ADC acquisition and physical parasitics are not qualified. Startup below
4.5 V is outside the specified operating supply and remains diagnostic even if
its numerical solve is later completed. PCB routing and native DRC are separate
work; none of these simulation results approve assembly or fabrication.

See the [board specification](SPEC.md) and
[model qualification requirements](../../../docs/simulation-qualification.md).
