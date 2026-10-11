# Simulation model and numerical qualification

Use [simulation](simulation.md) for supported commands, plan formats and exports.
These are review requirements for trustworthy numerical evidence, not proposed
source syntax or proof that every model/scenario is automatically qualified.
Pending adapter capabilities live in [contributor backlog](contributor-backlog.md).

## Model identity and applicability

Record exact model/bundle bytes and transitive includes, origin/licence, engine
build/dialect/settings, supported analyses and validity ranges. Map package pins
to model terminals explicitly; SPICE subcircuit order need not match a footprint.
Distinguish typical from corner coverage and record omissions. A filename, part
number, successful parser or available executable does not establish suitability.

Use managed asset inventories for reviewed model bundles. Constrain includes and
control/code content to the trusted bundle; never inherit arbitrary user startup
settings or execute source-selected commands. Use isolated output directories,
timeouts and retained logs. Model availability and redistribution rights are
separate facts; reject unsupported dialects rather than silently changing behavior.

## Circuit and scenario coverage

A selected subset must identify reference nodes, external boundaries, omitted
components and operating assumptions. Do not silently drop unsupported models.
Bound startup/inrush scenarios by actual source impedance, load, capacitor/ESR,
initial conditions, switching/control states and operating corners. A nominal
DC solve cannot qualify load steps or charging/startup safety.

AC checks need a defined bias point, source/reference and complex transfer
quantity. Distinguish absolute voltage/current from dimensionless gain in dB.
Bandwidth/ripple/crossing checks need declared frequency windows and crossing
rules; ambiguous or out-of-range results are incomplete. Gain/phase margins need
an actual loop-gain definition/sign convention, not merely output/input gain.

Keep phase wrapping explicit and unwrap only contiguous valid samples. Zero or
negligible denominators produce invalid results; zero magnitude has undefined
phase and negative-infinite dB. Plot display floors cannot change measurements.

## Numerical and output acceptance

Require every requested analysis/vector, finite applicable values and complete
time/frequency coverage. Distinguish preparation errors, solver failure, timeout,
completed solve and requirement pass/fail/incomplete. Exit status alone is
insufficient. Retain convergence messages, assumptions and model coverage.

For sharp events, bound internal timestep and qualify results by tightening step
and tolerances. Integrate on the actual nonuniform solver grid. Normalize delivered
source-current polarity and document any inserted sensing elements/model vectors.
Compare cross-platform waveforms with numerical tolerances, not identical bytes.

Retain the resolved deck/plan, model manifest, node/element map, engine identity,
logs, raw/normalized waveforms, measurements and source/plan/model hashes.
Requested graphs need labelled units, source/model context and visible failed
requirements. Preserve numerical data if rendering fails and report the missing
output separately. Keep offline report rendering independent from the solver;
use the implemented plan/export examples rather than invented schemas.

Numerical success does not approve PCB parasitics, control stability, thermal,
RF/USB performance or hardware behavior outside the model's verified scope.
