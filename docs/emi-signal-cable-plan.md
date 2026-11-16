# EMI: fast digital signals and cable common mode — implementation plan

Status: plan, not started; revised 2026-11-16 after review (see "Changes from
the review" at the end). Implementation is on hold until the revised plan is
accepted. Base: `main` at `842112a`. This plan
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
   drives a common-mode current along the cable; about 5 µA at 100 MHz on a
   0.5 m cable already reaches the CISPR 32 class B line (30 dBµV/m at 10 m),
   using `E = 1.257e-6 f l I / r` (valid while the cable is shorter than a
   quarter wavelength, 0.75 m at 100 MHz).

**What this plan does and does not give the motivating boards.** Multi-gigabit
lanes almost always leave on *shielded* cables. For those the plan delivers the
on-board part (return-path breaks, mode conversion, P2 and P3) and a report of
the shield bond (P4). It does not estimate the field of a shielded cable: that
needs the shield's transfer impedance and the chassis, which are out of scope.
The common-mode current estimate of P4 applies to unshielded cables only.

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
| P0 | Native rules evaluate footprint-membership conditions, so native fills exist on more boards (a separate fix, its own change) | — | S |
| P1 | Signal source description (language + model) | — | M |
| P2 | `EMI-RETURN-PATH`: reference-plane gaps, layer changes, plane and board edges | P0 for filled planes, P1 for ranking | L |
| P3 | `EMI-MODE-CONVERSION`: differential imbalance → common-mode spectrum | P1, P2 | M |
| P4 | `EMI-CABLE-COMMON-MODE`: connector noise voltage → cable common-mode current and ranking (unshielded), shield-bond report (shielded); no field margin | P1–P3 | M |
| P5 | Ground transfer inductance by quasi-static extraction: **deferred** until its port definition and mesh budget are settled (a written definition and a spike first) | P4 | spike S |
| P6 | Full-wave mixed-mode region: **feasibility spike only**, no product code until the spike passes its criteria | P3 | spike S |
| P7 | Report, plots, issues, documentation, examples | each phase | M |

Each phase is one or more commits that build, pass `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`,
and update the docs it changes. Phases P1–P4 are the core. P5 and P6 are not
committed work: each starts with a spike whose result decides whether it is
built.

---

## P0. Footprint-membership conditions in native rules

This is an independent fix to native DRC and fill. It lands as its own change
(own branch and review), not as part of the EMI commits; it is listed here only
because P2 needs filled planes on boards that carry such a rule.

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

**Spectrum.** Clocks: the trapezoid harmonics at `clock_frequency` (the
existing `trapezoid_envelope`). NRZ data gets **two** figures, both reported and
labelled, because they differ by tens of dB:

- `repeating_pattern_bound`: the line spectrum of an alternating 1010 pattern
  (fundamental `data_rate / 2`, `trapezoid_envelope` with duty 0.5 and period
  `2 / data_rate`). Real links send repeating idle, training and comma patterns,
  so this is reachable; it is an upper bound, not the typical level.
- `random_data_in_rbw`: random NRZ has a continuous `sinc^2` power spectrum with
  the edge roll-off; the level a receiver reads is the power in its resolution
  bandwidth (the CISPR band's bandwidth, as the conducted pipeline uses). For
  2.5 Gbps in 120 kHz this is roughly 40 dB below the pattern bound
  (`10 log10(rbw / (data_rate / 2))` before the envelope shape). Check the
  closed form against a numerical spectrum of a long random sequence in a test.

Every later phase that takes a data signal's spectrum (P2's ranking, P3, P4)
carries both figures through; nothing collapses them into one. Whether a link
scrambles its data is not declared in v1 and is listed in `not_modelled`.

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
3. *Reference change at a via.* (Native DRC already has
   `check_single_ended_return_vias` in `crates/copper-drc/src/lib.rs`: a pass or
   fail rule for single-ended nets that declare `require_return_vias`, by the
   declared distance and return net. That rule stays as it is. This check is the
   screening counterpart: it runs on every subject net whether or not the rule
   is declared, includes pairs, and reports the distance and the reference
   change instead of a violation. Share the geometry helper
   (`return_span_covers`) rather than copying it, and when the DRC rule already
   failed for a via say so in the finding instead of reporting it twice.) Compare the reference planes before and after a
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

**Conventions (stated in the finding and the docs).** `V_diff = V_p - V_n`,
peak-to-peak as `signal_swing` declares it for a differential pin;
`V_cm = (V_p + V_n) / 2`. With these, an ideal pair whose one member is delayed
by `Δt` has

`|V_cm(f)| = (|V_diff(f)| / 2) · |sin(pi f Δt)|`

(each member carries `V_diff / 2`; the factor one half is part of the result,
omitting it is a 6 dB error). An amplitude imbalance `a = (|V_p| - |V_n|) /
(|V_p| + |V_n|)` gives `|V_cm| = (|V_diff| / 2) · a`.

**Estimate.** `V_cm(f) = (V_diff(f) / 2) · max(|sin(pi f Δt)|, a)`. Report the
conversion `20 log10(|V_cm| / |V_diff|)` in dB at the fundamental and its worst
value up to the edge corner, and the CM voltage envelope that P4 uses, for both
spectrum figures of P1. Lane-to-lane skew of a match group is **not** a
contributor; say so in the finding's notes. The skew and via-count terms are
closed forms; the amplitude imbalance from asymmetric return-path breaks is a
rule of thumb and is tagged as one.

**Tests.** Hand calculations of the closed form including the factor one half
(a numerical two-waveform check: delay one of two opposite trapezoids by `Δt`
and compare the spectrum of their mean); a synthetic pair with a known length
skew; zero skew gives no conversion from that contributor.

---

## P4. `EMI-CABLE-COMMON-MODE`

New module `crates/copper-drc/src/emi/cable.rs`. One finding per cable
connector (`cable_connectors()` in `emi.rs`: parts with `external_cable = true`
and `emission` targets), replacing today's single `unsupported` entry.

**Scope, decided in review.** P4 reports the common-mode *voltage* at the
connector, the common-mode *current* on an unshielded cable as a band, and a
ranking of the drivers. It does **not** report a radiated field or a
`margin_db` against the limit. An earlier review of this tool removed absolute
common-mode field figures because the cable, its routing and the chassis are
not modelled, and `copper-emi/src/estimate.rs` still says so; a margin built
from a rule-of-thumb cable impedance, a rule-of-thumb ground inductance and an
unmodelled cable would undo that. The field and margin become a later step,
gated on one measured case (`VALIDATION`), and are listed under "Out of scope".

**Prerequisite.** `cable_common_mode_field` documents `mu0 f l I / r` as
including the factor 2 of the ground-plane reflection. Check that against the
cited sources (Paul, Ott) before anything in this phase quotes the function or
its constant, and correct the comment or the constant. The function stays
unused by the report in this phase; the "Why" figure above uses it only as an
illustration.

**Language additions** (all optional; section 2–3 of `emi-noise-analysis.md`):

| Where | Attribute | Meaning |
|---|---|---|
| pin | `emi_role = shield` | A connector pin bonded to the cable's shield or shell (new `EmiRole::Shield`, a victim-side role, not an aggressor) |
| part | `cable_shielded` | bool: the cable the connector takes is shielded |
| part | `common_mode_impedance` | impedance at `common_mode_impedance_frequency` (frequency), for common-mode chokes and ferrites in series with a cable's lines |
| `emission` constraint | `cable_length` | already exists; recorded in the finding (it sets the quarter-wave frequency above which the cable is resonant), not used for a field |

**Drivers at the connector.** Each gives a common-mode voltage spectrum between
the cable's conductors as a bundle and the board reference at the connector.
A signal whose return conductor is in the same cable is **not** common mode by
itself: only its imbalance and the noise on its reference are.

1. *Differential pairs leaving on the cable* (`NetClasses::external`, already
   derived): `V_cm` from P3, both spectrum figures. A recognised common-mode
   choke or ferrite in the path divides by `Z_cmc / (Z_cmc + Z_cable)`; the
   input-filter walk in `emi/conducted.rs` is the pattern for finding it.
2. *Single-ended signals leaving on the cable.* Their swing is **not** taken as
   common-mode voltage (a 3.3 V line into 150 ohm would read as 22 mA of
   common-mode current, four orders of magnitude over the level that matters,
   on every board). Their contribution is the ground-noise term below, driven
   by their own return current, plus a ranking-only entry (`ranking_only`,
   no voltage) that lists them with edge time and swing so a reviewer sees which
   fast single-ended lines reach a cable. Signal-to-return imbalance inside the
   cable is `not_modelled`.
3. *Ground noise (current-driven).* `V = 2 pi f L_gnd I_return` with the existing
   `current_driven_cm_voltage`, for every return current that shares reference
   copper with the connector. The rule the code applies, so that "passes between
   the source and the connector" is decidable:
   - the return current of a signal that leaves on this connector always counts
     (its return enters the connector's ground pins);
   - the return current of another fast net or of a switching cell counts when
     the straight corridor between its source and load pins, widened by the
     `3 H` of P2, overlaps the corridor between this connector's ground pins
     and the nearest point of that net's return path on the same reference
     plane; otherwise it is listed as `not_counted` with the distance, not
     silently dropped.

   `L_gnd` is a closed-form ground-plane partial inductance for the geometry:
   name the published form used, state its validity range, and check it by hand
   in tests. A slot from P2 on the path raises it by the slot's detour (a rule
   of thumb, tagged). `I_return` for data signals uses both spectrum figures.
   This term is the weakest number in the plan (published closed forms for a
   finite plane differ from each other substantially): its band says so, and
   its place in the driver ranking is flagged when the band overlaps another
   driver's.
4. *Switch-node coupling (voltage-driven).* The island capacitance of
   `EMI-SWITCH-NODE` to the connector's shell and pins (a new closed-form
   capacitance by separation and area), times the switch voltage spectrum.
   Ranking only, like the existing `cm_coupling_score`: a score in dB, no
   voltage or current.

**Cable (unshielded only).** Common-mode current `I_cm(f) = V_cm(f) / Z_cm(f)`
for drivers 1 and 3, with `Z_cm` a rule-of-thumb band (nominal 150 ohm, the
common-mode impedance used by IEC 61000-4-6 coupling networks; band from the
resonant low end to a few hundred ohms), tagged
`rule_of_thumb_cable_impedance`. Reported as a band in dBµA, never a single
number, with `not_modelled`: cable routing and height, chassis, enclosure, the
far-end termination, cable resonance above the quarter-wave frequency (which
the finding states from `cable_length`).

**Shielded cable** (`cable_shielded = true` with `shield` pins): report the
shield bond (pins, via count to ground, estimated bond inductance) and the
connector's `V_cm`; mark the shield transfer impedance `not_modelled`; give no
common-mode current. `cable_shielded = true` without any `shield` pin is its own
finding (the shield has no declared bond). A connector with neither attribute is
treated as unshielded and tagged `assumed_unshielded_cable`.

**Finding.** `EMI-CABLE-COMMON-MODE:<connector>` with measures per driver:
`V_cm` spectrum (both figures for data), and for an unshielded cable the `I_cm`
band in dBµA, directly comparable with a clip-on current probe (say so in the
finding: it is the one quantity here a bench measurement can check without a
chamber). Drivers 1 and 3 are ranked by `I_cm`; drivers 2 and 4 are listed
beside them as ranking-only and are never summed with them. No field, no
`margin_db`. The `EMI-MARGIN` coverage text says that cable common mode is
reported as current and ranking only and why.

**Tests.** Closed forms by hand; a synthetic two-connector board where a pair
with known skew leaves on a cable (P3 → P4 chain checked end to end, with the
factor one half); a choke on the pair lowers the result by the expected ratio;
a single-ended line on a cable produces no `V_cm` from its swing; the corridor
rule on a net that does and one that does not share the connector's reference
path; a shielded cable gives the bond report and no current.

**Accept.** On the example, the finding gives `V_cm` and the `I_cm` band per
driver, the ranking, and no field or margin anywhere in the report.

---

## P5. Ground transfer inductance by extraction (deferred: definition and spike first)

Not committed work. The original idea ("a port at the source's return pins and
one at the connector's ground pins, both against the plane") does not exist in
`copper-extract` as built, for three reasons that must be resolved on paper and
in a spike before any product code:

1. **Definition.** The extraction's ports are loops: two terminals on copper,
   closed through the network. The voltage along a plane between two points is
   a partial-inductance quantity and depends on the path of the voltmeter (here:
   the cable). Write down the definition first, for example: port A is the
   signal's own loop (trace and plane return, driven at the source), port B is
   a sense loop from the connector's ground pins along a stated path to the
   source's reference; `L_gnd` is the mutual `L_AB`. State the sense path and
   show on the analytic microstrip case what the number converges to and how
   much it changes with the sense path. If it changes by more than the band P4
   already carries, P5 does not improve P4 and stops here.
2. **Mesh budget.** The window spans the plane from source to connector. The
   hot-loop window of a dense board already reaches the default budget of
   9000 filaments, and a mutual of a fraction of a nanohenry needs the
   convergence rules of section 13.10 to hold for an off-diagonal entry. The
   spike measures filaments and run time on the P2 example and on one real
   board, and whether the series is `established`.
3. **Validation.** `--extract-reference` is a DC check: at DC the plane current
   spreads by resistance, so agreement there says little about the return-current
   distribution at the frequencies that matter. It remains a topology and
   geometry check only; the frequency behaviour needs the analytic case above.

**Spike acceptance.** A written definition; the analytic case within its stated
tolerance; an `established` mesh series on the example within the default
budget. Only then is the phase planned in detail (port construction in
`board.rs`, `--extract` wiring, the rule that an extraction with status `error`
is not used).

---

## P6. Full-wave mixed-mode region (feasibility spike only)

Not committed work. The full-wave path as built reads the hot-loop inductance
with a first-order cell-size error and an absolute accuracy of roughly 10 to
15 %, and a long run on a real board did not decay. Mode conversion from a few
mils of skew is a far smaller effect than that, and a staircase mesh makes a
symmetric pair asymmetric unless both members lie on the grid identically.

**Spike** (in `crates/copper-fullwave` tests, no CLI flag, no report fields): a
grid-aligned symmetric pair over a solid plane with four ports, and the same
pair with one member lengthened by a known skew.

**Spike acceptance, all three:**
- the symmetric pair's Scd21 is at least 20 dB below the skewed pair's over the
  band up to the edge corner;
- the skewed pair's Scd21 matches `(1/2)|sin(pi f Δt)|` within the two-mesh
  sensitivity, and that sensitivity is below 3 dB;
- the same holds for the pair rotated off the grid axes (45 degrees), since real
  pairs are not grid-aligned; and the run time for a pair of realistic length is
  stated.

If any fails, P6 is closed with the numbers recorded in
`docs/emi-noise-analysis.md` and P3's closed form stays the only source. If all
pass, plan the product phase then (`--fullwave-signals`, cost stated by
`describe()` before running, budget refusal, the tag
`fullwave_mode_conversion`).

---

## P7. Report, plots, issues and documentation

- Report: the new findings and coverage go in the `copperscript-emi-check`
  document (additive fields; update `docs/schemas.md`; bump the schema only if an
  existing field changes) and therefore in `route-report.json`'s `emi` section.
- `copper issues` / `copper-report/src/issues/emi.rs` and `explain.rs`: explain
  texts for `EMI-RETURN-PATH`, `EMI-MODE-CONVERSION`, `EMI-CABLE-COMMON-MODE`.
- Plots (`--plot-spectrum`, `--plot-summary`): the `I_cm` band per driver in
  dBµA, without a limit line (there is no field estimate to compare with one);
  return-path gaps and reference changes marked on the board plot.
- `docs/emi-noise-analysis.md`: new sections for P1–P4 (and the recorded spike
  results of P5 and P6) in the style of sections
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
- A radiated field and a `margin_db` for cable common mode. `I_cm` is reported;
  turning it into a field against the limit waits for one measured case, so
  that the first absolute cable figure the tool prints has been compared with
  a measurement.
- Signal-to-return imbalance inside a cable for single-ended signals.
- Measurement import and correlation (keep `VALIDATION` unchanged until a
  measured case exists).

## Order of work

1. P0 as its own change. P1 (independent of P0).
2. P2, which catches most real fast-signal problems without any estimate.
3. P3, then P4 in its reduced scope (the `cable_common_mode_field` check comes
   first inside P4).
4. P7 for the delivered phases.
5. The P5 and P6 spikes, each ending in a decision recorded in this file.

## Changes from the review (2026-11-16)

- **P4, single-ended signals:** their swing is no longer taken as common-mode
  voltage; they contribute through ground noise and a ranking-only entry.
- **P4, scope:** common-mode voltage, current band (unshielded) and ranking
  only. The radiated field and `margin_db` are removed, consistent with the
  earlier decision to keep common mode a ranking until a measured case exists.
- **Shielded cables:** stated up front that the motivating multi-gigabit case
  gets the on-board checks and a shield-bond report, not a field or current.
- **P1, data spectrum:** two labelled figures (repeating-pattern bound and
  random data in the receiver bandwidth) instead of the bound alone.
- **P3, convention:** `V_cm = (V_diff / 2) |sin(pi f Δt)|` with the definitions
  written out; the earlier form was ambiguous by 6 dB.
- **P4, ground noise:** a decidable rule for which return currents count; the
  term's weakness stated in its band.
- **P4, prerequisite:** verify the ground-reflection factor documented on
  `cable_common_mode_field`.
- **P2:** relation to the existing `check_single_ended_return_vias` DRC rule.
- **P5:** deferred behind a written port definition and a spike (definition,
  mesh budget, what the DC reference can and cannot check).
- **P6:** reduced to a feasibility spike with acceptance criteria.
- **P0:** lands as its own change. **"Why" example:** corrected to a cable
  length inside the formula's validity. **Base:** `main` at `842112a`.

Update the status line at the top of this file as phases land.
