# Final-native CAM and RF/power qualification

The automated qualification commands `audit-cam`, `assess-engineering` and
`qualification-gate`, and the read-only RF layout review, were removed with the
Python implementation (last Python commit `b16122a`). The `copper` binary has
no automated CAM, RF, power or interface qualification. This page keeps the
qualification principles. Consumers supply the evidence through external tools,
simulation, bench measurements and review.

## What does not qualify a board

ERC, legal geometry, exact DRC and native KiCad DRC do not establish
electrical, RF, power, assembly, CAM or prototype qualification. A routed,
refilled and DRC-clean board is a checkpoint, not an approval of a BOM,
assembly preview, battery system or production order.

`export-manufacturing --skip-independent-cam` generates explicitly unqualified
files; see [manufacturing files](manufacturing-files.md).

## Evidence must match the final native board

- Assess the actual final native board and files, without rerouting,
  refilling, modifying copper, ordering or approving parts.
- Use the package's exported/refilled native PCB. A pre-export PCB with
  identical-looking geometry but different bytes is not the same evidence.
- Keep assessment reports outside the immutable fabrication package.
- A changed board, selection, stackup, operating plan, contract, algorithm or
  referenced evidence invalidates the corresponding report. Changed inputs
  invalidate old evidence.
- Reviewer, solver and bench attestations are accountable external evidence,
  not independently verified truths.

## CAM

- A CAM check must cover every package checksum, the native-board binding,
  the native DRC result, the layer inventory, drill commands and IPC-D-356
  contacts. Signal aliases never replace physical-contact identity. Missing
  or merged/split net partitions fail.
- Two independent Gerber parsers that agree on parse or render are not
  electrical copper proof. Native IPC reconciliation does not prove that the
  Gerber copper connects correctly.
- Copper connectivity and fabrication geometry from final artwork need a
  qualified vector extractor (polarity, drill composition, curve-error bounds,
  barrel contacts, pad surfaces). Without one, these checks stay incomplete.

## RF/power contracts and project plans

- Reusable CopperLib packages declare exact sources and mandatory
  design/simulation/bench requirements in companion JSON. RF matching is
  `required`, `integrated` or `unresolved`.
- Keep manufacturer facts in CopperLib and board-specific bindings in the
  consumer. Missing bounds stay null, never implicit zero; an assumed curve or
  leakage bound is labelled as an assumption with its own unresolved evidence
  requirement.
- Numerical screens (reference-plane coverage, route width and length, pair
  geometry, path geometry, divider and programming windows, microstrip
  estimates) are auxiliary. They cannot replace mandatory qualification.
- Impedance/transient simulation and RF/power prototype measurements remain
  mandatory. Interface contracts (USB and similar) need reviewed operating-mode
  and layout evidence, signal-integrity simulation and prototype measurements
  for the whole channel, including inline components and power states.
- Do not apply solid-plane rules beneath matching circuitry whose reference
  deliberately requires an inner-layer keepout.
- `routing(...){ clearance = ...; }` is exported to same-stem KiCad custom rules
  (`.kicad_dru`) so native zone refill keeps source net clearance. Keep the
  `.kicad_dru` alongside the PCB and project. A source clearance is a minimum,
  not proof that coplanar ground is present or at a uniform RF gap.

See the [implementation checklist](qualification-todo.md) and CopperLib's
`docs/engineering-contracts.md` for unresolved integration and qualification
work.

Sources for algorithm/input boundaries: [official Gerber](https://www.ucamco.com/en/gerber),
[Nordic reference circuitry](https://docs.nordicsemi.com/r/bundle/ps_nrf52832/page/ref_circuitry.html),
[TI TPS63020](https://www.ti.com/lit/ds/symlink/tps63020.pdf),
[KiCad IPC exporter](https://github.com/KiCad/kicad-source-mirror/blob/master/pcbnew/exporters/export_d356.cpp).
