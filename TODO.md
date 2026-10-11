# Current backlog

Read only when planning unfinished work. Checked implementation history is not
a current task list. This backlog retains previously unclosed roadmap obligations;
some are source-specific and need a fresh evidence check before implementation.
Routing closure does not by itself close manufacturing or electrical gates.

## LLM efficiency

See [the future improvement plan](docs/llm-workflow-improvements.md): measured
token/intervention evaluation; reviewed reusable bundles, caching and transitive
version policy; remaining final-copper verifiers and broader acceptance.
Fabrication/prototype measurements remain deferred until a later request.

## Solver and qualification work

Choose only the relevant focused record for detailed acceptance criteria:

- [Routing research](docs/routing-research-tasks.md): unresolved investigations,
  including endpoint-only surface-chain readback. Earlier recommendations may
  be implemented; check their status and current code before starting work.
- [Routing quality](docs/routing-quality-todo.md): joint decoupler placement,
  ordinary-area crossing quality and remaining geometry checks.
- [Routing optimization](docs/routing-optimization-todo.md): profiled bottlenecks
  and guarded comparisons; preserve output quality and evidence.
- [Qualification gaps](docs/qualification-todo.md): geometry adapters, derating/
  thermal extraction, native release integration and mandatory external evidence.
- [Global/package access](docs/global-routing-and-escape-todo.md) and
  [routing review](docs/routing-review-todo.md): source-specific unresolved
  acceptance/research. Dated counts in these records are not current board status.

## Retained roadmap obligations

These entries are carried forward as unverified open scope, not claims that
current boards still have the old checkpoint failures. Reconcile each against
the intended source, implemented API and fresh evidence before closing it.

- [ ] P8. Independent CAM qualification: second-tool parsing/rendering and
      comparison, layer/drill/netlist reconciliation, polarity and extents,
      golden corpus, version-pinned qualification matrix, and release evidence.

- [ ] Qualify an installed libgerbv build and expand the official golden
        corpus/matrix, including polarity and metamorphic render comparisons.

- [ ] P9. Frontend and full-board closure: express every new rule in `.copper`,
      lower it into typed IR, resolve real CopperLib footprints, then place,
      route, DRC, export and independently verify the full acceptance board.

- [ ] Qualify the user's 5 V / 2 A input target with a measured worst-case
        power budget. Decide between a 5 V / 2 A USB-PD contract and a 3 A
        Type-C advertisement while limiting actual draw to 2 A; add input
        overvoltage, inrush, ESD, and reverse-current protection.

- [ ] Resolve the reported real CopperLib/KiCad footprint gaps and complete
        acceptance-board placement, routing, DRC, manufacturing, and CAM closure.

- [ ] Evaluate and qualify a six-layer `Signal / GND / Signal / Signal / GND /
      Signal` stack against an actual JLCPCB laminate/copper specification.
      Reserve In1.Cu and In4.Cu as common-GND reference planes, with only
      manufacturer-required RF exclusions. Moving In4.Cu from signal routing
      to GND reduces routing capacity and requires a complete fresh reroute.

- [ ] Express explicit layer roles and distinguish dedicated reference planes
      from auxiliary signal-layer pours; replace plane-role inference from
      zone coverage. Add frontend support for local zone boundaries, stitching
      policy, island removal, and per-pad connection overrides, lowered into
      physical IR rather than electrical connectivity or schematic geometry.

- [ ] Add selective stitched GND fill on F.Cu/B.Cu, retaining the provisional
      0.5 mm board-edge inset. Remove all unconnected fill islands and reject
      ineffective narrow slivers. Use assembly-appropriate thermal reliefs for
      ordinary solder pads and solid connections for ground vias and explicitly
      designated RF/power ground lands. Keep default via/pad overlap forbidden;
      qualified via-in-pad remains an explicit opt-in, never a fill side effect.

- [ ] Keep In2.Cu primarily for signals/USB, without blanket fill initially.
      Use In3.Cu for general signals and localized power distribution; evaluate
      any auxiliary inner-layer GND fill only after routing and reference-path
      review, not as a substitute for a continuous plane.

- [ ] Define local V5 input, V3V8 modem, and V3V3 distribution regions. Size
      copper necks and vias against the declared 2 A input budget, actual rail
      currents, modem transients, voltage drop and thermal requirements. Protect
      the selected regulator's switching-node, feedback and capacitor-return
      geometry; do not flood low-current SIM/1.8 V nets indiscriminately.

- [ ] Add source-backed Johanson antenna clearances and Nordic RF matching
      keepouts before enabling generic fill. Preserve the reference C3
      (`C_BT_MATCH`) ground path through nRF52832 VSS pin 31 rather than allowing
      automatic fill to bypass it; honor required inner-layer copper exclusions.
      Review cellular and GNSS feed ground geometry separately.

- [ ] Qualify USB 90-ohm differential and applicable RF 50-ohm feed geometry
      using the selected stackup, including nearby-fill clearance. Existing USB
      width/gap values remain provisional; the Nordic chip-side matching net
      must not be treated as a generic 50-ohm transmission line.

- [ ] Reserve off-pad ground access and return vias during routing, then refill
      and iterate on actual connected copper. Accept only fresh native/KiCad
      DRC, zero unintended via/pad contacts, connected GND regions/planes,
      reviewed critical return paths, and assembly/copper-balance checks.
      Record profiled full rerouting and individual-layer plots; zone creation
      alone is neither continuity evidence nor manufacturing signoff.

- [ ] Support arbitrary via/free boundary ports in global, package-access,
      critical and detailed routing. Account for macro occupancy before coarse
      guides; do not treat owner copper as disposable fanout.

- [ ] Add hierarchical module-to-macro frontend selection and movable-macro
      placement feedback with complete boundary revalidation.

- [ ] Qualify complete Nordic support circuit, actual reference ground/copper,
      Johanson land/tee/corner geometry, via fence, stackup, impedance, tuning and
      enclosure; then integrate into the full-vertical design and reroute.

- [ ] Add explicit qualified local zones/arcs/mirrored macro variants and
      independent export/fill/CAM verification; never infer RF qualification.

- [ ] Every new geometry type participates in serialization, fingerprints,
      KiCad export, DRC coverage, and stale-signoff invalidation.

- [ ] Tests include unit geometry, adversarial regression, determinism,
      cross-stage compatibility, and negative release-gate cases.

- [ ] The full acceptance design completes without proxy footprints, unrouted
      nets, unwaived required DRC findings, or incomplete required coverage.

- [ ] A version-pinned independent CAM flow accepts the final manufacturing
      package and its evidence is recorded in the release manifest.
