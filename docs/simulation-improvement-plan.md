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

Open: no vendor model compared (the opt-in `#[ignore]` test and the
gitignored `tests/reference/vendor/` are in place, with no deck); release
timings of the vendor transients and the slew checks not rerun; the listed
ngspice behaviours not adopted (reverse cubic, breakdown branch, substrate
`G_MIN`, `r_bb` without its derivative, the non-`badmos3` level 3, `NLEV`
noise); the sub-cause of the inverter's transient difference; JFET and
MOSFET noise outside saturation; the PSpice and LTspice dialects (no
simulator to compare with); the compatibility table's TI parts have no
ngspice column.

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

Open: the CopperLib blocks of TLV75718PDRVR, TPS7A2018PDBVR and
TPS7A2025PDBVR still carry no `sim spice` block, and the amplifier blocks no
`note` (CopperLib was not changed; the edits are listed in the I2 report).
The OPA2189 follower step at the reference comparison's tolerance
(`reltol` 1e-5) still collapses at 26.85 ns, and whether it does depends on
the last bit of `max_step`: the control node of the model's
output-impedance source (`N3828449`, a 1.6 ps lead network) jumps by 1 to
5 % between steps of 1e-17 s, every step is accepted as a discontinuity and
the restart at a tenth of it shrinks the step below the floor; restarting
at the same step instead only crawls on, so the jumping itself is not yet
explained. The TL431 under a current source and PSpice's behaviour there
are not settled. TLV75718P's quiescent current (5.3 uA against 25 uA) and
the TPS7A20's line rejection (75 dB against 95 dB) were not compared with
ngspice. No release timings were taken.

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
