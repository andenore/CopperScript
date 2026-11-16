# EMI: fast digital signals and cable common mode — implementation plan

Status: plan, not started (2026-11-16). Base: `main` at `4be75b2`. This plan
extends `copper emi-check` and the EMI specification in
[`emi-noise-analysis.md`](emi-noise-analysis.md); read sections 1–8 of that
document first, because every convention there applies here unchanged.

## Why

`emi-check` treats switching converters as the only noise sources and radiates
them from the hot-loop area. On boards with multi-gigabit differential lanes and
cables, this misses the two mechanisms that usually decide a radiated-emission
test:

1. **Fast digital signals.** A balanced differential pair barely radiates. It
   radiates when something converts part of its signal to common mode: a break
   in the return path (a plane gap or slot under the trace, a layer change
   without a nearby return via, a trace beyond the plane edge), imbalance inside
   the pair, or routing near the board edge. Single-ended clocks and strobes
   radiate through the same return-path breaks.
2. **Cable common mode.** Below about 1 GHz the cables are the antennas. A small
   noise voltage between the cable's reference conductor and the board's ground
   drives a common-mode current along the cable; about 2–3 µA at 100 MHz on a
   1 m cable already reaches the CISPR 32 class B line (30 dBµV/m at 10 m),
   using `E = 1.257e-6 f l I / r`.

Today these appear only as `unsupported` coverage: `EMI-RETURN-PATH` and
`EMI-CABLE-COMMON-MODE` (`crates/copper-drc/src/emi.rs`, the coverage block in
`emi_check_inner`). `cable_length` is parsed from the `emission` constraint but
unused; `current_driven_cm_voltage` and `cable_common_mode_field` exist in
`crates/copper-emi/src/estimate.rs` but nothing calls them.

## Ground rules (from `emi-noise-analysis.md`, binding here)

- **Screening evidence only.** Results rank likely sources and make review
  repeatable. Nothing prints pass or fail, a missing input never becomes a pass,
  and every absolute estimate carries its parameter band (`band_db`), its
  rule-of-thumb tags and `not_modelled` mechanisms. `VALIDATION` stays honest:
  no estimate here is correlated with measurement until one is.
- **Optional, datasheet-style syntax.** Flat `name = value;` attributes on
  existing declarations, values with units, no tool settings in source.
  Unannotated libraries compile to the same IR and produce no new diagnostics.
  Net classes are derived, never declared on nets. The serialized IR carries new
  data only when declared.
- **Coverage for everything.** Each new check reports `executed`,
  `not_applicable` or `unsupported` with a reason, per subject where it makes
  sense (`EMI-RETURN-PATH:<net>`).
- **Defaults are documented.** Every rule-of-thumb value goes in
  `crates/copper-drc/src/emi/defaults.rs` with a `rule_of_thumb_*` tag, its band,
  and its published source.
- **No board names.** Commits, docs, tests and examples in this repository use
  synthetic boards and generic descriptions only, never a consumer board's
  name, nets, reference designators or decision numbers.
- **Keep `emi.rs` from growing.** It is 4.5 k lines. New checks go in new
  modules under `crates/copper-drc/src/emi/` (as `conducted.rs` did), new
  closed forms in `crates/copper-emi/src/`.

## Phase overview

| Phase | Delivers | Depends on | Size |
|---|---|---|---|
| P0 | Native rules evaluate footprint-membership conditions, so native fills exist on more boards | — | S |
| P1 | Signal source description (language + model) | — | M |
| P2 | `EMI-RETURN-PATH`: reference-plane gaps, layer changes, plane and board edges | P0 for filled planes, P1 for ranking | L |
| P3 | `EMI-MODE-CONVERSION`: differential imbalance → common-mode spectrum | P1, P2 | M |
| P4 | `EMI-CABLE-COMMON-MODE`: connector noise voltage → cable current → field | P1–P3 | L |
| P5 | Ground transfer inductance by quasi-static extraction (`--extract`) | P4 | M |
| P6 | Full-wave mixed-mode region for a pair at a connector (`--fullwave-signals`) | P3, P4 | L |
| P7 | Report, plots, issues, documentation, examples | each phase | M |

Each phase is one or more commits that build, pass `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`,
and update the docs it changes. Phases P1–P4 are the core; P5 and P6 refine
numbers that P4 already reports.

---

## P0. Footprint-membership conditions in native rules

**Problem.** The KiCad export writes a custom rule for a connector's own
locating holes (`crates/copper-pcb/src/kicad_pcb.rs`, the `hole clearance` rule
with `A.memberOfFootprint('<ref>') && B.memberOfFootprint('<ref>')`). The native
rule evaluator (`crates/copper-cam/src/rules.rs`) supports `NetName`, `NetClass`
and `Reference` only, so `fill_zones` and native DRC stop with `property
memberOfFootprint is not supported`. Every check below that needs filled planes
(`--run` with native fill, `--extract`, P2, P5) fails on such boards.

**Do.**
- Parse and evaluate `memberOfFootprint('<ref>')` on both rule operands in
  `copper-cam`'s condition evaluator, for pads, holes and any item that knows its
  owning footprint; non-footprint items are not members.
- Tests: a synthetic board with a footprint-scoped `hole_clearance` rule fills
  and checks natively; the rule applies to that footprint's holes only.
- `docs/native-drc.md`: list the supported condition.

**Accept.** `copper build --verify native` and `copper emi-check --run … --extract`
succeed on a board with that rule.

---

## P1. Signal source description

**Language** (section 2 of `emi-noise-analysis.md`; `crates/copper-lang/src/lower.rs`,
`model.rs`, `serializer.rs`, `docs/language-reference.md`). Pin and device-pad
attributes, all optional:

| Attribute | Type | Meaning |
|---|---|---|
| `rise_time`, `fall_time` | time | Already accepted; now also used for signal pins (10–90 %) |
| `data_rate` | data rate | Bits per second of the pin's signal (`2.5Gbps`); NRZ assumed |
| `clock_frequency` | frequency | For a clock output: the fundamental (`24MHz`) |
| `signal_swing` | voltage | Single-ended swing, or for a differential pin the differential peak-to-peak (`200mV`) |
| `spread_spectrum` | ratio | Down-spread fraction for a spread clock (`0.5%`); recorded, not modelled in v1 |

`data_rate` needs a new `Dimension::DataRate` in `crates/copper-units` (units
`bps`, `kbps`, `Mbps`, `Gbps`), following the pattern of the existing
dimensions. `spread_spectrum` needs a dimensionless ratio with `%`; reuse one if
it exists, add it otherwise.

Validation errors use the existing `CMP130` family. `rise_time` without any
rate is still accepted (it sets the edge, the fundamental stays unknown).

**Model** (`crates/copper-lang/src/emi.rs`).
- Add `SignalSource { pins: Vec<Endpoint>, kind: SingleEnded | Differential,
  partner: Option<String>, data_rate, clock_frequency, rise_time, fall_time,
  swing, provenance }` and `EmiModel::signals: BTreeMap<String, SignalSource>`
  keyed by net (both members of a pair point at one entry, keyed by the P net).
- Pair membership comes from the existing `routing(...) { kind = differential;
  differential_partner = … }` rules; `kind = clock` already raises `FastEdge`.
- Keep `AggressorClass::FastEdge` as the class; `SignalSource` carries the
  numbers. A `FastEdge` net with no numbers gets defaults (below) and is tagged.

**Defaults** (`emi/defaults.rs`): when a fast net has no `rise_time`, use a band
by declared kind (e.g. differential with `data_rate`: 0.2–0.35 UI; clock without
data: a rule-of-thumb band); when no `signal_swing`, a band by kind (differential
100–400 mV, single-ended the driving pin's supply). Each value gets a source and a
tag (`rule_of_thumb_signal_edge`, `rule_of_thumb_signal_swing`).

**Spectrum.** NRZ data: envelope of a random bit stream with the trapezoid's
edge roll-off (fundamental `data_rate / 2`, the existing
`trapezoid_envelope` with duty 0.5 and period `2 / data_rate` is the upper
bound; document that it is a bound for random data). Clocks: the trapezoid
harmonics at `clock_frequency`.

**Tests.** Lowering accepts and rejects the attributes; serializer round-trip
only when declared; `EmiModel::signals` for a synthetic pair and clock;
unannotated example boards produce identical IR (`examples/emi_buck`,
`examples/full_vertical`).

**Accept.** `copper query BOARD emi` lists the signal sources with their
numbers and provenance.

---

## P2. `EMI-RETURN-PATH`

New module `crates/copper-drc/src/emi/return_path.rs`, called from
`emi_check_inner`; replaces the `unsupported` coverage entry when routed copper
and filled planes exist (else `unsupported` with the specific reason, as today).

**Subjects.** Every net with a `SignalSource`, plus nets with routing kind
`differential`, `clock` or `rf_feed` (the set `signal_integrity::needs_reference_plane`
already uses).

**Inputs.** The routed `PhysicalBoard`, the stack-up, and native zone fills
(`copper_cam::fill::fill_zones`, as `copper-extract/src/board.rs` and
`copper-fullwave/src/fill.rs` already do — share one helper rather than a third
copy). Reference planes: the nearest filled copper of a ground net or declared
supply net above and below each signal layer (reuse `nearest_plane`,
`height_over_plane` in `emi.rs` and `copper_layer_depths_nm` in
`signal_integrity.rs`).

**Checks per routed segment.**
1. *Reference coverage.* Sweep the segment's footprint widened by `3 H` each side
   (H = height to the reference) against the reference fill. Report the length
   with no reference copper underneath and each gap crossed.
2. *Slot crossing.* For each gap a segment crosses, measure the void's length
   along the direction perpendicular to the crossing (the connected component of
   the plane's hole, clipped to a sensible window). Report the slot length `L`
   and its half-wave resonance `c / (2 L sqrt(eps_eff))`. Rows of via antipads
   whose clearances merge count as a slot.
3. *Reference change at a via.* Compare the reference planes before and after a
   via (layer and net). On a change, find the nearest return path: a stitching
   via joining the two reference layers (same net), or a decoupling capacitor
   between the two nets when the nets differ. Report the distance. Threshold:
   the declared `maximum_return_via_distance` when the net has one, else
   `lambda / 20` at the knee frequency `0.35 / t_r` (rule-of-thumb, tagged).
4. *Plane edge and board edge.* Distance from the segment to the reference
   fill's edge and to the board outline, against `20 H` (rule-of-thumb, tagged).
   Report outer-layer fast copper along the edge without a stitched ground
   guard.

**Finding.** `EMI-RETURN-PATH:<net>` (pairs: one finding per pair, members
listed) with `measures`: uncovered length, gaps crossed (position, slot length,
resonance, layer), reference changes (via position, layers, nets, return-path
distance, kind of return), edge distances, and the spectral level of the source
at each slot resonance. No field value: severity is a **ranking score** in dB,
`20 log10(V_source(f_res) · L_slot / (1 V · 1 mm))` or similar, tagged
`ranking_only` like `cm_coupling_score`, with `not_modelled` naming slot
radiation efficiency, plane-pair cavity modes and enclosure. Positions go in
`measures` so CopperReview and the plots can mark them.

**Tests** (`crates/copper-drc/tests/emi_return_path.rs`): a synthetic board with
(a) a pair over a solid plane — no gap findings; (b) the same pair crossing a
cut in the plane — one gap, slot length and resonance checked by hand; (c) a via
that changes reference with and without a stitching via within the threshold;
(d) a trace within `20 H` of the board edge. Each as a fixed-placement,
hand-routed example under `examples/emi_return_path/`.

**Accept.** Coverage `executed` with per-net entries on the example; the
existing EMI tests unchanged.

---

## P3. `EMI-MODE-CONVERSION`

New module `crates/copper-drc/src/emi/mode_conversion.rs`; closed forms in
`copper-emi/src/estimate.rs` (or a new `signal.rs`).

**Per differential pair**, sum the imbalance contributors as a time skew and an
amplitude imbalance:
- intra-pair routed length difference (reuse
  `signal_integrity::routed_match_length_nm`; delay from the stack-up's
  effective permittivity per layer, as `si-check` computes it);
- via-count difference between the members;
- asymmetric return-path breaks from P2 (one member over a gap the other does not
  cross: amplitude imbalance from the uncovered length fraction, a rule of
  thumb, tagged);
- asymmetric proximity to the plane or board edge (from P2), ranking only.

**Estimate.** Common-mode fraction for skew `Δt`: `|sin(pi f Δt)|` (exact for an
ideal odd-mode source); common-mode voltage spectrum
`V_cm(f) = V_diff(f) · max(|sin(pi f Δt)|, imbalance)`. Report the conversion in
dB at the fundamental and its worst value to the edge corner, and the CM
voltage envelope that P4 uses. Lane-to-lane skew of a match group is **not** a
contributor; say so in the finding's notes.

**Tests.** Hand calculations of the closed form; a synthetic pair with a known
length skew; zero skew gives no conversion from that contributor.

---

## P4. `EMI-CABLE-COMMON-MODE`

New module `crates/copper-drc/src/emi/cable.rs`. One finding per cable
connector (`cable_connectors()` in `emi.rs`: parts with `external_cable = true`
and `emission` targets), replacing today's single `unsupported` entry.

**Language additions** (all optional; section 2–3 of `emi-noise-analysis.md`):

| Where | Attribute | Meaning |
|---|---|---|
| pin | `emi_role = shield` | A connector pin bonded to the cable's shield or shell (new `EmiRole::Shield`, a victim-side role, not an aggressor) |
| part | `cable_shielded` | bool: the cable the connector takes is shielded |
| part | `common_mode_impedance` | impedance at `common_mode_impedance_frequency` (frequency), for common-mode chokes and ferrites in series with a cable's lines |
| `emission` constraint | `cable_length` | already exists; now used |

**Drivers at the connector.** Each gives a common-mode voltage spectrum between
the cable's conductors and the board reference at the connector:

1. *Signals leaving on the cable* (`NetClasses::external`, already derived):
   differential pairs use `V_cm` from P3; single-ended fast signals their full
   source spectrum. A recognised common-mode choke or ferrite in the path divides
   by `Z_cmc / (Z_cmc + Z_cable)`; the input-filter walk in
   `emi/conducted.rs` is the pattern for finding it.
2. *Ground noise (current-driven).* Return currents of fast signals and of the
   switching cells whose return path passes between the source and the
   connector's ground pins: `V = 2 pi f L_gnd I_return` with the existing
   `current_driven_cm_voltage`. `L_gnd` is a closed-form ground-plane partial
   inductance for the geometry (cite the published form used and check it by
   hand in tests; a slot from P2 on the path between source and connector raises
   it); P5 replaces it with extraction.
3. *Switch-node coupling (voltage-driven).* The island capacitance of
   `EMI-SWITCH-NODE` to the connector's shell and pins (a new closed-form
   capacitance by separation and area, ranking-grade, tagged), times the switch
   voltage spectrum.

**Cable.** Common-mode current `I_cm(f) = V_cm(f) / Z_cm(f)`, with `Z_cm` a
rule-of-thumb band (nominal 150 Ω, the common-mode impedance used by
IEC 61000-4-6 coupling networks; band from the resonant low end to a few hundred
ohms), tagged `rule_of_thumb_cable_impedance`. Field with
`cable_common_mode_field` while `cable_length < lambda / 4`, and capped above
that at the resonant half-wave-dipole bound (`60 I / r`, with the same
ground-reflection factor as `cable_common_mode_field`; verify the factor
against the cited source). Add a validity record like `dm_field_validity`.

A shielded cable (`cable_shielded = true`) with `shield` pins: report the shield
bond (pins, via count to ground, estimated bond inductance) and mark the
shield transfer impedance `not_modelled`; do not apply the unshielded estimate.

**Finding.** `EMI-CABLE-COMMON-MODE:<connector>` with measures per driver and in
total: `V_cm` spectrum, `I_cm` in dBµA (directly comparable with a clip-on
current probe — say so in the finding), field in dBµV/m at the standard's
distance, and `margin_db` against the radiated line (absolute estimate, wide
band, `not_modelled`: cable routing and height, chassis, enclosure, cable
termination at the far end). The worst driver is named. `EMI-MARGIN` coverage
text is updated to include it.

**Tests.** Closed forms by hand; a synthetic two-connector board where a pair
with known skew leaves on a cable (P3 → P4 chain checked end to end); a choke on
the pair lowers the result by the expected ratio; a shielded cable gives the
shield report and no unshielded estimate.

**Accept.** On the example, the finding gives I_cm and a margin with a band, and
the driver breakdown sums correctly.

---

## P5. Ground transfer inductance by extraction

Extend `crates/copper-extract` (`board.rs` port definitions, `extract.rs`) with a
two-port case: port A at the source's return pins (or a signal's via return),
port B at the connector's ground or shield pins, both against the plane. The
mutual and self partial inductances give `L_gnd` for P4. Wire it into
`copper emi-check --extract` beside the hot-loop extraction, same mesh series and
convergence reporting (`measures.extract`), same rule: an extraction whose mesh
loses the copper's connectivity has status `error` and is not used.

**Tests.** A solid plane against the closed form within the stated tolerance; a
slotted plane gives a larger `L_gnd`; `--extract-reference` agreement at DC.

---

## P6. Full-wave mixed-mode region (opt-in)

Extend `crates/copper-fullwave` (`spec.rs` `PortSpec`/`RegionSpec`, `solve.rs`
`PortMatrix`) to build a window around a pair from a launch point to the cable
connector, with a port on each member at each end (four ports), and return
mixed-mode S-parameters (Sdd21, Scd21) and the common-mode voltage at the
connector. New flag `--fullwave-signals` on `emi-check` (never default),
honouring `--fullwave-budget`. Multi-gigabit content needs a much finer mesh
than the hot loop: `describe()` must state the cost first and the budget refuse
anything too large. Scd21 replaces the P3 closed form in P4 where it exists
(tagged `fullwave_mode_conversion`). The cable itself stays analytic.

**Tests.** A symmetric pair gives Scd21 near the numerical floor; a pair with a
known skew matches `|sin(pi f Δt)|` within the mesh sensitivity.

---

## P7. Report, plots, issues and documentation

- Report: the new findings and coverage go in the `copperscript-emi-check`
  document (additive fields; update `docs/schemas.md`; bump the schema only if an
  existing field changes) and therefore in `route-report.json`'s `emi` section.
- `copper issues` / `copper-report/src/issues/emi.rs` and `explain.rs`: explain
  texts for `EMI-RETURN-PATH`, `EMI-MODE-CONVERSION`, `EMI-CABLE-COMMON-MODE`.
- Plots (`--plot-spectrum`, `--plot-summary`): the cable common-mode field and
  the I_cm spectrum with the limit line; return-path gaps and reference changes
  marked on the board plot.
- `docs/emi-noise-analysis.md`: new sections for P1–P6 in the style of sections
  8–13 (what is measured, how the numbers are made, what is not modelled), the
  phase table, section 12 test boards; `docs/language-reference.md` for the new
  attributes; `copper emi-check --help` text.

---

## Out of scope for this plan

- Immunity (ESD, radiated and conducted immunity).
- Plane-pair cavity resonance and decoupling-network PDN impedance (a natural
  next phase after P2; record as `not_modelled` until then).
- Spread-spectrum, jitter and data-pattern statistics beyond the random-NRZ
  envelope.
- Enclosures, chassis, shielded-cable transfer impedance.
- Measurement import and correlation (keep `VALIDATION` unchanged until a
  measured case exists).

## Order of work

1. P0, then P1 (they are independent and small).
2. P2, which catches most real fast-signal problems without any estimate.
3. P3 and P4 together; this is where `cable_length` and the existing
   current-driven estimator are finally used.
4. P7 for the delivered phases, then P5 and P6 as refinements.

Update the status line at the top of this file as phases land.
