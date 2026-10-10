# Final-native CAM and RF/power qualification

The commands below assess the actual final native board and files without
rerouting, refilling, modifying copper, ordering or approving parts. They produce
deterministic reports with `pass`, `fail` or `incomplete`. Exit codes: 0 all
reported gates pass; 1 failed/incomplete; 2 invalid input/tool invocation. Reports
are created exclusively and must be outside the immutable fabrication package.
Every report keeps `qualified_release=false`: passing this engineering checkpoint
does not approve a BOM, assembly preview, battery system or production order.

Install `copperscript[qualification]` for pinned PyGerber 2.4.3 and Shapely 2.1.2.
The second parser and explicit KiCad Python interpreter are host/CI inputs, never
executables selected by a source, contract or evidence file.

```console
copper audit-cam build/manufacturing --kicad-python /path/to/kicad/python --report build/cam-audit.json
copper assess-engineering build/manufacturing/board.kicad_pcb --plan docs/engineering-plan.json --kicad-python /path/to/kicad/python --report build/engineering-audit.json
copper qualification-gate build/manufacturing --plan docs/engineering-plan.json --cam-report build/cam-audit.json --engineering-report build/engineering-audit.json --report build/qualification-gate.json
```

Use the package's exported/refilled native PCB for engineering checks: a pre-export
PCB with identical-looking geometry but different bytes is not the same evidence.
Existing `export-manufacturing --skip-independent-cam` still generates explicitly
unqualified files. These commands add assessment/checkpoint support; they do not
silently upgrade the older Physical-IR strict release API or its four-layer profile.

## CAM checks and qualification boundary

`audit-cam` checks every package checksum/coverage, native-board manifest binding,
native DRC result, exact X2 layer inventory, strict metric through-drill commands
and IPC-D-356 contacts. With the native read-only probe it compares drill hit
multisets, contact/via multisets and the complete expected/exported net partition.
Coordinate quantization tolerances are 1000 nm for decimal drill centres and
1270 nm for IPC-D-356 centres/diameters. Six-character reference/four-character pin
fields and signal aliases never replace physical-contact identity. Missing or
merged/split net partitions fail. Slotted contacts and non-through native via
reconciliation remain incomplete; the strict XNC parser itself understands G85.
IPC coordinates are reconciled against the native auxiliary origin; through-drill
coordinates use the exporter's explicitly selected absolute origin.

Supply `--profile profile.json --gerbv /path/to/gerbv --gerbv-version VERSION` to
run the two-parser corpus/agreement checks. Profile schema:

```json
{
  "schema": "copperscript-cam-profile/v0.1",
  "id": "reviewed-toolchain",
  "gerber_spec_revision": "exact applicable revision",
  "xnc_spec_revision": "exact applicable revision",
  "tools": [{"name": "PyGerber", "version": "2.4.3", "executable_sha256": "replace with actual identity"},
            {"name": "libgerbv", "version": "exact installed version", "executable_sha256": "replace with actual identity"}],
  "corpus": [{"id": "positive", "path": "corpus/positive.gbr", "sha256": "actual file hash", "expect_parse": true},
             {"id": "negative", "path": "corpus/negative.gbr", "sha256": "actual file hash", "expect_parse": false}]
}
```

The illustrative profile is NOT usable until actual pins/corpus are reviewed.
Use `available_adapters()` identities; never auto-accept installed identities as
reviewed release pins. The authored regression corpus now covers line/inch,
polarity, aperture macros, arcs and regions with two negative cases; it is not
a complete official-format or production-artifact qualification corpus.
The current libgerbv adapter independently parses/re-exports, but uses PyGerber's
shared raster normalizer. Parse/render agreement is NOT electrical copper proof.
Native IPC reconciliation also does not prove the Gerber copper connects correctly.

`cam_geometry.verify_vector_copper` supplies adapter-neutral algorithms for final
positive/drill-subtracted polygon copper, physical contact surfaces, plated-link
annuli, net partition and planar clearance. Unassigned islands are incomplete.
`minimum_polygon_web` and `rectangular_paste_area_ratio` add bounded mask/paste
screens. A qualified geometry extractor must establish complete polarity/drill
composition, curve-error bounds, full barrel contact inventory and expected pad
surfaces. This adapter is NOT implemented yet; the audit retains mandatory
`cam.copper-connectivity` and `cam.fabrication-geometry` incomplete gates.

## RF/power contracts and project plans

Reusable package-local CopperLib `qualification.json` contracts declare exact
sources and mandatory design/simulation/bench requirements. They are companion
JSON, not added Copper grammar. RF matching must be `required`, `integrated` or
`unresolved`. Mandatory qualification cannot be replaced by a simple numerical
screen. The consumer plan declares scoped contracts, input files and bindings:

```json
{
  "schema": "copperscript-engineering-plan/v0.1",
  "input_files": {"source": "../board.copper", "library_lock": "../copper.lock", "assembly_selection": "../assembly.lock"},
  "context": {"stackup": null, "operating_envelope": null},
  "contracts": [{"scope": "radio", "path": "../../CopperLib/packages/parts/simcom/sim7670g-lngv/qualification.json", "bindings": {}}],
  "evidence": {}
}
```

Reviewed stackup context requires a selected profile, URL/revision/locator/hash
source object, alternating physical copper/dielectric layers with thicknesses,
dielectric Dk/loss tangent and finished via plating. Native layer order and total
thickness must match. Reviewed operating context requires sourced, named scenarios
with minimum/maximum input voltage, peak/RMS current and ambient temperature.
Consumers must add applicable battery/USB, load pulse, charging and protection
conditions; the generic scenario screen is not a completeness certification.

Algorithm bindings require an explicit `basis`, actual inputs and numerical limits.
Supported algorithms: `reference_coverage`, `route_width`, `route_length`,
`placement_distance`, `range`, `calculation` and `external`. Numerical limits have
metric keys with `minimum`/`maximum`. Null inputs are incomplete, not assumed.

- RF coverage uses the union of actual adjacent-layer native filled polygons,
  preserving holes and checking the full projected track envelope, not midpoints.
  Exact reversed-edge native hole bridges are decoded by unchanged linework and
  even/odd faces; arbitrary invalid/crossing polygons are never healed. This is
  geometric coverage only, not plane connectivity/impedance or Gerber fidelity.
  Do NOT apply solid-plane rules beneath matching circuitry whose reference
  deliberately requires an inner-layer keepout.
- Width checks include every selected net segment. Length checks sum all segments
  and are not pin-to-pin path solvers. Placement distance is explicitly between
  component origins, never a substitute for pin-relative/loop geometry review.
- Power calculations use SI units: temperature-adjusted trace resistance, exact
  finished-hole barrel resistance, explicitly selected series-path drop/RMS loss,
  effective-capacitance/ESR pulse droop, converter demand with minimum efficiency,
  and sourced reduced-order thermal estimates. No thermal/current-capacity,
  control-loop, EMI or battery-safety certification is implied. Do not feed an
  entire branched net into a series-path model or nominal MLCC values into droop.

## External evidence adapters

### Reusable supplier stackup selection and width screening

The consumer's `context.stackup` may contain only `profile_path` and `reviewed`.
The relative profile path selects library-owned companion JSON with schema
`copperscript-fabrication-stackup/v0.1`; supplier facts cannot be overridden in
the consumer. Its bytes are bound as `inputs.files.stackup_profile` and rechecked
at the checkpoint. CopperLib provides the nominal JLC06161H-3313 construction.

Profiles retain exact physical construction, nominal finished thickness, sourced
finished-thickness tolerance and provenance. Published construction need not sum
exactly to the nominal order thickness; check the stated tolerance without
stretching dielectric spacings. Native nominal thickness and copper order must
still match. Missing loss tangent or minimum barrel plating remains incomplete;
published average plating is never substituted for a minimum.

Optional plan `microstrip_screens` entries contain `id`, `net`, `reference_layer`
and `reference_net`. The report's separate `screening` section inventories actual
native layer/width combinations and estimates isolated outer microstrip from
the selected construction. Interior routes are explicitly unsupported by that
model. Filled-reference coverage is retained separately; nearby coplanar copper,
mask, matching structures, differential gap and discontinuities are not modelled.
Screening remains labelled incomplete and is not a mandatory qualification check:
the existing external impedance gates require real solver evidence. It neither
approves a route nor forces a hypothetical width rewrite into owned macros.

Optional `differential_pair_screens` contain `id`, `positive_net`, `negative_net`
and explicit positive `maximum_search_gap_nm`. `pair_geometry` extracts actual
same-layer, exactly parallel straight-section edge gaps and interval-union
coverage per member, with separate copper lengths, widths, vias and adjacent
filled-copper coverage/separation. Nearby power copper is identified by net, not
silently called ground. Source nominal gap is not used as actual geometry.
Candidates can be ambiguous; bends, pad fields, vias and total path delay are
not modelled. These auxiliary screens cannot satisfy mandatory solver evidence.
The extractor bytes are included in algorithm hashes checked at release.

Optional `route_path_screens` contain `id`, `net`, `return_net` and two explicit
`terminals` (`component`, physical `pad`, `layer`). The read-only native probe
now retains each contact's actual copper layers and each via's type. The generic
`route_path_geometry` graph splits straight tracks at exact endpoint/terminal/via
coincidences, reports a unique centreline-tree path, and keeps branch/off-path
copper separate. Cycles, ambiguous lands, unknown via types and unsupported
crossings never become a guessed shortest-path length. Copper-edge/pad-area
contacts, zones and component internals are outside this model.

Via transitions, unused-barrel lengths between published construction layer
centres and nearest geometrically spanning return vias are retained separately.
They do not establish actual finished-board stub length, propagation delay,
plane connectivity or return transfer to a power reference. Missing construction
leaves numerical barrel spans null. All path results remain auxiliary screening.

Reusable `kind: interface` contracts identify a `protocol` and require reviewed
external `operating-mode`/`layout-review` design evidence, `impedance`/
`signal-integrity` simulation evidence and `prototype-interface` bench evidence.
Consumers bind the whole channel, including inline components and power states,
not just independent PCB net sections. The existing report kind `rf-power` is
retained for backward compatibility but can include interface contracts. The
checkpoint derives mandatory coverage from these contracts exactly as for RF
and power, so absent USB measurements cannot be hidden in auxiliary geometry.

For CAM, `--evidence-index index.json` accepts schema
`copperscript-cam-evidence-index/v0.1` with `results` mapping
`cam.copper-connectivity` and/or `cam.fabrication-geometry` to local evidence
documents. Evidence and retained report paths are relative to the index. They
use the same reviewed engineering-evidence schema below; report bytes must match,
and the checkpoint rechecks them. This is an external-result adapter, not the
missing independent Gerber vector extractor.

The project plan's `evidence` maps a scoped requirement ID to a local JSON document.
No commands are executed from it. Schema `copperscript-engineering-evidence/v0.1`
requires `id`, `stage`, `inputs_sha256`, `status`, `reviewed_by`, `report_path`,
`report_sha256`, `settings_sha256`, `model_sha256` and a `tool` object containing
`name`, `version`, `identity_sha256`. `inputs_sha256` is copied from the assessment
report only AFTER all plan paths and engineering inputs are fixed. Report paths
are relative to the plan. External evidence bytes are recorded separately to
avoid a self-referential input digest; the checkpoint rechecks those bytes too.
A changed board, selection, stackup, operating plan, contract, algorithm or
referenced evidence invalidates the corresponding report. Reviewers/solver/bench
attestations are accountable external evidence, not independently verified truths.

Impedance/transient simulation and RF/power prototype measurements remain mandatory.
The `divider_voltage_window` explicit-input calculation reports independent
feedback-reference/resistor corners and an explicit positive/negative regulation
budget against an operating window. Its remaining voltage margins are not
measured droop: ripple, dynamic response, bias/leakage, wiring loss, temperature
and startup need their own bounds/evidence. No nominal-voltage screen can replace
mandatory supply/interface simulation and prototype qualification.
See [implementation checklist](qualification-todo.md) and CopperLib's
`docs/engineering-contracts.md` for unresolved integration/qualification work.

Sources for algorithm/input boundaries: [Shapely manual](https://shapely.readthedocs.io/en/stable/manual.html),
[official Gerber](https://www.ucamco.com/en/gerber),
[Nordic reference circuitry](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/ref_circuitry.html),
[TI TPS63020](https://www.ti.com/lit/ds/symlink/tps63020.pdf),
[KiCad IPC exporter](https://github.com/KiCad/kicad-source-mirror/blob/master/pcbnew/exporters/export_d356.cpp).
