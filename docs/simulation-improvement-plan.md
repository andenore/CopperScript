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
bench (firmware rules 1 and 2) is not built; a `sim regulator` whose
`output_current_limit` is below the load fails to solve its operating point
(singular at the model's internal node), so an active limit can be checked
only where a stand-in drives the rail; `copper-sim` exposes no held operating
point, so the bias point repeats the hold of its small-signal analyses rather
than reading theirs.

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
