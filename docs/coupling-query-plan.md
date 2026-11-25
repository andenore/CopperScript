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
arm, ported to B.Cu at both ends. Coarse fidelity (0.5 mm cell: the trace is one cell wide), 4 threads:

| Case | Model | Coupling in band (2.40 to 2.48 GHz), near end / far end of the trace | Peak over the sweep | S11 change |
| --- | --- | --- | --- | --- |
| antenna alone | 0.29 M cells, 9,001 steps, 12 s | | | |
| trace 2.5 mm below the antenna region | 0.33 M cells, 12 s | -34.8 to -35.1 / -38.5 dB | -34.3 dB | at most 0.01 dB in band |
| trace 18 mm below the region | 0.44 M cells, 17 s | -54.7 to -55.3 / -52.3 to -52.8 dB | -51.6 dB | at most 0.05 dB |

Checked: the coupling falls by 17 dB with distance, no port receives more than the feed delivers, and a far victim
leaves S11 unchanged. Not checked: accuracy. There is no closed form for this geometry, the far case sits in the
weak range of section 1 (resolved where the reference is exact), and the trace is one cell wide at coarse fidelity. The coupling is
referenced to the wave incident at the feed; the bare antenna has an S11 of -3.3 to -4 dB in band (it is matched
by an L-section on the board), so relative to the power the antenna accepts the numbers are about 2.5 dB higher.

## 3. Plan

1. **Separate the weak-coupling floor.** Done (section 1): not the ports, not radiation; the quasi-static
   reference's lack of modal dispersion. Open: a check against a dispersive coupled-microstrip reference.
2. **Victim geometry.** Take victims from the board translator of `copper-fullwave` (any-angle tracks, arcs, vias,
   inner layers, pads, real zone fills) instead of the prototype's axis-aligned F.Cu tracks, and share it between
   the two models.
3. **Terminations.** Allow each victim end its driver's or receiver's impedance (from the part's pin model where
   one exists) rather than 50 ohm, and say which was used.
4. **Query surface.** A `copper coupling BOARD --from NET|PAD --to NET [--band ...]` command that picks the
   antenna site model when `--from` is an antenna feed and the region model otherwise, prints the model
   description and cost before running (as `describe()` does), and returns `S_k1` per victim port with the
   evidence grade and the limits above.
5. **Desensitisation finding.** Combine the victim's signal spectrum (section 14 of `docs/emi-noise-analysis.md`)
   with `|S_k1|^2` for the noise power at the antenna port per harmonic, against a declared receiver sensitivity,
   as an `EMI-RF-DESENSE` finding; and a constraint to declare a required isolation.
