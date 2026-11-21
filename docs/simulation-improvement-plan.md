# Simulation improvement plan

Status, 21 November 2026: proposal, nothing here is built. It follows the
[integrated simulation plan](integrated-simulation-plan.md), whose phases F1
to F11 are built on branch `integrated-simulation-plan` except F5 (layout
parasitics) and the CopperReview view of F10.

The simulator now does what the first plan asked for. What it lacks is
grounds for trusting it, real models on its first consumer, and checks that
compare instead of list. The phases below are ordered by that: trust first,
then the analyzer with real parts, then coverage.

The decisions of the first plan still hold. Decision 6 (existing results do
not move) applies to every phase here.

## Phases

Size as before: S is one focused session, M two to three, L more.

| Phase | What | Size | Needs |
| --- | --- | --- | --- |
| I0 | Land the branch | S | the owner's yes |
| I1 | Reference comparison | M | a decision on reference data |
| I2 | Vendor model failures | M | none |
| I3 | Analyzer on real models | M | hand-downloaded ADI files |
| I4 | Checks that compare | M | none |
| I5 | Layout parasitics | L | a routed board |
| I6 | Solver robustness | M | I1 |
| I7 | Macro model fidelity | M | I1 |
| I8 | Views | M | CopperReview checkout repaired |
| I9 | Device and model-kind coverage | L | a part that needs each item |

### I0. Land the branch

The branch is eleven phases ahead of main and the CopperLib branch
`sim-models` cannot merge before it. Every further phase makes the merge
larger.

Scope:

- Rebase or merge `integrated-simulation-plan` onto main; rerun the per-crate
  tests, the seven simulation examples and the six EMI examples after it.
- Run the release build and `cargo test --workspace --release` once (the
  branch has only been tested per crate in the debug profile).
- Merge CopperLib `sim-models`, then move the root `copper.mod` pin.
- Install the new `copper` so `make sim` works without `COPPER=`.

Acceptance: main builds and tests in release; every example's measure values
equal the branch's; `make EXAMPLE=rc-filter sim` runs with the installed
binary.

### I1. Reference comparison

The largest gap. Results are checked against closed forms and against
themselves; only one bias point (OPA810) has been compared with another
simulator.

Scope:

- A set of reference decks in `crates/copper-spice/tests/reference/`: one
  per device (diode, BJT, JFET, MOSFET levels 1 and 3), per analysis (op, ac,
  noise, tran, dc) and per temperature (-40, 27, 85 degC), all self-written,
  so they can be committed.
- Reference results for each deck as committed CSV, produced once with
  ngspice by a developer. `copper` never runs ngspice; the test only reads
  the CSV. The command that produced each file is recorded beside it.
- A test that runs each deck and compares within a stated tolerance per
  quantity. Differences found are fixed or listed with their cause.
- The same for vendor models where the licence forbids committing the file:
  an opt-in test that reads the model from the cache and a reference CSV
  from a gitignored directory.
- Items the first plan left unverified, settled by this: BJT substrate
  junction temperature scaling, the level-3 `KAPPA` difference, flicker
  noise in transient, the integration-floor rule on more than one circuit.
- Release-build timings of the vendor transients (the OPA2277 follower step
  takes 100 s in debug); the opt-in slew checks rerun.

Decision needed: whether ngspice-produced reference data may be committed.
It adds no runtime or build dependency, so it does not conflict with
decision 1, but it is the first time another simulator's output enters the
repository.

Acceptance: every reference deck within tolerance or with a written cause;
the compatibility table gains a column for the comparison.

### I2. Vendor model failures

Scope:

- TL431: find why it does not regulate (2.0 V against 2.495 V). Start from
  the reference comparison of its subcircuit, element by element.
- `.IC` inside a subcircuit (TLV757) and global parameters read by a
  subcircuit (TPS7A20).
- OPA2182 follower step: the collapsing time step.
- Vendor figures that differ from the data sheet (OPA548 supply current,
  OPA2189 slew): establish whether the model or the reader is the cause and
  say so in `--check-model`.

Acceptance: all three regulators and the TL431 pass `--check-model` with
their vendor model; the compatibility table has no load failure without a
named unsupported construct.

### I3. Analyzer on real models

Scope:

- ADA4817-1, ADA4807-1 and AD8130 from files the owner downloads, registered
  with `copper models add`; the reader fixed where they fail.
- `SoilSweep`, `LoopStability` and `Distortion` run with `--models vendor`;
  the difference from the macro models reported per measure.
- LTC6268 block once its data sheet is at hand.
- Absolute ratings for the 26 unrated pins (AD8130, amplifier outputs and
  disable pins) from the data sheets.

Acceptance: the analyzer's benches pass with vendor models, or each failing
limit is explained in SPEC.md; `copper check --sim-results` reports no
unrated simulated pin.

### I4. Checks that compare

Today the evidence checks list simulated values. Only pin voltages are
compared with anything.

Scope:

- A declared current budget per rail or per supply, and `power-check`
  failing when the simulated mean or peak exceeds it. Regulator output
  current limits from the parts used the same way.
- Rail totals: a module bench gives the module's share; sum over module
  instances, and say which loads on the rail were in no bench.
- Bias points of ac, noise, loop and pz benches recorded and checked like
  `op` results.
- Component stress from results: resistor power, capacitor voltage, diode
  and transistor current against part ratings.
- A board-level bench on the analyzer covering the power and digital side,
  so the 85 unsimulated rated pins drop to those that are digital only.

Acceptance: a board whose simulated rail current exceeds the declared budget
fails `power-check` with the bench named; the analyzer declares budgets for
its four analog rails.

### I5. Layout parasitics

F5 of the first plan, unchanged in scope. Proposed order inside it:

1. Capacitance extraction in `copper-extract` against closed forms.
2. `use layout;` proven on the Howland pump, which routes today.
3. The analyzer once it is placed and routed.

The `use layout` syntax is still a proposal.

### I6. Solver robustness

Scope:

- Shooting on strongly nonlinear circuits (the 1 MHz, 0.65 V `Distortion`
  step): an exact Jacobian product by sensitivity integration instead of
  difference quotients.
- Deterministic sparse ordering independent of thread count (open question
  of the first plan), with a test that runs the analyzer on 1 and N threads.
- Start-up transients at the default tolerances carry 0.2 % error; a
  tolerance preset per bench (`accuracy = high`) instead of hand-set
  `reltol` and `max_step`.

Acceptance: `Distortion` runs without a fallback note; analyzer results are
bit-identical across thread counts.

### I7. Macro model fidelity

Scope:

- `cmrr_corner` and `psrr_corner` feed-forward near the gain bandwidth (the
  33 MHz OPA2189 follower): a second pole so the term rolls off.
- Temperature dependence of bias current, gain bandwidth and slew rate;
  part-level default `tempco`.
- `--check-model` against the part: data-sheet figures entered as limits,
  not only the macro model against its own block.

Acceptance: every CopperLib part with a block passes `--check-model` with
both corners declared.

### I8. Views

Scope:

- The CopperReview checkout repaired (`git status` fails on a missing
  object), then the simulation view fed by `sim-results.json`.
- Report fixes found in review: the floor label over the stems on the
  `Distortion` spectrum; steps with different periods squeezed on one time
  axis; a density view for windowed spectra.
- The VS Code extension run in VS Code; `--watch` following packages outside
  the board directory.
- One real design task done with the reports, and what it showed written
  down.

Acceptance: each view looked at on the analyzer's results before it is
called done.

### I9. Device and model-kind coverage

Built only when a part in use needs it, in this order:

- Model kinds with no built-in block: comparators, adjustable regulators,
  fully differential amplifiers, current-sense amplifiers.
- `VDMOS`, `SDT`, `FREQ` tables, BJT quasi-saturation, lossy lines,
  saturating and hysteretic inductors.
- `TM1`, `TM2`, `BEX`, `BEXV`, `TCV` away from `TNOM`.

BSIM and encrypted files stay out of scope.

## Suggested first three

I0, I1, I2. They add no features. They decide whether the numbers of the
other phases can be believed.

## Open questions

- Reference data from ngspice in the repository (I1).
- Whether a rail budget is declared on the rail, on the supplying regulator,
  or both (I4).
- Whether I5 waits for the analyzer layout or proceeds on the Howland pump.
