# Soil impedance analyzer

The soil impedance analyzer of [SPEC.md](SPEC.md) as one CopperScript board:

| File | Content |
| --- | --- |
| `board.copper` | Board `ImpedanceAnalyzer`: the digital and power section (STM32G474, AD9833 DDS, battery charger with power path, every rail, the 1.65 V mid-supply reference) and the analog front end as module instance `AFE` |
| `analog/front_end.copper` | Module `AnalogFrontEnd`: DDS filter, Howland pump, electrodes, guards, sense buffers, receiver, ADC drivers, transimpedance amplifier, probe header |
| `parts/` | Example-local parts: `analog_*`, `digital_*` and `power_*` files; the board and the module both import the directory |
| `benches/` | Seventeen benches, each `bench NAME for AnalogFrontEnd`, and four fixtures that hold their shared setup ([Benches](#benches)) |

Status, 19 November 2026: schematic, ERC, power-state check, P0 benches and
the P1 and P2 benches (noise, Monte Carlo and worst case, start-up and
distortion in time). No layout, so every stray capacitance below is an
estimate from the SPEC's geometry. All seventeen benches pass. The board meets
1 % and 1 degree only over a rated range: soil up to 100 ohm m, sense contacts
within 100 ohm of each other, and layout strays bounded (SPEC.md, "Usable
range"). Each sense node's 1.6 pF against the contacts sets that limit; the
owner accepted it, and the circuit is unchanged. A bench steps over the rated
range and beyond it; its accuracy limits apply to the rated steps only (a
`where` on the limit), and the other steps are reported without them.
`SenseBufferTrade` is a trade study of a part the board does not use.
Electrical evidence only: placement, routing, footprints, converter stability
and thermal behaviour are not qualified, and every current and battery figure
is an estimate.

## Running

```console
target/debug/copper check examples/simulation/impedance_analyzer/board.copper
target/debug/copper check --pins error examples/simulation/impedance_analyzer/board.copper
target/debug/copper power-check examples/simulation/impedance_analyzer/board.copper
target/debug/copper sim examples/simulation/impedance_analyzer/board.copper --out build/sim
target/debug/copper sim examples/simulation/impedance_analyzer/board.copper --bench SoilSweep
target/debug/copper check examples/simulation/impedance_analyzer/board.copper --sim-results build/sim/sim-results.json
target/debug/copper power-check examples/simulation/impedance_analyzer/board.copper --sim-results build/sim/sim-results.json
```

```text
OK: ImpedanceAnalyzer passed ERC
OK: ImpedanceAnalyzer passed ERC
OK: ImpedanceAnalyzer passed power-state analysis
```

`copper sim` prints each measure's worst step and writes `sim-results.json`,
one CSV and one HTML page per bench into `--out`. The exit status is 0: every
limit holds. The accuracy limits (1 % and 1 degree) apply over the rated range
only; the headroom limits (pump swing, sense input common mode, ADC linear
range) apply at every step of every bench that has them. A full run took
31 minutes here (with other jobs on the machine), most of it `Distortion`
(about 19 minutes) and `ResistorMatching` and `ResistorWorstCase` (about 6
and 7). ERC and the
power-state check cover the whole board; the benches cover the analog front
end.

## Structure: the front end is a module, the benches simulate it alone

The board is the digital and power section, drawn flat, plus one instance of
the module `AnalogFrontEnd`. A bench must model, replace or ignore every
component of what it simulates, so a bench of the whole board would have to
ignore the roughly 110 digital and power components and stand in for the
rails they make, in every bench. Instead every bench targets the
module (`bench SoilSweep for AnalogFrontEnd`): `copper sim board.copper` finds
the benches beside the board, and each simulates the module's own components
and nets under their local names, with its ports as nets. The bench supplies
what the board around it would:

| Port | Board net and source | In the benches |
| --- | --- | --- |
| `DDS_OUT` (input) | AD9833 VOUT, unfiltered: 38 mV to 0.65 V, 200 ohm on chip; off between measurements | `model DDS: source { out = DDS_OUT; ref = GND; ac = 0.3V; output_resistance = 200ohm; }` (0 V AC where a bench drives another node) |
| `V_MON_1`, `V_MON_10`, `V_MON_100` (outputs) | sense voltage at gain 1, 10, 100 into PA1 (ADC2_IN2), PA6 (ADC2_IN3), PA7 (ADC2_IN4), fast channels | open: the ADC input is not modelled (its 470 pF reservoir, 1 nF on `V_MON_1`, is in the module) |
| `I_MON` (output) | current measurement into PA2 (ADC1_IN3), fast channel | open |
| `V_MID` (power in) | 1.65 V, REF2033 VBIAS, from VP: switched | `supply MID { voltage = 1.65V; net = V_MID; }` |
| `VP`, `VN` (power in) | +4.50 V (TPS7A4901, 150 mA) and -4.48 V (TPS7A3001, 200 mA): switched | `supply POS`, `supply NEG`, ideal DC sources |
| `V3V3_ANA` (power in) | switched 3.3 V of the ADC drivers and the transimpedance amplifier: `DDS_SW` (TPS22918 `U_SW_DDS`, on with `ANA_EN`) through ferrite `FB_ANA`, 10 uF `C_ANA_BULK` | `supply ANALOG_3V3` |
| `GND` | ground | `supply GROUND`, the simulation ground |

The benches therefore exercise exactly the module definition the board
instantiates; what they replace is its surroundings, and the results list
those stand-ins as bench supplies and a bench model. The ideal DC supplies are
AC shorts, as the standalone analog board's external supplies were. `NEG` is
-4.48 V, the board's LDO value, where the standalone board said -4.5 V; the
AC models do not depend on rail voltages (the transient ones clip at them).
These port conditions are the fixture `FrontEndPorts`; `Startup` replaces the
supplies by models of the board's regulators. Converting the benches from the
standalone analog board (whose placeholder header the benches replaced by the
DDS source) changed no result: all 488 per-step measure values and every
plotted vector are bit-identical before and after; only the names of the
stand-ins in the model lists changed.

The shared setup then moved into fixtures ([Benches](#benches)) and each
rated bench was merged with its study bench (limits scoped with `where`). That
changed no number either: matching steps by their params, all 2722 per-step
measure values of the fourteen benches before (`SoilSweepDry` into
`SoilSweep`, `CalibrationCardStudy` into `CalibrationCard`,
`ContactImbalanceStudy` into `ContactImbalance`, `StrayCapacitanceStudy` into
`StrayCapacitance`, `SenseCommonModeDry` into `SenseCommonMode`) and all
27199 plotted values are bit-identical in the nine benches after, which add
steps and measures but change none.

The two placeholder headers that joined the halves before (`J_BOUNDARY` on
both standalone boards) and their part are gone. The conditions the board side
sets for the front end:

- **ADC input voltage.** PA1, PA2, PA6 and PA7 are TT_a pins: absolute
  maximum -0.3 V to 4.0 V, positive injection not allowed (DS12288 Tables 14
  and 15). The ADC drivers run from `V3V3_ANA`, which is at most `V3V3`, so a
  powered driver cannot leave 0 V to 3.3 V.
- **Source impedance.** At most 100 ohm with 2.5-cycle sampling (Table 70):
  49.9 ohm plus the driver's closed-loop output impedance on PA2, PA6 and
  PA7, 100 ohm plus it on PA1 (`V_MON_1`). The ADC timing (3.125 Msps,
  16 ADC clocks a trigger) leaves no room for a longer sampling time.
- **V_MID loading.** The front end adds no capacitance to `V_MID` (three
  47 kohm bias resistors and one amplifier input); the board's 1.1 uF there is
  the REF2033's output capacitor, well inside its 0 to 10 uF.
- **Current.** At most 70 mA per rail (51.8 mA typical, 56.9 mA maximum
  drawn; simulated with typical parts: 51.8 mA idle, at most 52.3 mA while
  driving 1 MHz, `SupplyCurrent`).
- **Nothing on the always-on `V3V3`.** The front end's 3.3 V loads are on
  `V3V3_ANA`, which is off in sleep.

## Firmware rules

Rules the hardware relies on. `copper power-check` cannot check them: the
AD9833 and the front end's amplifiers declare no power domains, so the tool
sees no back-power path through them.

1. **DDS lines before `ANA_EN` falls.** Drive SCLK (PB3), SDATA (PB5), FSYNC
   (PA4) and MCLK (PA8) low, or set them to analog mode, before `ANA_EN`
   (PB10) goes low, and keep them so while it is low. Otherwise the MCU feeds
   the unpowered AD9833 through its input clamps. FSYNC is on PA4 because PA15
   has a pull-up at reset (DS12288 Table 12 note 4).
2. **ADC inputs stay analog.** PA1, PA2, PA6 and PA7 stay in analog mode
   (no output, no pull-up) whenever `ANA_EN` is low, Standby included: in
   Standby leave their bits in PWR_PUCRA clear (a pull-down is harmless but
   not needed). Each pin faces an unpowered ADA4807 output through 49.9 ohm
   (PA1 through 100 ohm);
   a pin driven high or pulled up would push current through that output's
   protection diode into `V3V3_ANA` and from there into the AD9833 and the
   other three ADC drivers. In analog mode the only current is the pin's
   leakage.
3. **Program the DDS after the rails are up.** The AD9833 shares the switched
   3.3 V with the front end; write its registers after `ANA_EN` rises and its
   supply has settled, and start sampling about 1 s after `ANA_EN` rises (every
   ADC input is within 1 mV of its final value 0.68 s after it, `Startup`; see
   [Power cycling](#power-cycling)).
4. **Gain range.** At 1.3 kohm m the gain-10 channel is past its linear range;
   the firmware uses gain 1 there ([SoilSweep above the rated range](#soilsweep-above-the-rated-range)).
5. **Power and charging.** PB11 (buck-boost PS/SYNC) high while measuring, low
   in sleep; EN1 (PB13) high only once USB is enumerated; battery voltage on
   PB1 with PB0 high for at least 25 ms before sampling; stop measuring and go
   to Standby below about 3.2 V.

## Analog front end: the circuit

Module `AnalogFrontEnd` in `analog/front_end.copper`; on the board its
components are `AFE/...` (`AFE/U_DRV`), in the benches they keep their local
names.

| Stage | Parts and values | Why |
| --- | --- | --- |
| Reconstruction filter | `C_F1` 270 pF, `L_F1` 22 uH, `C_F2` 270 pF, `R_TERM` 200 ohm | 3rd-order Butterworth, 3 MHz, between the AD9833's own 200 ohm and `R_TERM`. The termination halves the DDS's 0.3 V peak open-circuit amplitude to 0.15 V peak, so no separate divider is needed. About 54 dB down at the 24 MHz image. Its phase does not matter: the current is measured. |
| Drive buffer | `C_IN` 1 uF, `R_INB` 10 kohm (16 Hz), `U_DRV` ADA4807 follower | The pump's R1 must see a source far below 0.01 % of 10 kohm. |
| Improved Howland pump | `U_PMP`, `U_FBB` ADA4807; `RN_HOW` LT5400-1 (four 10 kohm, 0.0125 % matched); `R_SET` 10 kohm 0.1 %; `C_CMP_N`, `C_CMP_P` 2.2 pF across R4 and R2; `C_NDC` 4.7 uF in series with R3 | `I = V(DRV_BUF) / R_SET` = 15 uA peak. Equal capacitors across R4 and R2 keep the bridge balanced. `C_NDC` gives the pump unity DC gain, so offsets cannot walk the regulated node to a rail (see [DC path](#dc-path)). |
| Blocking and HC guard | `C_BLK` 100 nF (C0G or film); `U_FBB`'s output drives `GUARD_HC` through `R_GHC` 49.9 ohm | No DC through the soil. The guard driver is the pump's own feedback buffer; 49.9 ohm isolates its 120 pF guard load (with 0 ohm the guard tracking peaks to 376 %). |
| Sense buffers | `C_CHP`, `C_CLP` 4.7 nF; `R_BHP`, `R_BLP` 33 Mohm (0.15 s, 1 Hz); `U_BHP`, `U_BLP` ADA4817 followers; each drives its guard through 49.9 ohm | FET input, lowest input capacitance on the board (1.3 pF). This capacitance, not the guard, limits dry-soil accuracy (see [Results](#results)). |
| Differential receiver | `U_DIF` AD8130, gain 1 (FB to OUT, REF to ground) | High input impedance on both inputs and 80 dB minimum CMRR at 2 MHz. |
| ADC drivers | `U_B1` gain 1, `U_G10` gain 10.09 (9.09 k / 1 k, `C_G1G` 22 uF for unity DC gain), `U_G100` another gain 10.09 on `V_10`; `C_G1F`, `C_G2F` 2.2 pF (C0G) across each 9.09 k; all ADA4807 on 0 V / `V3V3_ANA` (the switched 3.3 V); inputs AC-coupled with 1 uF and biased at `V_MID` through 47 kohm (3.4 Hz), 1 kohm series into each input | Powered from 0 V and at most the MCU's 3.3 V, no output can leave the ADC's range in any condition, saturation included. Each ADC input has its own channel; no analog switch. The 2.2 pF give each gain-10 stage's loop phase lead (8 MHz with 9.09 k), for the margin the vendor model needs (SPEC.md, "Vendor models"); they also make 8 MHz the stage's closed-loop pole: 5.8 MHz to -3 dB, 98.5 % and -9.3 degrees at 1 MHz (an untracked scratch bench, `ChannelCosts`). |
| Transimpedance amplifier | `U_TIA` ADA4807 on 0 V / `V3V3_ANA`, `R_TIA` 20 kohm 0.1 %, `C_TIA` 10 pF (`C_FB_TIA`); non-inverting input at `V_MID` | LC sits at `V_MID` (no DC current: every other electrode is capacitor-coupled). 0.3 V peak at 15 uA. 10 pF gives a 0.8 MHz pole and no peaking against 21 pF at LC; it was 1.5 pF (5.3 MHz) until the vendor model's margin needed more (SPEC.md, "Vendor models"). The cost is at 1 MHz: the stage passes 62.5 % with 52 degrees of lag, and with its anti-alias RC the channel 61.9 % with 60.7 degrees (1.5 pF: 98.6 % and 19.7 degrees; `ChannelCosts`), so `I_MON` carries at most 0.19 V peak at 1 MHz instead of 0.30 V (`MeasurementNoise`). Calibration removes the response, not the lost signal. |
| ADC interface | 100 ohm into 1 nF at `V_MON_1`; 49.9 ohm into 470 pF at `V_MON_10`, `V_MON_100`, `I_MON` | Anti-alias poles 1.59 MHz on the gain-1 channel and 6.8 MHz on the other three; the capacitors are the charge reservoirs for 2.5-cycle sampling. The gain-1 network filters the follower's own closed-loop peak at the pin, which the vendor model needs (SPEC.md, "Vendor models"); it was 49.9 ohm into 470 pF like the others. The cost is at 1 MHz: the gain-1 channel passes 84.7 % with -32.8 degrees (-3 dB at 1.59 MHz), the gain-10 channel 97.5 % with -18.2 degrees and the gain-100 channel 96.1 % with -28.0 degrees (with 49.9 ohm and without the 2.2 pF: 98.9 % and -9.1, 98.9 % and -11.8, 98.8 % and -15.2 degrees; `ChannelCosts`, 800 ohm soil, ADC pin against the receiver output). Calibration removes each channel's own response. Source impedance at the pin: 49.9 ohm plus the amplifier's closed-loop output impedance on `V_MON_10`, `V_MON_100` and `I_MON`, below the 100 ohm the STM32G474 needs with 2.5-cycle sampling; 100 ohm plus it on `V_MON_1`, at that limit. |
| Decoupling | 100 nF at every supply pin (role `decoupling`), 10 uF bulk on VP and VN | ADA4817 data sheet p. 17. |
| Probe access | `J_PROBE` 1 x 8: HC, guard HC, HP, guard HP, LP, guard LP, LC, ground | Each electrode beside its guard, so a coaxial lead can take the guard on its shield. |

The front end puts no capacitor and no DC load on `V_MID`: three 47 kohm bias
resistors and the transimpedance amplifier's input. The 1.1 uF on `V_MID` is
the REF2033's own output capacitance on the board (the part allows up to
10 uF).

### Supply current

| Rail | Parts | Typical | Maximum |
| --- | --- | --- | --- |
| +4.5 V and -4.5 V (each), while measuring | 2 x ADA4817 (19 / 21 mA), AD8130 (10.8 / 11.6 mA), 3 x ADA4807 (1.0 / 1.1 mA) | 51.8 mA | 56.9 mA |
| `V3V3_ANA` (switched 3.3 V), while measuring | 4 x ADA4807 (0.915 / 1.0 mA at 3 V) | 3.7 mA | 4.0 mA |
| every front-end rail between measurements | off with `ANA_EN` | 0 | 0 |

Quiescent currents from the data sheets. The hand estimate said signal
currents add under 2 mA (the guard drivers charge about 120 pF each at
1 MHz). `SupplyCurrent` simulates it: every amplifier draws its typical
quiescent current (the values in the table) and the current its output
delivers, from the rail that sources it:

| Rail | Idle (operating point) | Driving 1 MHz, 0.3 V DDS, dry / moist soil | Driving 1 MHz, 0.65 V DDS, dry soil |
| --- | --- | --- | --- |
| +4.5 V (`I(POS)`), mean | 51.80 mA | 52.02 / 51.89 mA | 52.28 mA (peak 53.2 mA) |
| -4.5 V (`-I(NEG)`), mean | 51.81 mA | 52.02 / 51.89 mA | 52.28 mA (peak 53.2 mA) |
| `V3V3_ANA`, mean | 3.66 mA | 5.57 / 5.10 mA | 6.04 mA |
| `V_MID` | -4.7 uA (the ADC drivers' and TIA's bias currents flow back into the REF2033) | | |

What the simulation confirms and corrects:

- **+/-4.5 V: confirmed.** Idle, the simulated current is the sum of the
  typical quiescent currents to 1 uA (the outputs carry almost no DC). The
  signal current at 1 MHz, the worst frequency, is 0.09 to 0.22 mA at the
  nominal drive and 0.48 mA at the top of the DDS range, a quarter of the
  "under 2 mA" estimate: each rail carries only the half-waves its outputs
  source. The typical load with signal is therefore 52.0 mA, the maximum
  (data-sheet maximum quiescent currents, not simulated) about 57.4 mA, both
  inside the 70 mA the rails are sized for (the bench's limit).
- **`V3V3_ANA`: corrected for 1 MHz.** The idle 3.66 mA matches the hand
  value (4 x 0.915 mA), but driving 1 MHz the four ADC-side amplifiers source
  1.4 to 1.9 mA more at the nominal drive (2.4 mA at 0.65 V): each drives
  470 pF (339 ohm at 1 MHz) through 49.9 ohm, the gain-1 driver 1 nF through
  100 ohm, besides its feedback network. The
  single-supply amplifiers sink into ground, so only the sourcing half-waves
  reach the rail. The 470 pF take a tenth of that current at 100 kHz and a
  thousandth at 1 kHz (not simulated). The battery budget below keeps 3.7 mA:
  a continuous 1 MHz measurement would add about 2 mA at 3.3 V, about 2 mA
  from the cell (under 1 % of the measuring current; estimate).
- **`copper power-check`** passes the board (`MEASURE` and `SLEEP`). On its
  own it checks power states and back-powering only; with `--sim-results` it
  compares the simulated rail currents with the budgets the board declares:
  `current_max = 70mA` on VP and VN (what the TPS65130 and the post-filters
  are sized for), and the parts' ratings on the rails' sources (TPS7A4901 OUT
  150 mA, TPS7A3001 OUT 200 mA, REF2033 VBIAS 20 mA). VP's total, every load
  on it in `PowerSide`, is 52.17 mA at DC (the front end's 51.8 mA, the
  REF2033's 0.36 mA, the LDO's divider); the front end's worst share is a
  mean of 52.28 mA and a peak of 53.2 mA (`SupplyCurrent`), inside 70 mA.
  V3V3_ANA has no budget: its only feed, FB_ANA, declares no rated current.
  VPOS_F and VNEG_F are declared at a 70 mA drop across the 1 ohm
  post-filters (5.27 V, -5.17 V), where the simulated 52.3 mA gives 5.29 V
  and -5.19 V, so the declared values are conservative by 18 mV.

The first version used eight
ADA4817s and drew 165 mA per rail. 51.8 mA misses the battery target of about
40 mA per rail but stays under the 70 mA ceiling the rails are sized for; the
difference is the two sense buffers, and the trade below is why they stay. The
battery budget with these currents is under
[Current budget and battery life](#current-budget-and-battery-life).

| Position | Part, current per rail | Why it is sufficient, or why it is needed |
| --- | --- | --- |
| Sense buffers (2) | ADA4817, 19 mA each | Needed. They set the sense nodes' capacitance and the sense path's common-mode rejection. The candidate at a fifth of the current, the OPA810 (3.7 mA, FET input, 70 MHz), would bring the rails to 21.2 mA typical / 24.1 mA maximum, but `SenseBufferTrade` shows its 2 pF input doubling the moist-soil error (100 ohm m, eps_r 25: 0.76 % / 0.13 deg against 0.38 % / 0.11 deg) and raising the dry-soil error (1 kohm m: 2.8 % / 7.9 deg against 1.8 % / 4.9 deg), and its CMRR falls to about 42 dB at 1 MHz (its Figure 7-50). With two units the path's 63 dB at 1 MHz would rest on how well two parts' common-mode errors match, which no data sheet bounds (a 10 % mismatch leaves about 62 dB; hand estimate). |
| Receiver | AD8130, 10.8 mA | Needed: both inputs high impedance and 80 dB minimum CMRR at 2 MHz; no lower-power receiver with a guaranteed CMRR at MHz was found. |
| Pump `U_PMP`, `U_FBB` | ADA4807, 1 mA each | The pump's output impedance at 1 MHz is set by the strays on HC (87 kohm, `PumpOutputImpedance`), not by gain-bandwidth, and the current is measured. `LoopPeaking` pump_current 5.3 % peaking. The rail-to-rail input also removes the ADA4817's +VS - 2.8 V input limit from the pump node. |
| Drive buffer | ADA4807, 1 mA | Unity gain at 1 MHz; 1.8 % peaking. |
| ADC drivers (gain 1, 10, 10) | ADA4807 on `V3V3_ANA`, 0.9 mA each | 5.8 MHz closed-loop bandwidth at gain 10 with the 2.2 pF across 9.09 kohm (23 MHz without); the 9.3 degrees it costs at 1 MHz (3.0 without) are calibrated out (the errors below are identical on all gain channels). |
| Transimpedance | ADA4807 on `V3V3_ANA`, 0.9 mA | Bipolar input current (1.6 uA max) flows through `R_TIA` only: 32 mV of DC. No peaking with 10 pF; the 0.8 MHz bandwidth it gives costs signal at 1 MHz (see the circuit table above). |

### Power cycling

Every rail of the front end switches with `ANA_EN`: VP and VN (and with them
`V_MID`, whose REF2033 runs from VP) and `V3V3_ANA`, the 3.3 V of the four
ADC drivers and the transimpedance amplifier. The microcontroller and its
`V3V3` stay on. With `ANA_EN` low every amplifier of the front end is
unpowered, and the TPS22918's quick output discharge pulls `DDS_SW`, and
through the ferrites `V3V3_ANA` and the AD9833 supply, to 0 V. Checked against
the absolute maximum ratings read from the data sheets:

| Connection with the front end off | Level | Rating | Result |
| --- | --- | --- | --- |
| ADC pins PA1, PA2, PA6, PA7 (MCU powered) to the unpowered ADA4807 outputs, through 49.9 ohm (PA1 100 ohm) | 0 V while the pins are in analog mode; current only the pins' leakage, out of the pin | ADA4807 output: no rating read for an unpowered output; its protection conducts above +VS = 0 V | inside with firmware rule 2; a pin driven high or pulled up (25-55 kohm) would back-power `V3V3_ANA` through that diode |
| DDS output into `U_DRV` through the filter, 1 uF and 10 kohm | AD9833 unpowered: 0 V | inputs -VS - 0.2 V to +VS + 0.2 V (ADA4807 Table 5) | inside with firmware rule 1 (no DDS line driven) |
| `V_MID` into the transimpedance amplifier and, through 47 kohm, the ADC drivers | 0 V (REF2033 off with VP) | ADA4807 inputs -0.2 V to +VS + 0.2 V | inside in sleep |
| Sense buffers (ADA4817), AC-coupled to the electrodes | electrodes at 0 V: LC follows the unpowered TIA input | -VS - 0.5 V to +VS + 0.5 V | inside |
| AD8130 inputs | driven by the unpowered sense buffers, about 0 V | its input absolute maximum was not read | open item |
| Always-on `V3V3` to any front-end rail | no connection: `V3V3_ANA` comes only through `U_SW_DDS`, which blocks when off | | no back-powering path |

The change from the two standalone boards: the ADC drivers and the
transimpedance amplifier used to stay powered from the always-on 3.3 V
(3.7 mA typical, which would have been 90 times the sleep current). On the
merged board they switch with the rest of the front end, so in sleep the
ADC pins face unpowered amplifiers instead of powered ones, which is what
firmware rule 2 covers. Of the two transitions the first is simulated
([Startup](#startup-pass-settled-within-1-s)), the second is a hand estimate:

- **`ANA_EN` rises.** `V3V3_ANA` comes up through the load switch within its
  rise time (CT open; tens of microseconds, not read), long before the
  converters' soft-start brings up VP, VN and `V_MID` (simulated with models of
  the regulators: VN leaves 0 V at 0.57 ms and regulates at 9.3 ms, VP 0.73 ms
  and 14.3 ms, `V_MID` 5.9 ms and 10.7 ms, the REF2033 regulating only from
  VP = 3.32 V). The 3.3 V amplifiers then run for a while with `V_MID` at 0 V:
  their inputs sit at 0 V, inside -0.2 V (simulated: the ADC drivers' inputs
  reach -0.17 V while the receiver's output settles). The AD9833 comes up with
  them, and its output's DC level reaches `U_DRV`'s input, 0.18 V above its
  unpowered rails in the simulation (inside the 0.2 V rating; see
  [Startup](#startup-pass-settled-within-1-s)).
- **`ANA_EN` falls.** The 52 mA load discharges VP's roughly 20 uF in a few
  milliseconds, and `V_MID` falls with VP; `V3V3_ANA` falls through the
  4 mA load and the switch's output discharge. If `V3V3_ANA` fell faster than
  `V_MID`, `V_MID` would exceed it by more than 0.2 V for a moment and drive
  current into the transimpedance amplifier's non-inverting input, which
  connects to `V_MID` without a series resistor (the ADC drivers have
  47 kohm). The estimate says VP and `V_MID` fall first, but it rests on
  discharge rates nobody has measured; see
  [Unresolved and doubtful](#unresolved-and-doubtful).

Start-up, the hand calculation that `Startup` now checks. When the rails and
`V_MID` come up, LC steps to 1.65 V and the soil follows; every coupling
capacitor then charges:

| Network | Time constant | Notes |
| --- | --- | --- |
| Sense inputs: 4.7 nF, 33 Mohm | 155 ms | was 10 nF, 100 Mohm: 1 s. The 1.65 V step reaches the buffer inputs and decays with this; with dry-soil signal on top it is outside the ADA4817's input range (+VS - 2.8 V) for about 1.4 time constants. 33 Mohm loads a 75 kohm contact by 0.2 % (an uncalibrated gain error). |
| Blocking capacitor: 100 nF through the pump's 20 kohm DC resistance and the soil (up to 160 kohm) | 2 to 18 ms | was 1 uF: up to 180 ms. At 1 kHz, 100 nF drops 24 mV at 15 uA. |
| Pump DC servo: `C_NDC` 4.7 uF with R3 10 kohm | 47 ms | was 22 uF: 220 ms |
| Drive coupling: 1 uF, 10 kohm | 10 ms | |
| ADC drivers: 1 uF, 47 kohm; gain-stage DC blocks 22 uF, 1 kohm | 47 ms, 22 ms | |

The slowest is 155 ms. Five time constants, 0.8 s after the rails and `V_MID`
are up, leave under 1 % of the step (simulated: every ADC input within 1 mV
after 0.68 s, the gain-100 channel last; `Startup`), which the I/Q demodulation over whole
periods of a 1 kHz or faster signal rejects further: a reading is valid about
1 s after switching the rails on (firmware rule 3; the simulated 0.68 s
includes the rails' soft-start, from models of the regulators). The shorter time constants change the
bench results by at most 0.1 percentage points; `PumpOutputImpedance` at 1 kHz
falls from 29 Mohm to 6.2 Mohm, still four times its limit.

### DC path

SPEC.md proposed 10 Mohm from the regulated node to ground. The improved Howland
pump turns its input offset into an output current, `2 V_OS / R_SET` (0.4 uA at
the ADA4817's 2 mV maximum), and 10 Mohm turns that into up to 4 V. Instead
`C_NDC` sits in series with R3: below its 3.4 Hz corner the pump has unity
gain and a 20 kohm output resistance, so the node sits at the offsets
(`DcOperatingPoint`: +24 mV), while at 1 kHz the output impedance is still
6.2 Mohm (22 uF gave 29 Mohm; 4.7 uF was chosen for start-up, see
[Power cycling](#power-cycling)).

## Benches

Every bench targets the module (`bench NAME for AnalogFrontEnd`). The setup
they share lives in four fixtures in `benches/`, which a bench names with
`using` and overrides where it differs (a bench's own `param`, `model` or
component of the same name takes the fixture's place):

| Fixture | Holds | Used by |
| --- | --- | --- |
| `FrontEndPorts` | The port conditions of [Structure](#structure-the-front-end-is-a-module-the-benches-simulate-it-alone): the rails as ideal DC sources, the DDS as a 0.3 V source with 200 ohm, the electrode pads ignored | every bench but `Startup`, which takes its rails from models of the regulators |
| `SoilModel` | The soil model of SPEC.md and the board strays ([Strays](#strays)), the headroom params | every bench with soil |
| `FirmwareReadout` | What the firmware computes: `z_true`, `i_lc`, `i_meas`, `z1`, `z100` | the benches that compute the impedance |
| `MatchingTolerances` | The tolerances, the LT5400 rebuilt as a matched group, the reference runs of `ResistorMatching` | `ResistorMatching`, `ResistorWorstCase` |

The soil model: a contact resistance in series with each electrode, the
transfer impedance between the two sense points (`S_HP`, `S_LP`), and a
capacitor across each. For 8 mm pads (equivalent radius 4.5 mm) on a 20 mm
pitch the contacts are 6.98 times the transfer resistance and every resistor R
carries `eps0 eps_r rho / R` (derived params, so the contacts follow the
stepped `R_TRANSFER`). The firmware's result is
`(V_MON / gain) / (I_MON / R_TIA)`; it is calibrated with a `reference` run on
the card's 1 kohm with 0 ohm contacts, which removes each channel's own
frequency response (about 28 degrees at 1 MHz with the gain-1 channel: the
current channel's 60.7 degrees of lag, mostly the TIA's 0.8 MHz pole, less
the gain-1 channel's 32.8 degrees).

| Bench | Analysis | Question |
| --- | --- | --- |
| `SoilSweep` | AC | Impedance error after calibration, 5 ohm m to 1.3 kohm m, rated to 100 ohm m |
| `CalibrationCard` | AC | The card's positions, rated and not |
| `ContactImbalance` | AC | Unequal contacts, rated (100 ohm, 200 ohm) and up to 10 times |
| `StrayCapacitance` | AC | Residual and mask capacitance, rated (1 pF, 0.2 pF) and up to 100 pF |
| `SenseCommonMode` | AC | Common-mode rejection of the sense path, rated contacts and drier soil |
| `PumpOutputImpedance` | AC | The pump's output impedance and guard tracking, electrodes in air |
| `LoopPeaking` | AC | Peaking of every loop to 1 GHz |
| `LoopStability` | loop gain, poles | The phase margin of every amplifier's loop, and the poles of the front end in soil |
| `GuardStability` | poles, loop gain | Whether the HC guard loop is stable with the electrodes in air |
| `SenseBufferTrade` | AC | Trade study: OPA810 sense buffers |
| `DcOperatingPoint` | operating point | DC levels with typical offsets |
| `MeasurementNoise` | noise | Noise, SNR and contributors of the four ADC channels (one noise analysis with four outputs), and the noise of the computed impedance |
| `ResistorMatching` | AC, Monte Carlo | Spread from component tolerances; what calibration removes |
| `ResistorWorstCase` | AC, worst case | The same at the sensitivity corners |
| `Startup` | transient | Rails coming up from models of the regulators, coupling networks charging, input ranges, settling |
| `Distortion` | transient, periodic steady state | Drive-current harmonics and spectrum, clipping and slew headroom |
| `SupplyCurrent` | operating point, transient | The current per rail, idle and driving 1 MHz ([Supply current](#supply-current)) |
| `PowerSide` | operating point | The whole board at the `MEASURE` state, DC: every rail with all its loads (the REF2033 on VP and V_MID included), and the regulators', switches', DDS's and MCU's pins at their DC levels, with the DDS lines low and high ([Supply current](#supply-current)) |

`StrayCapacitance` adds its mask capacitor to `SoilModel`'s soil node `S_LP`
by declaring the net again with the capacitor's pin (nets of one name are one
net); its values are those of the version that wrote the soil out, to the last
digit.

### Strays

Layout facts the schematic cannot hold, added to every bench as bench
components, from SPEC.md's stack-up (pads on layer 1, traces on layer 2 inside
guard copper, guard on layer 3, ground on layer 4, 0.2 mm FR4 between layers 1
and 2 and between 3 and 4):

| Stray | Value | Basis |
| --- | --- | --- |
| Electrode node to its own guard | 20 pF | 8 x 8 mm pad over guard 0.2 mm below: 12 pF (SPEC.md); 30 to 90 mm of 0.25 mm trace inside its guard, about 86 pF/m: 8 pF. |
| Guarded node to ground (residual) | 0.3 pF | SPEC.md's guarded figure; extraction must confirm it (plan phase P5). |
| Guard to ground layer | 100 pF | Guard strip 5 mm x 90 mm over ground 0.2 mm away (86 pF) plus the pad area (19 pF). |
| LC to ground | 20 pF | LC is surrounded by ground. |
| Soil to ground copper through solder mask (`StrayCapacitance` only) | 0 to 100 pF | 20 um mask, relative permittivity 3.5: 1.5 pF per mm2. |

The amplifiers' own input capacitances (1.3 pF ADA4817, 1 pF ADA4807, 4 pF
AD8130) are in their models.

The guard-to-ground 100 pF assumes ground on layer 4 under the guards, the
stack-up these estimates started from. The simulation showed that ground
copper facing the soil steals soil current ([StrayCapacitance](#straycapacitance-pass-the-rated-layout-strays)),
so SPEC.md now keeps ground off the outer layers of the buried part. The
guards then see less than 100 pF, which loads their drivers less; the benches
keep 100 pF as an upper bound (a smaller value was not simulated). The rating allows up to 1 pF of residual per
guarded node and 0.2 pF of mask coupling near LP.

## Results

All numbers from `copper sim` with the front end as committed. Errors are
after calibration. "Band" is the maximum over 1 kHz to 1 MHz; the other
columns are the error at one frequency (`at(x, f)`). "Usable to" is the
frequency up to which magnitude and phase stay within 1 % and 1 degree from
1 kHz on: in `SoilSweep` and `StrayCapacitance` the smaller of the benches'
`usable_to_magnitude` and `usable_to_phase` (`upto`, interpolated between the
20-per-decade samples), elsewhere the highest swept frequency read from the
plotted error curves (`BENCH.csv`; see [CalibrationCard](#calibrationcard-outside-the-rated-range)
for why); "full" is the whole band. Every `upto` value lies between the
sample the earlier tables read and the next one (checked step by step). The
gain-1 and gain-100 channels give identical errors in every bench: everything
that depends on the load happens before the channels split, and calibration
removes each channel's own response.

Each accuracy bench steps inside the rated range (SPEC.md, "Usable range") and
beyond it; the accuracy limits cover the rated steps only (`where`), the
headroom limits every step.

### SoilSweep: pass (rated: 1 % and 1 degree, 5 to 100 ohm m)

| Soil | eps_r | Band: magnitude | Band: phase | Pump node peak | Sense input peak | Common mode / sense |
| --- | --- | --- | --- | --- | --- | --- |
| 5 ohm m (40 ohm) | 5, 25, 40 | 0.022 % | 0.026 deg | 25 mV | 5.6 mV | 8.4 |
| 100 ohm m (800 ohm) | 5 | 0.36 % | 0.15 deg | 186 mV | 104 mV | 7.2 |
| 100 ohm m | 25 | 0.39 % | 0.11 deg | 180 mV | 99 mV | 7.1 |
| 100 ohm m | 40 | 0.39 % | 0.069 deg | 179 mV | 95 mV | 7.1 |

| Soil | eps_r | 10 kHz | 100 kHz | 1 MHz |
| --- | --- | --- | --- | --- |
| 5 ohm m | 5, 25, 40 | 0.022 %, 0.0002 deg | 0.022 %, 0.0025 deg | 0.022 %, 0.026 deg |
| 100 ohm m | 5 | 0.022 %, 0.0002 deg | 0.025 %, 0.0020 deg | 0.36 %, 0.15 deg |
| 100 ohm m | 25 | 0.022 %, 0.0002 deg | 0.025 %, 0.0019 deg | 0.39 %, 0.11 deg |
| 100 ohm m | 40 | 0.022 %, 0.0002 deg | 0.025 %, 0.0019 deg | 0.39 %, 0.069 deg |

Current measurement error after calibration: 7e-10 % (requirement 0.5 %; a
linear model with exact resistors cannot show more). The 0.022 % floor is the
AD8130's common-mode rejection (90 dB minimum) times the common-mode ratio of 8.
At 100 ohm m most of the remaining error comes from the sense nodes' 1.6 pF
(buffer 1.3 pF plus 0.3 pF residual) against the 5.6 kohm contacts, and it
appears only at the top of the band. With all four contacts equal, the LP
node's pole and the current it diverts from LC cancel to first order, which is
why the phase error is small.

### SoilSweep above the rated range

Equal contacts, from 150 ohm m to the board's limit:

| Soil | eps_r | Usable to | 100 kHz | 300 kHz | 1 MHz | Band |
| --- | --- | --- | --- | --- | --- | --- |
| 150 ohm m (1.2 kohm) | 5 | full | 0.029 %, 0.0019 deg | 0.088 %, 0.012 deg | 0.83 %, 0.28 deg | 0.83 %, 0.28 deg |
| 150 ohm m | 25 | full | 0.029 %, 0.0018 deg | 0.088 %, 0.0079 deg | 0.87 %, 0.11 deg | 0.87 %, 0.11 deg |
| 150 ohm m | 40 | full | 0.029 %, 0.0017 deg | 0.088 %, 0.0051 deg | 0.82 %, 0.0020 deg | 0.82 %, 0.010 deg |
| 200 ohm m (1.6 kohm) | 5 | 821 kHz | 0.035 %, 0.0018 deg | 0.14 %, 0.014 deg | **1.5 %**, 0.41 deg | 1.5 %, 0.41 deg |
| 200 ohm m | 25 | 828 kHz | 0.035 %, 0.0015 deg | 0.14 %, 0.0054 deg | **1.5 %**, 0.017 deg | 1.5 %, 0.021 deg |
| 200 ohm m | 40 | 889 kHz | 0.035 %, 0.0012 deg | 0.14 %, 0.0011 deg | **1.2 %**, 0.18 deg | 1.2 %, 0.18 deg |
| 300 ohm m (2.4 kohm) | 5 | 560 kHz | 0.051 %, 0.0014 deg | 0.29 %, 0.017 deg | **3.7 %**, 0.51 deg | 3.7 %, 0.51 deg |
| 300 ohm m | 25 | 586 kHz | 0.051 %, 0.0002 deg | 0.28 %, 0.012 deg | **2.7 %**, 0.54 deg | 2.7 %, 0.54 deg |
| 300 ohm m | 40 | 666 kHz | 0.051 %, 0.0006 deg | 0.27 %, 0.032 deg | **1.7 %**, 0.78 deg | 1.7 %, 0.78 deg |
| 500 ohm m (4 kohm) | 5 | 343 kHz | 0.10 %, 0.0012 deg | 0.76 %, 0.0054 deg | **10 %**, 0.64 deg | 10 %, 0.64 deg |
| 500 ohm m | 25 | 378 kHz | 0.10 %, 0.0062 deg | 0.67 %, 0.13 deg | **3.5 %**, **2.4 deg** | 3.5 %, 2.4 deg |
| 500 ohm m | 40 | 501 kHz | 0.099 %, 0.0097 deg | 0.56 %, 0.19 deg | **1.2 %**, **1.9 deg** | 1.2 %, 1.9 deg |
| 1 kohm m (8 kohm) | 5 | 176 kHz | 0.34 %, 0.025 deg | **2.8 %**, 0.43 deg | **20 %**, **11 deg** | 20 %, 11 deg |
| 1 kohm m | 25 | 207 kHz | 0.31 %, 0.062 deg | **1.5 %**, 0.95 deg | 0.065 %, **4.9 deg** | 1.8 %, 4.9 deg |
| 1 kohm m | 40 | 316 kHz | 0.28 %, 0.084 deg | 0.69 %, 0.93 deg | **1.0 %**, **2.5 deg** | 1.0 %, 2.5 deg |
| 1.3 kohm m (11 kohm) | 5 | 129 kHz | 0.61 %, 0.070 deg | **4.7 %**, **1.3 deg** | **16 %**, **19 deg** | 17 %, 19 deg |
| 1.3 kohm m | 25 | 157 kHz | 0.52 %, 0.16 deg | **1.5 %**, **1.8 deg** | **2.4 %**, **5.2 deg** | 2.4 %, 5.2 deg |
| 1.3 kohm m | 40 | 228 kHz | 0.42 %, 0.20 deg | 0.19 %, **1.4 deg** | **1.9 %**, **2.5 deg** | 1.9 %, 2.5 deg |

The 300 kHz column lies between the 20-per-decade samples: `at(x, f)` solves the
circuit at 300 kHz itself (it interpolated between 282 and 316 kHz before phase F2
of the plan; the exact values differ by up to 0.014 degrees and 0.018 % of
the reading). At 10 kHz every soil here stays within 0.028 % and 0.0022 degrees. The usable
frequency falls as one over the resistivity: about
`160 kHz x (1 kohm m / resistivity)` for the worst permittivity (`upto`:
821 kHz at 200 ohm m, 176 kHz at 1 kohm m, 129 kHz at 1.3 kohm m; the earlier
grid readings were 794, 158 and 126 kHz). 150 ohm m still meets 1 % and
1 degree over the band; the rating stops at 100 ohm m because a minimum-CMRR
sense buffer adds its common-mode error on top
([SenseCommonMode above the rated range](#sensecommonmode-above-the-rated-range)).

| Soil | Soil current at 1 kHz / 1 MHz | Pump node / output peak | Gain-10 ADC peak |
| --- | --- | --- | --- |
| 150 ohm m | 14.8 / 14.2 to 15.8 uA | 0.28 / 0.42 V | 0.19 V |
| 200 ohm m | 14.8 / 13.2 to 15.5 uA | 0.36 / 0.50 V | 0.25 V |
| 300 ohm m | 14.8 / 11.3 to 14.0 uA | 0.53 / 0.68 V | 0.36 V |
| 500 ohm m | 14.8 / 9.0 to 10.4 uA | 0.89 / 1.04 V | 0.60 V |
| 1 kohm m | 14.8 / 5.6 to 7.6 uA | 1.77 / 1.92 V | 1.19 V |
| 1.3 kohm m | 14.7 / 4.1 to 7.3 uA | 2.43 / 2.58 V | 1.63 V |

Compliance limits pass with margin (4.4 V allowed; the sense input peaks at
1.29 V of 1.7 V). At 1 MHz in dry soil only 4 to 8 uA reach the soil: the rest
charges the HC node's strays, which costs signal but not accuracy. At
1.3 kohm m the gain-10 channel is past its linear range (1.48 V) and the
firmware must use gain 1.

### CalibrationCard: pass (rated positions)

| Reference | Contacts | Band: magnitude | Band: phase | Pump node peak |
| --- | --- | --- | --- | --- |
| 39 ohm | 0 | 0.001 % | 0.006 deg | 24 mV |
| 39 ohm | 270 ohm | 0.022 % | 0.026 deg | 25 mV |
| 1 kohm | 0 | 0 (calibration point) | 0 | 28 mV |
| 1 kohm | 6.8 kohm | 0.52 % | 0.22 deg | 229 mV |
| 10 kohm | 0 | 0.0001 % | 0.0002 deg | 157 mV |

The soil-like contacts give the error of the soil they stand for: 270 ohm on
39 ohm is 5 ohm m (SoilSweep: 0.022 %, 0.026 deg); 6.8 kohm on 1 kohm is about
126 ohm m with almost no permittivity (0.52 %, between SoilSweep's 100 ohm m
and SoilSweep's 150 ohm m). The 10 kohm position drives the gain-10 channel
to 1.54 V, past its linear range; the firmware reads it on gain 1 (148 mV).

### CalibrationCard outside the rated range

| Reference | Contacts | Usable to | 10 kHz | 100 kHz | 1 MHz |
| --- | --- | --- | --- | --- | --- |
| 39 ohm | 47 kohm | none (3.8 % at 1 kHz) | **3.8 %**, 0.021 deg | **4.1 %**, 0.21 deg | **33.8 %**, **3.8 deg** |
| 1 kohm | 47 kohm | 178 kHz | 0.15 %, 0.0001 deg | 0.38 %, 0.0019 deg | **27.5 %**, **5.2 deg** |
| 10 kohm | 47 kohm | 200 kHz | 0.017 %, 0.0007 deg | 0.24 %, 0.010 deg | **27.3 %**, **5.3 deg** |
| 10 kohm | 68 kohm | 141 kHz | 0.026 %, 0.0015 deg | 0.49 %, 0.034 deg | **37.7 %**, **16.7 deg** |

"Usable to" is read from the plotted error here. The bench has
`usable_to_phase` (`upto` of the phase alone: 366 kHz for 68 kohm, 628 kHz,
618 kHz and 797 kHz for the 47 kohm positions, full on every rated position),
but no `usable_to_magnitude`: 39 ohm with 47 kohm contacts is 3.8 % off at
1 kHz already, and `upto` of a comparison that fails at the first sample was a
run-time error that stopped the whole bench. It is now "never" (phase F2 of
the plan); the bench has not been given the measure yet. On these positions
the magnitude is what limits.

The 47 kohm contacts of the first card design fail for two reasons in the
card, not the board: (1) on 39 ohm they make the common mode 1200 times the
sense voltage; 1 % would need 102 dB of rejection, and the AD8130 guarantees
90 dB (3.8 % from 1 kHz on). (2) 47 kohm against the sense node's 1.6 pF is a
2.1 MHz pole, so at 1 MHz every 47 kohm position behaves like about 850 ohm m
soil. The card therefore has soil-like contacts now (SPEC.md, "Blocks").

68 kohm on 10 kohm is soil-like and imitates about 1.26 kohm m with almost no
permittivity. It is outside the rated range and kept as a check: on the bench
the board should show this error, which tests the sense nodes' 1.6 pF. Compare
SoilSweep at 1.3 kohm m and eps_r 5 (usable to 129 kHz, 16 %, 19 deg). The
pump reaches 2.16 V at the node and 2.31 V at the output (4.4 V allowed), the
sense input 1.15 V (1.7 V allowed).

### ContactImbalance: pass (rated imbalances)

Contacts offset from the soil's own value, worst of eps_r 5, 25 and 40:

| Contacts | 5 ohm m: magnitude, phase | 100 ohm m: magnitude, phase |
| --- | --- | --- |
| equal | 0.022 %, 0.026 deg | 0.39 %, 0.15 deg |
| HC 10 times | 0.022 %, 0.026 deg | 0.39 %, 0.15 deg |
| HP +100 ohm | 0.52 %, 0.50 deg | 0.59 %, 0.44 deg |
| HP -100 ohm | 0.47 %, 0.56 deg | 0.34 %, 0.67 deg |
| LP +100 ohm | 0.43 %, 0.50 deg | 0.36 %, 0.61 deg |
| LP -100 ohm | 0.48 %, 0.44 deg | 0.56 %, 0.37 deg |
| LC +200 ohm | 0.044 %, 0.16 deg | 0.39 %, 0.28 deg |
| LC -200 ohm | 0.089 %, 0.11 deg | 0.42 %, 0.058 deg |
| HP +100, LC +200 ohm | 0.67 %, 0.67 deg | 0.55 %, 0.33 deg |
| HP -100, LC +200 ohm | 0.73 %, 0.997 deg | 0.34 %, 0.82 deg |
| LP +100, LC +200 ohm | 0.69 %, 0.94 deg | 0.36 %, 0.76 deg |

A worse HC contact costs nothing. The rated imbalances are small: 100 ohm is
36 % of a 5 ohm m contact (279 ohm) but only 1.8 % of a 100 ohm m one
(5.58 kohm). The worst cases are the combinations whose phase errors add, at
5 ohm m, where 200 ohm on LC raises the common-mode ratio from 8 to 13.5.
Common-mode ratio over the rated steps: 3.5 to 13.5; pump node at most 0.94 V.
The worst, HP 100 ohm low with LC 200 ohm high at 5 ohm m, is 0.997 degrees
of the 1 degree limit since the transimpedance and gain-1 anti-alias changes
of 23 November 2026 (SPEC.md, "Vendor models"); it was 0.91 degrees.

### ContactImbalance outside the rated range

One contact worse than the other three, eps_r 25 (the bench also runs eps_r 5
and 40 for these steps). Usable to, read from the plotted error: the bench's
`usable_to_phase` cannot be paired with a magnitude `upto` for the same reason
as in [CalibrationCard](#calibrationcard-outside-the-rated-range) (a sense
contact tens of kohm too high is over 1 % off at 1 kHz); where the phase
limits, `usable_to_phase` agrees with this table (for example 5 ohm m, HP
500 %: 194 kHz against 178 kHz read; 100 ohm m, HP 120 %: 193 kHz against
178 kHz):

| Worse by | 5 ohm m: HP | LP | LC | 100 ohm m: HP | LP | LC |
| --- | --- | --- | --- | --- | --- | --- |
| 105 % | full | full | full | 708 kHz | 708 kHz | full |
| 110 % | full | full | full | 355 kHz | 398 kHz | full |
| 120 % | full | full | full | 178 kHz | 200 kHz | full |
| 150 % | full | full | full | 71 kHz | 79 kHz | 562 kHz |
| 200 % | 708 kHz | 708 kHz | full | 35 kHz | 40 kHz | 282 kHz |
| 500 % | 178 kHz | 200 kHz | full | 9 kHz | 10 kHz | 71 kHz |
| 1000 % | 79 kHz | 89 kHz | 562 kHz | none | none | 32 kHz |

Errors at three frequencies (all four equal, and HC 10 times worse: as in
SoilSweep):

| Soil | Worse contact | 10 kHz | 100 kHz | 1 MHz |
| --- | --- | --- | --- | --- |
| 5 ohm m | HP 120 % | 0.021 %, 0.0024 deg | 0.024 %, 0.024 deg | 0.30 %, 0.27 deg |
| 5 ohm m | HP 200 % | 0.016 %, 0.013 deg | 0.032 %, 0.13 deg | **1.4 %**, **1.4 deg** |
| 5 ohm m | HP 1000 % | 0.037 %, 0.12 deg | 0.13 %, **1.2 deg** | **14.4 %**, **12.0 deg** |
| 5 ohm m | LP 120 % | 0.023 %, 0.0025 deg | 0.020 %, 0.025 deg | 0.23 %, 0.29 deg |
| 5 ohm m | LP 200 % | 0.028 %, 0.012 deg | 0.013 %, 0.12 deg | **1.2 %**, **1.4 deg** |
| 5 ohm m | LP 1000 % | 0.074 %, 0.10 deg | 0.046 %, **1.0 deg** | **8.4 %**, **13.4 deg** |
| 5 ohm m | LC 120 % | 0.027 %, 0.0006 deg | 0.026 %, 0.0061 deg | 0.0040 %, 0.064 deg |
| 5 ohm m | LC 200 % | 0.045 %, 0.0021 deg | 0.044 %, 0.021 deg | 0.068 %, 0.21 deg |
| 5 ohm m | LC 1000 % | 0.23 %, 0.017 deg | 0.22 %, 0.17 deg | 0.76 %, **1.7 deg** |
| 100 ohm m | HP 120 % | 0.0048 %, 0.052 deg | 0.018 %, 0.52 deg | **2.2 %**, **5.6 deg** |
| 100 ohm m | HP 200 % | 0.11 %, 0.26 deg | 0.051 %, **2.6 deg** | **16.2 %**, **26.0 deg** |
| 100 ohm m | HP 1000 % | **1.1 %**, **2.4 deg** | **5.6 %**, **22.6 deg** | **305 %**, **100.4 deg** |
| 100 ohm m | LP 120 % | 0.046 %, 0.046 deg | 0.039 %, 0.46 deg | 0.21 %, **5.2 deg** |
| 100 ohm m | LP 200 % | 0.14 %, 0.23 deg | 0.18 %, **2.3 deg** | **7.4 %**, **24.9 deg** |
| 100 ohm m | LP 1000 % | **1.2 %**, **2.0 deg** | **8.3 %**, **19.1 deg** | **333 %**, **53.4 deg** |
| 100 ohm m | LC 120 % | 0.030 %, 0.0067 deg | 0.031 %, 0.067 deg | 0.25 %, 0.83 deg |
| 100 ohm m | LC 200 % | 0.061 %, 0.033 deg | 0.057 %, 0.33 deg | 0.17 %, **3.7 deg** |
| 100 ohm m | LC 1000 % | 0.37 %, 0.29 deg | 0.43 %, **2.9 deg** | **9.5 %**, **31.4 deg** |

**The rule.** The phase error grows in proportion to frequency and to the
contact difference `dR`. Coefficients in degrees per 100 ohm of difference per
MHz, from the steps whose phase error is under 10 degrees (at 10 kHz and
100 kHz from contacts at least 279 ohm off, where the small base error no
longer matters); the last column is the usable frequency times `dR` from the
first table:

| Contact too high | Soil | 10 kHz | 100 kHz | 1 MHz | Usable to x dR |
| --- | --- | --- | --- | --- | --- |
| HP (phase negative) | 5 ohm m | 0.45 to 0.46 | 0.45 to 0.46 | 0.39 to 0.44 | 198 to 200 kHz kohm |
| HP | 100 ohm m | 0.45 to 0.47 | 0.45 to 0.46 | 0.50 to 0.51 | 198 to 199 kHz kohm |
| LP (phase positive) | 5 ohm m | 0.41 | 0.41 | 0.38 to 0.42 | 198 to 224 kHz kohm |
| LP | 100 ohm m | 0.40 to 0.41 | 0.40 to 0.41 | 0.45 | 198 to 223 kHz kohm |
| LC (phase positive) | 5 ohm m | 0.065 | 0.065 | 0.067 | 1.41 MHz kohm |
| LC | 100 ohm m | 0.058 | 0.058 | 0.065 | 1.57 to 1.59 MHz kohm |

The coefficients were read before the changes of 23 November 2026 (SPEC.md,
"Vendor models") and not regenerated. The 10 kHz and 100 kHz errors they come
from did not move at the precision shown; the 1 MHz ones did (the table
above), so the 1 MHz column, the 5 ohm m rows most, is out of date. The last
column's 198 kHz kohm for LP at 5 ohm m is the new 708 kHz reading at 200 %.

The hand estimate `7 x 2 pi f x dR x 1.6 pF`, 0.40 degrees per 100 ohm per
MHz, holds for a high LP contact (0.40 to 0.41 up to 100 kHz, 0.38 to 0.45 at
1 MHz). A high HP contact costs about 15 % more, because the HP contact also
forms a pole against the sense voltage itself: the factor is the common-mode
ratio plus one, 8, which gives 0.46. For the LC contact the estimate does not
apply: with equal sense contacts the raised common mode cancels between HP and
LP, and what is left is the current the LP node's 1.6 pF takes from the soil
before LC, `2 pi f x dR x 1.6 pF`, 0.058 degrees per 100 ohm per MHz. A hand
derivation that reproduces all three (first order, not simulated):
`phase error = 2 pi f x 1.6 pF x ((R_LC - R_HP) + k (R_LP - R_HP))` with
`k = R_LC / R_TRANSFER`, the common-mode ratio. A worse LC contact therefore
also scales the sense-contact term through `k`, as the rated combinations show.

Where the rule does not hold: above about 10 degrees the error grows more
slowly than the rule says (HP 10 times worse at 100 ohm m: 100.4 degrees at
1 MHz, the rule gives about 250); below 10 kHz a sense contact tens of kohm too
high adds a flat magnitude error (1.1 % at 10 kHz for 50 kohm; by hand, the
33 Mohm bias resistor loading the contact at 8 times the sense voltage); and
at 1 MHz and 5 ohm m a high HP contact also costs magnitude, about 0.5 % per
100 ohm (1.43 % for 279 ohm), which reaches 1 % at about the contact
difference where the phase reaches 1 degree (1.44 degrees for 279 ohm). Calibration
cannot remove any of this: the contacts change at every insertion. Remedies,
not built: capacitance neutralization of the sense buffers, a
lower-capacitance buffer (LTC6268, 0.45 pF, needs its own +/-2.5 V rails), or
restricting the result to lower frequencies when the firmware sees unequal
contacts (it can measure each contact through the probe header).

### StrayCapacitance: pass (the rated layout strays)

100 ohm m, guards driven; band maximum of magnitude and phase:

| Residual on each node | Mask coupling at LP | eps_r 5 | eps_r 25 | eps_r 40 |
| --- | --- | --- | --- | --- |
| 0.3 pF | 0 | 0.36 %, 0.16 deg | 0.39 %, 0.11 deg | 0.40 %, 0.071 deg |
| 0.3 pF | 0.2 pF | 0.42 %, 0.57 deg | 0.53 %, 0.50 deg | 0.58 %, 0.45 deg |
| 1 pF | 0 | 0.75 %, 0.17 deg | 0.76 %, 0.074 deg | 0.73 %, 0.012 deg |
| 1 pF | 0.2 pF | 0.83 %, 0.58 deg | 0.91 %, 0.46 deg | 0.94 %, 0.38 deg |

The two limits are taken together: with both at once the error stays inside
1 % at every permittivity (0.94 % worst). Pump node at most 186 mV.

### StrayCapacitance beyond the rating (moist loam, eps_r 25)

| Guard | Residual on each node | Mask coupling at LP | Usable to | 100 kHz | 1 MHz | Band |
| --- | --- | --- | --- | --- | --- | --- |
| driven | 0.3 pF | 0 | full | 0.025 %, 0.0021 deg | 0.39 %, 0.11 deg | 0.39 %, 0.11 deg |
| driven | 0.5 pF | 0 | full | 0.026 %, 0.0021 deg | 0.49 %, 0.10 deg | 0.49 %, 0.10 deg |
| driven | 1 pF | 0 | full | 0.029 %, 0.0020 deg | 0.76 %, 0.074 deg | 0.76 %, 0.074 deg |
| driven | 1.5 pF | 0 | 960 kHz | 0.032 %, 0.0019 deg | **1.08 %**, 0.020 deg | 1.08 %, 0.020 deg |
| driven | 2 pF | 0 | 830 kHz | 0.036 %, 0.0018 deg | **1.4 %**, 0.062 deg | 1.4 %, 0.062 deg |
| driven | 3 pF | 0 | 651 kHz | 0.045 %, 0.0014 deg | **2.3 %**, 0.31 deg | 2.3 %, 0.31 deg |
| driven | 4 pF | 0 | 534 kHz | 0.057 %, 0.0009 deg | **3.2 %**, 0.69 deg | 3.2 %, 0.69 deg |
| driven | 5 pF | 0 | 452 kHz | 0.071 %, 0.0002 deg | **4.2 %**, **1.2 deg** | 4.2 %, 1.2 deg |
| driven | 20 pF | 0 | 134 kHz | 0.57 %, 0.053 deg | **10.5 %**, **17.0 deg** | 11.7 %, 17.0 deg |
| grounded | 20.3 pF | 0 | 133 kHz | 0.58 %, 0.057 deg | **8.9 %**, **17.1 deg** | 10.8 %, 17.1 deg |
| driven | 0.3 pF | 0.1 pF | full | 0.026 %, 0.022 deg | 0.46 %, 0.30 deg | 0.46 %, 0.30 deg |
| driven | 0.3 pF | 0.2 pF | full | 0.027 %, 0.042 deg | 0.53 %, 0.50 deg | 0.53 %, 0.50 deg |
| driven | 0.3 pF | 0.3 pF | full | 0.027 %, 0.062 deg | 0.60 %, 0.70 deg | 0.60 %, 0.70 deg |
| driven | 0.3 pF | 0.5 pF | 918 kHz | 0.029 %, 0.10 deg | 0.75 %, **1.1 deg** | 0.75 %, 1.1 deg |
| driven | 0.3 pF | 1 pF | 488 kHz | 0.032 %, 0.20 deg | **1.13 %**, **2.1 deg** | 1.13 %, 2.1 deg |
| driven | 0.3 pF | 2 pF | 247 kHz | 0.040 %, 0.40 deg | **2.0 %**, **4.0 deg** | 2.0 %, 4.0 deg |
| driven | 0.3 pF | 10 pF | 50 kHz | 0.15 %, **2.0 deg** | **12.7 %**, **18.1 deg** | 12.7 %, 18.1 deg |
| driven | 0.3 pF | 100 pF | 5.0 kHz | **6.6 %**, **19.2 deg** | **288 %**, **64.3 deg** | 288 %, 64.3 deg |
| driven | 5 pF | 10 pF | 50 kHz | 0.25 %, **2.0 deg** | **19.6 %**, **14.5 deg** | 19.6 %, 14.5 deg |
| driven | 5 pF | 100 pF | 5.0 kHz | **7.2 %**, **19.1 deg** | **287 %**, **55.3 deg** | 287 %, 55.9 deg |
| driven | 20 pF | 10 pF | 50 kHz | 0.93 %, **1.9 deg** | **24.4 %**, **5.7 deg** | 24.4 %, 5.7 deg |
| driven | 20 pF | 100 pF | 5.0 kHz | **9.3 %**, **18.5 deg** | **232 %**, **30.3 deg** | 232 %, 39.0 deg |
| grounded | 20.3 pF | 10 pF | 50 kHz | 0.95 %, **1.9 deg** | **22.4 %**, **5.8 deg** | 22.4 %, 5.8 deg |
| grounded | 20.3 pF | 100 pF | 5.0 kHz | **9.3 %**, **18.5 deg** | **226 %**, **30.5 deg** | 226 %, 38.8 deg |

At 10 kHz every step stays within 0.12 %; the phase there is 0.2 degrees per
10 pF of mask coupling (2.0 degrees for 100 pF). The guards are worth what
SPEC.md claims: grounding them is as bad as 20 pF of residual. Residual
capacitance mostly costs magnitude at the top of the band; 1 pF is the most
that keeps 1 % to 1 MHz. Mask coupling costs phase in proportion to frequency:
current that leaves the soil between the sense electrodes through capacitance
to board copper never reaches LC. About 2.0 degrees per pF per MHz
(0.1 pF: 0.20 degrees more than none at 1 MHz), which matches
`2 pi f x C_mask x R_LC` with the 5.58 kohm LC contact (by hand). 10 pF, 7 mm2
of ground copper under mask near LP, costs 18 degrees. The buried part
therefore has no ground copper on its outer layers (SPEC.md, "Board shape and
layout notes for later").

### PumpOutputImpedance: pass (electrodes in air, HC guard connected)

| Measure | Value |
| --- | --- |
| Output impedance at 1 kHz | 6.22 Mohm (limit at least 1.5 Mohm) |
| at 100 kHz | 974 kohm |
| at 1 MHz | 86.8 kohm |
| Guard tracking, maximum over 1 kHz to 1 GHz | 100.0 % (limit 106 %, the guard loop's unity-gain point 21.3 / 20) |
| Largest output impedance | 45 Mohm at 4.07 kHz: `C_NDC` makes the low-frequency output impedance inductive, and it resonates with the node's 21 pF (half-power width 3.13 to 5.25 kHz in a sweep from 10 Hz: Q 1.9, the 4.06 kHz pole pair of `GuardStability`) |
| Most negative real part | -42.5 kohm (near 600 kHz) |
| Negative conductance against 1 / 160 kohm | -451 times (reported) |

A guard that lags its node by angle `phi` feeds the node a current
`-omega C_g sin(phi) V`: a negative conductance. The bench first asserted it
against half the soil's conductance; that failed by three orders of magnitude
above the band, where the soil's own contact capacitances dominate, and is the
wrong test: whether the node oscillates depends on the poles of the whole loop,
which an AC sweep cannot see. The guard loop's gain stays below one
(tracking at most 100 %), `LoopPeaking` finds no peaking, and the measure is
reported without a limit. `Startup` runs the pump with the electrodes in air
through the rails' start-up and finds it settling (73 ms) without oscillating,
but with time steps of up to 1 ms. The pole-zero analysis of `GuardStability`
settles the question: stable (see there).

### SenseCommonMode: pass (rated: at least 63 dB at 1 MHz)

The rejection is the common-mode voltage over what arrives at `V_D`, the
receiver's output, so the ADC channels' own responses (their gain stages and
anti-alias poles, which the reading shares) do not enter it.

| Contacts | LP buffer CMRR | Rejection at 1 MHz (also the band minimum) |
| --- | --- | --- |
| 0 | 90 dB (typical, as HP) | 84.5 dB |
| 0 | 77 dB (data-sheet minimum) | 69.8 dB |
| 5.58 kohm (100 ohm m) | 90 dB | 84.4 dB |
| 5.58 kohm | 77 dB | 66.7 dB |

The limit was 70 dB (SPEC.md), which a buffer at its data-sheet minimum misses
even without contacts. It is now 63 dB, from the error budget: the common mode
is 7 to 8 times the sense voltage, so 63 dB leaves at most 0.50 % to 0.57 % of
the reading, and with SoilSweep's 0.39 % that stays inside 1 %. With matched
buffers the path has 84 dB (the receiver at its guaranteed minimum). A buffer
at the minimum CMRR loses 15 to 18 dB: its common-mode error falls to about
66 dB at 1 MHz (Figure 27 corner at 300 kHz), and its guard, driven from its
output, pushes that error back into the node through the 20 pF to the guard,
multiplied by `omega C_g R_contact`. The model's error has one sign; real
units can have either, so the worst case is about 3 dB worse still (hand
estimate), which 66.7 dB still covers. Driving each guard from a separate,
filtered copy would not help: the guard must follow the node.

### SenseCommonMode above the rated range

| Contacts | LP buffer CMRR | 10 kHz | 100 kHz | 1 MHz (also the band minimum) |
| --- | --- | --- | --- | --- |
| 11.2 kohm (200 ohm m) | 90 dB | 90.0 dB | 89.9 dB | 84.2 dB |
| 11.2 kohm | 77 dB | 82.2 dB | 81.2 dB | **62.7 dB** |
| 27.9 kohm (500 ohm m) | 90 dB | 90.0 dB | 89.9 dB | 83.9 dB |
| 27.9 kohm | 77 dB | 82.2 dB | 80.3 dB | **55.3 dB** |
| 55.8 kohm (1 kohm m) | 90 dB | 90.0 dB | 89.9 dB | 84.2 dB |
| 55.8 kohm | 77 dB | 82.1 dB | 78.2 dB | **50.0 dB** |
| 72.5 kohm (1.3 kohm m) | 90 dB | 90.0 dB | 89.9 dB | 84.7 dB |
| 72.5 kohm | 77 dB | 82.1 dB | 76.9 dB | **48.9 dB** |

The loss grows with the contact resistance and with frequency. Up to 100 kHz a
minimum-CMRR buffer keeps at least 76.9 dB at every contact here, so in the
degraded range the common mode costs at most 0.1 % below 100 kHz (7 times
10^(-76.9/20), by hand); at 1 MHz it falls below 63 dB between 100 ohm m
(66.7 dB) and 200 ohm m (62.7 dB), which is why the rating stops at 100 ohm m.

### LoopPeaking: pass

Every loop at most 6.3 % (0.53 dB) above its in-band gain from 1 kHz to 1 GHz:
drive buffer 1.8 %, pump current 5.3 %, sense buffer 6.3 %, guard 0 %,
receiver 3.2 %, gain-1 driver 1.8 %, gain-10 stages 0 %, transimpedance 0 %.

### LoopStability: pass (every loop at least 45 degrees; no pole in the right half-plane)

The loop gain at each amplifier's output pin (`analysis loop`, Tian's method:
a 0 V probe in series with the pin, every other loop closed), moist loam
attached as in `LoopPeaking`:

| Loop (probe) | Phase margin | Crossover | Gain margin |
| --- | --- | --- | --- |
| Pump (`U_PMP.OUT`) | 78.6 deg | 37.7 MHz | 50.7 dB |
| Pump feedback buffer and HC guard (`U_FBB.OUT`) | 72.4 deg | 150 MHz | 39.2 dB |
| Drive buffer (`U_DRV.OUT`) | 61.6 deg | 159 MHz | |
| HP sense buffer (`U_BHP.OUT`) | 58.9 deg | 299 MHz | |
| Receiver (`U_DIF.OUT`) | 61.2 deg | 227 MHz | |
| Gain-1 driver (`U_B1.OUT`) | 49.1 deg | 117 MHz | |
| Gain-10 stage, gain-100 stage | 91.6 deg, 91.7 deg | 64.4 MHz | |
| Transimpedance (`U_TIA.OUT`) | 80.2 deg | 51.0 MHz | |

The poles (`analysis pz`): 55, none in the right half-plane. The least
damped pairs: 16.4 Hz with damping 0.17 in the pump and its guard (`DRIVE`,
`U_PMP`, `GUARD_HC`; far below the band), 2.92 MHz with 0.51 in the gain-100
stage, 135 MHz with 0.51 in the gain-1 driver.

The gain-1 driver has the least margin, and `LoopPeaking` does not show it:
its own closed loop, `T / (1 + T)`, peaks 23 % at 135 MHz, but its input
`B1_IN` sits behind 1 kohm and follows the output through the amplifier's
input capacitance, so `V(V_1) / V(B1_IN)` peaks only 1.8 %; from the source
side, `V(V_1) / V(B1_C)`, it is 6 % (an ac sweep by hand). For the followers
the closed loop and `LoopPeaking` agree: drive buffer 1.7 % and 1.8 %,
receiver 3.1 % and 3.2 %. The HP sense buffer's 6.3 % in `LoopPeaking` is
the guard's positive feedback on `SNS_HP`, outside the buffer's own loop
(1.9 %).

### GuardStability: pass (electrodes in air: no pole in the right half-plane)

`PumpOutputImpedance`'s circuit with the test current at 0 A (open). 51 poles,
none in the right half-plane; three at the origin, the electrode nodes `E_HP`
and `E_LP`, which in air connect through capacitors only. The least damped
pair is at 4.06 kHz with damping 0.27 and dominates `DRIVE`, `U_PMP` and
`GUARD_HC`: the pump, its DC servo `C_NDC` and the guard feeding the node
back. It is the 45 Mohm output-impedance resonance of `PumpOutputImpedance`
(4.07 kHz, Q 1.9). The pump loop has 78.6 degrees of phase margin and the
feedback buffer 74.1 degrees, as in soil.

The loop gain at the guard resistor's input (`R_GHC.1`) is reported without
a limit and is no stability margin: that break is two-way (the pump's
positive feedback through `R2` returns to the guard as well), so Tian's loop
gain there is not the return ratio of one loop. It peaks at 6.2 and shows 2.5
degrees of "phase margin" at 4.04 kHz, where the poles show damping 0.27.

### SenseBufferTrade: reported (both sense buffers as an OPA810 model)

| Soil, eps_r 25 | ADA4817 (board) | OPA810 (3.7 mA) |
| --- | --- | --- |
| 100 ohm m | 0.38 %, 0.11 deg | 0.76 %, 0.13 deg |
| 1 kohm m | 1.8 %, 4.9 deg | 2.8 %, 7.9 deg |

Guard tracking at 1 MHz is 99.9 % with either. The bench cannot show the
OPA810's real cost, its 42 dB of CMRR at 1 MHz, because two identical models
cancel each other's common-mode error; see [Supply current](#supply-current).

### DcOperatingPoint: pass

Pump node +24 mV, pump output +12 mV, sense buffer +0.3 mV, receiver +0.4 mV,
ADC inputs `V_MID` +56 mV (gain 1), +45 mV (gain 10 and 100), -23 mV
(current); LC at 1.65 V. Typical offsets and bias currents, one sign.

### MeasurementNoise: reported (no requirement in SPEC.md)

One bench, `MeasurementNoise`, whose noise analysis has the four ADC inputs as
its outputs (its measures are named `gain1_...`, `gain10_...`, `gain100_...`,
`current_...`; the four benches it replaced gave the same numbers to the last
digit), the stake in soil at relative permittivity 25. Detection bandwidth 1 kHz (SPEC.md's
figure; the I/Q demodulation's noise bandwidth is one over the record length,
so a longer record lowers the noise by the square root): the noise is the RMS
over 1 kHz centred on the drive frequency, the signal the drive's at the same
input, and SNR their ratio (signal RMS over noise RMS). The amplifier noise
uses the 1/f corners now in the part files ([Data-sheet facts](#data-sheet-facts-analog-front-end)).

| Channel | Soil | Signal at the ADC (peak) | SNR at 1 kHz | 10 kHz | 100 kHz | 1 MHz | Referred to the sense voltage, 1 kHz / 100 kHz |
| --- | --- | --- | --- | --- | --- | --- | --- |
| gain 1 | 5 ohm m (0.59 mV) | 0.59 mV | 55.2 dB | 58.4 dB | 58.8 dB | 58.9 dB | 22.4 / 15.2 nV/rtHz |
| gain 1 | 100 ohm m | 11.9 mV | 79.9 dB | 81.9 dB | 82.2 dB | 82.1 dB | 26.5 / 20.6 nV/rtHz |
| gain 1 | 1.3 kohm m | 162 mV | 94.3 dB | 94.9 dB | 94.4 dB | 85.3 dB | 69.7 / 63.1 nV/rtHz |
| gain 10 | 5 ohm m | 6.0 mV | 55.1 dB | 58.2 dB | 58.6 dB | 58.6 dB | 22.8 / 15.6 nV/rtHz |
| gain 10 | 100 ohm m | 120 mV | 79.8 dB | 81.8 dB | 82.1 dB | 82.0 dB | 26.8 / 21.0 nV/rtHz |
| gain 100 | 5 ohm m | 60.4 mV | 55.1 dB | 58.2 dB | 58.6 dB | 58.6 dB | 22.8 / 15.6 nV/rtHz |
| gain 100 | 100 ohm m | 1.21 V | 79.8 dB | 81.8 dB | 82.1 dB | 82.0 dB | 26.8 / 21.0 nV/rtHz |
| current | every soil | 0.29 to 0.30 V | 98.3 dB | 99.5 dB | 99.5 to 99.6 dB | 96.7 to 99.1 dB | 4.0 / 3.2 to 3.5 pA/rtHz (referred to the current) |

The signal column is at 1 kHz. At 1 MHz the current channel carries 0.07 to
0.19 V peak (0.12 to 0.30 V before the transimpedance amplifier's 10 pF of
23 November 2026; `current_signal_1mhz`); its noise falls with it, so its SNR
there moved by at most 0.24 dB.

Contributors, share of the noise power in the 1 kHz band at 1 kHz / at
100 kHz (gain 1; gain 100 within a few points, its second stage adds 2 to 6 %):

| Soil | Sense buffers (ADA4817) | Receiver (AD8130) | ADC driver | Soil and contacts (thermal) | Drive (pump, DDS, filter) | 33 Mohm bias |
| --- | --- | --- | --- | --- | --- | --- |
| 5 ohm m | **59 %** / 15 % | 33 % / **68 %** | 6 % / 13 % | 2 % / 4 % | 0 % / 0 % | 0.3 % / 0 % |
| 100 ohm m | **43 %** / 8 % | 24 % / 37 % | 4 % / 7 % | 27 % / **47 %** | 1 % / 2 % | 0.2 % / 0 % |
| 1.3 kohm m | 6 % / 2 % | 4 % / 4 % | 1 % / 1 % | **55 %** / **66 %** | 34 % / 27 % | 0.2 % / 0.2 % |

Current channel: the drive's own current noise 85 % (the thermal noise of
R_SET and the LT5400's four 10 kohm, each 1.29 pA/rtHz as a current), the
transimpedance stage 15 % (R_TIA's 0.91 pA/rtHz, the ADA4807's current noise
of 0.7 pA/rtHz below its 2 kHz corner and its voltage noise over the 21 pF at
LC).

What it means:

- **The smallest signal.** 5 ohm m gives 0.59 mV peak at the sense
  electrodes (0.42 mV RMS). Referred there, the noise is 22.4 nV/rtHz at
  1 kHz and 15 nV/rtHz from 10 kHz up, 0.71 uV RMS in 1 kHz: 55 dB. The noise
  splits evenly between the reading's magnitude and its phase, so that is
  0.12 % of the magnitude and 0.070 degrees (one standard deviation each; see
  the next point). The gain stages add nothing that
  matters (gain 1 and gain 100 within 0.3 dB); gain 100 is there for the ADC's
  resolution, not for noise.
- **What sets it.** At 5 ohm m the sense buffers and the receiver: the
  ADA4817's 1/f noise (8 kHz corner, fitted to its Figure 30: 12 nV/rtHz at
  1 kHz) and the AD8130's 12.5 nV/rtHz. That is why the 1 kHz SNR is 3 dB below
  the rest of the band. From 100 ohm m the soil's own thermal noise (the HP
  and LP contacts, 5.6 kohm each: 9.6 nV/rtHz each) takes over; it is
  physics, not circuit. At 1.3 kohm m the drive's current noise times the soil
  impedance adds a third; it is common to both channels.
- **Resolution of the impedance.** The bench computes the noise of the
  impedance the firmware reports, `z1` of `FirmwareReadout` (`V_MON_1` over
  the current read at `I_MON`), from every generator with the correlated ones
  combined (`noise(z1)`: the ratio linearised in the two channels at each
  frequency). The drive's current noise reaches both channels and cancels in
  `V / I` completely (`impedance_drive_share_1khz`: 1e-30 %, round-off; the
  soil sees one current). Relative to `|z1|` in the 1 kHz band
  (`impedance_noise_1khz`; the phase's RMS in radians is the same number,
  `impedance_phase_noise_1khz` in degrees): at 5 ohm m 0.12 % and 0.070
  degrees (0.049 ohm of 40 ohm); at 100 ohm m 0.0071 % and 0.0041 degrees; at
  1.3 kohm m 0.0012 % and 0.0007 degrees; at 100 kHz 0.081 %, 0.0055 % and
  0.0012 %. The 1 % and 1 degree accuracy is therefore not noise-limited at
  1 kHz bandwidth: three standard deviations at 5 ohm m and 1 kHz are 0.37 %
  and 0.21 degrees, and a 10 Hz bandwidth (a 100 ms record per frequency)
  brings them to 0.037 % and 0.021 degrees. (The hand combination this
  replaces, the channels' noise without their drive shares added as
  uncorrelated, gave 0.17 % and 0.10 degrees: it took the whole noise-to-signal
  ratio for each of magnitude and phase, which share it, so it was high by
  the square root of 2.)
- **SPEC's estimate.** The current channel was estimated at 0.9 pA/rtHz (R_TIA
  alone); simulated 4.0 pA/rtHz referred to the current at 1 kHz, 1.55 pA/rtHz
  of it the transimpedance stage, the rest the drive's current noise, which
  cancels in the result.

Not in the noise model: the ADC's own noise and quantisation (12 bits: one
LSB is 0.8 mV, so at 5 ohm m the gain-100 channel's 60 mV peak is 7 bits; the
averaging in the I/Q demodulation recovers resolution, and the circuit's own
noise above dithers it), the AD8130's REF/FB current noise, rail noise (the
benches' rails are ideal), interference picked up by the stake.

### Startup: pass (settled within 1 s)

The rails come from models of the board's supply parts (`sim regulator` and
`sim reference`, built-in macro models at data-sheet values; the bench lists
each value and its source): `ANA_EN` enables the TPS65130's boost and
inverter, averaged converters with a 1 ms soft-start ("about 1 ms" in the data
sheet); the post-filters feed the TPS7A4901 and TPS7A3001, each enabled when
its input passes its EN threshold (EN tied to IN) and ramping over its
soft-start (14 ms and 9 ms with the board's 10 nF on NR/SS); the REF2033 runs
from VP. `V3V3_ANA` (a load switch whose rise was not read) still ramps in an
estimated 50 us, and the AD9833's unprogrammed output steps to an estimated
0.34 V with it. The drive stays at DC; 2 s in time.

| Rail | Leaves 0 V (0.1 V) | Regulating | Estimated ramp before |
| --- | --- | --- | --- |
| VPOS_RAW (boost) | at once | 0.99 ms (5.3 V) | not modelled |
| VN | 0.57 ms | 9.3 ms (-4.43 V) | 1 ms to 10 ms |
| VP | 0.73 ms | 14.3 ms (4.45 V) | 1 ms to 15 ms |
| `V_MID` | 5.9 ms | 10.7 ms (1.64 V) | 1 ms to 7.2 ms |

| Electrodes | Settled within 1 mV: gain 1 / 10 / 100 / current | ADC inputs linear from (gain 100) | Pump output, lowest / highest | Pump node settled within 1 mV |
| --- | --- | --- | --- | --- |
| in air | 356 / 470 / 668 / 11 ms | 165 ms | -1.82 V / 2.22 V | 73 ms |
| 5 ohm m | 355 / 472 / 667 / 578 ms | 165 ms | -1.82 V / 2.15 V | 716 ms |
| 100 ohm m | 355 / 473 / 672 / 364 ms | 161 ms | -1.53 V / 2.06 V | 449 ms |
| 1.3 kohm m | 385 / 477 / 675 / 131 ms | 168 ms | -0.39 V / 1.45 V | 80 ms |

- **Rails.** The regulator models start the rails earlier than the estimated
  ramps did (each LDO enables as its input, rising with the converters'
  soft-start, passes its EN threshold at about 0.35 to 0.4 ms) and bring
  `V_MID` up later: the REF2033 needs VIN 20 mV above its 3.3 V VREF to
  regulate, so `V_MID` reaches 1.65 V only when VP passes 3.32 V. Below that
  the model's `V_MID` is VP less 1.67 V, an estimate (how VBIAS falls below
  that input was not read). VP's LDO delivers up to 60 mA while its 20 uF
  charge: the front end's 52 mA plus the ramp. Not modelled: the boost's path
  through L1 and D1, which puts VSYS less a diode drop on the LDO's input as
  soon as the boost's load switch closes, so VP would start sooner still; the
  converters' switching ripple (the models are averaged) and their loops'
  transients.
- **Settling.** Every ADC input is within 1 mV (about one LSB) of its final
  value 0.68 s after `ANA_EN` rises, in air or in soil: inside the 1 s that
  firmware rule 3 and the battery budget allow, with 0.32 s to spare (the
  limit of the bench). With the estimated ramps it was 0.66 s; the later
  `V_MID` adds up to 9 ms, and 30 ms to the current channel at 5 ohm m. The
  hand estimate was 0.8 s from the slowest network (155 ms x 5). The slowest
  output is gain 100: its two 22 uF DC blocks and the ADC drivers' 47 kohm
  couplings, not the sense inputs, which reach the receiver as common mode.
  The current channel settles with the soil: the sense couplings' 4.7 nF
  charge through the soil and LC (0.58 s at 5 ohm m). The ADC inputs swing
  between their rails while the rails come up (10 mV to 3.22 V) and are
  linear (0.1 V inside the rails) from 0.17 s on; the pins never leave 0 to
  3.3 V.
- **Pump.** It swings up to about 2.2 V either way while the coupling networks
  charge and comes back to its +24 mV operating point; in air it settles in
  73 ms, no latch-up and no oscillation in the model (the transient steps are
  up to 1 ms; a fast oscillation would need finer steps to show).
- **Input ranges.** No amplifier input passes its absolute maximum in the
  model, but two come close:
  - `U_DRV`'s input rises 0.18 V above its rails at 50 us and stays above VP
    until VP passes it, 0.92 ms (1.5 ms with the estimated ramps): the AD9833
    comes up with `V3V3_ANA`, before VP and VN, and its output's DC level
    (0.34 V, estimated, halved by the termination) passes `C_IN` while the
    ADA4807 is unpowered. The rating is 0.2 V. The regulator models do not
    change the peak (179 mV either way), which is set before any rail moves;
    only how long it lasts. The estimate of the AD9833's reset level decides
    it: at the 0.65 V top of its range it would be 0.33 V, through the input
    diodes of an unpowered amplifier, with only the filter's 100 ohm in series.
    Switching the DDS supply on after VP, or a series resistor ahead of
    `U_DRV`'s input, would remove it (not built).
  - The ADC drivers' inputs (`B1_IN`, `G1_IN`) go to -0.17 V while the
    receiver's output settles, inside -0.2 V by 35 mV (-0.14 V with the
    estimated ramps, whose `V_MID` came up 3.5 to 5 ms sooner); the 1 kohm series
    resistors limit any current.
  - The sense buffers' inputs stay inside their linear range (+VS - 2.8 V)
    once VP is up (from 9 to 14 ms; 0.15 V to spare at 5 ohm m with no
    signal on top), and inside their absolute maximum throughout.

### ResistorMatching and ResistorWorstCase: pass

The tolerances of `MatchingTolerances`: the LT5400 at 7.5 % absolute and
0.0125 % ratio (its data sheet), R_SET and R_TIA at 0.1 % (the design), every
other resistor 1 %, C0G capacitors 5 %, X7R coupling capacitors 10 %, the 22 uF
DC blocks 20 % (assumed: the module states no tolerance for them). Amplifier
parameters are not varied. 100 Monte Carlo boards (gaussian, tolerance at
three standard deviations) and the worst-case corners (42 variables):

| Measure, 100 ohm m unless stated | Nominal | Monte Carlo sigma (min to max) | Worst case low / high | Calibrated out? |
| --- | --- | --- | --- | --- |
| Drive current at 1 kHz | 14.85 uA | 0.16 % (14.79 to 14.90 uA) | 14.75 / 14.94 uA (-0.6 / +0.6 %) | no, and not needed: it is measured |
| Pump output impedance at 1 kHz | 6.22 Mohm | 0.26 Mohm (5.52 to 6.91) | 5.07 / 7.55 Mohm | no; sets how much current reaches the soil |
| at 10 kHz | 11.3 Mohm | 0.24 Mohm (10.8 to 12.0) | 9.8 / 13.1 Mohm | no |
| Gain-1 channel before calibration (10 kHz) | -0.07 % | 0.038 % (-0.14 to +0.04 %) | -0.22 / +0.07 % | yes |
| Gain-10 channel before calibration | -0.07 % | 0.35 % (-0.76 to +0.81 %) | -2.0 / +1.9 % | yes |
| Gain-100 channel before calibration (against 100) | +1.73 % | 0.59 % (+0.22 to +3.0 %) | -2.0 / +5.6 % | yes |
| Phase before calibration at 1 MHz | 27.7 deg | 0.55 deg (26.2 to 29.0) | 24.2 / 31.1 deg | yes |
| Error after calibration, band (limit 1 %, 1 deg) | 0.39 %, 0.11 deg | 0.013 %, 0.004 deg (max 0.43 %, 0.12 deg) | max 0.44 %, 0.12 deg | what is left is the soil's, not the parts' |
| 5 ohm m, after calibration | 0.022 %, 0.026 deg | max 0.054 %, 0.059 deg | max 0.069 %, 0.026 deg | |
| Current error after calibration, every soil (limit 0.5 %) | 1e-11 | max 2e-11 | max 1e-11 | yes |
| Sense-path rejection at 10 kHz (5 / 100 ohm m / 1.3 kohm m) | 89.1 / 90.0 / 90.0 dB | 3.6 / 3.9 / 5.2 dB (min 82.5 / 82.8 / 81.9) | 80.6 / 80.6 / 77.3 dB | no |
| at 1 MHz | 84.3 / 84.5 / 84.5 dB | 1.1 / 2.4 / 3.3 dB (min 81.0 / 79.4 / 73.7) | 79.6 / 77.6 / 71.1 dB | no |

Yield 100 % against the limits (calibrated accuracy on the rated soils,
current error everywhere); 1.3 kohm m is outside the rated range (2.4 %,
5.2 deg calibrated, as in `SoilSweep`).

- **Calibration removes the parts.** Every gain and phase error the
  tolerances cause (R_TIA, the gain resistors, the anti-alias and coupling
  parts) is a property of the board, which the card calibrates: after
  calibration the spread is 0.013 % at 100 ohm m. What remains is the soil
  error of `SoilSweep`, which calibration never removed.
- **The LT5400's matching hardly matters.** The pump's output impedance at
  1 kHz is set by the `C_NDC` DC servo (sensitivity 1.3 Mohm over its 10 %)
  and the network's common 7.5 % (1.0 Mohm); the 0.0125 % ratio moves it by
  34 kohm per resistor. The hand estimate of 200 Mohm from the ratio
  (SPEC.md) is far above what the servo leaves, so no board comes near the
  1.5 Mohm the README asks for (worst case 5.07 Mohm).
- **The drive current** spreads by +/-0.6 %, almost all of it `R_TERM`'s 1 %
  against the AD9833's fixed 200 ohm (the divider that halves the DDS); R_SET's
  0.1 % adds 0.1 %. The current is measured, so this costs nothing.
- **Common-mode rejection is not calibrated and the coupling capacitors set
  it.** With matched buffers the path rejects 84.5 dB at 1 MHz (at `V_D`, as
  in `SenseCommonMode`); a 10 % X7R
  mismatch between `C_CHP` and `C_CLP` (4.7 nF each) turns the common mode
  into a differential signal through their divider against the buffers' 1.3 pF
  inputs (sensitivity 14 dB over each capacitor's range at 10 kHz), down to
  77.6 dB at 100 ohm m and 71.1 dB at 1.3 kohm m in the worst case. Inside the
  rated range that alone costs up to 0.09 % of the reading (7 times
  10^(-77.6/20), by hand), and it adds to the buffers' CMRR mismatch of
  `SenseCommonMode` (66.7 dB with one buffer at its minimum), which was
  simulated with equal capacitors; the
  combination was not simulated. C0G coupling capacitors at 5 % or better
  would halve the term (a design choice, not made).
- **Worst case against Monte Carlo.** The corner search assumes each measure
  moves monotonically with each tolerance; the rejection does not (it is best
  when the two capacitors match, so both corners at 1.3 kohm m lie below the
  nominal 90 dB at 10 kHz). Its corners are therefore not bounds; the Monte
  Carlo minimum (81.9 dB) is above the worst corner (77.3 dB) because 100
  boards rarely put both capacitors at opposite ends.

### Distortion: reported (no requirement in SPEC.md)

The drive in time, two periods of its periodic steady state (found by
shooting, phase F9), relative permittivity 25. The op-amp models are linear
except for their transient limits (output swing, slew rate, output current),
so every harmonic here is clipping or slewing, or integration error: the
spectrum plot shades each run's integration floor, 3e-7 of the fundamental
where the steps stay uniform (the 100 ohm m runs) and 2e-4 where the error
control shortens some of them (the 1.3 kohm m runs).

| Drive | Soil (drive impedance) | Drive current, fundamental | THD of the drive current | Pump output peak | Headroom to clipping (4.42 V) | Slew needed (2 pi f x peak) |
| --- | --- | --- | --- | --- | --- | --- |
| 1 kHz, 0.3 V (nominal) | 1.3 kohm m (160 kohm) | 14.7 uA | below the floor (0.0009 % shown, 4e-6 % with a five times shorter step) | 2.59 V | 1.83 V | 0.016 V/us |
| 1 kHz, 0.3 V | 100 ohm m (12 kohm) | 14.8 uA | below the floor (0.00001 % shown) | 0.34 V | 4.08 V | 0.002 V/us |
| 1 kHz, 0.65 V (DDS maximum) | 1.3 kohm m | 28.3 uA | **9.5 %** (H3 8.8 %) | 4.42 V, clipped | 0 | 0.028 V/us |
| 100 kHz, 0.3 V | 1.3 kohm m | 13.5 uA | below the floor (0.0010 % shown) | 2.37 V | 2.05 V | 1.5 V/us |
| 100 kHz, 0.3 V | 100 ohm m | 14.8 uA | below the floor (0.00001 % shown) | 0.34 V | 4.08 V | 0.21 V/us |
| 100 kHz, 0.65 V | 1.3 kohm m | 27.1 uA | **5.6 %** (H3 4.8 %) | 4.42 V, clipped | 0 | 2.8 V/us |
| 1 MHz, 0.3 V | 1.3 kohm m | 5.9 uA | below the floor (0.0012 % shown) | 0.61 V | 3.81 V | 3.8 V/us |
| 1 MHz, 0.3 V | 100 ohm m | 15.3 uA | 0.00016 % | 0.33 V | 4.09 V | 2.1 V/us |
| 1 MHz, 0.65 V | 1.3 kohm m | 12.8 uA | below the floor (0.0020 % shown) | 1.31 V | 3.11 V | 8.2 V/us |

- **At the nominal drive nothing clips or slews.** The drive current's THD is
  below the integration floor at every frequency and load but one (1 MHz into
  100 ohm m, 0.00016 %, -116 dB); into 1.3 kohm m the values the table shows
  (about 0.001 %) are integration error, as a run of the 1 kHz case with a
  1 us step shows (4e-6 %). The 0.010 % that the settling run of earlier
  phases gave at 1 kHz into 100 ohm m was its settling: a 400 ms run reaches
  the steady state's 339.2 mV pump peak (the 60 ms run had 347 mV) and a THD
  of 3e-5 % on its way down. The largest swing, 2.59 V
  at the pump output into 1.3 kohm m at 1 kHz, leaves 1.8 V to the clipping
  level. At 1 MHz the pump output swings only 0.61 V into the driest soil: the
  current there goes into the HC node's strays (5.9 of 15 uA reach the
  soil), so the slew rate needed is 3.8 V/us against the ADA4807's 225 V/us.
- **The ADC channels**: at the nominal drive the gain-1 and current channels
  carry the drive current's THD (within 0.0002 %); the gain-10 channel clips at 1 kHz in
  1.3 kohm m soil (2.0 % THD, 1.63 V peak), as `SoilSweep` showed in AC: the
  firmware reads that soil on gain 1.
- **A DDS at its maximum amplitude (0.65 V, a wrong setting) clips the pump**
  in dry soil up to at least 100 kHz: the current doubles to 28 uA and would need
  about 5.6 V at the pump output, so it clips at 4.42 V with 9.5 % THD at 1 kHz.
  Both channels see the same distorted current and the soil is linear, so the
  fundamentals' ratio, which the firmware computes, is unchanged in the model;
  the gain-10 channel is then at 26 % THD. At 1 MHz the strays keep the swing
  at 1.3 V and nothing clips. The firmware should hold the DDS at its nominal
  amplitude (or below) in dry soil; nothing in the hardware limits it.

## SPEC estimates against the simulation

| Quantity | SPEC.md (estimated) | Simulated |
| --- | --- | --- |
| Pump output impedance from mismatch | 200 Mohm, capped at 10 Mohm by the DC path | 6.2 Mohm at 1 kHz with exact resistors, set by the `C_NDC` DC servo that replaced the 10 Mohm DC path (which would move the node by up to 4 V); with the LT5400's 0.0125 % ratio and the other tolerances 5.07 to 7.55 Mohm (worst case), the ratio moving it by 34 kohm, `C_NDC` and the network's common 7.5 % by about 1 Mohm each (`ResistorWorstCase`) |
| Pump output impedance at 1 MHz | 1.3 Mohm from gain-bandwidth | 87 kohm: set by the strays on HC (buffer input, residual, guard lag), not by gain-bandwidth; the same with a 180 MHz or a 410 MHz amplifier |
| Same with a 50 MHz amplifier | 170 kohm, why a fast amplifier is specified | not needed: the 1 MHz output impedance is stray-limited and the current is measured |
| Pump output swing at the limit | 2.4 V plus up to 0.7 V for strays, of 3.1 V | 2.43 V at the node, 2.58 V at the amplifier, of 4.4 V; strays do not add swing, they divert current (4 to 8 uA of 15 uA reach the soil at 1 MHz) |
| Extra drop on 10 kohm for HC capacitance (guard table) | 0.42 to 1.7 V | none; the regulated current splits between strays and soil instead |
| Common-mode error | 0.2 % at 70 dB | 0.022 % (receiver at 90 dB minimum, matched buffers); one buffer at minimum CMRR gives 66.7 dB at 100 ohm m |
| Sense contact pole | 0.3 pF guarded node: 0.6 deg (5.6 kohm), 6 deg (56 kohm) | the node is 1.6 pF (the buffer's own 1.3 pF); equal contacts: 0.15 deg at 100 ohm m, 2.5 to 11 deg at 1 kohm m, up to 19 deg at 1.3 kohm m |
| Transimpedance phase lag at 1 MHz | 7 deg (8 MHz) | 52 deg (0.8 MHz with 10 pF since 23 November 2026; 10.7 deg with the 1.5 pF before), 60.7 deg for the current channel with its anti-alias RC (`ChannelCosts`); the impedance before calibration 27.6 to 27.7 deg on the rated soils (`SoilSweep`; the gain-1 channel's 32.8 deg subtract), 0 after |
| Current-channel noise | 0.9 pA/rtHz | 4.0 pA/rtHz referred to the current at 1 kHz; 1.55 pA/rtHz of it the transimpedance stage, 85 % of the power the drive's own current noise, which cancels in the impedance (`MeasurementNoise`) |
| Slew rate needed | 20 V/us | 3.8 V/us at 1 MHz into 1.3 kohm m (0.61 V peak at the pump output: the strays take the current there), 2.59 V peak at 1 kHz; ADA4807 225 V/us, no harmonic from slewing (`Distortion`) |
| Start-up | 0.8 s from the slowest network | 0.68 s to 1 mV on every ADC input (`Startup`, rails from models of the regulators) |
| Guard loop stability | unknown | stable: no pole in the right half-plane with the electrodes in air (`GuardStability`) or in soil (`LoopStability`); the least damped pair, 4.06 kHz with damping 0.27 in air, is the guard and pump mode behind the -42.5 kohm negative real part, which therefore does not oscillate |
| Card within 1 % and 1 deg | required, with 47 kohm contacts on every position | 47 kohm is not soil-like: 27 to 34 % and 3.8 to 5.3 deg at 1 MHz (CalibrationCard). With soil-like contacts (270 ohm on 39 ohm, 6.8 kohm on 1 kohm) at most 0.52 % and 0.22 deg (CalibrationCard) |
| Amplifier supply | about 10 V total | ADA4817 5 to 10 V, abs. max 10.6 V; +/-4.5 V fits. ADA4807 2.7 to 11 V |

## What the simulation cannot tell

- Noise covers the circuit only: not the ADC (its noise and quantisation),
  rail noise and ripple, or interference picked up by the stake. The noise
  of the impedance is combined from two channels by hand.
- Tolerances vary passive values only. Amplifier parameters (each one's CMRR
  error and its sign, offsets, gain) are single values; the buffers' CMRR
  mismatch and the coupling capacitors' mismatch were simulated apart, not
  together. Most part tolerances are assumptions (`MatchingTolerances`).
- The transient models clip, slew and limit current, but have no input
  common-mode range, no input protection diodes and no behaviour unpowered:
  `Startup` reports how far each input goes against its rails, it cannot
  show what the parts do there. Overload recovery from deep saturation is
  not modelled. The regulators in `Startup` are macro models (the converters
  averaged, every output stiff, the boost's input-to-output path before it
  switches left out); `V3V3_ANA`'s load-switch ramp, the converters' soft-start
  time and the AD9833's level at power-up are estimates, and the power-down
  order is not simulated.
- Stability is settled for the macro models as they are (`LoopStability`,
  `GuardStability`): every pole in the left half-plane. Real amplifiers have
  more poles than the two of the macro model, so the gain-1 driver's 49
  degrees and the pump's 16 Hz and 4 kHz modes are model figures.
- The AC benches' headroom measures still check amplitudes against the linear
  range (the AC solver never clips); `Distortion` checks the pump in time at
  three frequencies.
- No layout extraction: every stray above is an estimate.
- Each rated bench varies one condition at a time with the others nominal, so
  the rated range is not a combined worst case (SPEC.md, "Usable range").
- The benches run built-in macro models, fitted to data-sheet tables and
  figures; the amplifiers' vendor models run with `copper sim --models vendor`
  (SPEC.md, "Vendor models"), where two loops still fail. CMRR is a single
  corner fit; the AD8130's feedback pair has no input model; the ADA4807 has
  no output resistance, so the default run does not show what the gain-1
  driver's 100 ohm and 1 nF change (its margin is 49.2 degrees with either
  network).
- The ADC itself (sampling capacitor, kickback, injection) is not modelled;
  the 470 pF reservoirs and 49.9 ohm (1 nF and 100 ohm on PA1) are sized by
  hand.
- The BAT54S clamps of the first version were removed: the ADC drivers'
  supplies make them unnecessary.
- Power-down is a hand estimate ([Power cycling](#power-cycling)).
- Only the front end is simulated. The benches feed its ports with ideal DC
  rails (`Startup`: averaged regulator models without ripple or noise) and an
  ideal DDS stand-in, so converter and LDO ripple and noise, the
  ferrite and bulk capacitor on `V3V3_ANA`, the AD9833's images and the ADC's
  input are outside every bench.

## Digital and power section: power architecture

The flat part of `board.copper`. It runs from one protected Li-ion or LiPo
cell on a JST-PH connector; USB-C is kept for charging and data.

```text
USB-C VBUS --> BQ24072 IN       OUT = VSYS (3.0-4.5 V) --+--> TPS63020 buck-boost --> V3V3 (always on)
                  BAT <--> cell (J_BAT)                  |          V3V3 --FB_A--> V3V3A (VDDA, VREF+)
                                                         |          V3V3 --TPS22918 (ANA_EN)--> DDS_SW --FB_DDS--> AD9833
                                                         |                                          --FB_ANA--> V3V3_ANA (front-end ADC drivers, TIA)
                                                         +--> TPS22918 (ANA_EN) --> TPS65130 boost   -> +5.34 V -> RC -> TPS7A49 -> VP +4.50 V -> REF2033 -> V_MID
                                                         +-----------------------> TPS65130 inverter -> -5.24 V -> RC -> TPS7A30 -> VN -4.48 V
               cell --TPS22918 (PB0)--> 100k/100k --> PB1 (ADC1_IN12): battery voltage
```

### Always-on domain

The always-on domain is BAT, VSYS, V3V3 and V3V3A. It feeds the charger, the
buck-boost, the MCU with its RTC and the LSE crystal, and the shutdown inputs of
the switched parts.

### Switched domain

`ANA_EN` (PB10) switches everything that runs only while measuring. A 100 kohm
pull-down holds it low, so the switched domain stays off through reset and
Standby. It covers:

- the boost input switch (`U_SW_BST`), so the boost has no DC path from VSYS
  while off;
- both TPS65130 stages; the inverter disconnects its own input (SLVS493E
  7.3.4);
- through those, the two RC post-filters, both LDOs, VP, VN and the REF2033 that
  makes V_MID;
- the DDS supply switch (`U_SW_DDS`), whose output `DDS_SW` feeds the AD9833
  through `FB_DDS` and the front end's 3.3 V, `V3V3_ANA`, through `FB_ANA`.
  The TPS22918 carries 2 A; the AD9833 and the front end draw 8.2 mA typical,
  9.5 mA maximum (0.5 mV across its 53 mOhm).

The LDO enables are tied to their own inputs. EN must not exceed IN, and the
inputs exist only while `ANA_EN` is high.

### Leakage into the switched-off rails

The power path from the always-on domain into the switched rails is cut by
the switches above. The remaining paths are signals, which the
[firmware rules](#firmware-rules) handle: the DDS lines (rule 1) and the four
ADC inputs facing the unpowered front end (rule 2).

### Charger: TI BQ24072

The BQ24072 is a linear 1-cell charger with power path. VSYS runs the board from
USB while the cell charges; without USB, VSYS equals BAT through the internal
FET. The input withstands 28 V, so VBUS needs no separate protection. It is
reused from the pinned CopperLib dependency, and the TI PDF fetched in this
session has the same SHA-256 as the one CopperLib cites. The settings:

- **Input current limit.** EN2 is tied low. EN1 (PB13) selects USB100 (100 mA),
  which is the reset default through an internal 285 kohm pull-down, or USB500
  (450-500 mA) after enumeration.
- **Charge current.** R_ISET = 2.94 kohm gives 890 / 2940 = 303 mA (272-332 mA).
  That is 0.15 C for a 2000 mAh cell; precharge and termination are 30 mA.
- **Safety timer.** R_TMR = 68 kohm gives a 9.1 h fast-charge timer.
- **Temperature window.** TS has a 10 kohm NTC next to J_BAT, so the cell
  charges only between about 0 and 50 degC. The window matters outdoors, where
  charging a Li-ion cell below 0 degC damages it. The NTC senses the board, not
  the cell, because a 2-pin pack has no thermistor wire.
- **Status.** CHG and PGOOD are open-drain outputs read on PB7 and PB9. They use
  the MCU's internal 25-55 kohm pull-ups, so there are no external resistors and
  no current flows while the MCU sleeps.

Firmware should raise EN1 only once USB is enumerated. When the measurement load
plus the charge current exceeds the input limit, the charger's dynamic power
path management (DPPM) lowers the charge current.

### Battery voltage

TPS22918 `U_SW_VB` connects the cell to a 100k / 100k divider with 100 nF only
while PB0 is high; PB0 has a 100 kohm pull-down. The divider gives VBAT / 2,
2.1 V at 4.2 V, into PB1 (ADC1_IN12, a slow channel).

Use a long sampling time: Table 70 allows 33 kohm at 640.5 cycles, and the
100 nF supplies the charge. Wait at least 25 ms after switching on, five
50 kohm x 100 nF time constants. The divider draws 21 uA only while enabled.
Firmware should stop measuring and go to Standby below about 3.2 V, and the
cell's own protection disconnects it lower still.

### 3.3 V digital rail: TI TPS63020

The TPS63020 is a buck-boost that regulates 3.28 V from 1.8-5.5 V, so it covers
the whole VSYS range of 3.0-4.5 V. An LDO cannot do that once the cell falls
below about 3.4 V.

- **Switching.** It switches at 2200-2600 kHz in forced PWM. PS/SYNC (PB11) goes
  high while measuring, which puts the switching above the measurement band. In
  sleep PB11 is low (100 kohm pull-down) and the part runs in power-save mode.
  Power-save bursts land at audio frequencies, but nothing is measured then.
- **Quiescent current.** 25 uA typical, 50 uA maximum into VIN and VINA, plus
  5 uA (10 uA maximum) into VOUT.
- **Why not a lower-quiescent part.** A part such as the TPS63802 would save
  about 15 uA. That is under 1 % of the duty-cycled average current computed
  below, so the board keeps the TPS63020, which is already verified in CopperLib. It is
  reused from CopperLib (same PDF hash), with the TI typical application: 1.5 uH
  Coilcraft XFL4020-152ME, 2 x 10 uF in, 3 x 22 uF out, 1 M / 180 k divider,
  100 nF on VINA.

### Analog rails

The rails were sized for 40 mA typical and 70 mA maximum per rail, while
measuring only; the front end draws 51.8 mA typical and 56.9 mA maximum, inside
that range.

**Converter: TPS65131 replaced by the TPS65130.** The two have the same pins and
the same datasheet. The TPS65130's switch limit is 700-900 mA instead of
1.8-2.2 A, which is enough and means lower fault current and less inductor
stress.

- **Undervoltage lockout.** 2.1 V minimum, 2.35 V typical, 2.7 V maximum, below
  the 3.0 V cell minimum.
- **Peak currents at 3.0 V in and 70 mA per rail** (above the 56.9 mA
  drawn). TI Eq. 3 gives a boost peak inductor current of 5.34 x 0.07 / (3.0 x 0.64) = 0.20 A. Eq. 4 gives an
  inverter peak of (3.0 + 5.24) x 0.07 / (3.0 x 0.64) = 0.30 A. Both are well
  under the 0.7 A minimum limit.
- **Lower intermediate rails.** They are now +5.34 V and -5.24 V instead of
  +-6 V, which saves about 12 % of the analog power. +5.34 V is the lowest
  value that keeps VPOS at least 0.5 V above VIN with VSYS at its maximum of
  4.50 V (BAT 4.23 V + 0.27 V on USB). LDO headroom stays about 0.7 V after the
  1 ohm filters.

**LDOs: TPS7A4901 (150 mA) and TPS7A3001 (200 mA).** These replace the 1 A
TPS7A47 and TPS7A33. They have twice the needed current or more, use smaller
DGN packages and less ground current (49 uA typical for the TPS7A49), and are as
quiet: 15.4 and 17.5 uVRMS. Their quiescent current does not matter in sleep,
because their inputs are unpowered.

**Supply rejection.** At the switching frequency the RC post-filters give about
-39 dB, ahead of the LDO's own rejection.

### Clocks and sleep

**LSE crystal.** The board has a 32.768 kHz crystal, Abracon ABS07-32.768KHZ-7-T. The
internal LSI runs at 29.5-34 kHz over temperature (DS12288 Table 48), -10 % to
+3.8 %, which would drift the wake-up interval and the RTC timestamps by up to
6 minutes per hour. The crystal is +-20 ppm plus its temperature curve.

- **Drive level.** Its gm_crit is at most 0.96 uA/V. That needs LSEDRV =
  medium-high: Gmcritmax is 1.7 uA/V and the current 500 nA typical (Table 45).
- **Load capacitors.** 8.2 pF each for CL 7 pF.

**Sleep mode: Standby with the RTC wake-up timer.** It keeps BOR active and lets
firmware choose a pull per pin (DS12288 3.11.4). Wake-up is a reset, so state
that must survive goes into the backup registers or SRAM2 (+0.3 uA). Shutdown
mode would save under 1 uA but loses BOR, which is not worth it here.

## Current budget and battery life

Everything in this section is an **estimate** built on the data-sheet values
in the facts tables and the stated assumptions; nothing has been measured. The
measuring load uses the front end's actual supply current, 51.8 mA typical and
56.9 mA maximum per analog rail plus 3.7 mA typical and 4.0 mA maximum on the
switched 3.3 V ([Supply current](#supply-current); the simulation confirms the
analog rails within 0.5 mA and finds the switched 3.3 V up to 2 mA higher
while driving 1 MHz, not included here); the digital section was
first sized for an assumed 40 mA typical and 70 mA maximum per rail.

### Sleep (Standby with RTC, front end off, no USB)

Typical values are at 25 degC. Maximum values are data-sheet maxima, mostly
specified at 85 degC.

| Always-on consumer | Rail | Typ | Max | Source |
| --- | --- | --- | --- | --- |
| BQ24072 battery current, no input | BAT | 4.3 uA | 6.5 uA | SLUS810N 8.5 IBAT(PDWN) |
| TPS63020 quiescent, VIN + VINA | VSYS | 25 uA | 50 uA | SLVS916I 6.5 |
| TPS63020 quiescent into VOUT | V3V3 | 5 uA | 10 uA | SLVS916I 6.5 |
| TPS63020 feedback divider 1 M + 180 k | V3V3 | 2.8 uA | 2.8 uA | Ohm's law |
| STM32G474 Standby + RTC on LSE, medium-high drive | V3V3 | 1.0 uA | 2.8 uA | Table 35 low-drive row 0.66-0.88 uA at 3.0-3.6 V plus the 0.25 uA drive difference from Table 45; max column is 85 degC |
| TPS65130 shutdown (ENP = ENN = 0) | VSYS | 0.2 uA | 1.5 uA | SLVS493E 6.5 ISD |
| 3 x TPS22918 off (boost; DDS and front-end 3.3 V; VBAT sense) | VSYS, V3V3, BAT | 1.5 uA | 12 uA | SLVSD76C 6.5: 0.5 uA typ / 3.5 uA max at 3.3 V, 4.5 uA max at 5 V |
| Pull-downs on ANA_EN, PB0, PB11, BOOT0 | - | 0 | 0 | the pins are low |
| CHG / PGOOD pull-ups | - | 0 | 0 | internal, off in Standby; outputs high-Z without USB |
| ADC pins PA1, PA2, PA6, PA7 | - | 0 | 0 | analog mode, no pull (firmware rule 2); leakage is in the Standby figure |
| Front end, REF2033, AD9833, LDOs, divider | switched | 0 | 0 | unpowered; `V3V3_ANA` shares the DDS switch, so no switch is added |
| Cell protection IC | inside the pack | not counted | - | typically a few uA (memory) |

The 3.3 V loads total 8.8 uA typical and 15.6 uA maximum. Referred to VSYS with
an assumed 80 % power-save efficiency (unverified) and 3.7 V in, they become
9.8 uA and 17 uA.

**Sleep current from the cell: about 41 uA typical, under 90 uA worst case**
(4.3 + 25 + 9.8 + 0.2 + 1.5 uA; 6.5 + 50 + 17 + 1.5 + 12 uA). The TPS63020's
own quiescent current is 60 % of it. Moving the front end's 3.3 V onto the
switched rail keeps it so; on the always-on rail it would have added 3.7 mA.

### Measuring (front end on, 150 MHz, DDS running, no USB)

| Block | Load | Power from VSYS | Basis |
| --- | --- | --- | --- |
| 3.3 V domain | 39.7 mA typ, 49.0 mA max at 3.3 V | 146 mW typ / 190 mW max | MCU 24.5 mA at 150 MHz (Table 21, peripherals off); about 5 mA for peripherals and the HSE (estimate); 2 ADCs at 0.7 mA (Table 69); AD9833 4.5 mA typ / 5.5 mA max (Table 1); front-end ADC drivers and transimpedance amplifier 3.7 / 4.0 mA. TPS63020 efficiency 90 % typ / 85 % at max (assumed; curves not read) |
| +5.34 V boost | 52.2 mA typ, 57.4 mA max | 349 mW / 383 mW | VP load 51.8 / 56.9 mA + REF2033 0.36 mA + TPS7A49 ground current 0.05 / 0.1 mA; boost efficiency 80 % (assumed; TI quotes "up to 89 %") |
| -5.24 V inverter | 51.9 mA typ, 57.0 mA max | 363 mW / 398 mW | VN load 51.8 / 56.9 mA + LDO ground current 0.1 mA; inverter efficiency 75 % (assumed; TI quotes "up to 81 %") |
| **Total from VSYS** | | **857 mW typ, 971 mW max** | |
| **Current from the cell** | | **232 mA typ / 263 mA max at 3.7 V; 286 mA / 324 mA at 3.0 V** | power / cell voltage |

The typical figure is higher than the first sizing (185 mA at 40 mA per
rail); the maximum is lower (307 mA at 70 mA per rail), because the front
end's measured maximum is 56.9 mA, not 70 mA.

### Battery life, 2000 mAh cell

The 2000 mAh is nominal. Count on about 90 % of it being usable, which is not
applied below. Self-discharge of a few percent per month is not included either.

| Use | Average current | Life (estimate) |
| --- | --- | --- |
| Continuous measurement, typical load | 232 mA | **about 8.6 h** |
| Continuous measurement, maximum load | 263 mA | about 7.6 h |
| 5 s measurement every 10 min, typical | (232 mA x 6 s + 0.041 mA x 594 s) / 600 s = 2.36 mA | **about 850 h = 35 days** |
| Same, maximum load and worst-case sleep | (263 x 6 + 0.09 x 594) / 600 = 2.71 mA | about 740 h = 31 days |
| Sleep only | 0.041 mA | about 5.6 years, so self-discharge dominates |

The duty-cycle rows add 1 s per cycle on top of the 5 s measurement. That second
covers the wake from Standby (2.46 mA, Table 35), HSE and PLL start, rail
soft-start, front-end settling (0.68 s simulated from `ANA_EN` to every ADC
input within 1 mV, `Startup`, the rails' soft-start included; most of that
second) and storing the result. The sleep current is only about 1.7 % of the duty-cycled average, so
measurement time dominates battery life. Shortening the measurement, or
lowering the 150 MHz clock outside the sampling window, helps more than any
change to the sleep current.

### Charging

At 303 mA, a 2000 mAh cell takes roughly 7-8 h including the constant-voltage
phase (estimate). That is inside the 9.1 h safety timer. Cells above about
2300 mAh need a larger charge current or a longer timer: R_TMR up to 72 kohm,
which gives 9.6 h.

While measuring on USB the system draws about 195 mA typical and 221 mA
maximum at VSYS of about 4.4 V. With the 303 mA charge current that is 498 mA
typical and 524 mA maximum against the USB500 limit of 450-500 mA, so DPPM
cuts the charge current while measuring on USB even at the typical load. In
USB100 mode, before enumeration, the cell makes up the difference.

## Parts of the digital and power section

The front end's parts are in [The circuit](#analog-front-end-the-circuit) and its data-sheet table.

| Ref | Part, orderable number | Package / KiCad footprint | Why |
| --- | --- | --- | --- |
| J_BAT | JST S2B-PH-SM4-TB(LF)(SN) | `Connector_JST:JST_PH_S2B-PH-SM4-TB_1x02-1MP_P2.00mm_Horizontal` | JST-PH 2-pin battery connector. Pin 1 = BAT+, pin 2 = BAT-: check each cell's polarity. Assumes a protected cell |
| U_CHG | TI BQ24072RGTR (CopperLib `ti/bq24072`) | VQFN-16 | Linear charger with power path; USB100/USB500 limits; 28 V input |
| TH_BAT | Murata NCP18XH103F03RB, 10 kohm NTC | 0603 | Charger temperature window |
| U_BB | TI TPS63020DSJR (CopperLib `ti/tps63020`) | VSON-14, CopperLib footprint | 3.3 V across 3.0-4.5 V; 2.4 MHz PWM / power save |
| L3 | Coilcraft XFL4020-152ME, 1.5 uH | `L_Coilcraft_XxL4020` | TI Table 2: Isat 5.1 A, DCR 15 mOhm |
| U_SW_BST, U_SW_DDS, U_SW_VB | TI TPS22918DBVR | SOT-23-6 | Load switches (`U_SW_DDS` also feeds `V3V3_ANA`). ON VIL 0.5 V suits the STM32's 0.4 V VOL; the TPS22917 (VIL 0.35 V) failed ERC on exactly that |
| U_PWR | TI TPS65130RGER | VQFN-24 RGE | Boost +5.34 V / inverter -5.24 V, 1.25-1.5 MHz, forced PWM, 0.7-0.9 A limit |
| L1, L2 | Coilcraft XFL4020-472MEB, 4.7 uH | `L_Coilcraft_XxL4020` | TI: 4.7 uH (3.3-6.8 uH) |
| D1, D2 | onsemi MBR120VLSFT1G | SOD-123FL | 20 V, 1 A; reverse stress at most about 9.7 V |
| U_LDOP | TI TPS7A4901DGNR | HVSSOP-8 | +4.50 V, 150 mA, 15.4 uVRMS |
| U_LDON | TI TPS7A3001DGNR | HVSSOP-8 | -4.48 V, 200 mA, 17.5 uVRMS |
| U_REF | TI REF2033AIDDCR | SOT-23-THIN | V_MID 1.65 V +-0.05 % |
| U_MCU | ST STM32G474CET6 | LQFP48 | Microcontroller; two fast ADCs in dual simultaneous mode |
| U_DDS | ADI AD9833BRMZ | MSOP-10 | DDS; on the switched supply `V3V3_DDS` |
| Y1 | Abracon ABM8-25.000MHZ-10-D1G-T | 3225 | 25 MHz HSE (CL 10 pF for the gm limit) |
| Y2 | Abracon ABS07-32.768KHZ-7-T | `Crystal_SMD_3215-2Pin_3.2x1.5mm` | LSE for the RTC |
| J_USB | GCT USB4105-GF-A (CopperLib) | | USB 2.0 Type-C; Rd 5.1 kohm on CC |
| U_ESD | TI TPD4E1U06DBVR (CopperLib) | SOT-23-6 | ESD on D+, D-, CC1, CC2 |
| FB_A, FB_DDS, FB_PWR | Murata BLM18PG221SN1D | 0603 | ferrites |
| R_PF_P, R_PF_N | 1 ohm 0603 (Yageo RC0603FR-071RL) | | RC post-filters |
| J_DEBUG | Samtec TSW-108-07-G-S | 2.54 mm header | SWD + UART + BOOT0 |
| FB_ANA | Murata BLM18PG221SN1D | 0603 | `DDS_SW` to `V3V3_ANA` |
| C_ANA_BULK | 10 uF, 0805 | 0805 | bulk for `V3V3_ANA` after the ferrite |

Parts dropped in earlier revisions: TPS65131RGER (the TPS65130 is the right
size), TPS7A4701RGWR and TPS7A3301RGWR (oversized for 70 mA), TLV75533PDBVR (it
cannot hold 3.3 V from a cell), and the placeholder boundary header.

**MCU pins** beyond the ADC inputs, SPI1, TIM1, USART1, USB and SWD (all in `configure` blocks):
FSYNC is on PA4, not PA15 (reset pull-up); CC sensing on PB12/PB15 was dropped,
the charger limits the input current.

| Pin | Function |
| --- | --- |
| PC14 / PC15 | LSE crystal |
| PB0 | BAT_SENSE_EN |
| PB1 | BAT_SENSE, ADC1_IN12 |
| PB7 | CHG |
| PB9 | PGOOD |
| PB11 | buck-boost PS/SYNC |
| PB13 | charger EN1 |

## Clock configuration (verified against DS12288 Table 49 and the ST HAL)

- **PLL.** HSE 25 MHz / M 5 = 5 MHz, then x N 60 = 300 MHz VCO.
- **System clock.** / R 2 = 150 MHz SYSCLK. That is the Range 1 normal-mode
  limit, so boost mode is not needed.
- **ADC clock.** / P 6 = 50 MHz, from PLLP.
- **USB.** 48 MHz from HSI48 trimmed by CRS from USB SOF.
- **DDS MCLK.** TIM1_CH1 on PA8, ARR 5 and CCR1 3: 150 MHz / 6 = exactly 25 MHz,
  20 ns high and 20 ns low. The fallback is MCO = HSE on PA8.
- **RTC.** From the LSE, 32.768 kHz.

The divider limits are from ST `stm32g4xx_hal_rcc.h`: M 1..16, N 8..127, P 2..31,
R in {2, 4, 6, 8}.

## ADC configuration

**Pairing.** ADC1 is the master and ADC2 the slave, sharing `ADC12_COMMON`, in
dual regular simultaneous mode (`ADC_DUALMODE_REGSIMULT`). ADC1 converts I_MON on
PA2. ADC2 converts the V_MON_x selected for the current range: PA1, PA6 or PA7.
The gain is therefore selected by the ADC2 channel register, with no analog
switch.

**Timing.**

| Setting | Value |
| --- | --- |
| Sampling time | 2.5 cycles |
| Resolution | 12 bits |
| Conversion | 15 cycles = 300 ns at 50 MHz |
| Trigger | TIM8 TRGO, ARR 47 |
| Sample rate | 150 MHz / 48 = 3.125 Msps = MCLK / 8, 16 ADC clocks per trigger |

**Coherent frequency grid.** f_out = m x 3.125 MHz / 2^j gives an integer DDS
tuning word m x 2^(25-j), and a record of 2^j samples holds exactly m periods.

**Trigger latency.** Constant latency with this asynchronous ADC clock is my
reasoning, not a datasheet guarantee. The fallback is the synchronous HCLK/4
clock, whose 2.125-cycle latency is fixed in Table 69, at 2.5 Msps.

The battery sense uses ADC1_IN12 between measurements, with a long sampling time.

## Footprint audit

`copper audit-footprints --locked --offline board.copper` fails, 22 of 27
footprints resolved. Two of the failures are outside this example's files:

| Footprint | Parts | Why | What would change |
| --- | --- | --- | --- |
| `Diode_SMD:D_SOD-123F` | D1, D2 | the root `copper.mod` binds no `Diode_SMD` library; the file exists at the locked kicad-footprints revision | `footprint-library Diode_SMD github.com/KiCad/kicad-footprints/Diode_SMD.pretty` in the root `copper.mod` |
| `Connector_JST:JST_PH_S2B-PH-SM4-TB_1x02-1MP_P2.00mm_Horizontal` | J_BAT | no `Connector_JST` binding; the file exists at the locked revision | `footprint-library Connector_JST github.com/KiCad/kicad-footprints/Connector_JST.pretty` |
| `Package_SO:HVSSOP-8-1EP_3x3mm_P0.65mm_EP1.57x1.89mm` | U_LDOP, U_LDON | the locked kicad-footprints revision, 7ebfa6b (2 October 2020), has no HVSSOP-8 footprint | a newer `require github.com/KiCad/kicad-footprints REVISION` that contains it (which revision first does was not checked), and its `copper.lock` entry; every other board's KiCad footprints would then resolve from the new revision, so their footprint evidence would have to be re-run |
| `0402` | 60 front-end passives | the front end's resistors and capacitors carry the `tiny` library's default footprint text, not a KiCad footprint | footprints chosen per part in `analog/front_end.copper` (0402 or 0603, film or C0G where the table above asks for it); left for the front-end revision |
| `L_4x4mm` | `AFE/L_F1` | the same, for the filter inductor | as above |

## Simulator extensions made for this board

| Extension | Syntax | Tests |
| --- | --- | --- |
| Derived bench params | `param R_C = R_T * 2 * 3.14159 * PITCH / (4 * PAD_RADIUS);` | `copper-lang/tests/simulation.rs` (`derived_params_zipped_steps_and_references`), `copper-simulate/tests/bench_runs.rs` (`derived_params_zipped_steps_and_a_reference_run`) |
| Zipped steps | `step F_HC, F_HP in (1000%, 100%), (100%, 1000%);` | same |
| Reference runs (calibration) | `reference CARD { R_T = 1kohm; }` and `reference(x, CARD)` | same |
| Second pole, finite CMRR with a corner | `second_pole`, `cmrr`, `cmrr_corner` in `sim opamp` | `copper-sim/tests/closed_forms.rs` (`opamp_finite_cmrr_in_a_follower`), `copper-lang` (`diffamp_kind_and_new_opamp_parameters`) |
| Active-feedback receiver | `sim diffamp { inp; inn; ref; fb; out; vp; vn; ... }` | `copper-sim` (`diffamp_receiver_gain_and_common_mode`), `copper-simulate` (`a_differential_receiver_rejects_common_mode_by_its_cmrr`) |
| Load blocks sharing pins | a resistor network with its capacitance to the exposed pad | `copper-lang` (`load_blocks_share_pins_across_roles`) |
| Report | dimensionless values in percent, param limits with their values, compact step columns | `copper-simulate` format tests, `copper/tests/sim_cli.rs` |
| Benches for a module | `bench NAME for MODULE { ... }`: the module on its own, ports as nets | `copper-lang` (`a_bench_simulates_a_module_on_its_own`, `module_benches_are_found_beside_the_board`), `copper-simulate` (`a_module_bench_runs_the_module_alone_with_its_port_conditions`, `a_module_bench_gives_the_numbers_of_the_same_circuit_drawn_flat`), `copper/tests/sim_cli.rs` |
| Bench supplies and bench models | `supply NAME { voltage = q; net = NET; }`, `model NAME: KIND { ... }` | `copper-lang` (`module_bench_supplies_and_models_are_checked`, `bench_models_and_supplies_parse`) |
| Params in modules | `param NAME = q;` in a module | `copper-lang` (`module_params_compile_into_the_module_definition`) |
| Several noise outputs, noise of a derived quantity (phase F2) | `output = V(V_MON_1), V(I_MON);`, `noise(V(V_MON_1), U_DIF)`, `noise(z1)` | `copper-simulate/tests/bench_language.rs` (`several_noise_outputs_and_the_noise_of_a_ratio`), `copper-lang/tests/bench_language.rs` (`noise_of_named_and_derived_quantities`), `copper-sim/tests/noise_closed_forms.rs` (`a_weighted_output_is_the_weighted_sum_of_node_voltages`) |
| Optional fixture components (phase F2) | a net declared again adds pins; a 0 F capacitor is absent; `fixture B for T using A` | `copper-simulate/tests/bench_language.rs` (`optional_components_in_fixtures`) |
| Stability (phase F3) | `analysis loop { ...; probe = U_PMP.OUT; }` with `phase_margin(loop_gain())`, `analysis pz;` with the built-in `stable` | `copper-sim` (`stability::tests`), `copper-lang/tests/stability.rs`, `copper-simulate/tests/stability_runs.rs` |

Since then the plan's later phases added what this board's benches now use:
fixtures (`using`), limits per step (`where`), `upto` and `downto`, windowed
reductions, noise analysis, tolerances with Monte Carlo and worst case, and
transient analysis with op-amp limits and waveforms.

Gaps and problems found while moving the benches onto them (stable binary,
sources 03f60a26cdc7), and where they stand after phases F1 and F2 of the
[plan](../../../docs/integrated-simulation-plan.md):

- **A bench could not add a pin to a fixture's bench net**, and a capacitor
  of 0 F was refused, so a fixture could not hold an optional stray. Fixed:
  nets of one name are one net, and a 0 F capacitor is absent.
  `StrayCapacitance` uses `SoilModel` now (same values to the last digit).
  `MatchingTolerances` still puts the LT5400's pad capacitance on `load`
  models; moving it to bench capacitors on `GND` would change its values in
  the last digits (another summation order) and was not done.
- **`upto` and `downto` had no "never" result.** Fixed: a comparison that
  fails at the first sample gives "never", which fails a limit and is
  reported without one. `CalibrationCard` and `ContactImbalance` still report
  the phase's usable frequency only; a `usable_to_magnitude` could be added.
- **A bench evaluated every `let`, used or not.** Fixed: a let nothing reads
  is not evaluated. `FirmwareReadout` stays a fixture of its own, which no
  longer matters to `Startup`.
- **A matched group could only be declared on passive components.** Fixed: a
  component of a part made of `load` blocks takes `tolerance`, `match` and
  `match_tolerance` (its units are the members), and a bench can match model
  parameters (`tolerance RN_HOW.resistance = 7.5% match 0.0125%;`).
  `MatchingTolerances` still rebuilds the network from bench resistors, so
  its results stand as they are.
- **A fixture could not override another fixture's param** (SIM002). Fixed: a
  fixture may use another and override its declarations. The benches that
  step contacts as plain values still declare `R_CONTACT` themselves.
- **A noise analysis had one output**, so the noise of four ADC channels was
  four benches and the impedance noise was combined by hand. Fixed:
  `MeasurementNoise` has four outputs and measures the impedance noise
  (`noise(z1)`).
- **One transient limit per part**: the ADA4807's slew and swing depend on its
  supply (Tables 2 and 4). A part block can now carry parameter sets per
  supply span (`supply 3.3V { ... }`), but the ADA4807 part has not been
  given them, so the 3.3 V stages still take the +/-5 V figures. An output
  current limit needs an output resistance, which the ADA4807 model does not
  have.
- **The analysis bounds printed** were the params' defaults when the bench
  steps them. Fixed: `Distortion` now reads "tran 0 to 30 us..60 ms,
  recorded from 20 us..50 ms".
- **Transient runs were slow**: phase F1 brought `Distortion` from about 19
  minutes to 19 s in a debug build; phase F9's periodic steady state to about
  8 s (12 threads), with no settling left in the record.
- `Gohm` exists now.
- Not taken up yet: the negative-conductance measure of `PumpOutputImpedance`
  still covers 1 kHz to 1 GHz; a window could now restrict it.

## Data-sheet facts: analog front end

| Part | Fact | Source | Status |
| --- | --- | --- | --- |
| ADA4817-1ARDZ | SOIC pinout, exposed pad floating | Chinese edition Rev. A, p. 6, p. 17 | verified (Chinese translation; the English PDF could not be downloaded) |
| | FB pin carries the output (board ties FB to OUT) | p. 17 | verified from the text; the internal connection itself is not stated |
| | 410 MHz GBW, 65 dB (62 min) open-loop gain, 1.3 / 0.1 pF input capacitance, 500 Gohm, 2 pA, 0.4 mV, 4 nV/rtHz, 2.5 fA/rtHz, 870 V/us, CM range to +VS - 2.8 V, CMRR 77 / 90 dB, swing VS - 1.1 V, 19 / 21 mA, 5 to 10 V, abs. max 10.6 V, PD > +VS - 1 V | Table 1 p. 3, p. 5, Table 8 p. 16 | verified |
| | second pole 600 MHz, CMRR corner 300 kHz, output resistance 10 ohm | figures 33, 27, 28 | derived (fits) |
| | transient limits: slew 870 V/us, swing VS - 1.1 V (minimum, RL = 1 kohm), output current 40 mA (linear); short circuit 100 / 170 mA | Table 1 p. 3 | verified |
| | voltage noise 1/f corner 8 kHz (Figure 30: about 12 nV/rtHz at 1 kHz, 5.4 at 10 kHz, 60 at 100 Hz); no current-noise corner | Figure 30 p. 11 | **estimate** (fit to a graph; the figure lies above the fit below about 300 Hz) |
| AD8130ARZ | pinout, gain setting, G = 1 without parts | Rev. C Figure 1, Figure 3 | verified |
| | 270 MHz (G = 1), 74 dB, CMRR 90 min / 110 typ to 100 kHz, 80 dB min at 2 MHz, 70 dB at 10 MHz, +/-3.8 V CM, +/-2.5 V differential, 6 / 4 Mohm, 3 / 4 pF, 0.5 / 2 uA, 0.4 / 1.8 mV, 12.5 nV/rtHz, 1090 V/us, 10.8 / 11.6 mA, PD high | Table 2 pp. 5-6 | verified |
| | GBW as 270 MHz, second pole 430 MHz, CMRR corner 630 kHz, 25 ohm output | Figure 74, 76; table fit | derived |
| | slew 950 min / 1090 typ V/us, output current 40 mA, short circuit -60 / +55 mA, swing +/-4.0 V minimum at +/-5 V (RL = 1 kohm), REF/FB current noise 1.4 pA/rtHz | Table 2 pp. 5-6 | verified |
| | swing 1.0 V from each rail at +/-4.5 V | Table 2 gives +/-5 V and +/-12 V only | **estimate** |
| | voltage noise 1/f corner 110 Hz, current noise corner 3 kHz | Figures 67, 66 p. 20 | **estimate** (fits to graphs) |
| | input model of the FB/REF pair | none given | unverified (left out) |
| ADA4807-1ARJZ | SOT-23 pinout, DISABLE active low | Rev. B Figure 6, Table 7 p. 10 | verified |
| | 180 MHz, 130 / 113 dB (+/-5 V / 3 V), 1 / 1 pF, 45 Mohm / 35 kohm, -1.2 / -1.6 uA, 20 / 125 uV, rail-to-rail +/-0.2 V, CMRR 110 dB, 1.0 / 1.1 mA, 2.7 to 11 V, inputs +/-VS +/- 0.2 V, differential +/-1.4 V abs. max | Tables 2, 4, 5 | verified |
| | GBW as 180 MHz, second pole 300 MHz, CMRR corner 25 kHz | figures 23, 25 | derived |
| | voltage noise 3.1 nV/rtHz (100 kHz), 3.3 (1 kHz), 5.8 (10 Hz), 1/f corner 29 Hz; current noise 0.7 pA/rtHz (100 kHz), 10 (10 Hz), corner 2 kHz | Table 2 p. 3 (the same in Table 4) | verified |
| | slew 225 / 250 V/us at +/-5 V, 118 / 237 V/us at 3 V (rise / fall); swing +VS - 0.08 V and -VS + 0.1 V minimum at +/-5 V, 0.04 V at 3 V; linear output current 50 / 60 mA, short circuit 80 mA; HD2 / HD3 -95 / -79 dBc at 1 MHz, 2 V p-p | Tables 2, 4 pp. 3-8 | verified; the model takes the +/-5 V rising slew and swing for every instance, the 3.3 V stages included |
| | output resistance | not given (Figure 54: closed-loop output impedance) | unverified (none in the model, so no output current limit either) |
| LT5400AIMS8E-1 | pinout (R1 1-8, R2 2-7, R3 3-6, R4 4-5), pad floating, 10 kohm, 0.0125 % matching, 5.5 pF to pad, 1.4 pF between resistors | Rev. A pp. 2-4 | verified; the 5.5 pF split 2.75 pF per end is an approximation |
| BAT54S | pinout, ratings | v.6 | verified; no longer used, part file removed |
| OPA810 (trade study only, `SenseBufferTrade`) | 3.7 / 4.6 mA, 70 MHz GBWP, 120 dB, 12 Gohm in parallel with 2 pF, 0.5 pF differential, 2 pA, CMRR 100 dB; SOT-23-5 pinout | ZHCSK40D (Chinese edition of SBOS799D), section 7.5 and pin table, sha256 f75562e535f6fe29ae64d8df795649d7dae49534e7f1cc6f795c34c63828d3b9 | verified |
| | CMRR corner 2 kHz, second pole 120 MHz, 4 ohm output | figures 7-50, 7-42, 7-49 | derived |
| Footprints (KiCad names for SOIC-8-EP, MSOP-8-EP, SOT-23-6) | land patterns | KiCad library names | unverified against the vendors' land patterns |
| Probe header Samtec TSW-108-07-G-S | part number | memory | unverified |
| Electrode pad | 8 x 8 mm, ENIG | SPEC.md | no footprint drawn yet |

Data sheets were read in this session from LCSC and datasheets.com copies;
their SHA-256 hashes are in each part file.

## Data-sheet facts: digital and power section

"Verified" means read in a document fetched in this session. The document's
SHA-256 is in the part file. "Memory" means not checked in this session. Facts
unchanged from the first revision are marked "as before".

| Fact | Value | Source | Status |
| --- | --- | --- | --- |
| STM32G474CET6 pinout, I/O structures, ADC channels, AF numbers | see part files | DS12288 Rev 3 Tables 11-13, ST open pin data @45c0459 | Verified (as before) |
| Supply ranges, absolute maxima, I/O levels, HSE, PLL, ADC, RAIN | | DS12288 Tables 14, 17, 44, 49, 57-59, 69, 70 | Verified (as before) |
| Reset pull-ups on PA13, PA15, PB4 and pull-down on PA14 | | DS12288 Table 12 note 4 | Verified |
| Standby: BOR on, per-pin pull configurable, RTC wake-up | | DS12288 3.11.4 | Verified |
| RTC runs in all low-power modes on LSE; on LSI not in Shutdown | | DS12288 3.25 | Verified |
| Standby current, RTC on LSE (low drive): 0.655 uA at 3 V, 0.875 uA at 3.6 V (25 degC); 2.1-2.8 uA at 85 degC; wake-up current 2.46 mA | | DS12288 Table 35 | Verified |
| Shutdown current, RTC off: 43 nA at 3 V | | DS12288 Table 36 | Verified |
| LSE current 250 / 315 / 500 / 630 nA and Gmcritmax 0.5 / 0.75 / 1.7 / 2.7 uA/V by drive level; start-up 2 s | | DS12288 Table 45 | Verified |
| LSI 31.04-32.96 kHz at 3 V, 30 degC; 29.5-34 kHz over -40 to 125 degC | | DS12288 Table 48 | Verified |
| Weak pull-up and pull-down 25 / 40 / 55 kohm | | DS12288 Table 57 | Verified |
| Run current 24.5 mA typ at 150 MHz | | DS12288 Table 21 | Verified |
| PLL, MCO and HSI48 settings | | ST HAL `stm32g4xx_hal_rcc.h` @a600128 | Verified (as before) |
| ADC1/ADC2 pair and dual regular simultaneous mode | | ST CMSIS `stm32g474xx.h`, HAL | Verified (as before) |
| Timer clock = PCLK at prescaler 1; TIM1/TIM8 synchronisation; DMA packing | | RM0440 | **Memory** |
| UCPD dead-battery behaviour of PA9/PA10 and PB4/PB6 at reset | | RM0440 | **Not checked** |
| AD9833 pinout, supply, absolute maxima, levels, MCLK timing, VOUT, decoupling | | AD9833 Rev. F Tables 1-4, Figure 2 | Verified (as before) |
| BQ24072 IN range 4.35-6.6 V, up to 26 V without damage; IN cap 1-10 uF | | SLUS810N Table 7-1 | Verified |
| BQ24072 EN1/EN2: 100 mA, 500 mA, ILIM, standby; internal 285 kohm pull-downs; ILIM open disables charging; KILIM 1500-1720 | | SLUS810N Table 7-1, Table 7-2, 8.5 | Verified |
| BQ24072 KISET 797 / 890 / 975; KPRECHG 70 / 88 / 106; KTMR 36 / 48 / 60 s/kohm, RTMR 18-72 kohm, fast-charge timer 10 x KTMR x RTMR | | SLUS810N 8.5 | Verified |
| BQ24072 VBAT(REG) 4.16 / 4.20 / 4.23 V; OUT 3.3-3.5 V when VBAT < 3.2 V, else VBAT + offset | | SLUS810N 8.5 | Verified. The offset value (225 mV typ, 150-270 mV) is from CopperLib's notes on the same PDF |
| BQ24072 IBAT(PDWN) 4.3 uA typ / 6.5 uA max, no input power | | SLUS810N 8.5 | Verified |
| BQ24072 TS needs a 10 kohm NTC or a 10 kohm fixed resistor; charging between 0 and 50 degC; CHG and PGOOD open drain | | SLUS810N Table 7-1; window from CopperLib's notes | Pin text verified; window **reused** |
| BQ24072 RGT pin numbers | | CopperLib part (SLUS810N, same PDF hash) | Reused |
| TPS63020 VIN 1.8-5.5 V | | SLVS916I 6.5 | Verified |
| TPS63020 fsw 2200 / 2400 / 2600 kHz | | SLVS916I 6.5 | Verified |
| TPS63020 Iq 25 / 50 uA (VIN + VINA) plus 5 / 10 uA (VOUT); shutdown 0.1 / 1 uA | | SLVS916I 6.5 | Verified |
| TPS63020 VFB 0.5 V; EN and PS/SYNC VIH 1.2 V, VIL 0.4 V | | SLVS916I 6.5 | Verified |
| TPS63020 PS/SYNC: 1 = power save off, 0 = on; must not float | | SLVS916I pin table, 7.4 | Verified |
| TPS63020 typical application: R1 1 M / R2 180 k; 1.5 uH XFL4020-152ME; 3 x 22 uF out; 10 uF in; VINA 0.1 uF (at most 0.22 uF) | | SLVS916I 8.2, Tables 2 and 3 | Verified |
| TPS63020 DSJ pin numbers | | CopperLib part (same PDF hash) | Reused |
| TPS22918 pins | | SLVSD76C Pin Functions | Verified |
| TPS22918 ON VIH 1 V, VIL 0.5 V | | SLVSD76C 6.3 | Verified |
| TPS22918 IQ 8.3 / 15 uA; shutdown 0.5 / 3.5 uA at 3.3 V (4.5 uA max at 5 V) | | SLVSD76C 6.5 | Verified |
| TPS22918 RON 53 mOhm; 2 A; absolute maximum 6 V | | SLVSD76C 6.1, 6.5 | Verified |
| TPS22917 ON VIL 0.35 V, shutdown 10 / 100 nA (the rejected part) | | SLVSDW8B 7.3, 7.5 | Verified |
| TPS65130 switch limits 700 / 800 / 900 mA (TPS65131 1800 / 1950 / 2200 mA) | | SLVS493E 6.5 | Verified |
| TPS65130 UVLO 2.1 / 2.35 / 2.7 V; shutdown current 0.2 / 1.5 uA | | SLVS493E 6.5 | Verified |
| TPS65130RGER is active | | SLVS493E orderable addendum | Verified |
| TPS65130 VPOS >= VIN + 0.5 V; Eq. 3 and 4 for peak inductor current; load disconnect behaviour; other facts | | SLVS493E 6.3, 7.3.4, 8.2 | Verified (as before) |
| TPS7A49 and TPS7A30 pinouts, ranges, VFB, enable, noise; TPS7A49 ground current 49 / 100 uA | | SBVS121E, SBVS125D | Verified (as before) |
| REF2033 pinout, ranges, accuracy, 0-10 uF | | SBOS600F | Verified (as before) |
| ABS07: ESR 70 kohm max; C0 2.0 pF max; CL code 7 = 7 pF; +-20 ppm; -40 to 85 degC | | Abracon ABS07, revised 08-10-22 | Verified |
| ABS07-32.768KHZ-7-T is stocked by distributors | | | **Not checked** |
| ABM8 25 MHz facts | | Abracon ABM8 | Verified; pad functions **memory** (as before) |
| JST S2B-PH-SM4-TB pin 1 and ratings | | CopperLib main `jst/ph-s2b-sm4` | **Reused**; the JST PDF could not be fetched |
| Murata NCP18XH103F03RB (10 kohm, B 3380 K) and BLM18PG221SN1D ratings | | Murata | **Memory**, not fetched |
| Coilcraft XFL4020-472MEB ratings (Isat must exceed 0.9 A) | | Coilcraft | **Memory**; the XFL4020-152ME ratings come from TI Table 2 |
| Converter efficiencies (90 / 80 / 75 %) | | TI efficiency curves | **Assumed**, curves not read |
| Cell protection IC current, cell self-discharge, Type-C and USB 2.0 rules | | | **Memory** |

## Unresolved and doubtful

- **Firmware rules are not checked.** Back-powering the AD9833 through its
  inputs, and `V3V3_ANA` through the ADC pins, is prevented only by firmware
  rules 1 and 2. `copper power-check` passes because neither the AD9833 nor
  the front end's amplifiers declare power domains, not because it verified
  those paths.
- **DDS output at power-up.** The AD9833 comes up with `V3V3_ANA`, about
  0.5 ms before VN and 0.7 ms before VP leave 0 V (`Startup`, regulator
  models), and its output's DC level drives `U_DRV`'s input above the
  unpowered amplifier's rails: 0.18 V in `Startup`, inside the 0.2 V rating,
  with the AD9833's level at power-up estimated (0.34 V); at its 0.65 V
  maximum it would be 0.33 V. Its reset output level was not read. Remedies,
  not built: switch the DDS supply after VP, or put a series resistor ahead of
  `U_DRV`'s input.
- **Sense-path rejection with real parts.** `SenseCommonMode` (a buffer at its
  minimum CMRR) and `ResistorWorstCase` (10 % X7R coupling capacitors
  mismatched: down to 77.6 dB at 1 MHz, 100 ohm m) each stay inside the 63 dB
  budget; both at once were not simulated.
- **Power-down order.** Whether `V_MID` always falls before `V3V3_ANA` when
  `ANA_EN` falls is a hand estimate; the TPS22918's output-discharge
  resistance and the load switch's rise time with CT open were not read. If
  measurement shows `V_MID` lagging, a series resistor into `U_TIA`'s
  non-inverting input, or a capacitor on CT, are the obvious remedies; neither
  is built (the front end is revisited later).
- **Inrush.** `U_SW_DDS` now charges about 20.5 uF at turn-on (10 uF and
  100 nF at the DDS, 10 uF and 4 x 100 nF on `V3V3_ANA`) with CT open. The
  resulting dip on the always-on `V3V3` (3 x 22 uF) was not estimated from
  data-sheet figures.
- **AD8130 inputs while unpowered.** Their absolute maximum was not read.
- **Footprint resolution.** See [Footprint audit](#footprint-audit): the root
  `copper.mod` needs two library bindings and a newer kicad-footprints
  revision, and the front end's passives have no footprints yet.
- **Documents not fully read.** DS12288 was read in Rev 3 and RM0440 was not
  read. The converter efficiency curves and the LDO supply rejection at
  1-2.6 MHz were not read either.
- **Switcher layout.** No hard macro or reference layout is bound for the
  TPS65130 or the TPS63020. The placement limits in the board are engineering
  values that need review.
- **Charging assumptions.** The NTC measures board temperature, not the cell.
  The charge current and timer are sized for 1000-2300 mAh cells. Charging
  while measuring on USB is cut back by DPPM.
- **Budget.** The whole budget is an estimate. Measure the sleep current and
  the measuring current on hardware.
- **Simulation scope.** Only the front end is simulated; the converters, the
  LDOs' noise and rejection, the DDS and the ADC are port conditions or
  absent ([What the simulation cannot tell](#what-the-simulation-cannot-tell)).
