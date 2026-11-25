# Coupling between nets

Status: implemented as `copper coupling` (steps 1 to 5 of section 3). Screening evidence only, like the rest of the
full-wave tiers (`docs/emi-noise-analysis.md` section 11, `docs/pcb-antenna-synthesis-research.md`).

The question: *how much does this net couple to that one*, for example a radio's antenna to a QSPI clock, or one
fast line to its neighbour. A coupling is the transmission `S_k1` from a port on the aggressor to a port on the
victim, each port a lumped element from one end of its net down to the ground plane under it, terminated in that
end's impedance. Both full-wave models run the pinned `ems` FDTD solver:

| Piece | Where | What it does |
| --- | --- | --- |
| Net ends, geometry, terminations, region coupling | `copper_fullwave::coupling` | A net's ends (its pads, or free track ends); its copper through the region model's translator (tracks at any angle, arcs, vias, pads, any layer); each end's termination; the region model of an aggressor, its victims and the ground nets (native zone fills, planes continued into the PML, the smoothing mesher), one solve per aggressor end |
| Antenna-to-net coupling | `copper_antenna::model::{Victim, cut_ground, evaluate_coupled}` | The antenna site model with victims from that translator, the site's ground cut around them, and a port at each of their ends; `S_k1` from the feed |
| Desensitisation | `copper_emi::desense` | The power a signal's lines deliver into the antenna port through `S`, against a declared sensitivity and margin |
| Command | `copper coupling` (`crates/copper/src/coupling.rs`, schema `copperscript-coupling/v0.1`) | `--from NET` (region model) or `--antenna NAME` (site model), `--to NET`, `--run RUN`, `--terminate END=OHMS`, `--describe`, `--json`; `EMI-RF-DESENSE` in antenna mode |
| Validation | `crates/copper-extract/tests/coupled_microstrip.rs`, `crates/copper-antenna/tests/victim_coupling.rs`, `crates/copper/tests/coupling.rs` | Sections 1 and 2; the command on `examples/coupling` |

`evaluate` is `evaluate_coupled` with no victims and builds the same model as before, so synthesis results and
`MODEL_REVISION` are unchanged.

## 1. Accuracy of line-to-line coupling (region model)

Two 1 mm strips 0.5 mm over a plane (er 4.3, about 50 ohm), 20 mm long, 50 ohm ports at all four ends, cell
0.25 mm, 0.5 to 3 GHz. Worst |error| in dB of the near-end (S31) and far-end (S41) coupling against the reference,
over 36 frequencies (4 threads, release):

| Gap | Reference NEXT peak | Full wave NEXT peak | NEXT worst error | FEXT worst error | Run |
| --- | --- | --- | --- | --- | --- |
| 1 mm (2 h) | -24.66 dB | -24.12 dB | 0.96 dB | 0.59 dB | 0.82 M cells, 16 s |
| 3 mm (6 h) | -38.13 dB | -36.55 dB | 2.70 dB | 1.00 dB | 18 s |
| 3 mm, planes continued into the PML | | -37.10 dB | 2.02 dB | 0.66 dB | 15 s |
| 5 mm (10 h) | -45.69 dB | -42.94 dB | 4.30 dB (0.89 at 0.5 GHz) | 1.43 dB | 18 s |
| 5 mm, planes continued | | -43.91 dB | 3.18 dB (0.08 at 0.5 GHz) | 0.95 dB | 16 s |
| 8 mm (16 h) | -53.59 dB | -48.75 dB | 6.62 dB (1.65 at 0.5 GHz) | 2.10 dB | 20 s |
| 8 mm, planes continued | | -50.39 dB | 4.92 dB (0.39 at 0.5 GHz) | 1.55 dB | 21 s |

A finer cell (0.167 mm) and a deeper decay threshold (-50 dB) changed the 3 mm case by at most 0.04 dB. The full
wave reads high in every case. Above about -40 dB it agrees within 1 to 3 dB; below that the difference grows with
frequency. Where it comes from was measured (`the_weak_coupling_excess_is_separated_by_length_and_a_stub_only_control`,
8 mm pair, planes continued; `excess` is `|S31_fw - S31_ref|`):

| Case | NEXT at 3 GHz, full wave / reference | Excess at 1.5 / 3 GHz | Same ports, stub pads only (no line) |
| --- | --- | --- | --- |
| 10 mm, er 4.3 | -54.27 / -54.74 dB | -77.3 / -72.0 dB | -95.5 / -90.4 dB |
| 20 mm, er 4.3 | -50.45 / -55.37 dB | -69.8 / -57.1 dB | -95.5 / -90.4 dB |
| 40 mm, er 4.3 | -48.72 / -53.90 dB | -61.1 / -55.7 dB | -95.5 / -90.4 dB |
| 20 mm, **air** (er 1) | -51.21 / -51.81 dB | -74.3 / -65.2 dB | -89.2 / -83.9 dB |

- **Not the ports.** With the lines removed the same four port elements couple at -84 to -105 dB, 30 to 40 dB
  below the excess.
- **Distributed along the line.** The excess grows with length (by 15 dB at 3 GHz from 10 to 20 mm).
- **Not radiation; the reference's quasi-static modes.** In air both modes are TEM and the reference is exact: the
  full wave then agrees within **0.60 dB at -52 to -62 dB of coupling** over the whole band
  (`weak_coupling_in_air_matches_tem_theory_within_one_db`). On FR-4 the field is partly in air and partly in the
  substrate, the modes disperse with frequency, and weak coupling is set by the difference of two nearly equal
  mode impedances (49.22 and 49.01 ohm for the 8 mm pair), so a small dispersive change of either moves it by
  several dB. The static 2-D reference leaves that out; the full wave does not.

**Reading the numbers:** the full wave resolves coupling to at least -62 dB within 1 dB where its reference is
exact, and its port elements are 30 dB below that. The weak-coupling difference on FR-4 is a limit of the
reference, not of the model; it is not proven against a dispersive reference (Kirschning-Jansen coupled-line
dispersion would be the next check), so the evidence grade stays `screening`. Coupling runs continue planes into
the PML (`RegionSpec::extend_planes`), which removed 1 to 2 dB of truncation error in every case.

## 2. Antenna-to-net coupling (antenna site model)

`examples/antenna_synthesis/board.copper` (2.4 GHz monopole in the top-right corner, GND on F.Cu and B.Cu over
1.51 mm FR-4) with its tuned geometry and a synthetic 0.5 mm F.Cu trace, 14 mm long, parallel to the monopole's
arm, ported to B.Cu at both ends (`crates/copper-antenna/tests/victim_coupling.rs`, coarse fidelity, 4 threads).
The victim's copper comes from the region model's translator, and the site's ground is cut around it by the board
clearance **widened to one coarse cell** (0.5 mm here), as the antenna model already widens its feed slot:

| Case | Model | At 2.44 GHz, near end / far end of the trace | Peak over the sweep | S11 change |
| --- | --- | --- | --- | --- |
| antenna alone | 0.29 M cells, 9,001 steps, 10 s | | | |
| trace 2.5 mm below the antenna region | 0.31 M cells, 10 s | -28.9 / -31.0 dB | -27.7 dB | at most 0.03 dB |
| trace 18 mm below the region | 0.41 M cells, 13 s | -48.3 / -48.9 dB | -46.8 dB | at most 0.04 dB |
| 2.5 mm below, with a 45-degree jog of 2 mm | 0.39 M cells, 38 s | -30.6 / -33.2 dB | -29.4 dB | |

Two mesh faults were found and fixed on the way, both of which made a victim touch the ground in the discretised
model while the geometry was right (`ground_is_cut_around_a_victim_and_its_stitching_removed` checks the geometry):

- **A cut narrower than the cell.** With the board clearance (0.2 mm) on a 0.5 mm mesh the ground edge snapped onto
  the victim's and shorted it: the far trace read -69 dB and the jogged trace's far end -62 dB. Widening the cut to
  one coarse cell and keeping its axis-aligned edges on mesh lines of their own fixed both.
- **Slivers.** Pinning every vertex of a cut (and, on a routed board, two cut edges microns apart where a track
  meets its pad) left cells of 0.002 mm and a time step 50 times shorter. Only cut edges at least one cell long get
  a line, and none within a quarter cell of a line already planned.

Checked: coupling falls with distance (by 19 dB from 2.5 to 18 mm), no port receives more than the feed delivers, a
far victim leaves S11 unchanged, and the jog moves the coupling by 2 dB. Not checked: accuracy against an
independent reference (there is no closed form for this geometry); the widened cut exposes the victim more than the
board's 0.2 mm clearance would, so the numbers lean high, the conservative side. The coupling is referenced to the
wave incident at the feed; the bare antenna has an S11 of -3.3 to -4 dB in band (an L-section matches it on the
board), so relative to the power the antenna accepts the numbers are about 2.5 dB higher.

## 3. The command on a routed board

`examples/coupling/board.copper`: the monopole above, a 48 MHz clock `CLK` from `U2` (declared as 3.3 V open circuit
behind 33 ohm, 1 ns edges) to `U3` routed 4 to 5 mm below the antenna region, and the radio `U1` at (30, 14) mm, whose
feed line the router ran from `U1` past `U2` to the feed land (0.75 mm from `U2.1`, within 0.7 mm of the start of
`CLK`). Routed with `route-board --checkpoint-dir`; 4 threads, coarse fidelity:

| Question | Model | At 2.44 GHz | Run |
| --- | --- | --- | --- |
| `--antenna bt --to CLK` | site model, 0.34 M cells | -36 dB to the driver end `U2.1` (33 ohm, declared), -41 dB to `U3.1` (50 ohm, default) | 15 s |
| `--from CLK --to ANT_FEED` | region model, 2.34 M cells, window clipped to the board | -16 / -13 dB from `U2.1` / `U3.1` to the feed land `ANT1.1`, -23 / -22 dB to the radio's pad `U1.1` | 307 + 348 s |

The two answer different questions, and the difference is the board's: the site model holds the radiator fed at
its land, the region model also holds the routed feed line, which runs next to the clock. The command says so in
antenna mode. Found on the way: the first region run continued the planes past the board edge into the space
around the antenna (3,480 mm^2 of GND per layer on a 1,200 mm^2 board); the window is now clipped to the board and
planes are continued only when every window edge is inside it (flag `planes_truncated` otherwise).

**EMI-RF-DESENSE** (antenna mode, `copper_emi::desense`). The victim's declared signal at its driver's end: clock
lines (`clock_frequency`) or, for `data_rate`, the continuous bound on any line (an upper bound). The power a line
puts into the network is `a^2 / (8 R)` for a Thevenin source (`open_circuit_swing` behind `output_impedance` `R`,
the available power) or `a^2 / (2 R)` for a loaded `signal_swing`; times `|S|^2` from the driver's end to the feed
(by reciprocity the run's `S` from the feed) it is the spur at the receiver. It passes when the strongest in-band
spur is at most `sensitivity_dbm - desense_margin_db`, both declared on `constraint antenna` (outside the site
digest: `tuned_for` stays valid, checked). On the example (synthetic receiver, -95 dBm, 6 dB margin): only the 51st
harmonic, 2448 MHz, falls in 2400 to 2483.5 MHz (a 50 % clock has no even ones), -39.8 dBm from the driver
(hand check: 5.3 mV open circuit behind 33 ohm, -39.7 dBm), -36.1 dB coupling, **-75.9 dBm at the feed: fail by
25.1 dB**. Without `sensitivity_dbm` the spurs are reported with no verdict; a net that is not a declared fast signal
is `not_applicable`.

## 4. Plan and status

1. **Separate the weak-coupling floor.** Done (section 1): not the ports, not radiation; the quasi-static
   reference's lack of modal dispersion. Open: a check against a dispersive coupled-microstrip reference.
2. **Victim geometry.** Done: `copper_fullwave::coupling::net_geometry`, shared by both models (section 2).
3. **Terminations.** Done: `--terminate END=OHMS`, else the driver's declared `output_impedance`, else 50 ohm, and
   every end says which. Open: the language declares no receiver input impedance, so a receiving end is 50 ohm.
4. **Query surface.** Done: `copper coupling` (section 3). Open: the feed line in antenna mode (today: ask the
   region model for it), back-side ends (a port goes down to a plane below its end), and a region model that
   reaches a board edge ends its planes at the window.
5. **Desensitisation finding.** Done for antenna mode (section 3). Open: desense in region mode (the receiver's
   pad as the antenna port), spurs of data signals as lines rather than a bound, and a channel bandwidth for
   random data.

Not checked against a measurement anywhere. The fit tests `copper-fullwave/src/fit.rs` (four of them) fail on the
base commit this work started from as well; they are not touched here.
