# Analog front end example

`board.copper` is the electrical board. The `copper sim` command, the ngspice
backend, the simulation plans and the model preparation script were removed
with the Python implementation (last Python commit `b16122a`), so this example
can no longer be simulated. The circuit and the last recorded simulation
evidence stay here as reference.

## Circuit

`board.copper` uses two OPA2197 dual amplifiers: U1A buffers a divider reference,
U1B provides gain four, and U2A/U2B form a low-pass filter near 2 kHz. The source
declares a 60 mm by 40 mm outline, pin-associated supply bypasses, power/input/output
headers and paired signal/ground measurement headers. TP_REF measures the
reference, TP_GAIN the gain stage, and TP_FILTER the first filter stage; J_OUT
also provides access to the final output.

R_TERM and C_TERM are populated bench termination, rather than fictitious external
components. The simulation stress cases changed their total equivalent termination.
The removed simulation plans left all measurement/output headers explicitly open,
and the power and sensor sources replaced their excluded connector endpoints.

Parts come from the existing versioned dependencies and bundled passive fixtures.
CopperLib is unchanged. The OPA2197 example definition exposes pin/unit mapping
but no reviewed operating-range or layout requirements. The board's 5 V operating
choice and bypass intent follow TI SBOS737C sections 6, 9 and 10. Passive SKUs,
orderable headers, mounting, routed geometry and assembly qualification remain open.

## Check and place

```console
copper check examples/simulation/analog_frontend/board.copper --locked --offline
```

For an unrouted two-layer placement draft with the selected real footprints:

```console
copper plan-layout examples/simulation/analog_frontend/board.copper --layers 2 --candidates 1 --locked --offline -o build/analog-layout/board.kicad_pcb --report build/analog-layout/layout-report.json
```

This generates a legal placement candidate and reports routing/native signoff
as pending. It does not establish the requested analog feedback or ground geometry.

## Recorded simulation evidence

The removed implementation used the off-the-shelf TI OPAx197 model (SBOMA34D)
with ngspice 46/47 built with XSPICE, loading both `analog.cm` and `xtradev.cm`.
It ran frequency/bias, biased step, step with half the maximum timestep, and
startup, and evaluated bandwidth relative to each case's measured gain at
100 Hz, nominal stopband rejection, overshoot, error throughout the complete
settling window and peak/excursion changes under timestep refinement.

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

An independent four-follower fixture validated bias, AC, a small biased transient,
input bias current, supply current and eight-pin mapping with the original model.
That fixture passed; it did not establish convergence for the complete board's
resistive bias/feedback networks. No generic model, relaxed requirement or fabricated
waveform substitutes for failed board evidence.

The TI model is typical. Channel interaction, temperature corners, thermal,
noise, ADC acquisition and physical parasitics are not qualified. Startup below
4.5 V is outside the specified operating supply and remains diagnostic even if
its numerical solve is later completed. PCB routing and native DRC are separate
work; none of these simulation results approve assembly or fabrication.

See the [board specification](SPEC.md) and the
[simulation research](../../../docs/simulation-analog-research.md).
