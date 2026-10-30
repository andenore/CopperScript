# Analog sensor front end board specification

Status: electrical source implemented, 13 October 2026. The simulation plans
and the `copper sim` command were removed with the Python implementation; the
last recorded results (biased-step checks passed; bias/AC and cold-start
convergence unresolved) are in the [README](README.md). This is not a
fabrication release.

## Purpose and architecture

Demonstrate a complete analog signal path, component corners and measurable
simulation requirements in CopperScript. Use two dual OPA2197 amplifiers:
U1A buffers the midpoint reference, U1B provides gain four, and U2A/U2B form
an approximately fourth-order Butterworth low-pass filter. Put the lower-Q
filter stage first. All filter returns and the gain-setting resistor return
connect to the buffered reference.

## Electrical targets

| Item | Requirement |
| --- | --- |
| Supply | External 5 V nominal; operating scenarios 4.75 V to 5.25 V |
| Reference | Buffered midpoint, nominal 2.5 V; within 10 mV of half the applied supply after settling |
| Input | Single-ended, centered on reference; signal excursion up to plus or minus 200 mV; nominal source resistance 1 kohm |
| Gain | Nominal four; 11.9 dB to 12.2 dB at 100 Hz under nominal loading |
| Bandwidth | Minus-3 dB frequency relative to measured passband gain: 1.8 kHz to 2.2 kHz |
| Stopband | At least 45 dB rejection at 10 kHz relative to passband gain, nominal case |
| Output | Nominal 10 kohm and 1 nF total bench termination; settled signal remains between 0.5 V and supply minus 0.5 V |
| Step response | After a 100 mV input step with 10 us rise, nominal overshoot at most 15 percent of output step; settle within 1 percent of final excursion in 2 ms and remain there |

These are initial acceptance targets. Failed targets require a reviewed change
to the circuit or requirement; do not loosen them automatically to match a run.

## Starting circuit values

| Section | Values |
| --- | --- |
| Reference divider | 10 kohm / 10 kohm; 1 uF at divider node before U1A |
| Gain stage | 30 kohm feedback, 10 kohm to buffered reference |
| U2A filter | Two 7.32 kohm resistors; 12 nF feedback capacitor, 10 nF to reference |
| U2B filter | Two 3.01 kohm resistors; 68 nF feedback capacitor, 10 nF to reference |
| Output isolation | 47 ohm; populated R_TERM/C_TERM define the bench termination |
| Supply bypass | One pin-associated 100 nF capacitor per package; 4.7 uF board bulk |

Use 1 percent resistors and filter capacitors with specified 5 percent or better
tolerance and reviewed bias behavior. Pin exact parts and footprints before
assembly selection. Provide two-pin power, input and output connectors. Paired
signal/ground headers expose reference, gain output and first-filter output;
the output connector exposes the final filter output.

Use a two-layer board with a continuous ground reference, short feedback paths
and bypasses close to supply pins. The selected outline is 60 mm by 40 mm;
mounting dimensions and routed physical verification remain open.
Follow the [OPA2197 datasheet](https://www.ti.com/lit/ds/symlink/opa2197.pdf) and
the standard bounded routing workflow; do not add board-specific routing repairs.

## Simulation and delivery

This section is the original delivery specification. Only the board source is
deliverable now; there is no simulator in the `copper` binary.

Deliver board source, separate bias/AC, biased-step and cold-start plans, a
bounded corner matrix, and fresh offline reports. Corners include supply endpoints,
resistor/capacitor variation, unequal capacitor ratios, 2 kohm output loading,
10 nF external capacitance and higher sensor impedance. Adjust input bias to the
applied supply's midpoint in each supply case. Model excluded connector endpoints
with the existing explicit boundary-coverage rules.

Use the off-the-shelf TI OPAx197 model with a checked dual wiring wrapper; no
generic fallback or native solver is planned for this example. Real-device
startup, noise, saturation and recovery require applicable model qualification;
otherwise those requirements remain incomplete. Repeat key transient measurements with
half the maximum timestep; require changes below 1 percent with an absolute
floor near zero. Relative bandwidth and settling-window checks need explicit
measurement logic where the current adapter cannot express them.

Do not expand CopperLib. Reuse inspected versioned definitions and keep board
composition/evidence here. Route/DRC and prototype qualification are separate
deliverables. See [board research](../../../docs/simulation-analog-research.md)
and [integrated simulation options](../../../docs/integrated-simulation-research.md).
