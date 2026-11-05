# PCB trace-antenna synthesis in CopperScript

Research date: 4 November 2026. Nothing below changes the qualification
boundary in [engineering qualification](engineering-qualification.md): a
synthesized and simulated antenna is a design draft with simulation evidence,
never bench qualification or regulatory compliance.

## Status

Implemented (see [the language reference](language-reference.md#trace-antenna-synthesis)
and `examples/antenna_synthesis`):

- `constraint antenna(...)` lowering to a generated one-member hard macro,
  with `geometry` (monopole, meander, pixel) and `tuned_for`, the site digest
  of every full-wave model input; a stale digest fails closed.
- The `copper-antenna` crate and `copper synthesize-antenna`: a board-level
  full-wave model on the pure-Rust FDTD solver `ems` 0.0.5 (pinned), L-section
  matching synthesis, resonance tuning by electrical-length scaling, bounded
  Nelder-Mead polish, fine-mesh verification with a length correction and
  radiation efficiency from a near-to-far-field box, and a pixel genetic
  algorithm. The report is `copperscript-antenna-synthesis/v0.1`.

- Inverted-F antennas, shorted through a declared two-land link component
  (`short = R1`): the macro's second member, with a feed-net leg to one land
  and a ground stub and through via at the other, proved as a plane return.
  The link is a real part (0 ohm, or a tuning inductor), which is also how
  commercial IFAs are tuned.

Not implemented:

- **Loops and copper-only shorts.** A short without a component would need
  a net tie: the footprint importer rejects KiCad `net_tie_pad_groups`, and
  internal pad groups refuse to bridge nets, so that would touch the
  footprint model, DRC, connectivity, routing, export and the frozen state
  codec. The link component sidesteps all of it.
- Pixel tiles are owned copper polygons, and hard-macro rules forbid a host
  zone on a layer where a macro owns area copper; pixel antennas therefore
  need the `F.Cu` ground zone drawn around the region.
- Multi-band synthesis, robust optimization over permittivity spread,
  E-series snapping and tolerance analysis of matching parts, NFC loops,
  back-side antennas and non-rectangular regions.
- An openEMS cross-check backend.

## The question

Can a board author reserve an antenna region, state the bands and the feed,
and have CopperScript generate a printed antenna (inverted-F, meander,
monopole, loop) inside that region, together with the keepouts, ground
requirements and matching intent the rest of the pipeline already understands?

Short answer: yes, in three tiers of increasing cost, and the existing
hard-macro contract is the right output container for all of them. The hard
parts are not the language surface. They are (1) an electromagnetic solver in
the loop, (2) modelling enough of the *host board* for the result to mean
anything, and (3) keeping the evidence honest.

## What exists today

| Capability | Where | Relevance |
| --- | --- | --- |
| Rectangular/polygon `keepout()`, `placement_region`, `copper_zone` with `polygon_mm`/`region`, `reserve_routing` | [language reference](language-reference.md#constraints) | The antenna region and its copper/component exclusion are expressible now |
| `stackup { copper ...; dielectric ... er, loss_tangent, thickness }` | [mechanical language](mechanical-language.md#board-stack-up) | Exactly the substrate data a solver needs |
| Hard macros: `track`, `via`, `polygon`, `keepout`, `protected_region`, `zone`, `port`, `plane_return`, `edge_opening`; rigid cluster, private regions, fill exclusions, locked export, port-continuity proof | [physical hard macros](physical-hard-macros.md), [macro definitions](macro-definitions.md) | A generated antenna is a one-owner hard macro: locked copper, private region, ground exclusion, one external port, an edge-opening contract to the board perimeter |
| Nordic/Johanson chip-antenna trial | `examples/nrf_antenna_macro`, CopperLib `nrf52832-johanson-reference` | Shows the feed/matching/exclusion pattern a trace antenna must also satisfy |
| `constraint routing(...)` with `target_impedance_ohms`, Hammerstad-Jensen effective permittivity | `crates/copper-route/src/critical.rs` (`effective_permittivity`) | Feed line geometry and the only analytic RF estimator in the tree |
| `rf_geometry.rs` | `crates/copper-route` | Despite the name, this is fill decoding and reference-plane coverage, not RF analysis |
| CopperLib `qualification.json` (`kind: rf`, `matching: required/integrated/unresolved`, `stage: design/simulation/bench`) | CopperLib `docs/engineering-contracts.md` | The evidence schema a synthesized antenna should populate |
| Removed `copper sim` (ngspice) and the [simulation research](integrated-simulation-research.md) | docs | The bounded disposable-worker pattern for an external solver already has a design |

There is no field solver, no antenna geometry generator and no S-parameter
concept anywhere in the workspace. `copper` cannot say anything about
resonance today.

## What antenna design actually needs

A printed antenna is not a trace. It is a resonator whose other half is the
board's ground plane, so its behaviour is a property of the *whole board*,
plus the enclosure and battery. Any synthesis that only looks inside the
reserved rectangle produces a shape, not an antenna. Concretely:

- **Electrical size.** At 2.45 GHz the free-space quarter wave is 30.6 mm. A
  trace on 1.6 mm FR4 with air above sees an effective permittivity of roughly
  2.5 to 3, so printed quarter-wave structures are 17 to 25 mm long. TI's
  AN043 inverted-F fits 15.2 x 5.7 mm with 0.5 mm traces; Nordic's guidance
  is about 23 mm of antenna length with at least 5 mm clearance to ground.
  Sub-GHz (868/915 MHz) meanders need 3 to 4 times that; a quarter wave at
  868 MHz is 86 mm in air.
- **Ground plane as counterpoise.** The antenna must sit at a board edge or
  corner with the ground plane ending at a defined line; ground size and shape
  shift resonance by tens of MHz. Nordic's advice to copy the reference
  layout exactly, and TI's warning that small dimension changes have a large
  effect, both come from this sensitivity.
- **Clearance.** No copper on any layer under the radiator, no components or
  metal (battery cans, shields, connectors) within a few millimetres, and
  routing kept out of the region on all layers. The existing all-layer
  keepout plus zone exclusion covers this once the region is known.
- **Feed.** A 50 ohm microstrip or coplanar line from the radio's matching
  network to the antenna feed point, over continuous ground, with return vias.
  This is already a `constraint routing(...)` single-ended critical net.
- **Matching.** Monopoles typically need one shunt element, meanders a pi
  network, inverted-F designs can be self-matched by the feed-to-short gap.
  The component *values* depend on the simulated or measured feed impedance
  and the final enclosure.
- **Bandwidth and tolerance.** Fabrication tolerance (etch, dielectric
  thickness, er spread of FR4 between 4.2 and 4.8) and enclosure detuning mean
  the design target has to be a margin across the band, not a notch at the
  centre frequency. The pixel-antenna literature optimises the *worst* S11
  across the band for this reason.
- **Bench tuning remains mandatory.** Every vendor note ends with a VNA
  measurement in the real enclosure and a matching-value adjustment.

### Antenna types a synthesizer can target

| Type | Typical 2.4 GHz footprint | Ground need | Matching | Analytic design | Notes |
| --- | --- | --- | --- | --- | --- |
| Inverted-F (IFA) | 15 x 6 mm | Edge, long ground | Often none (feed/short gap) | Transmission-line model exists; still needs tuning | Best first target: self-matched, compact, well documented (TI AN043, Silicon Labs AN1088) |
| Meander monopole | 20 x 5 mm | Edge | Pi network | Length formulas are approximations | Smallest area per wavelength; narrow bandwidth, sensitive |
| Straight/bent monopole | 25 x 3 mm | Edge | One shunt | Good | Easiest to tune, longest |
| Loop (NFC 13.56 MHz) | Board-perimeter spiral | None (near-field) | Series/parallel C from computed L | Fully analytic (Grover/Wheeler, NXP AN11276, ST CD00232630) | No field solver required; inductance plus Q is enough |
| Patch (GNSS, 5 GHz) | 25 x 25 mm on thick substrate | Full ground under patch | Inset or probe feed | Good closed forms | GNSS L1 needs circular polarisation; a ceramic patch is the realistic answer, not a trace |
| UWB (6 to 8.5 GHz) | 10 x 10 mm elliptical monopole | Edge | None | Weak | Wideband, needs full-wave for anything beyond a template |

Chip antennas (the existing Johanson path) remain a separate, vendor-geometry
route and are not synthesized.

## Proposed language surface

One new constraint kind. The target is the net at the antenna feed point.

```copper
constraint antenna(ANT_FEED) {
    name = "bt-antenna";
    region = "antenna";               // an existing placement_region polygon, or
    // polygon_mm = "35,0;50,0;50,8;35,8";
    side = front;
    ground = GND;
    ground_edge = "y=8mm";            // where the host ground plane must end
    bands = ["2400 MHz-2483.5 MHz"];  // one or more; enables multi-band search
    max_return_loss_db = -10;         // across each whole band
    topology = auto;                  // auto | inverted_f | meander | monopole | loop
    feed = U_NRF.ANT;                 // informs feed-line direction; the 50 ohm
                                      // line itself stays a routing(...) rule
    matching = report;                // none | report | shunt | pi
    min_track_width = 0.3mm;          // defaults from mechanical rules
    min_gap = 0.3mm;
    keepout_margin = 1mm;             // extra all-layer clearance around copper
    solver = template;                // template | parametric | pixel
    simulation_budget = 60;           // max full-wave runs for parametric/pixel
    evidence_digest = "<sha256>";     // optional pin of an accepted synthesis
}
```

Validation at lowering:

- `region` must touch the board outline along a stable edge (reuse the
  `edge_opening` perimeter check) and must not contain components, holes or
  copper zones of other nets.
- `ground_edge` must coincide with a boundary of a `copper_zone(GND)` on every
  layer, or the zone is clipped and reported. The zone's `reserve_routing`
  semantics are applied to the antenna region automatically.
- `bands` are typed frequency ranges; a sub-GHz band in a region shorter than
  roughly a tenth of the wavelength fails with the required length, before
  any solver runs.
- `stackup` must be declared with `er` on the dielectric under the antenna
  layer; otherwise the constraint fails closed with the missing property.

### What it lowers to

The constraint lowers to a *generated hard macro* in the existing v0.5 asset
contract, owned by the constraint and regenerated when any input changes:

- `track`/`polygon` copper of the radiator on the antenna layer, `via`
  records for a short to ground, one `port` at the feed point.
- A `protected_region` over the antenna region on all layers blocking tracks,
  vias and zones, plus the `keepout_margin` ring.
- An `edge_opening` for the perimeter edge(s) the region shares with the
  outline.
- `source = { generator = "copper antenna/v0.1"; parameters = {...};
  solver = {...}; }` and `unresolved = ["bench tuning in enclosure", ...]`.

This reuses rigid placement, private-region enforcement against all routing
(including same-net shortcuts), fill exclusion, locked KiCad export, port
continuity proof and fingerprinting without new physical machinery. A user
who prefers a vendor layout keeps the ordinary `physical_macro` binding; the
two paths produce the same kind of object.

The electrical source stays authoritative: synthesis never adds components.
With `matching = report`, the report states the simulated feed impedance and
the computed shunt or pi values; the author declares those parts in `.copper`.
A `matching = pi` variant that *requires* three declared parts between the
radio and the feed, and only fills in recommended values, is consistent with
how `physical_macro` references existing circuitry.

## Algorithm tiers

### Tier 0: parametric templates with analytic scaling (no solver)

A small library of published, measured reference geometries (TI AN043 IFA,
Silicon Labs AN1088 IFA, ST AN5129 meander, TI DN007 monopole, Nordic
reference meander) stored as dimension tables with their reference substrate.
Synthesis:

1. Pick the template by `topology` and region aspect ratio.
2. Scale electrical lengths by `sqrt(eps_eff_ref / eps_eff_board)` using the
   existing Hammerstad-Jensen estimator for the declared stackup, and shift for
   substrate thickness per the vendor tuning tables (AN043: thinner board,
   slightly longer antenna).
3. Legalize against region, minimum track and gap, edge clearance.
4. Emit the macro with `unresolved` stating that scaling is first-order and
   the design must be tuned on the bench.

This is cheap, deterministic, offline and already useful: it gives a
DRC-legal antenna with a correct keepout and feed port and the vendor's
expected bandwidth as a stated assumption. It is also the seed for the next
tiers. It cannot answer "does it resonate on *my* board".

### Tier 1: parametric optimization with a full-wave solver in the loop

Keep the template's topology but optimize its 3 to 6 parameters (arm length,
height, short-to-feed gap, meander pitch and count, trace width) against the
*actual* board model: outline, stackup, ground zones, large components as
metal blocks, and the feed port. Objective: minimum of return loss margin
across every declared band; constraints: region, minimum features.

- Optimizer: Nelder-Mead or a bounded Bayesian/surrogate search; 30 to 100
  solver runs. Each run on a 50 x 40 mm board at 2.4 GHz with 0.2 to 0.5 mm
  cells is on the order of a million cells and tens of thousands of time
  steps: tens of seconds to a few minutes per run on a laptop CPU, so a
  synthesis is minutes to an hour. These are estimates to be measured.
- Output adds the S11 curve, feed impedance, radiation efficiency and an
  NF2FF gain estimate to the report and to the macro's `source` record.

### Tier 2: pixel (topology) optimization inside the region

Discretize the region into a grid of tiles (0.5 to 1.5 mm), each conducting
or not; fix the feed stub and an optional shorting via; run a genetic
algorithm with the fitness equal to the worst S11 across the band. This is
what the 2024 *Scientific Reports* pixel-IFA work did (60-member population,
converged in 26 generations, measured 308 MHz bandwidth and 4 dBi at
2.44 GHz on RO4350B with a 40 x 40 mm ground). It is the direct realization
of "run an algorithm within the confines of that space" and it handles odd
region shapes and multi-band targets that templates cannot.

Costs and constraints:

- 1,500 to 5,000 solver runs: hours to a day of CPU, so it is a batch job
  with checkpoints, not a `route-board` step.
- Fabrication legality must be in the encoding: tiles that only touch at
  corners, isolated islands and sub-minimum necks must be repaired or
  penalized (the paper enlarged tiles by 2% to force defined neck widths).
  The exact DRC in `copper-drc` is the right checker for each candidate.
- Seed the population with Tier 1 results; random seeds waste most of the
  budget on non-resonant shapes.
- Results are less interpretable and harder to hand-tune on the bench; a
  reviewer should still be able to see a recognisable radiator.

### Loop antennas (NFC) as a separate, fully analytic track

A 13.56 MHz coil is a lumped inductor: rectangular spiral inductance from
NXP AN11276 / Grover formulas, resistance from copper geometry, resonance with
the tag or reader's tuning capacitance, Q from the series resistor. No field
solver, sub-second synthesis, and the result (turn count, pitch, width,
inner size) is exactly checkable. This is the cheapest complete feature and
a good way to prove the constraint-to-generated-macro plumbing before any
FDTD work.

## Solver options

| Option | Method | Licence | Integration | Assessment |
| --- | --- | --- | --- | --- |
| `ems` crate (KarpelesLab/ems) with `csx` | Pure Rust FDTD, openEMS model compatible; lumped/MSL/CPW ports, S-parameters, Z_in, NF2FF gain/efficiency, CPML | MIT | In-process Rust dependency; MSRV 1.95, edition 2024 | Best structural fit: no runtime install, deterministic, same language. Risk: versions 0.0.0 to 0.0.5 all published between 18 October and 1 November 2026; API declared unstable; validation is against openEMS fixtures (patch, monopole, filter), not independent measurement. Pin exactly and keep behind a feature flag |
| openEMS (C++) + CSXCAD, Python bindings | FDTD, mature, has a MATLAB inverted-F example and Python patch tutorials | GPLv3 | Disposable subprocess worker, XML model in, HDF5/CSV out, as the removed ngspice backend did | Proven solver; packaging is the cost (Windows wheels per Python version, Linux built from source, no PyPI/conda). GPL is fine across a process boundary |
| OpenParEM 2.1 | Frequency-domain FEM, wave ports, far field, adaptive mesh | GPLv3 | Subprocess, Linux/MPI oriented | Better for electrically small, high-Q structures than FDTD; heavier to run on Windows |
| Hertzon / EMerge, EdgeFEM | FEM, S-parameters | Open source | Subprocess | Young; watch |
| NEC2 / PyNEC | Wire method of moments | Public domain | Subprocess | Wrong tool for planar copper on dielectric |
| Analytic only (Tier 0 and NFC) | Closed forms | - | Native | Always available; results are labelled first-order |

Recommendation: implement the solver boundary as a trait (`AntennaSolver`:
build model from physical IR, run with a budget, return S11 per frequency,
Z_in, efficiency), with the `ems` crate as the first in-process backend and
openEMS as an optional external backend used to cross-check it. Both consume
the same CSX model, so one model builder serves both. The model builder maps:
`stackup` dielectrics to material boxes, copper tracks/polygons/zones to
conducting sheets, the antenna port to a lumped port at the macro's `port`,
component bodies with declared height to PEC blocks, and the outline to the
simulation box with CPML and an NF2FF surface.

Existing analogues worth reading before implementing: `antenna-cad` (MIT,
YAML spec to deterministic KiCad layout plus openEMS verification, focused on
patches and feed trees) and the KiCad `AntennaDesigner` plugin (FDTD from the
real board copper, proprietary solver). Neither does region-constrained
synthesis of edge antennas; both confirm the "spec in, geometry plus DRC plus
simulation out" shape.

## Evidence and qualification

Following [engineering qualification](engineering-qualification.md) and
CopperLib's contract schema:

- The generated macro's `source` records generator version, all constraint
  parameters, the solver backend and version, mesh settings, the model digest
  and the resulting S11/Z_in/efficiency per band. Changing the stackup,
  outline, ground zones, nearby placements or the constraint invalidates it;
  the existing input-snapshot mechanism does this if the model digest is part
  of the macro fingerprint.
- `route-board` and the preflight report gain an `antenna_synthesis` section:
  per antenna, the band table with worst return loss, the margin against
  `max_return_loss_db`, the feed impedance and recommended matching, the
  solver budget used, and the `unresolved` list. `copper issues` groups
  failures (region too small, ground edge not found, budget exhausted below
  target, DRC-illegal candidate).
- Simulation satisfies a `stage: simulation` requirement with
  `algorithm: "copper antenna fdtd"`; `stage: bench` stays `external` and
  mandatory. No regulatory statement is ever derived (radiated emissions
  depend on the transmitter, harmonics and enclosure).
- Placement should treat the antenna region exactly like the chip-antenna
  corner today: all-layer keepout, no components within `keepout_margin`,
  battery/connector bodies counted as metal through the mechanical height
  model.

## Validation plan before trusting any number

1. Reproduce TI AN043 in the solver on its stated 1.6 mm FR4: expect S11
   below -10 dB across 2.4 to 2.4835 GHz. Repeat for Silicon Labs AN1088.
   Disagreement of more than about 50 MHz in resonance means the model builder
   or mesh is wrong.
2. Run the openEMS inverted-F example through both `ems` and openEMS and
   compare S11 curves; this checks the Rust backend independently of our
   model builder.
3. Build the Nordic/Johanson probe board model with the chip antenna replaced
   by a synthesized IFA and confirm the ground exclusion and feed port
   materialize through the existing hard-macro lifecycle and export.
4. Fabricate one board (Tier 1 output) and measure S11 with a VNA; record the
   resonance shift. That number is the honest systematic error to print in
   every report until more boards exist.

## Validation results

All runs on the pinned `ems` 0.0.5 with 12 threads on a laptop CPU.

**Free-space strip dipole** (58 mm, 1 mm wide, 1 mm gap, 0.5 mm cells): the
input impedance at resonance is 73.3 - 0.1j ohm at 2.405 GHz, the textbook
73 ohm, about 7 % below c/2L as expected for a strip of that width. Two
seconds per run.

**TI AN043 meandered inverted-F**, rebuilt from the dimension drawing
(Figure 3, Table 1 of SWRA117D) on the CC2511 dongle stack quoted from its
readme (0.25/0.50/0.25 mm FR4, ground on layers 2, 3 and 4). The ground-plane
length and the FR4 permittivity are not published, so they were swept:

| Ground | er | Mesh | Resonance | Best S11 | -10 dB band |
| --- | --- | --- | --- | --- | --- |
| 20 mm | 4.3 | 0.25 mm | 2.415 GHz | -5.0 dB | none |
| 30 mm | 4.3 | 0.25 mm | 2.370 GHz | -9.7 dB | none |
| 30 mm | 4.3 | 0.15 mm | 2.345 GHz | -9.0 dB | none |
| 40 mm | 4.3 | 0.25 mm | 2.360 GHz | -27.7 dB | 2.255-2.515 GHz |
| 40 mm | 3.9 | 0.25 mm | 2.430 GHz | -19.6 dB | 2.315-2.600 GHz |
| TI measured, free space | | | 2.475 GHz | -24 dB | about 250 MHz |
| TI IE3D, mesh 5 | | | 2.43 GHz | -17 dB | |

With a 40 mm ground the match and bandwidth reproduce the measurement; with
er 3.9 the resonance lands on TI's own fine-mesh simulation and 45 MHz
(1.8 %) below the measurement, inside the 50 MHz criterion. The mesh is
converged to about 1 %. Sensitivities: about 18 MHz per 0.1 of er, and the
ground length sets the match (a 20 mm ground leaves it at -5 dB). Each run
took 3.5 to 8 minutes at 1 million cells.

**End-to-end synthesis on `examples/antenna_synthesis`** (40 x 30 mm, 1.51 mm
FR4 er 4.4, ground on both layers, 22 x 6 mm corner region, 1 mm track,
2400-2483.5 MHz, budget 10), 34.5 minutes with five unrelated routing runs
sharing the CPU:

| Stage | Geometry | Resonance | Bare | Matched |
| --- | --- | --- | --- | --- |
| Template, coarse | monopole 5.3 mm / 17.46 mm | 2.930 GHz | -0.9 dB | -9.7 dB |
| Resonance tuning, coarse | 22.00 mm | 2.451 GHz | -2.9 dB | -12.5 dB |
| Polish winner, coarse | 22.66 mm | 2.388 GHz | -3.3 dB | -12.7 dB |
| Fine verification | 22.66 mm | 2.387 GHz | -3.1 dB | -12.1 dB |

Coarse and fine meshes agree on the resonance to 1 MHz. The result meets
-10 dB over the band with 0.94 nH series then 2.61 pF shunt (antenna side
first) and 85 % radiation efficiency. The bare match is poor, as expected for
a monopole parallel to a ground edge 5.3 mm away; the matching network is
part of the design, not an afterthought. Coarse evaluations cost about
105 seconds at 1.4 million cells and fine ones five to eight minutes at 2.5
million, so a state-of-the-art pixel search of the published scale (about
1,500 evaluations) would take about two days on this machine; the pixel
algorithm is implemented with a budget and tested on small grids.

**Native verification.** KiCad's DRC reports a radiator's open end as
`track_dangling`. `route-board --verify-plane-fill` excludes exactly the
findings whose single item is a feed-net segment of a synthesized antenna
ending inside its region, lists them in the verification record as
`expected_antenna_open_ends`, and judges everything else as before. The
inverted-F example passes native verification with its link's ground island
joined through the refilled zone.

**Lessons that are now in the model**:

- The solver keeps every edge of a PEC sheet metal whatever lies over it, so
  ground is clipped geometrically around the antenna region and feed slot.
  An early model that covered ground with a higher-priority dielectric left
  the radiator on the ground pour: resonance did not move with length.
- Two unstitched ground planes form a parallel-plate cavity that a vertical
  port drives. The model stitches ground layers at a tenth of the guided
  wavelength and uses an in-plane port across the feed slot when `F.Cu` has
  ground at the feed; both are stated in every report.
- A one-cell gap whose two nodes touch metal discretizes as a PEC short, so
  the feed slot is modelled at least one coarse cell wide with a mesh line
  inside the port.
- The first-order template uses the microstrip permittivity, which assumes
  ground under the trace; an antenna region has none. On the example board
  the 17.46 mm template resonates at 2.93 GHz, 20 % high, which is why
  synthesis matters.

## Risks

- **Host dependence.** Whole-board modelling is mandatory; a region-only
  model will be confidently wrong. Enclosure and battery are outside the
  source today and need a mechanical body declaration or an explicit
  assumption in `unresolved`.
- **Solver maturity.** `ems` is two weeks old. Pin, feature-gate, and keep the
  openEMS cross-check. Expect API churn.
- **Run time in CI.** Even Tier 1 is minutes; Tier 2 is hours. Synthesis must
  be a separate cached command (`copper synthesize-antenna`) whose output is
  pinned by `evidence_digest`, so `route-board` and `check` stay fast and
  offline.
- **Fabrication sensitivity.** Optimize worst-case over the band and over an
  er spread (for example 4.2 to 4.8), not a point. Report bandwidth, not just
  depth.
- **Over-claiming.** Everything printed must say "simulated", carry the
  model assumptions, and keep bench tuning mandatory. The language should not
  let a user declare an antenna "qualified" from synthesis output.
- **Scope creep.** Multi-band, diversity, MIMO, cellular bands and tunable
  matching are each separate projects. Start with single-band 2.4 GHz IFA
  and the NFC loop.

## Proposed implementation sequence

1. **Language and lowering.** Parse `constraint antenna(...)`, typed frequency
   ranges, region/ground-edge validation, and lowering to a generated hard
   macro with keepouts and edge opening. Tests on `examples/nrf_antenna_macro`
   with the chip antenna swapped for a placeholder radiator. Done when
   `route-board` and KiCad export lock the generated copper and exclude fill.
2. **NFC loop synthesis (analytic).** Spiral generator, inductance/resistance
   model, resonance report. Done when a 13.56 MHz board example reports L, Q
   and recommended tuning capacitance and exports cleanly.
3. **Tier 0 templates.** AN043 and AN1088 IFAs, one meander, one monopole,
   scaled by stackup. Done when each emits a DRC-legal macro and the report
   lists the first-order assumptions.
4. **Solver boundary and model builder.** `AntennaSolver` trait, CSX model
   from physical IR, `ems` backend behind a cargo feature, openEMS external
   backend. Done when AN043 reproduces within the validation tolerance on
   both backends.
5. **Tier 1 parametric optimization** with a run budget, checkpointing and
   the `antenna_synthesis` report section plus `copper issues` causes.
6. **Tier 2 pixel optimization** as a batch command seeded by Tier 1, with
   DRC-legal encoding repair.
7. **One fabricated board** measured and its error recorded in the docs.

Steps 1 to 3 need no solver and deliver visible value (correct keepouts, feed
port, ground edge, matching intent) within the current offline toolchain.

## Sources

- TI AN043 (SWRA117), *Small Size 2.4 GHz PCB Antenna*: dimensions L1-L6,
  W1-W2, D1-D6; 15.2 x 5.7 mm; below -10 dB across the ISM band; sensitivity
  warning. https://www.ti.com/lit/pdf/swra117
- TI DN035, *Antenna Selection Quick Guide*. https://www.ti.com/lit/swra351
- Silicon Labs AN1088, *Designing with an Inverted-F 2.4 GHz PCB Antenna*.
  https://www.silabs.com/documents/public/application-notes/an1088-designing-with-pcb-antenna.pdf
- Nordic DevZone, *General PCB design guidelines for nRF52* (ground under RF
  path, meander needs pi matching, monopole one shunt, about 23 mm with 5 mm
  ground clearance). https://devzone.nordicsemi.com/guides/short-range-guides/b/hardware-and-layout/posts/general-pcb-design-guidelines-for-nrf52
- Mair and Baumgarten, *Evolutionary optimisation of pixelated IFA inspired
  antennas*, Scientific Reports 14 (2024). https://pmc.ncbi.nlm.nih.gov/articles/PMC11535421/
- openEMS Python tutorials and install notes.
  https://openems.readthedocs.io/en/latest/python/openEMS/Tutorials/Antenna_Tutorials.html
  and https://docs.openems.de/python/install.html
- `ems` crate (pure Rust FDTD, openEMS compatible, MIT). https://docs.rs/ems
  and https://github.com/KarpelesLab/ems
- OpenParEM 2.1 (frequency-domain FEM, GPLv3). https://openparem.org/
- `antenna-cad` (MIT design compiler: YAML spec to KiCad plus openEMS).
  https://pypi.org/project/antenna-cad/
- KiCad `AntennaDesigner` plugin releases.
  https://github.com/admin-2026/kicad_ems_plugin_pub/releases
- NXP AN11276 *NTAG Antenna Design Guide* and ST CD00232630 *How to design an
  antenna for dynamic NFC tags* (loop inductance and tuning).
  https://www.st.com/resource/zh/application_note/cd00232630-how-to-design-an-antenna-for-dynamic-nfc-tags-stmicroelectronics.pdf
- antenna-theory.com, *PIFA* (quarter-wave resonance, feed position sets
  impedance). https://antenna-theory.com/antennas/patches/pifa.php
