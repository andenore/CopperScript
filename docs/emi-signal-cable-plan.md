# EMI: fast digital signals and cable common mode — implementation plan

Status: plan revised five times on 2026-11-16 after five
reviews (see
"Changes from the reviews" at the end). **P0 is on `main` (commit 1670f86); P1, P2, P3 and P4a are implemented**; see sections 14, 15, 16 and 17 of
`emi-noise-analysis.md`. **P7 report migration implemented**: the `emi` section, `--plot-summary`
and `copper issues` work on a board with no switching converter (section 15.12); the rest of P7 stays open. P4 onward is
not started. Base: `main` at `842112a`. This plan
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

**Names, used the same way in P3, P4 and P6.** Input and output are different
planes and never share a symbol:

- `V_diff,in`: the differential voltage the driver launches, `V_p - V_n` at the
  driver's pins; its spectrum comes from the declared `signal_swing`, rate and
  edge (P1). This is the excitation.
- `V_p,out`, `V_n,out`: the two leg voltages at the observation plane (a named
  point of the channel: the end of a routed section, or the connector pins),
  each against the board reference there.
- `V_diff,out = V_p,out - V_n,out` and `V_cm,out = (V_p,out + V_n,out) / 2`.

The conversion P3 reports is `V_cm,out / V_diff,in`: common mode at the
observation plane per unit of launched differential voltage. It is **not**
`V_cm,out / V_diff,out`. For pure skew `Δt` the two are

- `|V_cm,out / V_diff,in| = (1/2) |sin(pi f Δt)|`
- `|V_cm,out / V_diff,out| = (1/2) |tan(pi f Δt)|`

(0.354 and 0.5 at `f Δt = 0.25`, a 3 dB difference; the second grows without
bound where the skew cancels the differential signal). Both are voltage ratios,
not power-normalized mixed-mode S-parameters (P6 gives the relation). The
finding names the ratio and both planes.

**The channel is an ordered cascade, not a sum.** Where a conversion happens
relative to a filter decides what the filter does to it: common mode generated
*before* a common-mode choke passes through the choke; common mode generated
*after* it does not. With a choke whose common-mode transfer is 0.1, upstream
conversion is suppressed by 20 dB and downstream conversion not at all, so
adding the skews of the whole channel and applying the choke to the total can
be wrong by the full attenuation. P3 therefore keeps the order the channel walk
of P1 gives, as a list of elements from driver to observation plane:

- *routed sections* (between two hops, per layer): each has its own leg
  responses, a complex gain per leg from that section's intra-pair length
  difference (delay from the layer's effective permittivity, as `si-check`
  computes it), via-count difference, and the amplitude imbalance from
  asymmetric return-path breaks or edge proximity found by P2 in that section
  (rule of thumb, tagged);
- *series parts* from the walk: a coupling capacitor (per-leg, its tolerance an
  amplitude imbalance), a common-mode choke or ferrite (acts on the modes, not
  the legs).

**Estimate: propagate the two leg amplitudes through the cascade.** State
`(V_p, V_n)`, starting at `(V_diff,in / 2, -V_diff,in / 2)`.

*One normalization for the whole cascade.* The state is the pair of **forward
(incident) voltage waves** on the legs, already launched into the line:
`V_diff,in` is the loaded voltage at the driver's pins, not an open-circuit
source voltage. One differential reference impedance `Z_d,ref` and one
common-mode reference impedance `Z_c,ref` hold from the driver to the
observation plane (the pair's modal impedances from the stack-up). Every
element is a transmission of incident to transmitted wave in that reference, so
an element that does nothing has a transfer of exactly 1.

- A routed section multiplies each leg by its own complex gain,
  `g_p = (1 + a) exp(-j 2 pi f t_p)`, `g_n = (1 - a) exp(-j 2 pi f t_n)`.
- A modal element converts to `(V_diff, V_cm)`, multiplies the differential
  part by its `T_dd(f)` and the common part by its `T_cc(f)`, and converts back.
  For a series common-mode impedance `Z_cmc` (a choke or ferrite) in the
  common-mode reference `Z_c,ref`, the transmission of the incident wave is
  `T_cc = 2 Z_c,ref / (2 Z_c,ref + Z_cmc)`: 1 for `Z_cmc = 0`, 1/2 for
  `Z_cmc = 2 Z_c,ref`, tending to zero for a large choke. It is **not** the
  open-circuit divider `Z / (Z + Z_cmc + Z)`, which gives 1/2 for a choke of
  zero impedance and would charge every element an artificial 6 dB; that
  divider belongs to a Thévenin source driving a load (P4a fixture 3) and the
  two agree once the source's own loading is accounted for (`V_inc = V_oc / 2`
  for a matched source).
- `V_cm,out` and `V_diff,out` are read from the state at the observation plane.

Assumption, stated in the finding: a matched cascade without reflections
between elements (each element sees its reference impedances); reflections and
mode conversion inside parts are `not_modelled`.

For one section with gain and delay mismatch and no modal element this reduces
to the closed form

`|V_cm,out / V_diff,in| = (1/2) sqrt( sin^2(pi f Δt) + a^2 cos^2(pi f Δt) )`

(skew alone `(1/2)|sin|`, imbalance alone `a / 2`; taking the larger of the two
would read about 3 dB low when both are 0.1, and is not used).

**Finding.** `EMI-MODE-CONVERSION:<pair>`: `|V_cm,out / V_diff,in|` in dB at the
fundamental and its worst value up to the edge corner; the `V_cm,out` spectrum
at each observation plane P4 needs (before and after each modal element, and at
the connector pins), for each spectrum quantity of P1; and the contribution of
each element in order, so a reviewer sees which section converts and whether a
choke is upstream or downstream of it. Lane-to-lane skew of a match group is
**not** a contributor; say so in the notes.

**Tests.** Independent of the estimator's formulas:

- a numerical two-waveform check: two opposite trapezoids, one scaled and
  delayed; the spectrum of their mean against the estimate for delay only,
  amplitude only and both together (including `a = sin(pi f Δt) = 0.1`), and the
  two ratios (`/ V_diff,in` and `/ V_diff,out`) told apart at `f Δt = 0.25`;
- **order fixtures:** two channels with identical total skew and the same
  choke (`T_cc = 0.1`, that is `Z_cmc = 18 Z_c,ref`), one with the choke after the skewed section and one
  with it before. The first reads 20 dB below the unfiltered conversion at the
  connector, the second reads the unfiltered conversion; a third with the skew
  split evenly on both sides reads between them, as the cascade gives it;
- **identity fixture:** inserting any number of zero-impedance modal elements
  and zero-length routed sections anywhere in a channel leaves every reported
  quantity unchanged, to rounding;
- a synthetic pair with a known length skew on a routed board; zero skew and
  zero imbalance give no conversion.

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
3. *Choke traversal, in both representations.* A series common-mode impedance
   `Z_cmc` in a reference `Z_ref`.
   (a) *Incident wave*, the cascade's representation:
   `T = 2 Z_ref / (2 Z_ref + Z_cmc)`. Cases: no choke (`T = 1`, exactly);
   `Z_cmc = 2 Z_ref` (`T = 1/2`); a choke far larger than the reference (`T`
   tends to zero).
   (b) *Thévenin source*, open-circuit voltage `V_oc` with `Z_source` into a
   load `Z_load`: `I = V_oc / (Z_source + Z_cmc + Z_load)`, load voltage
   `V_oc · Z_load / (Z_source + Z_cmc + Z_load)`.
   (c) *The two agree:* for `Z_source = Z_load = Z_ref` the load voltage of (b)
   equals `(V_oc / 2) · T`, the incident wave of a matched source times the
   transmission of (a), for every `Z_cmc` including zero.
   (d) *Identity:* any number of zero-impedance elements in series leaves the
   result unchanged in both representations.
4. *Simultaneous imbalance.* The two-leg network of P3 with gain and delay
   mismatch together, against P3's expression.
5. *Choke in the signal lines only.* A choke that carries the pair but not the
   ground conductor attenuates the pair's common mode (1) and leaves a voltage
   on the ground conductor (the ground-noise driver) unchanged.
6. *Cable termination.* The cascade's connector voltage against the nodal
   solver's steady-state voltage for three loads behind a choke: **matched**
   (`Z_cable = Z_ref`: they agree); **high impedance** (the 50 / 900 / 900 ohm
   example: the forward-only multiplier reads 0.189 V, the circuit 0.973 V);
   **open** (the circuit gives the full open-circuit voltage, twice the
   incident wave, whatever the choke). The test asserts agreement in the
   matched case and asserts the *size of the disagreement* in the other two,
   so that the forward-only figure can never be relabelled as a steady-state
   voltage without a test failing.
7. *Choke position.* The order fixtures of P3 (the same total skew with the
   choke before and after the skewed section), solved as a network of two legs
   and a common-mode series impedance by the nodal solver, against the cascade.
8. *Ground-noise transfer.* The network defined below:
   `V_B = sum of Z_Bk I_k` over the driven conductors, with each `Z_Bk` from a
   numerical Neumann integral over that conductor's loop and loop B
   (written in the test, sharing no code with `copper-emi`'s closed forms or
   with `copper-extract`), against whatever closed form P4 uses for the same
   geometry; and the sign cases listed below.

**The ground-noise network (defined here, used by P4's ground-noise driver).**
`omega L_gnd I_return` is not a calculation until these four are stated; the
corridor rule of P4 only selects which sources are considered, it does not give
their coupling.

- *Driven loop A.* One per source conductor: from the driver pin along the
  routed copper to the load (or the connector pin), back through the reference
  plane under the route to the driver's reference pin. `Z_A` is what the loop
  presents to the pin: the line's characteristic impedance (from the stack-up,
  as `si-check` computes it) while the edge travels on a terminated or long
  line, or the declared load. The current follows from **one** of two source
  representations, never a mixture:
  - *Loaded pin voltage* (the default, and what `signal_swing` and P3's
    `V_diff,in` mean: the voltage at the pin while it drives the line):
    `I_A = V_pin / Z_A`. The source impedance does not appear, because its
    loading is already in `V_pin`. A 1 V launched signal on a matched 50 ohm
    line is 20 mA.
  - *Thévenin source* (only when a part declares an open-circuit swing and an
    `output_impedance`): `V_pin = V_oc · Z_A / (Z_source + Z_A)` and
    `I_A = V_oc / (Z_source + Z_A)`.
  Dividing a loaded pin voltage by `Z_source + Z_A` applies the source loading
  twice (10 mA instead of 20 mA in the example, 6 dB low) and is the error this
  definition excludes. Test: a source declared both ways (`V_oc`, `Z_source`
  and the `V_pin` they imply) gives identical `V_pin`, `I_A` and `V_B`.
  Without a characteristic impedance or declared load for the route, `I_A` is
  **not** estimated (see the fallback).
- *Observation loop B.* The loop whose voltage is the ground-noise voltage at
  the connector: from the connector's ground pins through the reference plane
  to the source's reference pin, closed by a stated sense path. The sense path
  is part of the definition because the answer depends on it (it stands for the
  cable leaving the board and the antenna's other conductor): v1 uses a
  straight return at a stated height above the reference plane, a scenario
  parameter with a band, and the finding reports how much the result moves
  across that band.
- *Transfer, per conductor.* Each driven conductor `k` has its own loop and its
  own signed transfer to loop B: `Z_Bk = j omega M_Bk` plus the resistance of
  the copper the two loops share, with `M_Bk` the mutual inductance of that
  conductor's loop and loop B, sign included. "`L_gnd`" in this plan means such
  an `M_Bk` and nothing else.
- *Signed contributions: couple first, then sum.*
  `V_B = sum over k of Z_Bk I_k`. For a differential pair,
  `V_B = j omega (M_Bp I_p + M_Bn I_n)`. The leg currents are **not** added
  before the coupling is applied: with `I_p = -I_n` the summed current is zero,
  but the voltage is `j omega (M_Bp - M_Bn) I_p`, which is not zero whenever the
  two legs couple differently to loop B (one leg nearer the connector's ground
  path, or over a gap). Example: `+1 mA` and `-1 mA` with `M_Bp = 2 nH` and
  `M_Bn = 1 nH` give about 628 µV at 100 MHz with zero summed current. In modal
  terms `V_B = j omega [ (M_Bp + M_Bn) I_cm + (M_Bp - M_Bn) I_dm ]` with
  `I_cm = (I_p + I_n) / 2`, `I_dm = (I_p - I_n) / 2`: the common-mode current
  couples through the sum of the mutuals and the differential current through
  their difference, and both terms are kept. Several independent sources are
  summed as phasors only when they are harmonics of one clock, otherwise as
  powers.

*Sign and cancellation cases for fixture 8:* a single loop A beside B; A moved
away (the transfer falls); **symmetric cancellation**: a pair whose legs have
equal mutuals to B and equal and opposite currents gives zero; **asymmetric
residual**: the same currents with `M_Bp = 2 nH`, `M_Bn = 1 nH` give
`omega · 1 nH · 1 mA` (628 µV at 100 MHz), not zero; a pair with a common-mode
current added (the extra voltage follows `(M_Bp + M_Bn) I_cm`); the sense height
varied across its band.

*Fallback, binding for P4.* Where the route's characteristic impedance (or
declared load) or the geometry of any of the loops is not available, or where the result
moves by more than 6 dB across the sense-path band, no ground-noise voltage is
published for that source. It is ranked by geometry instead: corridor overlap
with the connector's ground path, distance, and any P2 slot between them.
Extraction of the `M_Bk` from the real copper stays deferred (P5); the definition
does not.

**Accept.** The definitions are in the docs, including the ground-noise
network and its fallback; the fixtures pass; every formula P4 uses is one of
these networks.

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
| pin | `output_impedance` | from P4a: a driver pin's output impedance, used only together with a declared open-circuit swing (the Thévenin representation); not needed when `signal_swing` is the loaded pin voltage |
| part | `cable_shielded` | bool: the cable the connector takes is shielded |
| part | `common_mode_impedance` | complex or magnitude impedance of a common-mode choke or ferrite at `common_mode_impedance_frequency`; may be given at several frequencies |
| `emission` constraint | `cable_length` | already exists; recorded in the finding (it sets the quarter-wave frequency above which the cable is resonant) |

**Drivers at the connector.** Each is a voltage between named conductors and
the board reference at the connector, with its network from P4a.

1. *Differential pairs leaving on the cable.* Found by the channel walk of P1
   (not by `NetClasses::external`, which stops after one series element). The
   voltage is `V_cm,out` of P3 at the connector pins, for each spectrum
   quantity, taken from P3's ordered cascade. P4 applies **no** choke divider
   of its own to it: a choke on the channel is already an element of that
   cascade at its position, so conversion upstream of it is attenuated and
   conversion downstream of it (the routing from the choke to the connector,
   the connector's own breakout) is not. The finding shows the two parts
   separately: common mode arriving from upstream of the last choke, and common
   mode generated after it.
   The choke's common-mode transfer is the cascade's incident-wave
   transmission, `T_cc(f) = 2 Z_c,ref / (2 Z_c,ref + Z_cmc(f))`, complex and a
   function of frequency, in the cascade's one common-mode reference impedance
   (P3; P4a fixture 3a). A choke of zero impedance changes nothing.
   *What the connector voltage means, and its limit.* The cascade is forward
   only: it has no reflections. Its output at the connector is the incident
   common-mode wave, and that equals the steady-state voltage there **only for
   a matched termination**, a cable whose common-mode impedance at the
   connector is `Z_c,ref`. That is the one case for which v1 publishes an
   absolute connector voltage, and the finding states the assumption
   (`matched_cable_termination`).
   For any other termination the wave reflected by the cable returns to the
   choke and is reflected again, and a final multiplier
   `2 Z_cable / (Z_c,ref + Z_cable)` on the forward wave does not recover the
   steady state: the complete relation has the load-feedback denominator
   `1 - S22 Gamma_load`, which the cascade does not carry. Example, all
   resistive: 1 V incident in 50 ohm, a 900 ohm choke, a 900 ohm cable load.
   The multiplier gives 0.189 V; the series circuit gives 0.973 V; the
   multiplier is 14.2 dB low. So v1 does **not** publish a steady-state cable
   voltage for an unmatched cable. It may publish the multiplier's result only
   under the name `first_arrival_voltage` (the first wavefront at the cable
   before any reflection returns), labelled as not a steady-state value, and
   never ranked against or compared with a steady-state quantity. Solving the
   terminated network with reflections is a later step and belongs with P4b's
   defined scenario.
   *Applicability of the declared impedance.* A choke's impedance is strongly
   frequency dependent and changes from inductive to resistive to capacitive.
   One declared magnitude at one frequency supports an attenuation **at that
   frequency only**. Between two or more declared points the magnitude is
   interpolated on log axes; nothing is extrapolated beyond the declared range,
   and outside it the choke is reported as present with its attenuation
   `not_modelled`. With magnitudes only (no phase), the series sum is bounded
   by `| |Z_cmc| - 2 Z_c,ref |` and `|Z_cmc| + 2 Z_c,ref`
   and the result is that band, not a single value.
2. *Single-ended signals leaving on the cable.* Their swing is **not** taken as
   common-mode voltage: a signal whose return is in the same cable is not
   common mode by itself. They contribute through the ground-noise term below,
   driven by their own return current, and a ranking-only entry lists them with
   edge time and swing. Signal-to-return imbalance inside the cable is
   `not_modelled`.
3. *Ground noise (current-driven).* The voltage `V_B = sum of Z_Bk I_k` of P4a's
   ground-noise network: driven loop, observation loop with its sense path,
   source and load impedances and signed contributions are defined there, and
   P4 uses that definition unchanged. This is the driver that acts on the
   **bundle** (every conductor of the cable is referenced to that ground), and
   a choke in the signal lines does not attenuate it (P4a fixture 5).
   - *Which sources are considered* (selection only, not coupling): the return
     current of a signal that leaves on this connector always; another fast net
     or a switching cell when the straight corridor between its source and load
     pins, widened by the `3 H` of P2, overlaps the corridor between this
     connector's ground pins and the nearest point of that net's return path on
     the same reference plane; otherwise it is listed as `not_counted` with the
     distance.
   - *When a voltage is published.* Only when P4a's inputs exist for that source
     (a characteristic impedance or declared load for the route, the geometry
     of each conductor's loop and of the observation loop) and the result stays within 6 dB across the sense-path band.
     Each `M_Bk` is then a closed form for the two loops over the plane, one
     per driven conductor (for a pair: `M_Bp` and `M_Bn`, never one value for
     the pair): name the
     published form, state its validity range, and it must agree with P4a's
     fixture 8 for the test geometries. A slot from P2 between the loops is not
     in the closed form: it makes the source ranking-only, with the slot named.
   - *Otherwise: geometry ranking only* (P4a's fallback): corridor overlap,
     distance and slots, tagged `ranking_only`, no voltage. This is expected to
     be the common case in v1, and the finding says which input was missing.
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
(driver 3 only for the sources that have a published voltage; the others are
ranked by geometry beside them) and shown side by side; they are not summed and not converted to one figure,
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
`not_modelled` elsewhere; the same total skew with the choke before and after
the skewed section, through the real code path, against P3's order fixtures; a
ground-noise source with all P4a inputs gives a voltage that matches fixture 8,
one without a characteristic impedance gives a geometry ranking and no voltage;
a pair routed symmetrically about the connector's ground path gives a
ground-noise voltage near zero and the same pair routed with one leg nearer
gives the residual of fixture 8; a channel with a zero-impedance choke reads
the same as the channel without it; the connector voltage is published as
absolute only with the matched-termination assumption, and an unmatched
scenario yields `first_arrival_voltage` and no steady-state figure; a single-ended line on a cable produces no voltage
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

1. **Definition.** (The network itself, driven loop, observation loop, sense
   path and the per-conductor transfers `Z_Bk`, is now defined in P4a and is not postponed to this
   phase; what remains here is how to realise it in the extraction.) The
   extraction's ports are loops: two terminals on copper,
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

**Quantities, specified before the spike** (names as in P3). Single-ended
port impedance `Z0` (stated, real), reference planes at the port positions, all
four ports terminated in `Z0`; ports 1 and 3 are the near end of the P and N
legs (input), ports 2 and 4 the far end (output). Mixed-mode conversion with
the conventional normalization `Z_d = 2 Z0`, `Z_c = Z0 / 2`. Two different
things are tested separately, and both have the **incident (input)
differential** quantity in the denominator:

- the **voltage ratio** `|V_cm,out / V_diff,in|`: common-mode voltage at the far
  reference plane per unit of differential voltage launched at the near one.
  For an ideal matched pair with skew `Δt` it is `(1/2)|sin(pi f Δt)|` (P3's
  quantity). It is not `|V_cm,out / V_diff,out|`, which is
  `(1/2)|tan(pi f Δt)|` for the same fixture; the spike computes both and
  checks each against its own expression, so that neither can be mistaken for
  the other.
- the **mixed-mode S-parameter** `|Scd21|`, the common-mode wave leaving the far
  end per incident differential wave at the near end, a ratio of power waves.
  For the same ideal fixture it is `|sin(pi f Δt)|`: with `Z_d = 2 Z0` and
  `Z_c = Z0 / 2` the normalization contributes a factor
  `sqrt(Z_d / Z_c) = 2` over the voltage ratio `V_cm,out / V_diff,in`.
  Expecting `1/2 |sin|` for `Scd21` would be 6 dB low and would fail a correct
  solver.

**Spike acceptance, all three:**
- the symmetric pair's `Scd21` is at least 20 dB below the skewed pair's over
  the band up to the edge corner;
- for the skewed pair, `|V_cm,out / V_diff,in|` matches `(1/2)|sin(pi f Δt)|`
  **and** `|Scd21|` matches `|sin(pi f Δt)|`, each within the two-mesh sensitivity, and
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

**Status: the migration bullet below is implemented for `EMI-RETURN-PATH`; the rest of P7 stays open.**
Done: the section is kept when the report has a cell, a finding or an executed `EMI-RETURN-PATH` coverage entry (a board with none
of these keeps the bytes it had); `--plot-summary` on a board with no cell draws the return-path map in place of the cell
pages (and after them when there are cells); `copper issues` locates each return-path finding at its gaps, unreturned vias and
edge points and lists them as `details.causes`; the `EMI-RETURN-PATH` explain text is rewritten; the end-to-end fixture is
`crates/copper/tests/emi_no_converter_cli.rs` on `examples/emi_return_path`. Still open: a grouping by subject kind for signals,
pairs and connectors (only the return-path group exists), the signal and connector page layout of the summary, the clock,
pair and connector parts of the end-to-end fixture (they need P3 and P4), and the other items below.

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
3. P3 (ordered cascade of leg and modal responses, input and output named
   apart). P0–P2 do not depend on the P3 and P4 contracts and can proceed while
   those are settled.
4. P4a: the cable's circuit and the ground-noise network written down and
   their fixtures passing.
5. P4 in its voltage-and-ranking scope (the `cable_common_mode_field` check
   comes first).
6. The rest of P7 for the delivered phases.
7. P4b, P5 and P6 only after their definitions or spikes, each ending in a
   decision recorded in this file.

## Changes from the reviews (2026-11-16)

Fifth review (cable termination):

- **Connector voltage restricted to a matched termination.** The forward-only
  cascade cannot give the steady-state voltage on an unmatched cable (a final
  load multiplier was 14.2 dB low in a 50 / 900 / 900 ohm example). v1
  publishes an absolute connector voltage only for a matched cable; for an
  unmatched one only a labelled first-arrival value, never a steady-state
  figure. Matched, high-impedance and open-load fixtures against the
  independent circuit solver.

Fourth review (errors in the newly specified calculations):

- **One cascade normalization.** The cascade carries incident voltage waves in
  one reference impedance per mode; a series common-mode impedance transmits
  `2 Z_ref / (2 Z_ref + Z_cmc)`, so a zero-impedance element is exactly 1 (the
  open-circuit divider used before cost 6 dB per element). The Thévenin divider
  stays as a separate fixture and the two are required to agree; an identity
  fixture requires that zero-impedance elements change nothing.
- **Source loading applied once.** The ground-noise current is
  `I_A = V_pin / Z_A` for the loaded pin voltage that `signal_swing` declares,
  or follows from an explicitly declared Thévenin source; the two must give
  identical results in a test.
- **Coupling before summing.** Each leg of a pair has its own signed transfer
  to the observation loop: `V_B = j omega (M_Bp I_p + M_Bn I_n)`. The
  differential current couples through the difference of the mutuals and is no
  longer cancelled by summing currents first; fixtures for symmetric
  cancellation and asymmetric residual.

Third review (remaining contracts):

- **Choke position respected.** P3 keeps the channel as an ordered cascade of
  routed sections and series parts and propagates the two leg amplitudes
  through it; a choke attenuates only the common mode generated upstream of it.
  P4 applies no divider of its own. Fixtures with identical total skew and the
  choke before versus after the skewed section.
- **Input and output voltages named apart** in P3, P4 and P6: `V_diff,in`,
  `V_diff,out`, `V_cm,out`. The reported conversion is `V_cm,out / V_diff,in`
  (`(1/2)|sin|` for pure skew), told apart from `V_cm,out / V_diff,out`
  (`(1/2)|tan|`). `Scd21` keeps its normalization, incident differential wave
  in the denominator.
- **Ground-noise network defined in P4a**, not postponed to P5: driven loop,
  observation loop and sense path, source and load impedances, signed
  contributions, `V_B = Z_BA I_A` checked against an independent two-loop
  fixture. Where the inputs are missing or the result depends too strongly on
  the sense path, the source gets a geometry ranking and no voltage.

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
