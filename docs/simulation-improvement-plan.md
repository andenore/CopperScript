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

Built on 22 November 2026, from the three files the owner downloaded from
analog.com (`ada4817.cir` Rev. 2.1, `ADA4807.cir` Rev. 4.0, `ad8130.cir`
2012), registered with `copper models add` into the gitignored
`.copper-cache/spice/<sha256>/`; nothing from them is committed.

- Blocks: `sim spice` with `sha256`, `download`, `subckt`, `dialect = pspice`
  and `note` on `analog_ada4817.copper`, `analog_ada4807.copper` and
  `analog_ad8130.copper`, terminals in each file's "Node assignments" order.
  The ADA4817's FB node is put on the OUT pin, as the board wires it: with
  FB a terminal of its own the loop probe at OUT left the internal 1 mohm FB
  path closed and `LoopStability.sense_buffer_hp` read 360 degrees. A
  checkout without the files builds and runs the macro models unchanged;
  `--models vendor` there stops every bench with `SIM108` and
  `--check-model` reports the vendor model as not run (checked with the
  cache moved aside).
- Reader: all three load and converge unchanged (PSpice resistor models with
  `T_ABS = -273`, `VSWITCH`, 1 uohm resistors, `POLY(1)` with `(a,b)` nodes,
  `POLY(2)` current controls, BF-only BJTs). ngspice-46 in local, gitignored
  decks (`tests/reference/vendor/`: each model as a follower at +/-4.5 V,
  op and ac to 1 or 3 GHz, plus the open loop through 1 GH): 24 series,
  1500 values; operating points within their bounds (supply currents
  19.000 mA, 1.0018 mA, 14.148 mA in both), AC within 2e-4 except the
  ADA4817 follower at its 1.06 GHz resonance (13 dB peak, 1.93 times the
  bound) and the ADA4807's 8e-11 A V- current at 112 MHz (1.79 times); the
  cause of those two is not established.
- Solver (`newton.rs`, general): the analyzer's front end with the ADA4807
  and ADA4817 models together had no operating point (Newton, gmin
  stepping, pseudo-transient continuation and source stepping all failed),
  although each model alone solved by pseudo-transient continuation. Traced
  with temporary output: gmin stepping fails at 1e-5 S; an adaptive variant
  (tried, not kept) stalls at 2.6e-5 S with steps below 1e-6 decades,
  DRV_BUF, HC_FB and the slew-clamp junctions of the three pump amplifiers
  unconverged (a fold of the pump loop, inferred, not shown); source stepping
  stalls at 3.8 % of the sources (also with steps down to 1e-9); and
  pseudo-transient continuation cycles between 6e-5 and 4e-3 S for its 400
  steps, the unconverged unknowns alternating between the pump's and the
  ADC drivers' amplifiers (the enable comparators' sources and junctions:
  each ADA4807 and ADA4817 enables itself with a smooth switch that feeds its
  own control). One schedule serves every part there, and each part settles
  alone. A power-up transient (sources ramped from zero, ngspice's
  `optran` idea) was tried and dropped: its step collapsed at 19 ns in the
  ADA4817's mid-supply network. Fix: when every aid fails, the parts whose
  operating points do not depend on each other (joined only through nodes a
  grounded voltage source fixes, or through capacitors) are solved one by
  one with the same aids and Newton finishes the whole (`dc_blocks`,
  `by_blocks`). It runs only after every other aid failed, so no operating
  point that solved before changes. Tests: `dc_blocks` splits at a fixed
  rail and a capacitor and joins a controlled source to its control; the
  solution by parts equals the plain one within 1e-9 (copper-sim unit tests;
  the circuit that needs it, the analyzer with vendor files, cannot be
  committed).
- `--check-model` at 9 V (the board's +/-4.5 V; `--tie PD=vp` for the
  ADA4817, `DISABLE=vp` for the ADA4807): every macro figure within its
  tolerance; vendor beside macro beside the data sheet as recorded in the
  part file:

  | Part | Figure | Vendor | Macro | Data sheet |
  | --- | --- | --- | --- | --- |
  | ADA4817 | supply current | 19.0 mA | 19.0 mA | 19 mA typ (+/-5 V) |
  | | open-loop gain | 64.0 dB | 65.0 dB | 62 min / 65 typ dB |
  | | gain-bandwidth | 415 MHz | 409 MHz | 410 MHz |
  | | follower | 1.37 GHz, 14.0 dB peaking | 585 MHz, 0.6 dB | not recorded |
  | | noise at 800 kHz / 10 Hz | 3.68 / 170 nV/rtHz | 4.02 / 113 nV/rtHz | 4 (100 kHz) / about 175 (Figure 30) |
  | | slew rate | 720 V/us | 880 V/us | 870 V/us |
  | ADA4807 | supply current (9 V / 3.3 V) | 1.002 / 1.001 mA | 1.0 / 0.915 mA | 1.0 / 0.915 mA |
  | | open-loop gain (9 V / 3.3 V) | 130.8 / 130.9 dB | 113 / 113 dB | 130 (+/-5 V) / 113 (3 V) dB |
  | | gain-bandwidth | 191 MHz | 180 MHz | 180 MHz (`G = +1` bandwidth) |
  | | unity crossing, margin | 97 MHz, 69.5 deg | 159 MHz, 62.6 deg | near 200 MHz, about 55 deg (Figure 23, +/-2.5 V) |
  | | follower | 204 MHz | 255 MHz | 180 MHz |
  | | slew rate | 203 V/us | 228 V/us | 225 V/us rising |
  | AD8130 | supply current | 14.15 mA | 10.8 mA | 10.8 typ / 11.6 max mA |
  | | output offset | 3.0 mV | 0.40 mV | 0.4 typ / 1.8 max mV |
  | | open-loop gain | 74.5 dB | 74.0 dB | 74 dB |
  | | gain-bandwidth, margin | 176 MHz, 58.0 deg | 269 MHz, 61.6 deg | 270 MHz (`G = +1` bandwidth); 58 deg (Figure 74) |
  | | follower | 245 MHz | 376 MHz | 270 MHz |
  | | slew rate | 2.19 kV/us | 1.08 kV/us | 1090 V/us |

  The vendor figures are the files' own (ngspice agrees); each block's
  `note` says so. Where the macro differs from the vendor model and the
  data sheet agrees with the vendor model: the ADA4807's open-loop gain at
  +/-4.5 V (130 dB at +/-5 V in Table 2, recorded; the block uses the 3 V
  113 dB for every supply) and its follower bandwidth (180 MHz recorded;
  vendor 204, macro 255), and the AD8130's follower bandwidth (270 MHz
  recorded; vendor 245 MHz, macro 376 MHz) with its 58 degrees (vendor 58.0,
  macro 61.6). Proposed, not made: `supply 10V { open_loop_gain = 130dB; }`
  on the ADA4807, and for both amplifiers a `gbw` lowered so that the macro
  follower meets the recorded `G = +1` bandwidth (both blocks read that
  bandwidth as a single-pole gain-bandwidth, which the second pole then
  raises by 40 %). Each moves rated analyzer results that README.md and
  SPEC.md quote, so it is the owner's call (decision 6).
- LTC6268: the analyzer does not use it; README.md names it as the
  lower-capacitance sense buffer. `parts/analog_ltc6268.copper` (example
  local, the pins of CopperLib's `LTC6268IS6`, which has no block) carries a
  `sim opamp` block from the data sheet the owner downloaded (62689f),
  every figure with its page; the curve readings (gain and phase, CMRR,
  PSRR, output impedance, noise) were taken numerically from the PDF's
  vector curves. `--check-model` passes at 5 V and 3.3 V (phase margin 54.2
  degrees against the curve's 54). The fitted `cmrr_corner` (50 kHz) is
  left out: I7's term stops rising only at the gain-bandwidth, so it still
  feeds forward at about -12 dB at the 405 MHz crossing (checked on the I7
  tree: margin 57.0 against 54.2 degrees, follower peaking 2.7 against
  1.2 dB; every check passes either way).
- Ratings: none added. The 26 unrated simulated pins of the front end (the
  AD8130's eight, OUT and DISABLE of the seven ADA4807s, OUT and PD of the
  two ADA4817s) need figures the part files do not record: the AD8129/AD8130
  Rev. C "Absolute Maximum Ratings" table (supply, input and differential
  input voltage, PD) and whether the ADA4817-1 Rev. A (p. 5) and ADA4807
  Rev. B (Table 5) tables rate OUT, PD and DISABLE at all. `copper check
  --sim-results`: 51 unrated simulated pins before and after (the I4 count).

Analyzer with `--models vendor` (release build, bench by bench; every
vendor operating point goes through all aids to the split, 12 to 67 s a
step), against the default run (worst rated step, macro then vendor):

| Bench | Default | Vendor | Changed limits |
| --- | --- | --- | --- |
| `SoilSweep` | 0.39 %, 0.16 deg | 0.34 %, 0.076 deg | none; dry-soil current at 1 MHz 4.1 to 1.7 uA (the ADA4807 model's 4.5 pF at PUMP) |
| `LoopStability` | margins 49 to 79 deg, stable | pump 54, feedback buffer 65, receiver 58, gain 1 56 deg | drive buffer 43.3, sense buffer 41.5, gain 10 and 100 34.4 and 34.7, transimpedance 10.6 deg (min 45); poles in the right half-plane (round-off of the pole analysis, fixed in the follow-up) |
| `GuardStability` | stable, 79 and 74 deg | 54 and 69 deg | poles in the right half-plane (round-off, fixed in the follow-up) |
| `LoopPeaking` | 1.00 to 1.06 | receiver 1.06, pump current 1.00 | drive buffer 1.33, sense buffer 1.52, gain 1 1.25, gain 10 1.73, gain 100 1.71, transimpedance 4.59 (max 1.12) |
| `CalibrationCard` | 0.51 %, 0.23 deg | 0.46 %, 0.13 deg | none |
| `StrayCapacitance` | 0.92 %, 0.58 deg | 0.86 %, 0.48 deg | none |
| `ContactImbalance` | 0.89 %, 0.91 deg | 0.84 %, 0.70 deg over the first 115 of 192 steps | no operating point at step 116 (800 ohm, 5.56 pF, LP contact 110 %), even split |
| `SenseCommonMode` | 66.8 dB | 64.0 dB | none (min 63 dB) |
| `PumpOutputImpedance` | 6.2 Mohm at 1 kHz | 7.2 Mohm | none |
| `DcOperatingPoint` | within 56 mV | within 53 mV | none |
| `SupplyCurrent`, `PowerSide` | 51.8 mA a rail | 55.2 mA (AD8130 14.15 mA) | none (max 70 mA) |
| `MeasurementNoise` | no limits | SNR within 5 dB of the default; the AD8130 model has no noise | - |
| `Startup` | settled in 0.67 s | no operating point at the start of the first step (rails at 0 V, electrodes in air): the unpowered ADA4817 models | not judged |
| `Distortion` | no limits | did not finish: out of memory after 86 min in the whole run (an 11.7 MB allocation, a dense 1209 by 1209 matrix's size), 11.2 GB after 30 min on two threads (the shooting method keeps every step of a period for its exact tangent) | - |
| `ResistorMatching`, `ResistorWorstCase` | pass | not run to the end (hundreds of operating points at 12 to 67 s each) | not judged |

SPEC.md, "Vendor models", explains each failing limit (follow-up below).

Decision 6: every measure of the seven examples is identical with the
default models (the spice blocks and the LTC6268 part add nothing to a
default run; the split never runs there).

Follow-up, 22 November 2026 (why the loop limits fail; scratch decks in the
session's scratchpad `i3/loop/`, not committed):

- Loop analysis checked. Each failing loop built stand-alone (the amplifier
  with the board's feedback network, load and supplies; the soil at 800 ohm;
  vendor models) and run through copper-sim and ngspice-46 (Tian's two
  injections in ngspice, a closed-loop AC run and a step in both). Margins
  and closed-loop peaking, copper / ngspice: transimpedance 10.14 / 10.14
  degrees, 4.987 / 4.987; gain 10 34.36 / 34.36, 1.732 / 1.732; gain 100
  34.78 / 34.78, 1.714 / 1.714; gain 1 55.86 / 55.86, 1.253 / 1.253; drive
  buffer 43.30 / 43.30, 1.332 / 1.332; sense buffer (with the real LP
  buffer and receiver) 41.55 / 41.55, 1.562 / 1.563. The analyzer's figures
  are reproduced (43.3, 41.5, 34.4, 34.7, 10.6 degrees; 1.33, 1.52, 1.25,
  1.73, 1.71, 4.59). Loop gains agree within 1e-4 of `max(|T|, 1e-3)` at
  every frequency, except the drive buffer below 10 MHz, where ngspice's Tian
  combination gives `|T|` = 7.7 at phase 180 degrees against copper's
  `A` = 1.9e5 (the closed loop, the step, the crossover and the margin agree;
  the cause is not established). Steps agree within 1.2e-3 of the step and
  ring as the margins say: transimpedance 16 % overshoot, ringing at
  5.7 MHz with damping 0.045; gain 10 38.5 %, 6.4 MHz, 0.35; gain 1 15 %,
  0.52. No copper bug in the loop analysis.
- Pole analysis: a copper bug, fixed (`eigen.rs`
  `with_slow_roots_inverted`, `stability.rs` `full_pencil_slow_roots`). The
  right half-plane poles of the vendor circuits were round-off: the QR
  iteration's absolute error scales with its matrix's norm, set by the vendor
  models' fastest roots (1e15 rad/s and more), so roots of a few rad/s came
  out on either side of the axis. Evidence: the pump circuit with its 0 V
  probe source at three places (the same circuit) gave three different sets
  (1.8 and 35.5 Hz, 4.7 and 193 Hz, 3.3 and 119 Hz), all unrefined; giving
  one of two equal coupling networks 47.5 kohm instead of 47 kohm removed the
  sense circuit's four; a 0.1 s transient of the sense circuit (1.5 million
  steps) decays at -6.4 rad/s where the analysis claimed +44 and +212 rad/s.
  Fix: the slow roots (those whose `mu = 1 / (s - sigma)` is at least 1e-4
  of the largest) are taken from the eigenvalues of `-(G + sigma C)^-1 C`,
  which the QR iteration gives to round-off relative to themselves: first on
  the deflated pencil inside `finite_eigenvalues` (no infinite eigenvalues
  there; about 1e-4 accurate on the test circuit), then on the whole pencil
  where its QR iteration succeeds (round-off); `sigma` is 0, or with roots
  at the origin a hundredth of the slowest other root. The stand-alone
  circuits now give -6.44 rad/s twice (4.7 nF and 33 Mohm), -21.28 twice
  (1 uF and 47 kohm) and no right half-plane pole; the three pump
  placements agree (least damped 16.4 Hz, damping 0.166). Test
  (self-written stiff circuit, 1 uohm to 1 Tohm, with and without a floating
  node): `slow_roots_of_a_stiff_pencil_are_exact` (the slow roots were
  1.25e-3 and 7.6e-3 off before; now within 1e-12, 5e-10 with the shift).
  Decision 6: three pole dampings move at round-off (Howland
  `Stability.poles` 1.3e-16 and 2.2e-16 relative, the analyzer's
  `LoopStability.poles` 3.3e-16, `GuardStability.poles` 3.7e-12: Newton's
  refinement converged from a slightly different start), everything else
  identical to the I7 baseline. Analyzer with vendor models: `LoopStability.poles`
  passes (197 poles, least damped 5.41 MHz with damping 0.088 and 16.4 Hz
  with 0.166, as the stand-alone circuits give); `GuardStability.poles`
  keeps one pair at 607 MHz with damping -0.067 whose mode spreads over four
  unrelated ADA4807 instances (not established; no transient run for it).
- Causes per loop, against the ADA4807 Rev. B and ADA4817 data sheets found
  on disk (read numerically from their vector curves). The ADA4807 model's
  open-loop phase lag between 2 and 50 MHz is the part's (Figure 23 at
  +/-2.5 V: -104.5 degrees at 5 MHz, -110.8 at 10 MHz, 94 MHz crossing with
  68 degrees; model -102.9, -110.3, 97 MHz with 69.5; the part file's earlier
  reading "near 200 MHz, about 55 degrees" was wrong and is corrected there);
  its output impedance is
  about twice the part's (open-loop 114 ohm against 52 to 56 ohm implied by
  Figure 54 and Figure 23; as a follower 2.1 to 2.9 times Figure 54; in the
  data sheet's Figure 67 circuit 5.1 dB of peaking with 470 pF and 20 ohm
  where Figure 68 shows 3 dB). With the output resistance at 56 ohm the
  stand-alone loops give: drive buffer 53.8 degrees (passes), gain 1 59.8
  (peaking 1.18 fails), gain 10 and 100 39.0 and 39.3, transimpedance 22.7
  (fail). The ADA4817 model matches its data sheet's output impedance at 10
  and 100 MHz (0.19 and 2.1 ohm against 0.25 and 2) and its crossing phase;
  the sense buffer loses its margin to the receiver's input (51.1 degrees
  without it) and is damped by the guard (4.7 degrees without it).
- Options (scratch variants, not applied), SPEC.md "Vendor models": board
  (2.2 pF across each 9.09 kohm, 10 pF transimpedance feedback, 200 ohm
  before the gain-1 driver's 470 pF) and macro (`output_resistance = 56ohm`
  on the ADA4807). Board, vendor: gain 1 60.8 degrees (peaking 1.12),
  gain 10 and 100 72.5 and 73.0 (1.03), transimpedance 47.3 (1.00), drive
  and sense buffers unchanged (43.3, 41.5); `SoilSweep` 0.35 % and 0.091
  degrees, `StrayCapacitance` 0.88 % and 0.48 degrees; `CalibrationCard`
  stops at one position (no operating point). Board, macro: gain stages 91.6,
  transimpedance 80.2, accuracy within limits. Macro `output_resistance =
  56ohm`: transimpedance 41.0 degrees and its peaking 1.17 and the drive
  buffer's peaking 1.18 fail in the default run, the rest passes, accuracy
  unchanged; a lower `second_pole` cannot represent Figure 23's mid-band lag
  without wrecking the followers (60 MHz: gain 1 28, drive buffer 23
  degrees).

Decision, 23 November 2026 (the owner): the board option is the circuit.
`analog/front_end.copper` has 2.2 pF (`C_G1F`, `C_G2F`) across each gain
stage's 9.09 kohm, `C_FB_TIA` 10 pF instead of 1.5 pF and, on the gain-1
channel, `R_AA1` 100 ohm and `C_AA1` 1 nF instead of 49.9 ohm and 470 pF; the
README and SPEC.md describe it ("Vendor models"). The option as first applied
had `R_AA1` 200 ohm into 470 pF. It was replaced the same day because PA1 may
see at most 100 ohm with 2.5-cycle sampling (DS12288 Table 70) and the ADC
timing (3.125 Msps, 16 ADC clocks a trigger) leaves no room for a longer
sampling time. The network mostly filters the follower's own closed-loop peak
at the pin (the bare vendor follower peaks 1.1 dB); vendor models,
`LoopPeaking.gain1_driver` and the `LoopStability.gain1_driver` margin:
49.9 ohm and 470 pF 1.253 and 55.9 degrees, 100 ohm and 470 pF 1.145,
150 ohm and 470 pF 1.125, 200 ohm and 470 pF 1.118 and 60.8, 100 ohm and
680 pF 1.133 and 62.0, 100 ohm and 1 nF 1.124 and 62.5, 100 ohm and 1.2 nF
1.121 and 62.7. 100 ohm into 1 nF has the best margin of those that meet the
100 ohm rule, and its peaking misses the 1.12 limit by 0.004 on the vendor
models. The follower at a noise gain of 2 (1.1 kohm feedback, 100 ohm across
the inputs) with 49.9 ohm into 470 pF was dropped: peaking 1.397, margin
46.7 degrees. `SenseCommonMode` and `MatchingTolerances` now take the
rejection at `V_D`, the receiver's output, instead of `V_MON_1`, so the gain-1
channel's response no longer enters it (with 200 ohm it had read 1.2 dB high
at 1 MHz).
Costs, default models: the transimpedance stage's bandwidth falls to 0.8 MHz
(at 1 MHz the current channel passes 61.9 % with 60.7 degrees of lag, 98.6 %
and 19.7 before, so `I_MON` carries at most 0.19 V there instead of 0.30 V);
the gain-1 channel's anti-alias pole is 1.59 MHz against 6.8 MHz on the
other three (at 1 MHz 84.7 % and -32.8 degrees against the gain-10 channel's
97.5 % and -18.2; 98.9 % and -9.1, 98.9 % and -11.8 before); `ContactImbalance`'s
worst rated phase is 0.9966 degrees of its 1 degree limit (0.91 before);
PA1's source impedance is 100 ohm plus the follower's, at the limit.
`SenseCommonMode` at 5.58 kohm is 66.66 dB (66.76 before any board change).
Every default bench still passes (gain stages 91.6 and 91.7 degrees,
transimpedance 80.2). With the vendor models on the final board:
`LoopStability` pump 54.25, feedback buffer 64.58, receiver 58.23, gain 1 62.54, gain 10 72.5, gain 100 73.0, transimpedance 47.31 degrees and its poles pass; the drive buffer (43.3) and the sense buffer (41.5) fail; `LoopPeaking` gain 10 1.027, gain 100 1.026, receiver 1.056 pass, the drive buffer (1.331), the sense buffer (1.517) and the gain-1 driver (1.124 against 1.12) fail; `SenseCommonMode` 63.95 dB (limit 63), `SoilSweep` 0.35 % and 0.091 degrees and `StrayCapacitance` 0.88 % and 0.48 degrees pass; `CalibrationCard` and `GuardStability` were not rerun on the final board (measured, `--models vendor`, 23 November 2026). With the vendor models on the board as first changed (200 ohm
into 470 pF; measured): `LoopStability` pump 54.25, feedback buffer 64.58,
receiver 58.23, gain 1 60.78, gain 10 72.5, gain 100 73.0, transimpedance
47.31 degrees and its poles pass, the drive buffer (43.3) and the sense
buffer (41.5) fail; `LoopPeaking` gain 1 1.118 (limit 1.12), gain 10 1.027,
gain 100 1.026, receiver 1.056 pass, drive buffer 1.331 and sense buffer
1.517 fail; `SoilSweep` 0.3497 % and 0.0906 degrees and `StrayCapacitance`
0.881 % and 0.4772 degrees pass; `CalibrationCard` stops at its 10 kohm
position with 0 ohm contacts (no operating point).

Second follow-up, 23 November 2026 (the vendor benches that did not finish,
and the two buffer loops; scratch in the session's `i3b/`, not committed):

- Where the time went. One operating point of the vendor front end took
  10.4 s, every aid but the last failing first: Newton from zero 1.1 s,
  gmin stepping 0.1 s, pseudo-transient continuation 3.0 s, source stepping
  1.3 s, then the split 4.9 s (each of its six parts: Newton and gmin
  stepping fail, pseudo-transient continuation solves). And a step solved
  its circuit several times: a reference run's circuit once more, and every
  `at(x, f)` between the swept samples twice more (the bench's circuit and
  the reference's), so `CalibrationCard` spent 65 to 72 s a step on eight
  operating points.
- Operating-point hints (`copper_sim::DcHint`, `with_dc_hint`, `dc_hint`).
  A small-signal bench with several steps or variation runs solves its first
  step alone; its operating point, its circuit and the aid that solved it
  hint every other step, a step's nominal points hint its Monte Carlo runs
  and corners (each reference run by the same reference at nominal values
  first), and a run's own point hints its reference runs. Only when Newton
  from zero fails does a hinted solve try Newton from the hint's points,
  then continuation from the hint's circuit (`Aid::Continuation`: its
  resistances, controlled-source gains and source values moved to the
  circuit's in steps, Newton at each), then the hint's aid, then the other
  aids; every attempt runs on a fresh solver, so the result depends on the
  circuit and the hint alone, and every run gets the same hint whatever the
  thread count. From a neighbouring step's point the vendor front end solves
  in 0.08 to 0.2 s. `ResistorMatching`'s Monte Carlo run 70 needed both:
  its `SHORT` reference has no operating point by any aid from zero but
  solves from the nominal `SHORT` reference's, and its own circuit (the
  resistors drawn far enough that `U_PMP` and `U_DRV` start elsewhere) solves
  only by continuation from the nominal one. Tests:
  `a_hint_starts_newton_from_a_related_operating_point`,
  `continuation_from_the_hints_circuit_solves_where_newton_from_its_point_does_not`,
  `hints_nest_and_restore`, `the_operating_point_by_parts_is_the_operating_point`
  (copper-sim), `hinted_steps_references_and_monte_carlo_runs_do_not_depend_on_threads`
  (copper-simulate, jobs 1 against 4).
- The last 32 linearisations are remembered process-wide by circuit and hint
  (`linearized_at_point`): the single-frequency solves and the reference runs
  that repeat across steps reuse them; the same solve would give the same
  result. Test: `a_linearisation_is_remembered_per_circuit_and_hint`.
- A transient run after an aided operating point stalled within the first
  microsecond (the step shrinking to 1e-15 s at the sense buffers, the pump or
  the gain stages) where the old solver went on: the caller's solver still
  held the factors of Newton's failed iterates, whose pivot order the run
  reused, and a solve in a stale order of a badly scaled matrix (1 uohm beside
  1 Tohm) is too inaccurate for Newton to converge. Fixed: the caller's
  solver is factored afresh at the aided solution, and a transient step whose
  Newton diverges is retried once on factors ordered afresh before a shorter
  step is tried (`Solver::forget_factors`; test:
  `freshly_ordered_factors_solve_the_same_newton_iteration`). The 2 us
  transient from the operating point matches the old solver's (5.696 mV peak
  at `BUF_HP`). Tried and dropped: a further retry on dense factors (partial
  pivoting at every column) before the shorter step; it got no transient
  further (`Distortion` stopped at 14.77 us instead of 15.09 us, after
  5738 s instead of 569 s) and moved `SupplyCurrent.driving` at 7e-6.
- The shooting method's memory (`JACOBIAN_BUDGET`, 512 MiB): it kept the
  factored Jacobian of every step of a period, 11.7 MB each where the sparse
  LU leaves a vendor matrix dense, 11 GB and out of memory over one period of
  the 1209-unknown front end. The factors beyond the budget are refactored at
  each tangent product from the same factors before them, so the tangent is
  the same bit for bit (test: `the_tangent_of_a_period_matches_its_difference_quotients`,
  with no factors kept and with half of them kept).
- Poles: a root whose Newton refinement fails (a repeated root, or a QR value
  with no eigenvalue near it) is now refined by inverse iteration at a fixed
  shift beside the QR value and certified by its backward error (at most
  1e-11; `nearest_eigenvalue`). All 192 vendor roots of `GuardStability.poles`
  are certified (52 were unrefined). The 607 MHz pair with damping -0.0667 is
  real for the model: certified, reproduced by a stand-alone deck of the two
  sense buffers and the receiver as that bench wires them, and a 1 uA, 1 ns
  kick into `SNS_HP` grows from 1.9 mV to a 0.57 V limit cycle within 40 ns
  in copper's transient and in ngspice-46's alike. The bench leaves the HP and
  LP guards unloaded (no electrode strays but HC's); with the HP and LP strays
  that are always there (20 pF to the guard, 100 pF guard to ground) the
  stand-alone sense buffers are stable (least damped 549 MHz, damping 0.090).
  The bench is unchanged, so it fails. Tests:
  `repeated_roots_are_certified_by_inverse_iteration`,
  `a_qr_value_with_no_eigenvalue_near_it_is_replaced`. Default runs: the
  three repeated 6.79 MHz anti-alias roots of `LoopStability.poles` and
  `GuardStability.poles`, unrefined before, are now certified (values within
  1.4e-11, imaginary parts of 1e-8 Hz now 0); every measure is unchanged.
- `Startup` cannot run on the vendor models: its first point has every rail at
  0 V, where the AD8130 model has no meaningful operating point (copper: a
  singular system; ngspice-46: 3.5e13 A from its rails) and copper does not
  solve the unpowered ADA4817 model either (Newton cycles between two
  iterates in its supply-current mirror below about 1 V on its rails, every
  aid too), though ngspice-46 does (19 mA quiescent, 7 uV out).
  The AD8130's is the model's limit; the ADA4817's is copper's, not fixed.
- `Distortion` does not finish (SPEC.md, "Vendor models"): no longer out of
  memory, but the vendor transient reaches 15.09 us of the first 1 kHz
  period after 569 s and stops (Newton diverging at every step down to the
  floor, at ADA4807 model nodes). On a plain 60 us transient of the same
  circuit the old solver takes 0.32 ns steps (570 s for 10 us, agreeing to
  2e-6 with the new one there) and had not stopped after 40 minutes, where
  the new one with the dense retry stopped at 14.8 us. One 1 ms period takes
  10 to 16 hours at either pace. Tried and dropped: the
  shorter step after a failed one restarting with backward Euler (as SPICE
  drops to first order; it stopped at 14.84 us, slower) and the dense retry
  above. Not established: why Newton diverges there.
- Buffers. Drive buffer: stand-alone in copper and in ngspice-46 (margin
  43.30 degrees both, closed-loop peaking 1.3316 both); the bare vendor
  follower has 50.8 degrees and peaks 1.14, with the pump's 10 kohm 51.3 and
  1.14, and the LT5400's 2.75 pF on `DRV_BUF` takes it to 43.3 and 1.33 (with
  the model's output resistance at the 56 ohm the data sheet implies: 53.8
  and 1.11). Fix: a 33 ohm and 10 nF snubber from the output to ground (no
  series resistor before R1): stand-alone 64.2 degrees and 1.11. The gain-1
  driver gets the same snubber beside its 100 ohm into 1 nF (1.124 to 1.109,
  62.3 degrees). Both are now on the board (`R_SN_DRV`, `C_SN_DRV`, `R_SN_B1`,
  `C_SN_B1`); their cost, SPEC.md "Vendor models": vendor bandwidth 179 to
  35.6 MHz and 70.8 to 29.7 MHz, accuracy and noise unchanged, default worst
  rail current 52.28 to 55.06 mA. Sense buffer: copper and ngspice-46 agree
  (41.55 degrees, peaking 1.5614 and 1.5616); the bare vendor ADA4817
  follower has 52.7 degrees but a second crossing at 1.3 GHz and peaks 5.6,
  4 pF of load (the AD8130's input) takes it to 35 degrees, and the guard's
  49.9 ohm into 120 pF is what damps it. Options measured, none meeting both
  limits: 200 ohm to 1 kohm in series with each receiver input, margin 50.3
  to 51.0 degrees (passes) and peaking 1.30 (fails; in the analyzer 50.26 and
  1.296); with a 10 ohm and 10 nF snubber besides, 58.8 and 1.15, but that
  loads each sense buffer with 18 ohm at 1 MHz under up to 0.8 V of common
  mode (44 mA); a larger guard resistor raises the peaking (100 ohm: 1.38,
  200 ohm: 1.69). Not applied.
- `MatchingTolerances` now varies `C_G1F` and `C_G2F` (5 %, declared last so
  the other parts' Monte Carlo draws are unchanged); the README's
  `ContactImbalance` rule-coefficient table is regenerated (it reproduced the
  I7 baseline but for 3 cells; new 1 MHz columns).
- Times, vendor models, release build, 12 threads (before: the binary at
  201ae83b on the same circuit, or as noted):

  | Bench | Before | After |
  | --- | --- | --- |
  | `CalibrationCard` | 72 s | 55 s |
  | `ContactImbalance` | stopped at step 116 of 192 (the circuit before the board change) | 398 s, every step |
  | `ResistorMatching` | not run to the end (hundreds of operating points at 10 to 67 s); with the hints alone, Monte Carlo run 70 had no operating point | 1078 s |
  | `ResistorWorstCase` | not run to the end (hundreds of operating points at 10 to 67 s) | 908 s |
  | `GuardStability.poles` | 52 of 190 poles unrefined | 89 s, all 192 roots certified |
  | `Distortion` | out of memory after 86 min | stops at 15.09 us after 569 s |
  | `Startup` | no operating point | no operating point, after 842 s |
  | All benches | not run to the end | 4788 s, at most 1.4 GB |

  Default models: every measure of the seven examples identical to the
  baseline taken at 201ae83b before any change (the analyzer at 201ae83b's
  circuit: 7002 of 7002; the six others: all), except the pole fields named
  above; the analyzer's default run with `--jobs 1` is identical to the
  default thread count. With the snubbers, the analyzer's default run moves
  in `SupplyCurrent.driving`, `Distortion`, `Startup` (0.67 s settling kept)
  and at 1e-13 in `ResistorMatching` and `ResistorWorstCase`.

The vendor results on the final circuit are in SPEC.md, "Vendor models".

Open: acceptance is not met. With the vendor models the sense buffer's
margin (41.5 degrees) and peaking (1.517) still fail (taken as the part's;
the options above each fail a limit or cost 44 mA), and
`GuardStability.poles` fails for a real 607 MHz pair of the sense buffers
with the HP and LP electrodes unloaded (stable with their strays; the bench
is the owner's call); `Startup` (no operating point with the rails at 0 V:
the AD8130 model's limit and copper's on the unpowered ADA4817) and
`Distortion` (the transient's pace on the vendor circuit, and Newton
diverging at 15 us) do not finish; 51 simulated pins stay unrated. Also
open: the proposed macro changes (I3 above and `output_resistance = 56ohm`,
pending the owner's macro decision); the ADA4807 macro block's `gbw`
180 MHz and `second_pole` 300 MHz, which do not reproduce Figure 23 (part
file); the drive buffer's low-frequency Tian difference in ngspice; the
vendor circuit's factorisations (the sparse LU often leaves its matrix to
the dense one, 11.7 MB of factors for 1209 unknowns), 84 % of
`ResistorMatching`'s time; the LTC6268 block in CopperLib; the two AC
differences of the I3 ngspice comparison; a committable deck that fails
without the split.

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

Steps 1 and 2 built on 24 November 2026; step 2 on a routed copy of the
Howland pump in a test, not on the example itself (below); step 3 not
started.

- Field solver (`copper_extract::capacitance`): a 2-D electrostatic solution
  of one cross-section, finite volumes on a tensor grid graded geometrically
  from every conductor edge and dielectric interface, conductors of any
  thickness (zero included) in horizontal slabs of their own permittivity,
  a grounded or mirror box, solved directly by banded Cholesky (no iteration
  tolerance; charges by Gauss's law on the grid, so the matrix is symmetric
  to 1e-15). The edge cells' error is first order in their size (four times
  finer leaves 0.26 of it), so each solve is done at the finest cell and at
  its half and extrapolated, `2 Q(h/2) - Q(h)`, which leaves the grading's
  error. Chosen over a 3-D panel method because the board's layered
  dielectric (air over FR-4) is exact in it without a layered Green's
  function; what it cannot see is the third dimension (below).
- Against closed forms (`tests/capacitance.rs`, zero-thickness strips), the
  default mesh (finest cell 1/20 of the smallest feature, growth 1.15):
  parallel plates in two slabs 1.5e-13; a wide strip over a plane
  (homogeneous, `w/h` 10, 30, 100, Hammerstad, 0.03 %) +0.20 to +0.24 %;
  microstrip on er 4.3 (`w/h` 0.25 to 10, Hammerstad and Jensen, 0.2 %)
  +0.12 to +0.26 %; edge-coupled strips between two planes (Cohn, exact) even
  and odd mode +0.10 to +0.20 %, their mutual capacitance +0.29 to +0.61 %
  (the weak one, gap = plane spacing, worst); two coplanar strips without
  ground (conformal map, exact) +0.18 to +0.20 %. Bounds in the tests 0.5 %
  and 1 % (mutual). The solver overestimates, always. The board uses a
  coarser mesh (`BOARD_MESH`: growth 1.3, coarsest cell 1/10 of the cut,
  nothing below 2 um), a fifteenth of the time on a board cut: on the same
  cases at most 0.77 % (capacitance) and 2.2 % (mutual), bounds 1 % and 3 %.
  Not in the closed forms and printed by the test: 35 um copper raises a
  0.25 mm microstrip on 0.2 mm by 3.9 %, a 20 um mask of er 3.5 over it by
  16 % in all.
- Board assembly, 2.5-D (`copper_extract::parasitics`): the routed board
  exported and filled as `copper_extract::board` does; every track, pad and
  via ring cut across at stations at most 0.5 mm apart, each cut the board's
  cross-section 2.5 mm to either side (all copper it crosses on every layer,
  the declared stack-up walked with each copper layer's thickness, 2.5 mm of
  air above and below, a grounded box), the station's copper at 1 V and every
  other piece at 0 V: the charges are the partial capacitances of that piece,
  summed over the pieces of two nets the capacitance between them. Pairs seen
  from both sides count half from each, fills (no stations) in full from the
  other side; overlapping fills of two nets on two layers add a parallel
  plate where no other fill lies between; tracks crossing at more than 45
  degrees on two layers add `eps w_a' w_b' / (d sin theta)` with
  Hammerstad's fringe widths, unless a fill shields them (an estimate, no
  closed form). Distinct cuts are solved once (a straight run is one), in
  parallel, deterministic in key order. For every net that is not a supply's,
  a network: a node per pad, via, fill piece and track end (T-junctions
  split), a branch per track piece with `rho l / (w t)` and the TEM
  self-inductance `[C_0^-1]_00 / c^2` of its middle cut in vacuum (other
  signal copper floating, fills and supply nets the return; the partial
  inductance of the bar where more than half the field reaches the box),
  chains merged in series and their capacitance shared between the ends,
  dead ends folded. Refused: a net whose pads are not joined by copper (it
  names the pad groups), copper of two nets overlapping on one layer.
- Assembly test (`tests/parasitics.rs`): two 0.25 mm tracks 0.45 mm apart on
  `F.Cu` over a plane on `In1.Cu` (0.2 mm, er 4.3), routed 10 mm and 20 mm
  long; the difference per millimetre against the 2-D cut of the middle:
  0.0852 pF/mm to the plane and 0.0107 pF/mm between the tracks, both within
  0.004 % of the cut; resistance 1.966 mohm/mm, exactly `rho / (w t)`;
  inductance 0.3379 nH/mm against the cut's 0.3398 (0.6 %; the station that
  gives it sits at a piece's middle), 11 % below a zero-thickness lone strip
  (the 35 um copper). 2.5 % of the track's capacitance reaches the box and is
  reported, not inserted. An unrouted net is refused with its pads.
- Syntax: `use layout;`, a bench (or fixture) statement beside the existing
  `use library "..."`, which it parallels: what the bench is built from, not
  what it does; no name, because the layout is the board's (one routed board
  per run). The routed board comes from the command line, `copper sim BOARD
  --layout RUN` (the run's `final-board.state.json`, what `route-board
  --checkpoint-dir` writes and `emi-check --run` reads), not from the source:
  the source has no layout and a run directory is not a source fact. A
  misspelling is `PAR011` with the suggestion (`use layot;`). New codes:
  `SIM118` a bench that uses layout with no routed board (no `--layout`, or
  none in RUN): the bench is not run, never run on the schematic alone;
  `SIM119` a routed board that is not this design's (placements or pad nets
  differ, as `emi-check --run` checks) or cannot be extracted; `SIM120` a
  bench of a module on its own. Language reference "Layout parasitics", the
  diagnostics table, `copper sim --help`, completion and hover (the `use`
  item of `sim_reference::BENCH_STATEMENTS`), `--watch` takes `--layout`.
- Insertion (`copper_simulate::layout`): each pin of a networked net on the
  node of its pad, the branches as R and L in series, the capacitances
  between the nodes of two nets (supply and ground nets one node); the net's
  name stays the node of its first pin's pad. Elements on nets the bench does
  not simulate are left out (nothing floats). The bench runs twice; its
  measures, plots and limits are the run with the layout, and it carries the
  schematic's measures (`layout` in `sim-results.json`, schema page "Layout",
  additive), the text table a line per measure (`layout z_out_1mhz: 112.1
  kohm schematic, 40.87 kohm with layout (-63.5 %)`) and the five largest
  capacitances and series nets, the page a Layout section. Test
  (`copper-simulate/tests/layout_parasitics.rs`): an RC with a hand-written
  layout (two nodes on VOUT, 0.5 ohm and 20 nH between, 3 pF and 1 pF) equals
  the nodal solution of the same circuit by hand within 1e-9 at 1 kHz, 1 MHz
  and 10 MHz; a bench without `use layout;` is bit-identical with and without
  a layout given; `SIM118`, `SIM119`, `SIM120`, `PAR011`.
- The Howland pump does not route in this worktree: `route-board` stops at
  `PKG011` (the KiCad footprint package is not in the offline cache), and
  with `--allow-proxy-footprints` physicalisation stops at `C_U1P: decoupling
  requires two connected lands` (the proxy lands of the `tiny` capacitor do
  not satisfy the decoupling check). `use layout;` is therefore not added to
  the example's benches (they would fail with `SIM118` in every checkout
  without a routed board). Instead `crates/copper/tests/sim_layout_cli.rs`
  builds a routed copy: the example's source and benches, proxy footprints, a
  four-layer stack-up (0.2 mm of er 4.3 from `F.Cu` to a ground plane on
  `In1.Cu`), fixed placements, its six signal nets hand-routed in 0.25 mm on
  `F.Cu` (BUF on `B.Cu` under LOAD, two vias), the supply nets left as pads,
  and `use layout;` in `output_impedance.copper` and `stability.copper`.
  With the release binary (`--layout run`, output
  `out-final` beside the fixture): 279 stations, 66 distinct cuts, 2.2 s of
  extraction (48.9 s with the default mesh); 15 branches (R and L), 44
  capacitances. Largest: PFB to GND 2.86 pF, DRIVE 2.13 pF, LOAD 1.83 pF, BUF
  1.82 pF, NFB 1.57 pF; series BUF 21.2 mm, 41.7 mohm, 11 nH; 0.597 pF reaches
  no copper within the cut and is left out; no crossing estimated (BUF's
  `B.Cu` leg crosses LOAD under the plane, which shields it). What the layout
  changes: output impedance at 1 MHz 112.1 kohm to 40.87 kohm (-63.5 %),
  which fails the bench's 100 kohm limit (the load node's 1.83 pF alone is
  87 kohm at 1 MHz, and the capacitance on PFB and NFB unbalances the bridge
  above that); at 1 kHz 5.927 to 5.867 Mohm (-1.0 %). The pump's loop at
  100 ohm: phase margin 20.25 to 13.82 degrees (below the bench's 15) and
  crossover 67.5 to 46.1 MHz, from the feedback nodes' pole (R3 parallel R4,
  5 kohm, with 1.57 pF on NFB is about 20 MHz); the buffer loop barely
  moves (90.0 to 89.9 degrees, crossover 400 to 413 MHz). The pole-zero
  bench turns unstable: two right-half-plane poles at 1.67 and 3.15 GHz at
  BUF, the follower's 11 nH of track against the pad and input capacitance,
  undamped because the tracks are lossless and the macro model's output has
  no resistance; not established as a property of the board (Open). Flatness,
  noise, matching and the step do not use layout and are unchanged. The test
  checks the refusals too: without `--layout` (`SIM118`, the bench without
  layout still passes), the same board unrouted (`SIM119`, DRIVE's pads in
  two pieces), the routed board of `examples/emi_return_path` (`SIM119`, not
  this design's), a run with no state file (`SIM118`).
- Decision 6: the seven examples with a release binary built before the
  changes and after: every file of every `--out` is byte-identical
  (`sim-results.json`, CSVs, pages; 7191 measure values and statuses by the
  comparison script), none of them uses
  layout.

Open: the analyzer (step 3, needs its layout); the Howland pump's own routed
board (PKG011 here; the proxy path's decoupling check); solder mask (outer
layers low by up to about 12 %); 3-D effects of the 2.5-D cuts (track ends,
corners, the field between a track end and a pad beyond it, crossings only
estimated), checked against no 3-D solver and not against the full-wave
crate; mutual inductance, skin effect, dielectric loss, via barrels (ideal
nodes) and the supply nets' own copper (one node each, so no rail
impedance); the lossless parasitics ring with amplifier models that have no
output resistance, so results above a few hundred megahertz are not
established (the poles above); SPEC.md's guard and mask-coupling figures
for the analyzer are not checked against extracted values yet; the bench
pages plot the run with the layout only, not the schematic curves beside it.

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

### I10. Solver gaps on vendor models

Found in I3 and its follow-ups; none is covered by an earlier phase.

- The operating point of an unpowered vendor amplifier: copper's Newton
  cycles on the ADA4817's supply mirror with every rail at 0 V, ngspice-46
  solves it. `Startup` cannot run on vendor models until this is fixed.
- The sparse LU refuses many vendor matrices and falls back to the dense
  one: 84 % of `ResistorMatching`'s time.
- Why Newton diverges 15 us into `Distortion` on vendor models, at every
  step size.
- A steady-state method for circuits whose models force steps far shorter
  than the period (harmonic balance, or shooting with fewer stored steps),
  if the two items above do not make `Distortion` runnable.
- Left from I6: a regulator's sinking side is unbounded; the op-amp
  transient current limit can run away; switch event times are not in the
  shooting sensitivity.

Acceptance: `Startup` reaches its first operating point on vendor models,
with a self-written deck that fails before the fix; the tolerance benches'
vendor run time stated before and after; the `Distortion` divergence
explained, fixed or not.

### I11. Mixed signal (proposal)

Not designed. The analyzer's benches drive the analog module with ideal
sources and treat the ADC as a source-impedance rule. A sampled ADC input
(switched capacitor, acquisition time, quantisation) and a DDS source with
its DAC steps would let a bench measure what the firmware will see. Scope
and syntax to be proposed before anything is built.

## Suggested first three

I0, I1, I2. They add no features. They decide whether the numbers of the
other phases can be believed.

## Open questions

- Reference data from ngspice in the repository (I1).
- Whether a rail budget is declared on the rail, on the supplying regulator,
  or both (I4).
- Whether I5 waits for the analyzer layout or proceeds on the Howland pump.
