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

Device decks run at -40, 27 and 85 degC.

## The comparison

27 decks, 1072 compared series, 168 932 compared values. The bounds per
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
| **MOSFET gate charge: linear-region partition** | `mos3_badmos3` drain current at 1 GHz, linear region | 0.95 fF | probable: copper-sim's source-drain split adds a transcapacitance of at most `C_ox / 6` (2.9 fF here) in the linear region; not isolated further. |
| **MOSFET gate charge in transient** | `cmos_inverter` | supply-current peak 7 to 8 %, output 0.35 % of the swing at the edge (1.4 % against a ten times finer ngspice step) | the intrinsic charge: without `TOX` the transient agrees within 0.7 of the bound, without `GAMMA` it does not (variant decks). Which of the charge-partition terms and ngspice's capacitance averaging (not charge conserving) causes it is not established. |
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

## Not covered

Lossy lines, `VDMOS`, BSIM, `ISS`, quasi-saturation, ngspice's `NLEV` noise
models of JFETs and MOSFETs outside saturation, the PSpice and LTspice
dialects (no PSpice or LTspice to compare with), and transient analyses at
other temperatures than the decks' own.

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
