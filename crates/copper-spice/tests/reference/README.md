# Reference comparison with ngspice

Phase I1 of the [simulation improvement plan](../../../../docs/simulation-improvement-plan.md).
Self-written SPICE decks (`decks/`), the results ngspice gives for them
(`results/`, CSV), and a test that runs every deck through copper-sim and
compares (`../reference_comparison.rs`).

**`copper` and its tests never run ngspice.** The test only reads the CSV
files. `generate.py` is run by hand, by a developer, when a deck is added
or changed; Python is needed for nothing else.

## Producing the results

ngspice-46 (shared library, creation date 14 April 2026), as KiCad 10.0
ships it: `C:\Program Files\KiCad\10.0\bin\ngspice.dll`, driven through
`ctypes` (`ngSpice_Init`, `ngSpice_Circ`, `ngSpice_Command`,
`ngSpice_AllPlots`, `ngSpice_AllVecs`, `ngGet_Vec_Info`). The library finds
no `spinit`, so no compatibility mode is selected (native ngspice
behaviour) and only the `spice2poly` code model is loaded (for `POLY`
sources), from `C:\Program Files\KiCad\10.0\lib\ngspice`. ngspice reports
SPARSE 1.3 as its linear solver. Its physical constants are CODATA 2014
(`const.boltz` = 1.38064852e-23), copper-sim's the exact 2019 values.

From the repository root, Python 3.13:

```
python crates/copper-spice/tests/reference/generate.py
python crates/copper-spice/tests/reference/generate.py crates/copper-spice/tests/reference/decks/diode.cir
```

The first regenerates every deck (about ten seconds), the second one deck.
`NGSPICE_DLL` and `NGSPICE_CODE_MODELS` name another library and code-model
directory. The first line of each CSV names the ngspice version and the deck.

For each analysis card of a deck and each temperature, the script runs the
deck's elements with that one card and `.temp T` and records the probed
quantities: `analysis,temp,quantity,x,re,im`, where `analysis` is
`index:kind` (the card's position in the deck), `x` the sweep value,
frequency or time (written exactly), and `re`, `im` the value to ten
significant digits. A noise card records `onoise` and `inoise` (V/sqrt(Hz)
or A/sqrt(Hz)). `v(a,b)` is written in quotes.

## Decks

A deck is an ordinary ngspice netlist. The test reads its elements as the
body of a subcircuit and runs its cards with copper-sim's own analyses at the
reference's sweep points. Comment directives:

| Directive | Meaning |
| --- | --- |
| `*@ probe v(a) v(a,b) i(vx)` | the quantities compared: node voltages, differences, currents of voltage sources |
| `*@ tran-probe ...` | the quantities of a transient, when they differ |
| `*@ temps -40 27 85` | the temperatures each card runs at (default 27) |
| `*@ tran-reltol 1e-6` | copper-sim's transient tolerance (default 1e-5) |
| `*@ ngspice-options badmos3` | `.options` for ngspice only |

A swept source names its `DC` value. Every deck sets `.options
reltol=1e-6 vntol=1e-9` (copper-sim's Newton tolerances) and a small
`abstol` and `chgtol`; copper-sim's reader notes and ignores them.

| Deck | What it covers |
| --- | --- |
| `diode` | forward, reverse, breakdown, `RS`, `CJO`, `TT`, `KF`; op, dc, ac, noise, tran (reverse recovery) |
| `bjt_npn`, `bjt_pnp` | Gummel-Poon with Early effect, high injection, `ISE`/`ISC`, `RB`, `RE`, `RC`, all capacitances, `XTF`/`VTF`/`ITF`, `XTB`; Gummel plot, output curves, y-parameters, noise of a stage, saturating switch |
| `bjt_base_resistance` | `RB` with `IRB`, `RB` with `RBM`; y-parameters, Gummel plot, noise |
| `bjt_substrate` | `CJS`/`VJS`/`MJS` of a vertical NPN, a lateral PNP (ngspice's default), a vertical PNP (`SUBS=1`), `TNOM = 50` |
| `jfet` | N and P channel, transfer and output curves, y-parameters, noise, switch |
| `mos1` | level 1 with body effect, Meyer gate charge, overlaps, area and perimeter junctions |
| `mos3` | level 3 with `VMAX`, `NFS`, `XJ`, `LD`, `WD`, `ETA`, `THETA`, `DELTA`, two `KAPPA`, without `VMAX`, PMOS |
| `mos3_badmos3`, `mos3_derivatives` | the two level-3 causes below |
| `switches` | `S` and `W` with hysteresis, dc and tran |
| `controlled` | `E F G H`, linear and `POLY` |
| `behavioral`, `behavioral_time` | `B` sources: `tanh`, `exp`, `sqrt`, ternary, `min`/`max`, `atan`, `pwr`, `u`, `I()`, `time` |
| `tline` | lossless lines with mismatched terminations |
| `temperature_tnom` | diode (`TNOM` 50), BJT (0), JFET (60, `XTI`, `TCV`, `BEX`), level 1 (60), level 3 from `NSUB` (50); `.dc TEMP` |
| `flicker` | `KF` of a diode, a BJT and a JFET in `.noise` |
| `noise_transient` | a diode's shot and 1/f noise, for the transient-noise test |
| `floor_rlc`, `floor_sallen_key`, `floor_ladder` | linear circuits on a 1 kHz sine, for the integration-floor test |
| `ce_amp`, `diffpair`, `cmos_inverter`, `rectifier`, `rlc`, `opamp_follower` | small circuits; the op-amp is a discrete two-stage BJT subcircuit in a follower |
| `pspice_behavioral`, `pspice_switch`, `pspice_ic`, `pspice_temperature`, `pspice_diode` | the PSpice dialect against ngspice's PSpice mode (`*@ dialect pspice`, `*@ ngbehavior psa`): `VALUE` sources with `IF`, `LIMIT`, `STP`, `MIN`, `MAX` and `.PARAM` expressions; `VSWITCH`; `.IC` inside a subcircuit; `TC1`/`TC2`, `T_MEASURED`, `T_ABS`; diode `ISR`/`NR` in forward bias (see "PSpice dialect") |

Device decks run at -40, 27 and 85 degC.

## The comparison

32 decks, 1119 compared series, 173 720 compared values. The bounds per
kind of quantity and their reasons are in the module documentation of
`reference_comparison.rs`: DC voltages 1e-5 + 1 uV, DC currents 3e-5 (the
`V_T` difference of the constants) + 1e-12 A, AC phasors and noise
densities 2e-4, transients 2e-3 of the waveform's range within one output
step (copper-sim at `reltol` 1e-5). A difference beyond its bound is either
fixed or listed below and in the test's `KNOWN` table with the size it may
reach; nothing is loosened. 128 series are listed.

`cargo test -p copper-spice --test reference_comparison -- --nocapture`
prints every series (worst error in units of its bound, where, both values).
`REFERENCE_DECK=name` runs the decks whose name contains `name`;
`REFERENCE_ROOT=dir` reads `dir/decks` and `dir/results` instead.

## Fixed

| Difference | Deck | Fix |
| --- | --- | --- |
| A BJT's `MJS` was accepted and ignored: the substrate capacitance stayed at `CJS` whatever the bias (2.1 times too large at -3 V) | `bjt_npn` | `MJS` read (`flatten.rs`) |
| ngspice makes a PNP lateral (substrate junction at the base) unless `SUBS=1`; copper-sim put it at the collector and refused `SUBS=-1` | `bjt_pnp`, `bjt_substrate` | `BjtParams::lateral`; spice3 dialect PNP lateral by default, `SUBS=±1` read; PSpice and LTspice keep vertical |
| BJT `G_MIN` leaks were in the transport current; SPICE3 puts them in the base currents (`cben`, `cbcn`): pA at the collector | `bjt_npn` | `semiconductors.rs` |
| Level 3 left `KAPPA` out of the saturation field `E_max` (SPICE3f5; ngspice adds it unless `.options badmos3`): 10 % in saturation at `KAPPA` = 0.2 | `mos3` | `semiconductors.rs` |
| Level 3 from `NSUB`: ngspice takes the intrinsic density at `TNOM`, `1.45e10 (T/300)^1.5 exp(E_g(T) / 2 V_T(1 K) (1/300 - 1/T))`, not the constant 1.45e10 /cm³: 0.4 mV of threshold at 27 degC, 92 mV of `PHI` at `TNOM` = 50 degC (reproduced to five digits) | `mos3`, `temperature_tnom` | spice3 dialect, level 3 only (`flatten.rs`) |
| ngspice's JFET divides the band-gap term of `IS(T)` by `N` (copper-sim's spice3 dialect did not) and its `(T/T_nom)^XTI` is not divided by `N` | `jfet`, `temperature_tnom` | spice3 dialect (`flatten.rs`) |
| ngspice's JFET has no grading coefficient: `M` is ignored (abrupt junctions) | `jfet` (`CGS` 7 % at 10 GHz) | spice3 dialect ignores `M` |
| ngspice's `pwr(x, y)` keeps the sign of `x` (PSpice's `pwrs`) | `behavioral` | spice3 dialect |
| A MOSFET's `KF` without `TOX` used `KF I^AF / f`; ngspice's default (`NLEV = 2`) is `KF g_m^2 / (f C_ox W L_eff)` with `TOX` = 100 nm when not given (1e10 times the density here; reproduced to four digits) | `flicker` (first version) | refused in the spice3 dialect, as `KF` with `TOX` already was in all |

None of these moved a result of the seven simulation examples: every
measure, `sim-results.json` and CSV is identical before and after.

## Listed differences

Established: shown by a test of its own in `reference_comparison.rs`, by a
variant deck, or by reproducing ngspice's number from a formula.

| Cause (`KNOWN`) | Where | Size | Established by |
| --- | --- | --- | --- |
| **base resistance linearisation** | `bjt_base_resistance` AC (`IRB`, or `RBM` without it) | up to 6 % of `y_21` at 85 degC, `y_12` 2.7 times | ngspice stamps `r_bb` as `1 / r_bb` at the bias point, without its derivative; copper-sim's law has its exact derivative. With `r_bb` frozen at copper-sim's bias value the AC results agree within 0.04 of the bound (`base_resistance_difference_is_its_linearisation`). copper-sim's is the derivative of the DC law; keeping it is a choice. |
| **substrate G_MIN** | `bjt_substrate` substrate currents, BJT collector currents of pA | `G_MIN` times the substrate junction voltage (0.35 to 7 pA) | ngspice-46 puts `G_MIN` across every BJT's substrate junction (its `ISS` diode); reproduced to 1e-3 (`substrate_difference_is_the_gmin_leak`). SPICE3f5 had none; not added, so every BJT keeps its element count. |
| **breakdown onset** | `diode` dc at 85 degC near `-BV` | at most about `IS` (25 pA) | copper-sim adds `IBV exp(-(v + BV) / N V_T)` at every reverse voltage; SPICE3 switches to its breakdown branch below `-x_BV` (where it carries `IS`) and has no breakdown current above. Established from the formulas; the difference is at the size of `IS`, invisible at 27 degC. |
| **reverse junction current** | JFET gate currents at 85 degC, `temperature_tnom` | up to 0.4 % of `IS` | below `-3 N V_T` SPICE uses `-IS (1 + (3 N V_T / (e v))^3)`, copper-sim the exponential; ngspice's gate currents reproduced to nine digits with the cubic. Diodes and BJTs have the same form in ngspice; their decks do not reach the bound. |
| **MOSFET gate charge: body-effect transcapacitance** | `mos1` gate admittance at 10 GHz | 15 to 33 % of its real part (0.2 % of the imaginary part) | copper-sim's Meyer charges depend on `v_bs` through `V_on`; ngspice's Meyer capacitances do not. With `RS = RD = 0` (no `v_bs` signal) they agree exactly (variant deck). |
| **MOSFET gate charge: ETA transcapacitance** | `mos3` gate current from a drain drive | 1.40 against 1.92 fF | the same through `V_on`'s dependence on `v_ds` (`ETA`); with `ETA = 0` they agree (variant deck). |
| **MOSFET gate charge: linear-region partition** | `mos3_badmos3` drain current at 1 GHz, linear region | 0.83 fF at 27 degC | copper-sim's `d q_gd / d v_g` against Meyer's `C_gd`: with ngspice's own `V_dsat` (1.39359 V), `V_dsat(V_on)` (0.02999 V) and `d V_dsat / d v_gs` (0.92654) read from the device, `(2/3) C_ox (b^3 + a b^2 + 4 a^2 b) / (a + b)^3 d V_dsat / d v_gs` minus ngspice's `C_gd` (6.8375 fF) is 0.8297 fF, the difference of the two drain currents to seven digits (22 November 2026). |
| **MOSFET gate charge in transient** | `cmos_inverter` | supply-current peak 7 to 8 %, output 0.35 % of the swing at the edge | copper-sim's charge-conserving partition against Meyer's capacitances, nothing else (variant decks at 27 degC, 22 November 2026): ngspice without `TOX` and with Meyer's `C_gs`, `C_gd`, `C_gb` as `i = C(v) dv/dt` sources reproduces its own native result (15.57 against 15.56 of the bound); ngspice without `TOX` and with copper-sim's three charge branches (each `dq/dt` through a 1 fF capacitor charged to `q`) reproduces copper-sim's (output 0.000 of the bound, supply current within it except 4.7 at the input's corner at 2.5 ns, where the gate current jumps); ngspice's peak is -0.5646 mA at maximum steps of 5 ps, 0.5 ps and 0.1 ps alike (its capacitance averaging is not the cause). With `v_bs = 0` and a level-1 `V_on` the only difference of the two forms is the linear-region partition. Meyer's form is not charge conserving: driven on gate and drain 90 degrees apart at 100 MHz, ngspice's MOSFET pumps 1.873e-14 C per period into the drain at 10 ps and at 1 ps steps, copper-sim's 1e-19 C. copper-sim's form is kept. |
| **level-3 derivatives** | `mos3`, `mos3_derivatives` AC at 1 kHz | `g_m` 7 %, `g_ds` 2.6 % | ngspice's hand-written level-3 derivatives (with `VMAX` and `ETA`) are not those of its own DC current: a fourth-order difference of ngspice's DC sweep equals copper-sim's exact `g_m` and `g_ds` to seven digits (`level3_difference_is_ngspice_derivatives`). |
| **level 3 without VMAX** | `mos3` `M7` | drain current up to 10 % (2.6 % at the bias point) | ngspice changes SPICE3f5's level 3 without `VMAX` unless `.options badmos3`; with that option the same devices agree in DC within the bound (`mos3_badmos3`). ngspice's changed equations are not reproduced. |

Differences that disappeared once found as the reference's own error: the
`jfet` and `opamp_follower` transients first used output steps (1 ns, 20
ns) at which ngspice's own result moved by 1 % when the step was refined;
both decks now use finer steps (0.2 ns, 2 ns) and agree within 0.03 of the
bound.

## Settled from the first plan

- **BJT substrate junction temperature scaling**: `CJS`, `VJS`, `MJS` over
  -40 to 85 degC and at `TNOM` = 50 agree with ngspice within the AC bound
  (`bjt_substrate`), vertical and lateral. (The `MJS` bug above was found
  here.)
- **Level-3 `KAPPA`**: SPICE3f5 leaves `KAPPA` out of `E_max`; ngspice (and
  the original model) has it, `.options badmos3` removes it. copper-sim now
  follows ngspice. PSpice was not available to compare.
- **Flicker noise**: diode, BJT and JFET `KF`/`AF` in `.noise` agree with
  ngspice to the bound at three temperatures (`flicker`). In transient
  noise, a diode's 1/f and shot noise over 10 s (Welch, octave bands from
  20 Hz to 640 Hz) is within 6 % of ngspice's noise analysis, inside a
  statistical bound of 7 to 29 % (`transient_flicker_noise_matches_the_noise_analysis`).
- **Integration floor**: on three more linear circuits the harmonics stay
  at or below `0.02 sqrt(reltol)` at `reltol` 1e-3 to 1e-6, at 0.01 to
  1.02 of it: Sallen-Key 0.54 to 1.02, ladder 0.08 to 0.54, RLC 0.01 to
  0.05 (`integration_floor_holds_on_more_linear_circuits`, which asserts at
  most twice the rule). The rule is an upper bound; it is not an estimate
  within a factor of 2 on every circuit (the RLC is 20 to 100 times below). The
  fundamental is within `(w h)^2 / 12` of ngspice's AC gain.

## PSpice dialect

ngspice's PSpice mode (`ngbehavior=psa`) is not PSpice: it reads PSpice
syntax and translates some constructs, but its devices keep ngspice's
equations. The five `pspice_*` decks compare what that mode runs as PSpice
does; all 47 series agree within their bounds (worst 0.28). Where it does
not, the comparison is against PSpice's documented closed form instead
(22 November 2026, each shown by a self-written deck in ngspice-46):

| Construct | ngspice's PSpice mode | Closed form in |
| --- | --- | --- |
| `E`/`G` `TABLE` | rounds the corners: 0.0125 V at the corner `(0,0)` of `(-1,-2) (0,0) (0.5,1.5)`, 0.95 V at the end point of a table that ends at 1 V; PSpice interpolates linearly. TI's TPS7A20 dropout table written as explicit linear segments agrees within the bound (0.053), the table itself 1.3 times beyond it | `spice_elements.rs`, `behavioural_value_table_b_and_functions` |
| `ISWITCH` | interpolates `ln R` linearly in the control current (42.2 kohm at 0.3 mA between `IOFF` 0.2 mA and `ION` 1 mA), not PSpice's cubic (74.3 kohm) | `spice_elements.rs`, `smooth_switches_follow_pspice_across_their_transition` |
| `VSWITCH` with `ROFF` below `RON` | open at `VOFF`, where PSpice's switch is `ROFF` | the same test |
| `VSWITCH` in `.ac` | the complex conjugate of the response: an 80 ohm closed switch into 20 nF gives `0.497 + 0.500j` at 100 kHz, the RC low-pass `0.497 - 0.500j` (copper-sim) | the RC itself |
| `C`/`L` `IC=` | ignored without `UIC`; PSpice holds the element there in the bias point | `vendor_constructs.rs`, `initial_condition_of_an_inductor_and_on_elements` |
| diode `IKF` | ngspice's knee `I / (1 + sqrt(I / IKF))`, not PSpice's `I sqrt(IKF / (IKF + I))`: 9.240e-5 A of ngspice's 9.208e-5 A at the `pspice_diode` bias, copper-sim 9.634e-5 A, PSpice's formula 9.634e-5 A | `device_and_netlist_coverage.rs`, `diode_recombination_and_high_injection` |
| diode `ISR` in reverse bias | 0.585 of PSpice's `ISR (exp(v / NR V_T) - 1) K_gen` at -1 V (27 and 85 degC); not established why | the same test |
| diode `ISR`, small signal | its conductance at 0.6 V is 0.26 % off the derivative of its own DC current (a central difference of its sweep); copper-sim's is the derivative | the same test |
| PNP | lateral (substrate leak `G_MIN` times 2.7 V across base and substrate), where PSpice's `PNP` is vertical (`LPNP` is its lateral type) | `bjt_substrate` compares the lateral form in the spice3 dialect |

The BJT's `IRB` is read the same in every dialect (compared in
`bjt_base_resistance`); quasi-saturation (`QCO`, `RCO`) is refused in every
dialect (`spice_elements.rs`).

The LTspice dialect has no reference on this machine and nothing of it is
compared with another simulator: `^` as exclusive or, `PWR` of its base's
magnitude, a vertical PNP, the JFET's `IS(T)` without `N` and with
`XTI = 0`, the JFET's `M`, `noiseless` resistors, `E a b c d LAPLACE =
H(s)`, and LTspice's readings of element `IC=` (refused, as in ngspice) and
of the diode's `ISR` and `IKF` (refused).

## Not covered

Lossy lines, `VDMOS`, BSIM, `ISS`, quasi-saturation, ngspice's `NLEV` noise
models of JFETs and MOSFETs outside saturation, the LTspice dialect and the
PSpice constructs above that ngspice runs differently (no PSpice or LTspice
to compare with), and transient analyses at other temperatures than the
decks' own.

## Vendor models

A model whose licence forbids committing it can be compared the same way,
locally: put a deck in `tests/reference/vendor/decks/` (gitignored) that
`.include`s the model from the model cache by its absolute path, run
`generate.py` on it (its results land in `tests/reference/vendor/results/`)
and run the ignored test:

```
cargo test -p copper-spice --test reference_comparison vendor -- --ignored --nocapture
```

Nothing from the vendor file or its results enters the repository.

Two directives serve these decks: `*@ dialect pspice` reads the deck in
copper-sim's PSpice dialect (spice3 without it), and `*@ ngbehavior psa`
runs it in ngspice's PSpice compatibility mode (`set ngbehavior=psa` before
the circuit is loaded; `unset` for every other deck, so the committed
results are unchanged). For a deck under `vendor/`, and for a run that
includes a deck with `*@ ngbehavior` (the `pspice_*` decks), the script also loads
the code models that mode translates PSpice constructs into (`analog.cm`
for `LAPLACE`, `xtradev.cm` with `digital.cm` for `VSWITCH`).

Plan phase I2 used five such decks (22 November 2026): TL431 fed through
1 kOhm (op and a dc sweep of the supply), TLV75718P at 3.45 V and 1 mA (op,
dc), OPA548 as a follower at +/-30 V (op, supply currents) and the OPA2182
and OPA2189 follower steps of `copper sim --check-model` (`*@ tran-reltol
1e-4`, the check's own tolerance): 19 series, 9229 values, all within their
bounds. What they and the variant decks behind them established is in
`docs/simulation-improvement-plan.md`, I2. Two cases are not comparable:
ngspice-46 in PSpice mode gives a false 100.45 V operating point for the
TL431 fed by a current source (the junction-current test copper-sim has,
see `newton.rs`), and it opens TI's TPS7A20 load switch, a `VSWITCH` whose
`ROFF` (1 uohm) is below its `RON` (1 Mohm), at the control voltage `VOFF`,
where PSpice's switch is `ROFF`.

Follow-up (22 November 2026): the two follower steps run at the
comparison's own `reltol` 1e-5 (the directive removed; OPA2189 0.280 of the
bound, OPA2182 0.426), and three decks were added: `tlv757_iq` (no load),
`tps7a20_iq` and `tps7a20_variant` (1 mA, input 1.5 V to 5 V). The TPS7A20
decks include `vendor/models/tps7a20_variant.lib`, TI's file with two exact
rewrites: the load switch with `RON` below `ROFF` (control reversed, `VON >
VOFF`, the same law in PSpice's formula) and the dropout `TABLE` as linear
segments (see "PSpice dialect"). 8 decks, 29 series, all within their
bounds. No `.ac` there: ngspice's PSpice-mode `VSWITCH` is conjugated in AC.
