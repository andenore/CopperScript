# Non-routing contributor backlog

Read only for the relevant implementation task. Each item needs a current
code/coverage check before work; public behavior belongs in its canonical guide.

## Editor and mechanical contracts

- [ ] Add a physical-IR body envelope when a backend needs exact body geometry;
  the editor's derived visual envelope is not a physical-schema guarantee.
- [ ] Add keyboard next/previous component selection and warning navigation,
  actual selected constraint-pad measurements and an optional package-wide
  pad-label display preference.
- [ ] Add typed profile use-site parameters/transforms with explicit datum,
  ownership and conflict semantics; no text substitution or implicit overrides.
- [ ] Complete exact enclosure/host 3D interference and mating/height contracts
  beyond the supported 2D and declared-height checks. Mechanical fit alone does
  not certify a board standard.
- [ ] Qualify curved/cutout/slot manufacturing topology, material webs and tooling
  through independent CAM; supported source/export geometry is not qualification.

## Electrical model and presentation

- [ ] Keep power-pin operating-range violations as errors when absolute ratings
  also exist; signal-pin warning policy must not accidentally approve an
  out-of-range supply. Verify present ERC severity before extending it.
- [ ] Represent disjoint operating voltage bands, optionally mode-selected;
  a single min/max envelope must not approve the space between valid bands.
- [ ] Add native multi-unit schematic presentation and language-server support
  for the implemented semantic endpoint model.

## Simulation

- [ ] Extend analysis/engine/model support only for a concrete project need;
  DC/noise, Xyce/XSPICE/Verilog-A and parasitic overlays need explicit adapter,
  model provenance and numerical acceptance coverage.
- [ ] Expand vendor-model qualification and reviewed convergence/operating-state
  evidence; an available simulator executable does not establish model validity.

## Architecture

- [ ] Evaluate separating the electrical Board's constraint tuple into a typed
  compiled constraint result without changing net connectivity or source syntax.
- [ ] Decide whether renamed hierarchical instances need persistent source IDs,
  how external edits may be back-annotated, and how multiple footprint candidates
  are selected. Existing source ownership, locks and digests remain authoritative.
