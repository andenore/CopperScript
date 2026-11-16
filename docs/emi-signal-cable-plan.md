# EMI: fast digital signals and cable common mode — implementation plan

Status: plan, not started; revised twice on 2026-11-16 after two reviews (see
"Changes from the reviews" at the end). Implementation is on hold until the revised plan is
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
P4 reports voltages and a ranking for either kind of cable, and no current.

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
| P4a | Circuit definitions and independent circuit fixtures for the cable (conductors, reference, return path, observation point; choke traversal; return-conductor cancellation) | P1, P3 | S |
| P4 | `EMI-CABLE-COMMON-MODE`: common-mode voltage at the connector per driver and a risk ranking; shield-bond report; **no current, no field, no margin** | P1–P3, P4a | M |
| P4b | Net bundle current for an explicitly defined termination and environment scenario: **not committed**, planned only after P4a's circuit is accepted | P4 | — |
| P5 | Ground transfer inductance by quasi-static extraction: **deferred** until its port definition and mesh budget are settled (a written definition and a spike first) | P4 | spike S |
| P6 | Full-wave mixed-mode region: **feasibility spike only**, no product code until the spike passes its criteria | P3 | spike S |
| P7 | Report, plots, issues, documentation, examples | each phase | M |

Each phase is one or more commits that build, pass `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`,
and update the docs it changes. Phases P1–P4 are the core, and P2 is the first
substantial delivery. P4b, P5 and P6 are not committed work.

**The main missing work is definitions, not solvers.** Every number from P3
onward is a quantity of an electrical network: which conductors, which
reference, which source and load impedances, where it is observed. Each phase
below states that network before its formula, and P4a exists to pin the cable's
network down with fixtures before P4 reports anything about it.

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

**Channel traversal.** A signal is one channel from its driver pin to the
connector, across the series parts in between, not one net. Today
`external_connectors` in `crates/copper-lang/src/emi.rs` reaches a connector
directly or through **one** series element and does not pair pins, so an
AC-coupling capacitor followed by a choke, or a four-pin common-mode choke,
cuts the source off from the connector: adding a choke would remove the signal
from the analysis instead of attenuating it. Replace it for signals with an
explicit walk:

- two-pin series parts pass pin 1 to pin 2, any number of them in a row:
  resistors and ferrites as `is_series_element` recognises them today, and
  additionally capacitors in series with a signal channel (AC coupling), which
  it does not recognise; a capacitor to a reference net is a shunt, not a hop;
- parts with more than two pins pass only across **declared pin pairs**: a new
  optional pin attribute `passes_to = <pin>` (datasheet-style, on the part; a
  four-pin common-mode choke declares two pairs). Without the declaration the
  walk stops at the part and the finding says where and why (never a silent
  drop);
- the walk never enters a declared supply rail (as today) and stops at the
  first connector with `external_cable = true`;
- it keeps the P and N identity of a pair through every part (P stays P across
  each declared pair; a part that swaps them is reported), and records the
  provenance: the ordered list of parts crossed, which P3 and P4 use for
  coupling capacitors and chokes.

`SignalSource` gets `channel: Vec<ChannelHop>` per member. Tests: direct
connection; one and two coupling capacitors; a four-pin choke with and without
`passes_to`; capacitors and choke combined; P/N identity preserved; a part
that would cross into a supply rail.

**Defaults** (`emi/defaults.rs`): when a fast net has no `rise_time`, use a band
by declared kind (e.g. differential with `data_rate`: 0.2–0.35 UI; clock without
data: a rule-of-thumb band); when no `signal_swing`, a band by kind (differential
100–400 mV, single-ended the driving pin's supply). Each value gets a source and a
tag (`rule_of_thumb_signal_edge`, `rule_of_thumb_signal_swing`).

**Spectrum.** Clocks: the trapezoid harmonic **lines** at multiples of
`clock_frequency` (`trapezoid_harmonic`); `trapezoid_envelope` is a continuous
envelope over those lines, not a line spectrum, and is named as such wherever
it is used. Data signals get three **different quantities**, each labelled;
none is called a bound on the others:

- `pattern_lines`: the line spectrum of a *named* repeating pattern. Computed
  for the alternating pattern (lines at odd multiples of `data_rate / 2`) and
  for the square patterns with runs of 2, 4 and 5 bits (the last covers the
  comma-like runs of 8b/10b codes). A longer pattern has lines where a shorter
  one has none: at 2.5 Gbps, `11110000` has its fundamental at 312.5 MHz, far
  below the 1.25 GHz of `1010`. So no single pattern bounds the others
  frequency by frequency, and the finding lists which patterns were evaluated.
- `continuous_bound`: a separately justified envelope that holds for every
  two-level waveform with the declared swing and edge time, whatever the
  pattern: the amplitude of any line is at most `2 A / pi` at low frequency
  with the edge roll-off above `1 / (pi t_r)` (state the derivation and its
  assumptions in the docs; test it against all the named patterns and against
  random sequences). It is loose by construction and is labelled so.
- `random_data_in_rbw`: random NRZ has a continuous `sinc^2` power spectrum with
  the edge roll-off; the level a receiver reads is the power in its resolution
  bandwidth. For 2.5 Gbps in 120 kHz this is roughly 40 dB below the lines of
  the alternating pattern. Checked against a numerical spectrum of a long
  random sequence.

Every later phase that takes a data signal's spectrum (P2's ranking, P3, P4)
carries the three through by name. Whether a link scrambles its data is not
declared in v1 and is listed in `not_modelled`.

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

**Network (stated in the finding and the docs).** Two legs driven by an ideal
odd-mode source and observed as voltages at one plane (the end of the routed
pair, or the connector pins after the channel walk of P1), each leg against
the board reference there, both legs terminated alike. `V_diff = V_p - V_n`,
peak-to-peak as `signal_swing` declares it for a differential pin;
`V_cm = (V_p + V_n) / 2`. This is a **voltage ratio at that plane**, not a
power-normalized mixed-mode S-parameter (see P6 for the relation).

**Estimate, from the two complex leg responses.** With leg gains `1 + a` and
`1 - a` (amplitude imbalance `a = (|V_p| - |V_n|) / (|V_p| + |V_n|)`) and one
leg delayed by `Δt`:

`V_p = (V_diff / 2)(1 + a)`, `V_n = -(V_diff / 2)(1 - a) exp(-j 2 pi f Δt)`

`|V_cm| / |V_diff| = (1/2) sqrt( sin^2(pi f Δt) + a^2 cos^2(pi f Δt) )`

Skew alone gives `(1/2)|sin(pi f Δt)|`, imbalance alone `a / 2`. Taking the
larger of the two is **not** used: with both terms at 0.1 it reads about 3 dB
low. Implement the leg responses as complex numbers and form `V_cm` from them,
so that further per-leg terms (a via, a coupling capacitor's tolerance) add to
the legs rather than to a formula. Report the conversion
`20 log10(|V_cm| / |V_diff|)` in dB at the fundamental and its worst value up to
the edge corner, and the CM voltage spectrum that P4 uses, for each spectrum
quantity of P1. Lane-to-lane skew of a match group is **not** a contributor;
say so in the finding's notes. The skew and via-count terms are closed forms;
the amplitude imbalance from asymmetric return-path breaks is a rule of thumb
and is tagged as one.

**Tests.** A numerical two-waveform check, independent of the formula: build
two opposite trapezoids, scale and delay one, and compare the spectrum of their
mean with the estimate, for delay only, amplitude only, and both together
(including `a = sin(pi f Δt) = 0.1`); a synthetic pair with a known length
skew; zero skew and zero imbalance give no conversion.

---

## P4a. The cable's circuit: definitions and fixtures (before P4)

No report output. A written circuit and a small independent circuit solver in
tests (nodal analysis with complex impedances, a few dozen lines, sharing no
code with the estimators), so that P4's formulas are checked against a network
instead of against themselves.

**Define, in `docs/emi-noise-analysis.md`:**

- *Conductors.* The signal conductors of a channel (one, or a P/N pair), the
  return or ground conductor inside the same cable when there is one, the
  shield when there is one, and **the bundle**: all conductors of the cable
  taken together.
- *Two different common modes.* (1) The pair's common mode against the board
  reference at the connector, `V_cm` of P3. Its current can return through the
  ground conductor inside the same cable; in the ideal case the net current of
  the bundle is then zero. (2) The **bundle's** common mode: the net current of
  all conductors together, which returns outside the cable through the
  environment and is what a clamp around the whole cable reads and what
  radiates. The published cable-radiation models drive (2) from a voltage
  between the cable and the antenna's *other* conductor (the board, its ground
  plane, other cables) through a stated coupling path (Hockanson et al.).
- *Reference and observation point.* For (1): the board reference at the
  connector's ground pins. For (2): the clamp position on the cable at the
  connector, and the other half of the antenna, which the board model does not
  contain.
- *Source and load.* Source impedance per driver, series parts from the
  channel walk (coupling capacitors, chokes), and the load each quantity sees.

**Fixtures (tests only), each solved by the independent nodal solver:**

1. *Return-conductor cancellation.* A pair with common mode and an in-cable
   return: the bundle's net current is zero; removing the return conductor
   makes it equal to the pair's common-mode current.
2. *Source and load impedance.* The divider for a given source, series and load
   impedance, complex, across frequency.
3. *Choke traversal.* A series common-mode impedance `Z_cmc` between a source
   `V_source` with `Z_source` and a load `Z_load`:
   `I = V_source / (Z_source + Z_cmc + Z_load)`, voltage at the load
   `V_source · Z_load / (Z_source + Z_cmc + Z_load)`. Three cases: no choke;
   equal impedances (`Z_source = 0`, `Z_cmc = Z_load`: half the voltage, not
   double); a choke far larger than the load (voltage tends to zero).
4. *Simultaneous imbalance.* The two-leg network of P3 with gain and delay
   mismatch together, against P3's expression.
5. *Choke in the signal lines only.* A choke that carries the pair but not the
   ground conductor attenuates the pair's common mode (1) and leaves a voltage
   on the ground conductor (the ground-noise driver) unchanged.

**Accept.** The definitions are in the docs; the fixtures pass; every formula P4
uses is one of these networks.

---

## P4. `EMI-CABLE-COMMON-MODE`: voltage and ranking

New module `crates/copper-drc/src/emi/cable.rs`. One finding per cable
connector (`cable_connectors()` in `emi.rs`: parts with `external_cable = true`
and `emission` targets), replacing today's single `unsupported` entry.

**Scope, decided in two reviews.** P4 reports, per driver, a common-mode
**voltage** at the connector against the board reference there, and a **risk
ranking** of the drivers. It reports **no current**, no radiated field and no
`margin_db`:

- A field or margin would undo the earlier decision to keep common mode a
  ranking until a measured case exists (`copper-emi/src/estimate.rs` still says
  the cable is not modelled).
- A current "comparable with a clamp around the cable" is the bundle's net
  current (P4a, common mode 2). P3's pair voltage is common mode 1, and the
  two are different quantities: dividing the pair's `V_cm` by a cable
  impedance, however wide its band, does not give the bundle current, because
  what is missing is the circuit (the in-cable return, the antenna's other
  conductor), not a parameter value. That conversion is P4b.

**Prerequisite.** `cable_common_mode_field` documents `mu0 f l I / r` as
including the factor 2 of the ground-plane reflection. Check that against the
cited sources (Paul, Ott) and correct the comment or the constant. The function
stays unused by the report.

**Language additions** (all optional; section 2–3 of `emi-noise-analysis.md`):

| Where | Attribute | Meaning |
|---|---|---|
| pin | `emi_role = shield` | A connector pin bonded to the cable's shield or shell (new `EmiRole::Shield`, a victim-side role, not an aggressor) |
| pin | `passes_to` | from P1: the pin this pin passes a signal to, for parts with more than two pins |
| part | `cable_shielded` | bool: the cable the connector takes is shielded |
| part | `common_mode_impedance` | complex or magnitude impedance of a common-mode choke or ferrite at `common_mode_impedance_frequency`; may be given at several frequencies |
| `emission` constraint | `cable_length` | already exists; recorded in the finding (it sets the quarter-wave frequency above which the cable is resonant) |

**Drivers at the connector.** Each is a voltage between named conductors and
the board reference at the connector, with its network from P4a.

1. *Differential pairs leaving on the cable.* Found by the channel walk of P1
   (not by `NetClasses::external`, which stops after one series element):
   `V_cm` of P3 at the connector pins, for each spectrum quantity. Coupling
   capacitors on the channel are in the leg responses.
   A common-mode choke on the channel is the series element of P4a's fixture 3:
   the voltage delivered beyond it is
   `V_source · Z_load / (Z_source + Z_cmc + Z_load)`, all complex and functions
   of frequency. The **load** `Z_load` (the pair's common-mode termination at
   the far end and the cable's pair-to-return common-mode impedance) is not on
   the board: it is a stated scenario value with a band, and the finding gives
   the voltage both before the choke (a board quantity) and beyond it (a
   scenario quantity, labelled).
   *Applicability of the declared impedance.* A choke's impedance is strongly
   frequency dependent and changes from inductive to resistive to capacitive.
   One declared magnitude at one frequency supports an attenuation **at that
   frequency only**. Between two or more declared points the magnitude is
   interpolated on log axes; nothing is extrapolated beyond the declared range,
   and outside it the choke is reported as present with its attenuation
   `not_modelled`. With magnitudes only (no phase), the series sum is bounded
   by `| |Z_cmc| - |Z_source + Z_load| |` and `|Z_cmc| + |Z_source + Z_load|`
   and the result is that band, not a single value.
2. *Single-ended signals leaving on the cable.* Their swing is **not** taken as
   common-mode voltage: a signal whose return is in the same cable is not
   common mode by itself. They contribute through the ground-noise term below,
   driven by their own return current, and a ranking-only entry lists them with
   edge time and swing. Signal-to-return imbalance inside the cable is
   `not_modelled`.
3. *Ground noise (current-driven).* The voltage along the board's reference
   between a noise source and the connector's ground pins,
   `V = 2 pi f L_gnd I_return` (`current_driven_cm_voltage`). This is the driver
   that acts on the **bundle** (every conductor of the cable is referenced to
   that ground), which is why it matters most for radiation, and a choke in the
   signal lines does not attenuate it (P4a fixture 5). The rule for which
   return currents count:
   - the return current of a signal that leaves on this connector always counts;
   - the return current of another fast net or of a switching cell counts when
     the straight corridor between its source and load pins, widened by the
     `3 H` of P2, overlaps the corridor between this connector's ground pins
     and the nearest point of that net's return path on the same reference
     plane; otherwise it is listed as `not_counted` with the distance.

   `L_gnd` is a closed-form ground-plane partial inductance for the geometry:
   name the published form, state its validity range, check it by hand in
   tests. A slot from P2 on the path raises it by the slot's detour (rule of
   thumb, tagged). This is the weakest number in the plan (published closed
   forms for a finite plane differ substantially): its band says so, and its
   place in the ranking is flagged when the band overlaps another driver's.
4. *Switch-node coupling (voltage-driven).* The island capacitance of
   `EMI-SWITCH-NODE` to the connector's shell and pins times the switch voltage
   spectrum. A score in dB like the existing `cm_coupling_score`; no voltage.

**Shielded cable** (`cable_shielded = true` with `shield` pins): the shield
bond (pins, via count to ground, estimated bond inductance) and the drivers'
voltages; shield transfer impedance `not_modelled`. `cable_shielded = true`
without any `shield` pin is its own finding (no declared bond). A connector
with neither attribute is treated as unshielded and tagged
`assumed_unshielded_cable`.

**Finding.** `EMI-CABLE-COMMON-MODE:<connector>` with, per driver: the
conductors and reference its voltage is between, the voltage spectrum (in
dBµV, per spectrum quantity for data), the channel provenance (parts crossed),
and its rank. Drivers 1 and 3 are ranked by voltage **within their own kind**
and shown side by side; they are not summed and not converted to one figure,
because they act on different conductors (pair against reference, bundle
against the environment). Drivers 2 and 4 are ranking-only. `not_modelled`:
the bundle's net current, cable routing and height, chassis, enclosure,
far-end termination, cable resonance above the quarter-wave frequency (stated
from `cable_length`). The `EMI-MARGIN` coverage text says cable common mode is
reported as voltage and ranking only, and why.

**Tests.** Every formula against its P4a fixture; a synthetic two-connector
board where a pair with known skew leaves on a cable through coupling
capacitors and a four-pin choke (channel walk → P3 → P4 end to end); the choke
cases of fixture 3 (none, equal, large) through the real code path; a choke
declared at one frequency gives an attenuation at that frequency and
`not_modelled` elsewhere; a single-ended line on a cable produces no voltage
from its swing; the corridor rule on a net that does and one that does not
share the connector's reference path; a shielded cable gives the bond report.

**Accept.** On the example the finding gives the per-driver voltages with
their conductors and reference, the ranking, and no current, field or margin
anywhere in the report.

---

## P4b. Net bundle current for a defined scenario (not committed)

Planned only after P4a's circuit is accepted. A current is reported only for an
explicitly defined termination and environment scenario, named in the output:
which conductors the cable has (in-cable return or not), how the far end is
terminated, what the antenna's other conductor is (for example the test
set-up's ground plane at a stated height), and the clamp position. Under such a
scenario the bundle current follows from the P4a network; without one it is not
reported. This is also the step that a bench measurement with a current clamp
can check, and it should be planned together with that measurement.

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

**Quantities, specified before the spike.** Single-ended port impedance
`Z0` (stated, real), reference planes at the port positions, all four ports
terminated in `Z0`. Mixed-mode conversion with the conventional normalization
`Z_d = 2 Z0`, `Z_c = Z0 / 2`. Two different things are then tested separately:

- the **voltage ratio** `|V_cm| / |V_diff|` at the far reference plane, which
  for an ideal matched pair with skew `Δt` is `(1/2)|sin(pi f Δt)|` (P3's
  quantity);
- the **mixed-mode S-parameter** `|Scd21|`, a ratio of power waves, which for
  the same ideal fixture is `|sin(pi f Δt)|`: with `Z_d = 2 Z0` and
  `Z_c = Z0 / 2` the normalization contributes a factor
  `sqrt(Z_d / Z_c) = 2` over the voltage ratio. Expecting `1/2 |sin|` for
  `Scd21` would be 6 dB low and would fail a correct solver.

**Spike acceptance, all three:**
- the symmetric pair's `Scd21` is at least 20 dB below the skewed pair's over
  the band up to the edge corner;
- for the skewed pair, the voltage ratio matches `(1/2)|sin(pi f Δt)|` **and**
  `|Scd21|` matches `|sin(pi f Δt)|`, each within the two-mesh sensitivity, and
  that sensitivity is below 3 dB;
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

- **Migration away from converter-only reporting (do this with P2, not at the
  end).** The report integration and the plots assume switching cells today:
  `crates/copper/src/emi_section.rs` drops the whole `emi` section from
  `route-report.json` when `cells` is empty, and `--plot-summary`
  (`crates/copper/src/emi_check.rs`) refuses a board without a switching cell
  and lays out one page per cell. So a board with a clock, a pair and a cable
  connector but no converter would compute the new findings and then lose them.
  Change: the section is kept when there is any finding or executed coverage
  entry (cells, signals or connectors); the summary and `copper issues` group by
  subject kind (converter cell, signal or pair, cable connector) instead of by
  cell only; the summary page has a signal and a connector layout. Required
  end-to-end fixture: a synthetic board with a clock, a differential pair and a
  cable connector and **no converter**, checked through `emi-check --json`,
  `route-report.json`, `copper issues --stage emi` and `--plot-summary`.
- Report: the new findings and coverage go in the `copperscript-emi-check`
  document (additive fields; update `docs/schemas.md`; bump the schema only if an
  existing field changes) and therefore in `route-report.json`'s `emi` section.
- `copper issues` / `copper-report/src/issues/emi.rs` and `explain.rs`: explain
  texts for `EMI-RETURN-PATH`, `EMI-MODE-CONVERSION`, `EMI-CABLE-COMMON-MODE`.
- Plots (`--plot-spectrum`, `--plot-summary`): the per-driver voltage spectra
  at each connector in dBµV, without a limit line (there is no field estimate
  to compare with one);
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
- A radiated field and a `margin_db` for cable common mode, and (until P4b) the
  bundle's net current. The first absolute cable figure the tool prints should
  have been compared with a measurement.
- Signal-to-return imbalance inside a cable for single-ended signals.
- Measurement import and correlation (keep `VALIDATION` unchanged until a
  measured case exists).

## Order of work

1. P0 as its own change. P1 (independent of P0), including the channel walk.
2. P2, the first substantial delivery: located findings without any estimate.
   The report migration of P7 lands with it, with the no-converter fixture.
3. P3 (complex leg responses).
4. P4a: the cable's circuit written down and its fixtures passing.
5. P4 in its voltage-and-ranking scope (the `cable_common_mode_field` check
   comes first).
6. The rest of P7 for the delivered phases.
7. P4b, P5 and P6 only after their definitions or spikes, each ending in a
   decision recorded in this file.

## Changes from the reviews (2026-11-16)

Second review (numerical contracts):

- **Choke formula corrected.** It was reversed (equal impedances doubled the
  voltage). Now the series network `I = V / (Z_source + Z_cmc + Z_load)` with
  complex, frequency-dependent impedances, tests for no choke, equal impedances
  and a large choke, and a stated limit on what one impedance at one frequency
  supports.
- **Pair common mode and bundle common mode separated.** P4 no longer converts
  P3's pair voltage into a clamp-comparable current. P4 is voltage and ranking;
  the bundle current is P4b, for a defined scenario only.
- **P4a added:** circuit definitions and independent circuit fixtures
  (return-conductor cancellation, source and load impedance, choke traversal,
  simultaneous imbalance, choke in the signal lines only) before P4.
- **Channel traversal planned in P1:** across any number of two-pin series
  parts and across declared pin pairs (`passes_to`), keeping P/N identity and
  provenance; the existing one-series-element discovery is not used for
  signals.
- **P3 combined imbalance:** `(1/2) sqrt(sin^2 + a^2 cos^2)` from the two
  complex leg responses instead of the larger of the two terms.
- **P6 acceptance:** voltage ratio and `Scd21` tested separately, with port
  impedances, normalization and reference planes stated; `Scd21 = |sin|`, not
  half of it.
- **P7 migration:** the report and summary stop assuming switching cells; a
  no-converter end-to-end fixture is required.
- **P1 spectrum:** named-pattern lines, a separately justified continuous
  bound, and random data in the receiver bandwidth, as three quantities; the
  alternating pattern is no longer called a bound.

First review:

- **P4, single-ended signals:** their swing is not taken as common-mode
  voltage.
- **P4, scope:** no radiated field and no `margin_db`.
- **Shielded cables:** stated up front what the motivating multi-gigabit case
  gets.
- **P4, ground noise:** a decidable rule for which return currents count; the
  term's weakness stated in its band.
- **P4, prerequisite:** verify the ground-reflection factor documented on
  `cable_common_mode_field`.
- **P2:** relation to the existing `check_single_ended_return_vias` DRC rule.
- **P5:** deferred behind a written port definition and a spike.
- **P6:** reduced to a feasibility spike with acceptance criteria.
- **P0:** lands as its own change. **"Why" example:** corrected to a cable
  length inside the formula's validity. **Base:** `main` at `842112a`.

Update the status line at the top of this file as phases land.
