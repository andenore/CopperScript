# Qualification implementation backlog

Current commands/contracts are in [engineering qualification](engineering-qualification.md)
and [manufacturing files](manufacturing-files.md). Reusable source requirements
belong to libraries; operating choices and measurements belong to consumers.

- [ ] Qualify Gerber polarity/drill/curve vector extraction and fabrication-profile
  integration for copper connectivity, clearance, annulus, mask/paste and alignment.
  Existing adapter-neutral algorithms do not qualify final artwork by themselves.
- [ ] Add source/model-specific capacitance derating, inductor/current limits and
  switching-loop/feedback/thermal geometry extraction; origin distances are not proof.
- [ ] Extend the strict physical-IR release API with final-native six-layer evidence
  and profiles. Existing qualification-gate results do not authorize an order.
- [ ] Add qualified coupled/embedded/stripline and discontinuity solver integration.
  Existing pair inventories/path geometry remain auxiliary screens.
- [ ] Provision/pin a second independent CAM parser and qualify its corpus/matrix:
  polarity, arcs, aperture macros, regions, slots and metamorphic comparisons.
- [x] Support partially-assembled boards: a per-selection `populate` mode
  (`supplier` / `consigned` / `none`) carried through `assembly.lock`, the check
  report and the BOM exporter, so a board whose specialist parts the assembler
  does not stock can still export everything it can place. Consigned and
  not-fitted components leave the BOM *and* the CPL together, because the CPL is
  built from the BOM's designators and their reference sets must stay equal.
  Exclusions are reported, never silent. The mode records intent only: it implies
  no sourcing, stock or assembly approval.

External release requirements remain mandatory: exact source/variant applicability,
stackup/plating/loss capabilities, actual operating/load/temperature envelopes,
biased MLCC/inductor models, reviewed solver results, prototype RF/power/charging
measurements, exact BOM/CPL review and supplier assembly approval. A generated
report or native DRC pass cannot manufacture missing evidence. Record these
obligations against the selected board revision, not a dated compiler test run.
