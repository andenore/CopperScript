# Integrated simulation options for CopperScript

> The `copper sim` command and its ngspice backend were removed with the Python
> implementation (last Python commit `b16122a`). This note is kept as research
> for a future simulator; "current implementation" below means that removed one.

Research date: 13 October 2026. This is a proposal, not an implemented backend.

Current project direction: use the off-the-shelf ngspice engine and TI model in
the [analog example](../examples/simulation/analog_frontend/README.md). Native
solver and generic-model implementation are deferred; the options below remain
research rather than the active implementation plan.

An integrated simulator is feasible. The practical choice is a layered design:
explicit built-in mathematical models for fast topology checks, a bounded native
linear solver if independence from ngspice matters, and the existing ngspice
backend for device models and nonlinear qualification. Bundling ngspice is the
shortest path to an integrated user experience; it does not remove the need for
device models or fix their compatibility failures.

The concrete first consumer is the
[analog front end specification](../examples/simulation/analog_frontend/SPEC.md).

## Engine integration and model ownership

An **engine** solves circuit equations. A **model** states how a component behaves.
SPICE model files are inputs to a solver, not external programs being called.
An internal engine still needs component equations and parameters. Moving the
engine into CopperScript cannot recover missing manufacturer behavior.

The removed implementation integrated ngspice as a DLL through a shared worker.
It invoked `ngSpice_Init` and `ngSpice_Command` in a disposable Python child,
retaining raw files and logs. Batch executables used the same evidence
pipeline. There was no native circuit solver; Python measurement and plotting
functions operated on results produced by ngspice.

## Options

| Approach | Dependency removed | Benefit | Main cost or limit |
| --- | --- | --- | --- |
| Bundle a pinned ngspice runtime | Separate user installation | Existing numerical engine and current reports | Platform packaging and model qualification remain |
| Ship authored generic model primitives | Vendor files for covered approximations | Portable filter/topology checks with explicit parameters | Limited physical fidelity; an engine is still needed |
| Add a native linear solver | ngspice at runtime for supported linear circuits | Direct electrical-IR execution and tighter product integration | New numerical implementation and validation burden |
| Integrate another simulator library | ngspice specifically | Reuse an existing numerical implementation | Runtime bridge, compatibility and independent qualification |
| Build a general nonlinear SPICE replacement | External engine dependency | Full control over implementation | A much larger solver and device-model project |

### Bundled ngspice

The [official shared-library interface](https://ngspice.sourceforge.io/shared.html)
supports netlist input, commands, callbacks and waveform access. CopperScript
could package a tested runtime, select it automatically, and eventually retrieve
vectors through callbacks instead of raw-file parsing. Preserve the disposable
worker for bounded execution and native-state isolation; a library API does not
require loading the engine into the compiler process.

Packaging should identify the engine build, platform, companion libraries and
startup profile. Optional compatibility/code-model profiles need their own
qualification, rather than inheriting a user's installed startup files. Ngspice
is primarily under the modified BSD license, with exceptions recorded by its
[developers](https://ngspice.sourceforge.io/devel.html); review the actual build's
notices and dependency inventory. Vendor-model rights are separate.

The earlier OPAx197 probes failed on local ngspice 46 even with compatibility and
an explicit analog code-model library. Bundling that same setup would reproduce
the failure. The pinned version 47 still needs investigation, as recorded in
[the analog research](simulation-analog-research.md#vendor-model-findings).

### Authored generic models

For the example board, an ideal amplifier can test connectivity and filter
mathematics. A finite linear model could add open-loop gain, a dominant pole,
input/output impedances and offset. For a single-pole approximation,
`A(s) = A0 / (1 + s/wp)`, with `wp = 2*pi*GBW/A0` when A0 is large. Its output
is centered on an explicit model reference. These are equations for a chosen
approximation, not an automatically accurate model of the OPA2197.

Keep generic equations in CopperScript; keep reusable part-specific parameter
sets and evidence in versioned dependencies. Board-specific assumptions and
scenarios belong to the consumer. A generated native SPICE subcircuit is a useful
first implementation: it removes vendor-file syntax from topology checks while
retaining the current solver. Hash the generated model and record all parameters.

Clipping, slew limits and output-current limits require additional nonlinear
equations. Supply-current behavior, noise, common-mode limits, startup and
overload recovery need separate validated behavior. A generic output clamp cannot
establish those properties. Never select an approximation silently when a
vendor model is unavailable, or infer its model from the reference designator.

### Native linear solver

A bounded native backend can consume a derived simulation IR directly, without
parsing arbitrary SPICE. Start with R/C/L, independent sources, controlled linear
sources and explicit finite linear amplifier models. Support operating point and
complex AC first, then linear transient with operating-point initialization,
PWL breakpoints and an explicit integration method.

Modified nodal analysis assembles local component contributions into equations
for node voltages and selected branch currents. Nonlinear devices additionally
require an iterative solve, usually Newton methods; see
[SpiceSharp's implementation guide](https://spicesharp.github.io/SpiceSharp/articles/tutorials/writing_behaviors/modified_nodal_analysis.html).
For linear circuits a useful formulation is `E*x' + G*x = b(t)`; AC solves
`(G + j*w*E)*x = b_ac`. The implementation must handle algebraic constraints,
singular systems, scaling and consistent initial states.

Transient simulation needs timestep/error control and source-event handling, not
just a loop applying a matrix inverse. The
[SpiceSharp transient guide](https://spicesharp.github.io/SpiceSharp/articles/structure/transient.html)
describes integration, truncation error, breakpoints and convergence concerns.
Use linear-system factorization rather than explicitly forming an inverse. An
optional numerical dependency can remain confined to the simulation extra;
the compiler need not acquire an unconditional numerical runtime.

This scope can simulate the proposed board with linear amplifier approximations.
It cannot qualify real startup, clipping, thermal behavior or nonlinear protection.
Reject unsupported elements and scenarios before solving. Do not expand the scope
to arbitrary PSpice expressions or general nonlinear vendor models in its first
release.

### Other library candidates

[SpiceSharp](https://github.com/SpiceSharp/SpiceSharp) is a managed .NET circuit
simulator under MIT, with programmatic circuit construction. Its separate
[behavioral extension](https://github.com/SpiceSharp/SpiceSharpBehavioral) provides
behavioral sources. It is a plausible embedded-engine candidate, but a Python
project would need a .NET bridge or packaged worker. It still needs model mapping,
compatibility review and the same numerical acceptance fixtures. No local
SpiceSharp integration or OPAx197 qualification was performed for this research.

Changing engines alone is unlikely to remove the vendor-model issue. For example,
[Xyce's FAQ](https://xyce.sandia.gov/documentation-tutorials/frequently-asked-questions/)
explicitly distinguishes its simulator from a parts library and notes incomplete
compatibility with other SPICE dialects. A second engine is useful only with a
specific capability or validation purpose.

## Proposed implementation sequence

1. Introduce an engine-neutral derived circuit representation and solver-result
   interface. Reuse selection, typed plans, measurements, validity checks and
   reports; keep the authoritative board immutable. Do not represent failed
   solves as valid waveforms.
2. Add an explicitly selected generic amplifier model and its dual-package
   mapping. Run the board's topology/AC/step checks through generated native
   ngspice models first. This isolates model design from solver development.
3. If runtime independence is a priority, implement bounded linear OP/AC and then
   transient. Feed their normalized results into the existing reporting pipeline.
   Record solver version, tolerances, model parameters, residual diagnostics and
   input hashes. Equivalent raw SPICE output is optional for this backend; retain
   the actual resolved numerical inputs.
4. Compare analytic RC/RLC and ideal-filter solutions, the existing passive
   examples, and generated finite-amplifier fixtures with ngspice. Include floating
   nodes, conflicting ideal sources, bad initial states and incomplete output as
   negative cases. Require numerical agreement within declared tolerances,
   timestep refinement, and unchanged pass/fail decisions for well-separated
   thresholds. Agreement checks the approximation, not the physical device.
5. Keep vendor-model qualification independent. Add noise, nonlinear models and
   temperature only after a concrete need and numerical acceptance coverage.

Expose both solver identity and model fidelity in every report. A mathematical
check can pass within its declared scope while real-device requirements remain
incomplete. The integrated path must preserve that distinction rather than making
an approximation look like a qualified hardware result.

The first development milestone is therefore **vendor-file-free topology and
filter simulation**, using authored generic models. A native linear backend is
a feasible next milestone if removing the external runtime is the intended goal;
a full SPICE replacement should be a separate project decision.
