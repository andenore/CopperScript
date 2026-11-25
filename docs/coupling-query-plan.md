# Coupling between nets: prototypes and plan

Status: two prototypes, no user-facing command. Screening evidence only, like the rest of the full-wave tiers
(`docs/emi-noise-analysis.md` section 11, `docs/pcb-antenna-synthesis-research.md`).

The question: *how much does this net couple to that one*, for example a radio's antenna to a QSPI clock, or one
fast line to its neighbour. Both full-wave models already run the pinned `ems` FDTD solver with lumped ports; a
coupling is the transmission `S_k1` from a port on the aggressor to a port on the victim. What exists after the
prototypes:

| Piece | Where | What it does |
| --- | --- | --- |
| Line-to-line validation | `crates/copper-extract/tests/coupled_microstrip.rs` | Four-port region model (`copper-fullwave`) of two parallel microstrips against coupled-line theory, with the even and odd modes from the 2-D field solver (`copper_extract::capacitance`) |
| Antenna-to-net coupling | `copper_antenna::model::{Victim, evaluate_coupled}`, test `crates/copper-antenna/tests/victim_coupling.rs` | The antenna site model with a victim net's F.Cu tracks, the ground cut back around them, and a passive 50 ohm port at each end; returns `S_k1` from the feed to every victim port |

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
wave reads high in every case (the conservative side). Above about -40 dB it agrees within 1 to 3 dB; below that,
a path the TEM reference lacks dominates at the top of the band and grows with frequency: either the direct
coupling of the vertical port elements (an artefact of the measurement) or radiation from the open ends and port
wires (physics the reference leaves out). It is not separated yet.

**Reading the numbers:** coupling stronger than about -40 dB is an estimate within a few dB; weaker coupling at GHz
frequencies is an upper bound until the open item below is closed. Coupling runs should continue planes into the
PML (`RegionSpec::extend_planes`).

## 2. Antenna-to-net coupling (antenna site model)

Pending: the first runs of `victim_coupling.rs` (coarse fidelity, a 0.5 mm trace 2.5 mm and 18 mm below the
example monopole's region) are recorded here when they finish.

## 3. Plan

1. **Separate the weak-coupling floor.** Sweep the line length of the 8 mm pair (10, 20, 40 mm) at fixed port
   geometry: a port-element artefact stays constant with length, real coupling scales with it. If it is the
   ports, compare against de-embedding the port pair, or a matched-load termination at the strip end instead of a
   vertical port.
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
