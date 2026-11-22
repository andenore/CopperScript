# Simulation improvement plan

Status, 21 November 2026: I0 and I1 built (below), the rest a proposal. It follows the
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

Done on 21 November 2026, except the pin. Main was an ancestor of the
branch, so CopperScript main was fast-forwarded to `06465ef7`. The workspace
tests pass in release (2692 passed, 43 ignored); the seven simulation
examples give the branch's values with the release binary (analyzer 8 s).
CopperLib main is `960fe6d` (main merged into `sim-models`, CI building this
compiler; 211 tests and the model checks pass). The installed `copper` runs
`make EXAMPLE=rc-filter sim`.

Open: the root `copper.mod` pin is still CopperLib `50655eff`. Moving it to
`960fe6d` crosses 54 CopperLib commits and 345 files, and the same pin is in
`examples/nrf52_coin_cell` and `examples/nrf_antenna_macro`; that needs the
routing benchmark ladder, not a simulation check. The EMI examples were not
rerun with the release binary beyond the workspace tests.

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

Built on 21 November 2026; the owner accepted committed ngspice reference
data. Details in
`crates/copper-spice/tests/reference/README.md`.

- 27 self-written decks: diode, Gummel-Poon NPN and PNP (with a base
  resistance and a substrate deck), JFET, MOSFET levels 1 and 3, switches,
  controlled and behavioural sources, lossless lines, flicker noise, devices
  measured at other `TNOM`s, six small circuits (common-emitter stage,
  differential pair, CMOS inverter, bridge rectifier, RLC, a two-stage BJT
  op amp as a follower) and three linear circuits for the integration floor;
  op, dc (including `.dc TEMP`), ac, noise and tran, devices at -40, 27 and
  85 degC.
- `generate.py` drives the ngspice-46 library KiCad 10 ships through
  `ctypes` and writes `results/*.csv` (8.7 MB). `copper` and its tests
  never run ngspice.
- `reference_comparison.rs`: 1072 series, 168 932 values, each against a
  bound per kind of quantity derived in its module docs (DC 1e-5, DC
  currents 3e-5 for ngspice's CODATA 2014 `k/q`, AC and noise 2e-4,
  transients 2e-3 of the swing within one output step). Four more tests
  establish listed causes or check statistics.
- Fixed in the reader and solver (no example result moved: every measure,
  `sim-results.json` and CSV of the seven examples is identical): BJT `MJS`
  was ignored; ngspice's lateral PNP default (`SUBS`); BJT `G_MIN` in the
  base currents as SPICE3; level-3 `KAPPA` in the saturation field; level-3
  `PHI` from `NSUB` with ngspice's intrinsic density at `TNOM` (spice3
  dialect); ngspice's JFET `IS(T)` and its missing `M`; ngspice's sign of
  `pwr`; a MOSFET's `KF` refused in the spice3 dialect (ngspice's default
  `NLEV = 2` form is not modelled).
- Listed, 128 series under ten causes, eight established by a test, a
  variant deck or a reproduced number: `r_bb` linearisation (up to 6 % AC),
  substrate `G_MIN` (pA), breakdown onset (`IS`), SPICE's reverse cubic
  (0.4 % of `IS`), the gate charge's transcapacitances (body effect, `ETA`),
  ngspice's level-3 derivatives (`g_m` 7 %), ngspice's level 3 without
  `VMAX` (`badmos3`, 10 %); the linear-region partition is probable and
  the CMOS inverter's 7 % supply-current peak is traced to the intrinsic
  gate charge only.
- Settled from the first plan: the BJT substrate junction's temperature
  scaling agrees with ngspice (vertical and lateral, -40 to 85 degC,
  `TNOM` 50); the `KAPPA` difference is SPICE3f5's omission, which ngspice
  corrects; flicker noise agrees in `.noise` (diode, BJT, JFET) and in
  transient noise (within 6 % over octave bands from 20 Hz to 640 Hz, bound
  7 to 29 %); the integration-floor rule holds as an upper bound on three
  more circuits (0.01 to 1.02 of `0.02 sqrt(reltol)`), but it is not within
  a factor of 2 on all of them (an RLC is 20 to 100 times below).

Follow-up, 22 November 2026:

- MOSFET gate charge, established by variant decks (README): the CMOS
  inverter's supply-current peak (7 to 8 %) is copper-sim's charge-conserving
  partition against Meyer's capacitances and nothing else. ngspice without
  `TOX` and with Meyer's `C_gs`, `C_gd`, `C_gb` as `i = C(v) dv/dt` sources
  reproduces its native result (15.57 against 15.56 of the bound); with
  copper-sim's three charges (`dq/dt` through a 1 fF capacitor charged to `q`)
  it reproduces copper-sim's (0.000 of the bound on the output; the supply
  current within the bound except 4.7 at the input's corner); ngspice's
  peak is -0.5646 mA at maximum steps of 5 ps, 0.5 ps and 0.1 ps alike, so
  its capacitance averaging is not the cause. `mos3_badmos3`'s 0.83 fF
  (27 degC) is the same term, reproduced to seven digits from ngspice's own
  `V_dsat`, `V_dsat(V_on)` and `d V_dsat / d v_gs`. Meyer's form pumps 1.87e-14 C per period into a
  drain driven 90 degrees from its gate at 100 MHz (at 10 ps and 1 ps steps);
  copper-sim's 1e-19 C. copper-sim's form is kept; the two causes are now
  established and stay listed (`KNOWN` unchanged).
- PSpice dialect: five self-written decks (`pspice_behavioral`,
  `pspice_switch`, `pspice_ic`, `pspice_temperature`, `pspice_diode`) run in
  ngspice's PSpice mode: `VALUE` with `IF`, `LIMIT`, `STP`, `MIN`, `MAX`,
  `.PARAM` expressions, `VSWITCH`, `.IC` in a subcircuit, `TC1`/`TC2`,
  `T_MEASURED`, `T_ABS` and the diode's `ISR`/`NR` in forward bias, 47 series
  within their bounds (worst 0.28). Where that mode is not PSpice, shown each
  by a self-written deck: `TABLE` (rounded corners), `ISWITCH` (log-linear
  in the current), a `VSWITCH` with `ROFF` below `RON` (open), a `VSWITCH` in
  `.ac` (the complex conjugate of the response), element `IC=` (ignored
  without `UIC`), the diode's `IKF` (ngspice's own knee, reproduced to 0.4 %),
  a vertical PNP (lateral); these are checked against PSpice's closed forms,
  one new (`smooth_switches_follow_pspice_across_their_transition`). Not
  established: ngspice's `ISR` in reverse bias (0.585 of PSpice's formula)
  and its `ISR` conductance (0.26 % off its own DC derivative). The LTspice
  dialect has no reference here; README "PSpice dialect" lists what stays
  uncompared. `generate.py` loads the PSpice-mode code models for a deck
  with `*@ ngbehavior`; the 27 earlier result files regenerate byte for byte.
  Totals: 32 decks, 1119 series, 173 720 values, 128 listed.
- Release timings (`CARGO_PROFILE_RELEASE_DEBUG=0`, this machine): the
  vendor follower steps of `--check-model` take 0.41 s (OPA2277UA), 0.14 s
  (OPA2182IDR), 0.05 s (OPA2189IDR) and 0.50 s (OPA548T); the whole check
  7.2 s, 1.8 s, 1.6 s and 1.7 s. The OPA2277 step that took 100.5 s in the
  debug build before I2 takes 1.59 s in it now (at HEAD before this follow-up
  too, so I2's accepted stalled iterations removed it); every figure equals
  the debug run's. The opt-in slew checks (`vendor_models.rs`,
  `COPPER_SPICE_VENDOR_TRAN=1`, release) ran for the two models on this
  machine: OPA2277 0.798 V/us (1069 steps) and OPA810 242 V/us (1010
  steps), each suite under 2 s; the other ten models of the test are not in
  a local cache and were not run.

Open: the listed ngspice behaviours not adopted (reverse cubic, breakdown
branch, substrate `G_MIN`, `r_bb` without its derivative, the non-`badmos3`
level 3, `NLEV` noise); JFET and MOSFET noise outside saturation; the
LTspice dialect (no simulator to compare with) and the PSpice constructs
ngspice runs differently (closed forms only, no PSpice); ngspice's diode `ISR`
in reverse bias; the compatibility table's TI parts have no ngspice column.

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

Built on 22 November 2026. Every cause below was shown by a node-by-node
comparison with ngspice-46 (the gitignored vendor decks of the reference
comparison, README "Vendor models": 5 decks, 19 series, all within their
bounds), a variant deck or a self-written deck.

- TL431: the reader runs the model as ngspice does. Fed from 12 V through
  1 kOhm (TI's own designs in the archive are resistor-fed) both give
  2.4980 V at the operating point, in a dc sweep and after a supply ramp.
  The two failures were the bench. At 10 mA from a current source the
  100.45 V operating point was false convergence, in ngspice too (100.454 V):
  the clamp diode `D_D11` has `N = 0.001`, so `n V_T` = 26 uV is below the
  Newton voltage tolerance at 100 V (100 uV), and its junction carried
  -3.7e4 A against 309 A in its series resistance. Junctions now also
  converge on their current (SPICE's `DIOconvTest`, relative 1e-3,
  `newton.rs`); the operating point is 2.4982 V. The transient "2.0 V" was
  one sample of a relaxation oscillation between 2.00 V and 2.57 V: fed by an
  ideal current source the model's loop has no load at the cathode, and
  ngspice oscillates the same way with trapezoidal and with Gear
  integration at 20 ns steps. What PSpice does with that bench is not
  known (no PSpice). `vendor_models.rs` now runs the resistor-fed bench too
  and asserts the data-sheet range (2.440 V to 2.550 V).
- Reader: `.IC V(node)` and `.IC I(inductor)` inside a subcircuit, per
  instance; `C`/`L` `IC=` in the pspice dialect (ngspice and LTspice use it
  only with `UIC`, so the other dialects refuse it). Both start every
  transient run held there, as PSpice's bias point does
  (`Circuit::set_initial_voltage` and siblings; a run's own conditions win;
  the operating point of the other analyses does not hold them). A value of
  the `sim spice` block's `params` that is not a `PARAMS:` of the
  subcircuit is a global parameter (PSpice's `.PARAM` of the calling
  netlist), set before the file's top-level `.PARAM`s, and must be a name
  the file reads. A subcircuit defined twice with the same content (the
  TLV75718P file repeats two in lower case; ngspice warns and keeps the
  first) is noted and the first used. Self-written tests:
  `tests/vendor_constructs.rs`.
- TLV75718P and TPS7A20 load; with the CopperLib blocks plus a `sim spice`
  block (in a scratch project; CopperLib unchanged) `--check-model` passes
  for TLV75718PDRVR, TPS7A2018PDBVR and TPS7A2025PDBVR with their vendor
  models. TLV75718P agrees with ngspice (1.8085 V at 1 mA). The TPS7A20 model
  was not compared: ngspice's PSpice mode opens its load switch, a
  `VSWITCH` with `ROFF` = 1 uohm below `RON` = 1 Mohm, at `VOFF` (shown by a
  self-written deck), where PSpice's switch is at `ROFF`.
- OPA2182 follower step: Newton never converged after about 83 ns because
  one unknown, the branch current of the model's mid-supply source `E_E1`,
  alternated between two values 6 pA apart (5e-6 of itself, against a
  1e-6 + 1 pA test) while every node voltage had converged; the switch
  `S_S2` in its transition carries 126 mA through that node and its control
  voltage changed by 4e-16 V, below one rounding step of the node voltages
  it is the difference of. A Newton iteration that reaches its limit is now
  accepted when for its last four iterations only branch currents remain,
  each within 1e-6 of the currents it is the sum of at its node (the
  precision Kirchhoff's law fixes it to). Accepting this earlier than the
  limit, or loosening the branch test in general, moved the analyzer's
  transients at the 0.5 % integration-error level, so it applies only
  where the iteration would otherwise fail. The step runs (0.85 s) and its
  slew rate is 5.33 V/us; ngspice gives 5.32 V/us.
- OPA548 supply current (4.295 mA) and slew (5.8 V/us), OPA2189 slew
  (11.6 V/us) and OPA2182 slew: ngspice gives the same from the same files
  within 2 % (4.295 mA, 5.9, 11.6 and 5.3 V/us; on a 10 V step 14.0 V/us for
  the OPA2189 and 5.6 V/us for the OPA2182). The figures are the models'.
  A `sim spice` block can now say so: `note = "..."`, which `--check-model`
  prints as `note: vendor model: ...`.
- Reference harness: `*@ dialect pspice` and `*@ ngbehavior psa`; the
  committed results regenerate unchanged.
- Decision 6: no measure of six examples moved. The impedance analyzer's
  `Distortion` step 7 (1 MHz, 0.65 V) moved in 11 measures, all without
  limits, because Newton solves that used to fail at the iteration limit
  (and were retried with a shorter step) are now accepted: `current_h2`
  6.40e-7 to 2.53e-7 (below the integration floor), `current_h3` -1.5 %,
  `current_thd` -1.1 %, `gain1_thd` +0.5 %, `gain10_thd` +0.2 %,
  `current_sfdr` +0.13 dB, `current_channel_thd` -0.09 %, and
  `current_fundamental`, `pump_output_peak`, `pump_headroom` and
  `slew_needed` within 4e-6. The junction-current test alone moves nothing.

Follow-up, 22 November 2026:

- OPA2189 follower step at `reltol` 1e-5: the jumps of `N3828449` are
  rounding noise, not the circuit. The node sits behind a 31.83 fF lead
  capacitor on a node at -140 V of a gain chain that starts at a 53 uF
  capacitor (`C_C10`, 0.53 s, in the output-impedance path); at a step `h`
  its `C/h` (5e12 S at 1e-17 s) swamps the 1 ohm and 27 ohm beside it, so the
  common voltage of the nodes it ties moves by `eps C/h`-sized amounts from
  one solve to the next. The noise grows as `1/h` (an estimate from the
  step's own matrix: 1.4e-4 V at 3e-14 s, 0.5 V at 1e-17 s; the observed jumps
  are about a tenth of that), so each rejection raised the next estimate and
  the step ran to the floor. Shown by variant decks: `C_C10` replaced by a
  voltage source at its operating-point voltage (its voltage moves by 1e-5 V
  in the run), or made 1000 times smaller, runs (smallest step 3.4e-14 s, no
  discontinuity); 10 times larger fails earlier (26.38 ns). ngspice takes
  steps of 1.7e-15 s and more there: its truncation error is that of the
  capacitor charges, which the common-mode noise does not move. Fix
  (`transient.rs`, `newton.rs`, general): a step whose estimate exceeds the
  tolerance is accepted when it is within the tolerance plus the rounding
  noise of each unknown (the response of the step's last matrix to `eps sum_j
  |a_rj x_j|` in every row), judged only for a step that would otherwise be
  rejected (`TranStats::noise_limited`). The step runs at 1e-5 (6 noise-limited
  steps, smallest 4.0e-14 s) and at 1e-6, for every `max_step` tried (seven,
  among them the last bits that failed before); the vendor step decks now run
  at the harness's 1e-5: OPA2189 0.280 of the bound (0.280 at 1e-4), OPA2182
  0.426 (0.424). No example measure moved and every reference series is
  unchanged.
- Analyzer run time: not real. HEAD and 1c8ca866, alternated three times
  each in the debug build: 52.6, 71.7 and 51.4 s against 64.3, 58.9 and
  54.8 s (bench sums of `--timing`); every bench's minimum is HEAD's or within
  noise, and the solver work is identical except `Distortion`, where HEAD does
  0.7 % more Newton iterations (132 660 against 131 758). The 47 s and 57 s
  were one run each.
- TPS7A20 compared with ngspice in a local variant (gitignored
  `vendor/models/tps7a20_variant.lib`): the load switch `S_S2` written with
  `RON` below `ROFF` (control reversed, `VON > VOFF`: the same law in
  PSpice's formula; copper-sim gives the same operating point to ten digits
  for both forms) and the dropout `TABLE` as explicit linear segments
  (ngspice's PSpice mode rounds a table's corners: with the table, 1.3 times
  the bound at 1.75 V in dropout; with the segments 0.053). Operating point
  at 1 mA, the input from 1.5 V to 5 V and no load agree (worst 0.37 of the
  bound). The line rejection is not compared in `.ac`: ngspice's PSpice-mode
  `VSWITCH` returns the complex conjugate of the response there (a
  self-written RC deck), so only magnitudes agree: TPS7A20 74.99 dB at 1 Hz in
  both, the model's own `psrr = 178u`; TLV75718P 64.99 dB. Quiescent current
  at no load: TLV75718P 5.2906 uA in both (its file: "quiescent current ...
  not modeled"), TPS7A2018 5.0615 uA in both. The data-sheet differences are
  the models'.
- Release timings: see I1's follow-up.

Open: the `sim spice` blocks of TLV75718PDRVR, TPS7A2018PDBVR and
TPS7A2025PDBVR are on CopperLib's branch `sim-vendor-regulators` only, and
the amplifier blocks carry no `note`. The TL431 under a current source and PSpice's
behaviour there are not settled. The TPS7A20's start-up transient and its
line rejection's phase are not compared with ngspice.

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

Built on 22 November 2026. Nothing in the language declared a rail current
or a component's stress, so three declarations were added (language
reference, "Supplies" and "Stress ratings"); a regulator's existing `sim
regulator` / `sim reference` `output_current_limit` is used as it is.

- Budgets: `supply VP { ...; current_max = 70mA; }` bounds the rail's mean
  (the operating point, a transient run's time mean, each point of a dc
  sweep) and, without `current_peak_max`, its peak. A rail with a `source`
  pin also takes the pin's `current_max` (mean) and the source part's
  `output_current_limit` (mean and peak) as budgets. `copper power-check
  --sim-results` fails with `SIM_RAIL_MEAN_HIGH` / `SIM_RAIL_PEAK_HIGH`
  (bench, step, value, budget and its source); `SIM_RAIL_INCOMPLETE` and
  `SIM_RAIL_NOT_CHECKED` say what a total or a budget leaves out. An `OK:`
  line still prints with warnings only.
- Totals: a board bench's figure is a total of what it simulated; module
  benches' shares are summed over the instances, each instance at its worst
  bench and step (peaks summed as if they coincided). Every total says how
  many of the rail's loads (the components on its net, its source excepted)
  it covers, and the rail lists the loads in no bench, capacitors apart. A
  supply driven by a component the bench simulates is now recorded (the
  current out of its `source` pin, `supplies[].driven_by`).
- Bias points: an `ac`, `noise`, `loop` or `pz` bench records `pins`,
  `supplies` and `stress` at the operating point of its circuit, solved once
  more per step (nodes with no DC path held to ground through 1 ohm, as
  copper-sim's small-signal analyses hold them, and nothing on them
  recorded); a step whose bias point repeats an earlier one's refers to it
  (`steps[].bias_step`), which keeps `ContactImbalance`'s 192 steps to one
  record. `copper check` judges them like `op` results.
- Stress: part `rated_power` and `rated_voltage` (two-terminal parts) and pin
  `current_max`, recorded per step as `steps[].stress` for the components a
  bench simulates with their own model; `copper check --sim-results` fails
  with `SIM_STRESS_POWER_HIGH` / `_VOLTAGE_HIGH` / `_CURRENT_HIGH`; power and
  current against the steady value, voltage against the peak.
  `SIM_STRESS_NOT_CHECKED` counts the simulated resistors, capacitors, diodes
  and transistors without a rating and names rated parts no bench simulated.
  Schema v1 gained `stress`, `driven_by` and `bias_step` (additive; shape
  test and `docs/sim-results-schema.md` updated). A board bench may name a
  module instance's net by its path (`net AFE/E_HC { ... }`).
- Tests: `crates/copper/tests/sim_evidence_cli.rs` gains a board whose rail
  (1.6 mA) exceeds its budgets (1.5 mA, and the regulator's 1.2 mA limit) and
  fails `power-check` naming `Overdrive`, and a 1 mW resistor carrying
  3.6 mW that fails `check` likewise.
- Analyzer: `current_max = 70mA` on VP and VN (the TPS65130 and the post
  filters are sized for 70 mA per rail, board.copper and README "Current"),
  and the parts' ratings from their cited data sheets: TPS7A4901 OUT 150 mA,
  TPS7A3001 OUT 200 mA (both IOUT of Section 6.3), REF2033 VBIAS and VREF
  20 mA (Section 6.5), TPS22918 VOUT 2 A (Section 6.1). V3V3_ANA has no
  budget: FB_ANA, its only feed, declares no rated current, and the switch's
  2 A is DDS_SW's. New board-level bench `PowerSide` (op, the `MEASURE`
  state, DDS lines low and high; unmodelled parts ignored and their rails
  ideal sources at their set points). Simulated against the budgets:

  | Rail | Budgets | Board total (`PowerSide`, loads covered) | Front end's worst share, mean / peak |
  | --- | --- | --- | --- |
  | VP | 70 mA; 150 mA (U_LDOP.OUT) | 52.17 mA, 18 of 18 | 52.28 mA / 53.2 mA (`SupplyCurrent.driving`) |
  | VN | 70 mA; 200 mA (U_LDON.OUT) | 51.82 mA, 16 of 16 | 52.28 mA / 53.21 mA (`SupplyCurrent.driving`) |
  | V_MID | 20 mA (U_REF.VBIAS) | 4.652 uA, 6 of 6 | 4.652 uA / 49.2 uA (`Distortion`, 100 kHz) |
  | V3V3_ANA | none | 3.66 mA, 9 of 10 (FB_ANA in no bench) | 6.086 mA / 23.96 mA (`Distortion`, 1 MHz) |

  Both checks pass. Rated pins without a simulated voltage: 85 before, 29
  now, all with no analog meaning at DC: U_MCU 25 (11 unconnected GPIOs, the
  four oscillator pins and NRST on capacitor-only nets, UART and SWD on the
  debug header, USB, and the three charger lines), U_PWR 3 (CP and CN, the
  charge pump's capacitor-only nets, and the unconnected BSW), U_DDS 1
  (CAP_2V5, capacitor only). Unrated simulated pins rose from 26 to 51 (the
  newly simulated TPS63020, BQ24072, LDO, TPS65130 VREF and REF2033 output
  pins declare no absolute rating). Stress: 2 ratings checked, the REF2033's
  VBIAS (4.65 uA, 0.0233 % of 20 mA) and VREF (no DC load); 132 simulated
  components not checked, 84 capacitors and 48 resistors (no part declares a
  rating); the rated LDOs and switches are in no stress record (ignored
  wherever they appear). `power-check` warns `SIM_RAIL_INCOMPLETE` on DDS_SW
  (its two ferrites have no model) and lists the rails without a budget.
- Decision 6: every measure of the seven examples is identical; the
  analyzer gains `PowerSide`'s measures. Its `sim-results.json` grew from
  16.3 MB to 18.8 MB (the other six by 0 to 25 %, 120 B to 71 kB); without
  `bias_step` the bias points alone made it 42 MB. Debug runs took 78 s
  before and 57 to 71 s after (single runs on a shared machine: no
  measurable cost).

Open: no part on the analyzer declares `rated_power` or `rated_voltage`
(the feed resistor's 0.1 W is unverified), so component stress checks only the
REF2033's outputs there; the ferrites' rated current (V3V3_ANA's budget) is
not read; `PowerSide` is DC only, with the regulators, the MCU and the DDS
ignored or stand-ins, so the rails' upstream currents (VSYS, BOOST_IN,
DDS_SW) are not their loads' and the DDS supply current is not simulated; a
load counts as covered when its own model is simulated, not what it feeds
(L1 covers BOOST_IN although the boost behind it is ignored); a `SLEEP`
bench (firmware rules 1 and 2) is not built; `copper-sim` exposes no held operating
point, so the bias point repeats the hold of its small-signal analyses rather
than reading theirs. Settled in I6: a `sim regulator` whose
`output_current_limit` is below its load solves (the singular point was a
load that draws more than the limit at any voltage; the rail now collapses
onto `gnd`), and `sim_evidence_cli.rs` checks the regulator's own limit.

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

Built on 22 November 2026.

- Regulator in current limit. The cause, shown on the I4 board and on
  copper-sim circuits: the limit keeps its equations (a limited regulator into
  resistors solved before, `current_limit_either_way`); the singular
  operating point is a load whose current does not depend on its rail. An
  amplifier's supply pin draws `I_q` plus its output current whatever
  `V(VP)` at the operating point (its swing and limits act in transient runs
  only), so the limit, a current source behind `R_out`, and that load are two
  current sources in series: `V(U2#x)` and `V(VP)` enter only through their
  difference (`singular at U2#x`), and a load above the limit has no solution
  at all. Fix (`regulator.rs`): the clamp's bound on the set point's side is
  taken from `max(s V(out), 0)`, so an output pulled past `gnd` is held there
  through `R_out`, at `-(I_load - I_lim) R_out`, the rail collapsed; on its
  own side of `gnd` the expression and its value are unchanged. The same model
  is the `sim reference`. An op-amp's `output_current_limit` acts in transient
  runs only, so it cannot make an operating point singular, and in a run its
  soft limit keeps a slope. Closed-form tests (`tests/regulators.rs`,
  `tests/transient_closed_forms.rs`): ten resistive loads across the limit,
  `V_out = min(5 RL / (RL + R_out), I_lim RL)`, both polarities and the
  reference; a 0.3 A sink against 0.2 A, -10 mV in either polarity; the fold
  from the limit (4 V into 20 ohm) into dropout below 4.32 V in, over a 0 to
  8 V sweep; a 0.3 A load step into 100 uF, falling at 1000 V/s, reaching
  `gnd` within 1 us of `1.0005 ms + 10 us ln 3 + 4.98 ms`, held at -10 mV and
  recovering at 2000 V/s; an op-amp follower charging 1 uF at its 10 mA limit
  (`I_lim RL (1 - e^(-t / RL C))`, 4.9 V within 1e-3 of 0.5024 ms) and
  settling out of it. `sim_evidence_cli.rs` adds the I4 board with its own
  regulator driving POS and a 25 kohm load: at 1 V in the amplifier draws
  1.1 mA, so VP = (1.2 mA - 1.1 mA) 25 kohm = 2.50005 V (to 1e-6); at 6 V in it
  draws 1.6 mA and VP collapses to -40 uV, which `power-check` reports above
  the limit.
- Shooting. At HEAD the 1 MHz, 0.65 V `Distortion` step no longer fell back
  (3 Newton iterations, 11 periods): since I2 accepts stalled Newton
  iterations its replays converge; the note last appears in the I1 results.
  The difference quotients are replaced all the same by the exact product
  (`transient.rs` `linearise`, `tangent`; `newton.rs` `jacobian_at`,
  `rhs_tangent`): each step of the period just integrated (held: size,
  method, switch states) has its Jacobian at its solution factored once per
  Newton step (junctions at their exact voltages, no limiting), and a product
  is one pass, each step's factors solved against the change of its
  right-hand side that the previous state's change makes. That change is
  written out per element: capacitor and charge companions, trapezoidal
  history currents, inductor flux, and through a current control of a
  capacitor or charge the law's, CCCS's and CCVS's slopes; charges and history
  currents are carried on as the integration carries them. Matrix-free for
  GMRES; no Newton iteration runs inside a product. Checked against central
  differences of the replayed period on a circuit with every such element:
  1.1e-8 of the largest entry over 195 steps
  (`the_tangent_of_a_period_matches_its_difference_quotients`; it fails, 77
  against -2.0, with the law's current-control term left out). Where it stops:
  a switch's instant does not move with the state (no event-time
  sensitivity), so near a switching event the Jacobian is that of the held
  events, as the replays were. `Distortion`: no fallback on any of its 9
  steps, 26 Newton iterations and 120 periods in all (27 and 127 before),
  the bench in 4.6, 6.6 and 6.5 s against 11.5, 16.2 and 14.5 s before
  (debug, `--timing`, the two builds alternated). Precision rectifier steady state against 20 periods: 3
  iterations, 6 periods, largest V(OUT) difference over the period 4.4e-4 V
  (the test's bound unchanged). The 1 MHz, 0.65 V step against a 400-period
  settling run (`max_step` 5 ns, 101 882 steps): `current_fundamental` 2.3e-7
  off it (4.2e-7 before), `pump_output_peak` 1.0e-5 (9.7e-6), the harmonic
  ratios (2e-5 and below) within 4e-7 of it, against the step's 2e-4 floor,
  before and after.
- Thread count. Established identical, not changed: the seven examples'
  outputs (`sim-results.json`, CSVs, HTML) are byte-identical at `--jobs`
  1, 4 or 5 and 12 and between repeated runs (the analyzer, all 31 benches,
  and the preamp's Monte Carlo and worst-case benches). Each step owns its
  circuit and solver, the Markowitz order breaks ties by size, then column
  and row, `parallel::map` returns in item order, and the simulation crates
  iterate no hash maps (`BTreeMap` only). New test
  `sim_results_do_not_depend_on_the_thread_count` (`sim_cli.rs`, 31 s in a
  debug build): every file and the table of `Distortion` at `--jobs 1` and
  4, and of the preamp's `GainMonteCarlo` and `OffsetWorstCase` at 1, 4 and
  4 again, bit for bit. Other machines were not compared.
- Accuracy preset: `analysis tran { ...; accuracy = high; }` (language
  reference, "Transient analysis"; `sim_reference`, so completion and hover
  have it): `reltol` 1e-6 and `max_step` `(stop - start) / 500` where not
  given (`copper_sim::Accuracy`), an explicit `reltol` or `max_step` wins, the
  default unchanged. Its level from the floor rule: `0.02 sqrt(reltol)` = 2e-5
  of a smooth waveform's range (2e-4 at the default), and 500 steps keep a
  record-long period's trapezoidal error at 1.3e-5. Against copper-sim at
  `reltol` 1e-9 on the 21 reference decks' transients (ignored test
  `accuracy_high_meets_its_error_level_on_the_reference_transients`, README
  "Accuracy preset"): 43 of 66 probes within 2e-5, the linear `floor_`
  decks 0.6 to 2.0 times it; at the fast edges of transistors, diodes and
  the slewing follower 9e-5 to 4.5e-3, 1.2 to 100 times below the default.
  `Startup` with `accuracy = high;` alone against `reltol` 1e-8 and
  `max_step = 0.1ms` (four steps): voltage measures within 75 uV (the bench
  as it is, its own `max_step = 1ms` at the default: 2.1 mV), times within
  0.19 % (0.82 %), the inrush peak of the positive rail's current 0.44 %
  (0.31 %; branch currents are not in the error control), in 12 037 steps
  against the bench's 11 986. Tests:
  `accuracy_high_tightens_the_step_control_where_it_is_not_given`
  (`transient_runs.rs`: the RC step's 63 % crossing 4.3e-6 of the range at
  high, 2.5e-4 at the default; explicit values win) and
  `analysis_tran_accuracy_preset` (copper-lang: lowering, JSON, the
  misspelt value with its hint).
- Decision 6: every measure of six examples is identical, and of the
  analyzer every measure but `Distortion`'s: its 99 values move, on all 9
  steps, because the Newton iterates of the shooting method differ; each
  steady state is the fixed point within one tolerance unit either way. The
  largest changes: harmonic ratios by at most 2.2e-5 absolute (`gain10_thd`,
  100 kHz, 0.65 V: 0.2325 by 9.3e-5 of itself), each below its step's
  integration floor (2e-4, or 3e-7 where the steps are uniform, there by
  7e-9 at most); `current_fundamental` by at most 2.0e-6 of itself;
  `pump_output_peak` and `slew_needed` 4.3e-5; `pump_headroom`, the
  difference of 4.42 V and the peak, by 8.1e-5 V (-0.850 mV to -0.769 mV at
  100 kHz, 0.65 V). Neither set is the better number by anything the run
  shows: the changes are below the integration floor, the last period's
  residual is smaller in 7 of 9 steps, and at the step that used to fall
  back a long settling run agrees with both within its floor.

Open: event-time sensitivity of switches in the shooting Jacobian; a
regulator sinking its limit against a source that pushes more in at any
voltage still has no operating point (only the sourcing side is bounded); an
op-amp in its transient current limit into a load beyond it, with nothing
else on the node, runs away (no output clamp to the rails); `high` at fast
nonlinear edges is above its level, and on `opamp_follower` and the BJT
decks' base node it improves by only 1.2 to 1.3 times (why is not
established); the peaks of branch-current spikes are not error-controlled;
`Startup` keeps its hand-set `max_step` (decision 6).

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

Built on 22 November 2026.

- Corners, established first. A corner's term was `(1 + j f / f_c) / R` with
  no pole (`R` the rejection in V/V): the error current into the dominant
  pole node rose with `f` while that node's impedance fell, so to the output
  the common-mode gain `A(f) / CMRR(f)` was flat at `GBW / (f_c CMRR)` for
  every `f` above `GBW`. In a follower `H = A (1 + e/2) / (1 + A (1 - e/2))`
  with `e = 1 / CMRR(f)` tends to `k / (1 - k)`, `k = GBW / (2 f_c CMRR)`,
  instead of falling. The OPA2189 block with the corner its own comment
  derives (168 dB, 0.08 Hz, `k = 0.35`) reproduced the note: follower
  33 MHz (closed form 32.9 MHz), phase margin 110 degrees, and a 64 V/us
  follower step against 20 V/us, because the common-mode current bypassed
  the slew-limited input stage. TI's models of the CopperLib amplifiers,
  run open loop with both inputs and then each rail driven (measured, a
  scratch harness): OPA2189 CMRR 130 dB falling from about 5 Hz to a 35 dB
  floor above about 355 kHz; OPA2182 168 dB, a 30 dB floor from about 3 MHz;
  OPA2277 140 dB, its output-referred common-mode gain flat at -37.5 dB from
  30 Hz to 1 MHz (its GBW) and falling above; OPA547 and OPA548 have no
  corner at all (143 and 161 dB to 10 kHz). Physically (inferred, a
  differential pair with tail impedance `R_t || C_t`) the conversion is
  `D (1 + s R_t C_t) / (1 + 2 g_m R_t + s R_t C_t)`: the zero is the corner,
  a pole at about `2 g_m / C_t` ends the rise, and the floor is the pair's
  mismatch `D`, at most the common-mode signal; the feedthrough to the
  output falls beyond the bandwidth. Neither `D` nor `C_t` is a data-sheet
  figure.
- The second pole (`copper_sim::models`, `rejection_pole`): `f_p = min(GBW,
  f_c R)`, `f_c R` with a corner at or above `GBW`, none for `R <= 1`, for
  the CMRR and the PSRR alike; the network is `R2 = f_p / f_c - 1` ohm in
  parallel with `R2 / (2 pi f_p)` henry behind the 1 ohm, exactly
  `(1 + j f / f_c) / (1 + j f / f_p)`. `f_c R` is where the declared curve
  reaches 0 dB (the error never exceeds the signal); `GBW` makes the
  output-referred term flat to the bandwidth and falling 20 dB per decade
  above it, the shape of TI's OPA2277 model and an upper bound on the other
  two (whose floors start lower: a data-sheet figure the block cannot
  carry). Below `f_p / 10` the declared curve moves by at most 0.5 %. In
  transient runs with a `slew_rate` the common-mode error of a corner is
  one of the limited input stage's pairs (`newton.rs`). The OPA2189 block
  with its corner now gives follower 25.1 MHz (closed form 25.1 MHz),
  phase margin 98 degrees and 19.7 V/us. Tests (`tests/rejection_poles.rs`,
  closed forms): the pole rule's cases; open-loop common-mode and supply
  responses equal to `A(f) / R(f)` from 1 Hz to 100 GHz (1e-9; 1e-7 where
  the network spans 1 ohm to 2e8 ohm); the output-referred term flat
  between corner and `GBW` and falling tenfold per decade above; the
  OPA2189-like step slewing within 3 % of 20 V/us. The regulator's
  `psrr_corner` is unchanged (no gain-bandwidth to place a pole at).
- Temperature (as F8 did, from 27 degC): `input_bias_current_tempco`
  (ppm/degC, compounding, `I_b exp(tc dT)`, so a FET or CMOS input that
  doubles every 10 degC is `69315ppm/degC`; the offset current follows),
  `gbw_tempco` and `slew_rate_tempco` (linear, `SIM116` where `1 + tc dT` is
  not above zero at the bench's `TEMP`). A two-terminal part's `tempco` and
  `tempco2` are the default of its components (each coefficient a component
  leaves out; `SIM111` on a part with other than two pins or with `sim`
  blocks; part JSON `metadata`). Tests (`copper-simulate/tests/temperature_coefficients.rs`):
  dividers of a part-tempco resistor and one overriding it at -40, 27 and
  125 degC to 1e-12; a bias current doubling in 10 degC through 1 Mohm to
  1e-6; the open-loop gain at 100 kHz with `GBW(T)` to 1e-9; a follower
  step's slew within 2 % at -40 and 125 degC; the diagnostics.
- Data-sheet figures. The block's parameters already are the data sheet's
  typicals and the macro model is judged against them, so a second typical
  would duplicate them; what was missing is the data sheet's guarantees and
  any judgement of the vendor model. `datasheet FIGURE { min; typ; max; }`
  in a built-in block (language reference, "Data-sheet figures"; `SIM117`;
  completion and hover from `sim_reference`): both models are judged
  against `min` and `max`, `typ` is shown and refused where the block
  declares the figure. A macro figure outside a limit fails the unit; a
  vendor figure outside one is marked and noted, the verdict staying the
  macro model's (the vendor file cannot be corrected in the part, and runs
  use the macro model by default). Figures: what `--check-model` measures,
  now including the common-mode and supply rejection at DC and at each
  corner (open loop, `A_OL / A_cm` and the worse rail's `A_OL / A_s`), so the
  corners themselves are checked.
- CopperLib (uncommitted, branch `sim-vendor-regulators`): OPA2189 data-sheet
  limits from SBOS830I's text (its `cmrr_corner` was tried and taken out
  again in review, see Open); OPA548 limits from SBOS070D; AP2112 limits
  from DS39724. Every part with a block passes `--check-model` (table in
  the I7 report): amplifiers at their data-sheet supplies, the rail models
  at their defaults.
- Decision 6: six examples identical (every measure). The analyzer moves
  where its parts and benches declare a corner (AD8130 630 kHz, ADA4807
  25 kHz, ADA4817 300 kHz, `SenseBufferTrade` 2 kHz, `SenseCommonMode`
  300 kHz; their poles at the GBW, 70 to 410 MHz): loop phase margins up by
  at most 0.58 degree and crossovers down by at most 0.56 %; the gain margins
  of `LoopStability.pump` (50.7 dB) and `.feedback_buffer` (39.2 dB) become
  `never`: their -180 degree crossings were at 1.9 and 2.2 GHz, 40 to 50 dB
  below unity, made by the old constant feedthrough's phase, and the loop
  phase now approaches -180 degrees without crossing it (-178.3 at 10 GHz);
  `LoopPeaking.receiver` +1.3 %; errors near 1e-5 to 1e-11 (calibration,
  contact, stray-capacitance, soil, matching) by up to 10 % of themselves;
  `Startup` settling by up to 1.3 % (0.5 % of it from the slew-limited
  common mode), `Distortion` harmonics below the integration floor. No
  measure's status changed. Which set is closer to the parts is not
  measured: their vendor models are I3's.

Open: a `cmrr_corner = 0.08Hz` on the OPA2189 still extrapolates to 3 dB at
the gain-bandwidth and makes its follower 25 MHz where TI's model gives
14.9 MHz and the data sheet 8 MHz at `AV = 1`, so the block keeps its flat
CMRR (follower 14 MHz) and the acceptance "both corners declared" is not met
for this part; TI's model flattens at 35 dB from 355 kHz,
which a block cannot say (a declared floor would be the next addition, once a
data-sheet figure shows one). The data-sheet limits are judged at the
check's conditions only (no supply or load per limit). The CopperLib parts'
data sheets give over-temperature maxima, not typical coefficients, so no
part declares the new tempcos. OPA2182, OPA2277, OPA547, TLV757, TPS7A20
and R-78 limits were not entered (their data sheets were not at hand);
the TLV757 and R-78 blocks have no `psrr_corner` (no single-pole figure, no
figure). TI's OPA2189 model is below the data sheet's 146 dB CMRR and PSRR
minimums (130.3 and 133.4 dB): noted, not compared with ngspice. The
CopperLib parts that use `datasheet` need this compiler.

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
