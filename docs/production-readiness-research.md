# Production-readiness research record

This document records the implementation recommendations for roadmap items
P1–P9. The detailed acceptance criteria live in `TODO.md`. Primary references
are linked here so that later algorithm changes remain reviewable.

## P1 — copper zones and planes

Keep semantic zone intent and generated fill separate. CopperScript owns zone
identity, net/layer scope, clearance, priority, thermal and island policy, plus
a digest of all fill inputs. For production, invoke a pinned KiCad filler with
`--refill-zones --save-board` before DRC and export. A future integer polygon
filler is preview-only until differential geometry tests qualify it.

References: [KiCad file formats](https://dev-docs.kicad.org/en/file-formats/),
[Clipper2](https://www.angusj.com/clipper2/Docs/Overview.htm), and
[CGAL polygon sets](https://doc.cgal.org/latest/Boolean_set_operations_2/).

## P2 — arbitrary-angle and push-and-shove routing

Use one exact integer geometry kernel, deterministic visibility/Theta* search,
and transactional copy-on-write shove operations. A shove either commits a
fully legal connected result or rolls back. Locked critical copper is immutable;
line and via shoving precede coupled-bundle shoving.

References: [KiCad PNS architecture](https://dev-docs.kicad.org/en/routing/),
[Theta*](https://idm-lab.org/bib/abstracts/papers/jair10b.pdf), and
[CGAL exact computation](https://www.cgal.org/exact.html).

## P3 — differential-pair completion

Treat a pair as one atomic route, including paired pad escape, coupled runs,
explicit uncoupled intervals, paired via transitions, return vias, propagation
delay and skew. Tune locally with bounded trombones only after topology is
stable. Protocol profiles are versioned; field-solver claims remain external.

References: [KiCad differential pairs](https://docs.kicad.org/9.0/en/pcbnew/pcbnew.html#routing_differential_pairs)
and [Saturn PCB differential-pair guidance](https://saturnpcb.com/saturn-pcb-toolkit/).

## P4 — stackups and via technology

Represent ordered copper and dielectric layers with stable IDs and material
properties. Fabrication profiles define named through, blind, buried and
microvia technologies, including legal spans, padstacks, aspect and annular
limits. Router transitions and DRC consume the same catalog; limits are sourced
from the selected fabricator rather than generic folklore.

References: [KiCad stackup documentation](https://docs.kicad.org/9.0/en/pcbnew/pcbnew.html#board_stackup)
and [IPC-2226 overview](https://www.ipc.org/TOC/IPC-2226A.pdf).

## P5 — exact-shape physical DRC

Share a versioned integer geometry service across routing, zones and DRC.
Canonical primitives include segments, arcs, disks, capsules, rounded
rectangles, ovals and polygon regions. Use an R-tree-style broad phase followed
by exact narrow-phase predicates; full and incremental runs must produce the
same stable findings.

References: [CGAL exact predicates](https://www.cgal.org/exact.html) and
[Boost.Geometry spatial indexes](https://www.boost.org/doc/libs/release/libs/geometry/doc/html/geometry/spatial_indexes.html).

## P6 — artwork, assembly and fabrication DRC

Derive exact mask, paste, silk, body, courtyard and hole geometry. Apply a
versioned fabrication/assembly profile with sourced capabilities. Keep
fabrication, stencil, assembly and final release gates separate; unknown
required capabilities block rather than defaulting optimistically.

References: [KiCad Library Conventions](https://klc.kicad.org/) and
[IPC-7351 overview](https://www.ipc.org/TOC/IPC-7351B.pdf).

## P7 — engineering analyses

Every result states status, evidence grade, claim scope and model validity.
Begin with stackup-driven analytical impedance/delay, return-path screening,
DC trace/via resistance, entered thermal/current limits, and creepage rules.
Use ngspice or field/thermal solvers as explicit external evidence; analytical
screening never masquerades as certification.

References: [ngspice](https://ngspice.sourceforge.io/docs.html),
[openEMS](https://docs.openems.de/), and [IEC 60664-1 overview](https://webstore.iec.ch/en/publication/59671).

## P8 — independent CAM qualification

Gate releases three ways: strict specification/metadata validation; two
independent parsers/renderers (recommended PyGerber and subprocess-isolated
libgerbv); and semantic comparison to signed physical IR. Reconcile layer
functions/polarity/extents, exact drill multisets and IPC-D-356 net partitions.
Pin executable hashes and qualify each toolchain tuple against positive,
negative, metamorphic and resource-limit corpora.

The current implementation hashes a small committed positive/negative corpus
and runs it for every required adapter immediately before a CAM-required
release. PyGerber is exercised locally; libgerbv is not installed or qualified
on the Windows host. Its subprocess adapter re-exports RS-274X, then uses the
same PyGerber rendering normalizer as the original file so byte-identical PNG
encodings from different renderers are not mistaken for geometry agreement.
The independent parser path and real production export remain unqualified
until a pinned libgerbv build passes a broader official and adversarial corpus.

References: [Ucamco format specifications and test files](https://www.ucamco.com/en/gerber/downloads),
[PyGerber](https://github.com/Argmaster/pygerber),
[libgerbv](https://gerbv.github.io/), and
[IPC-D-356B](https://www.ipc.org/TOC/IPC-D-356B.pdf).

## P9 — frontend and full-board closure

Keep the DSL closed, typed, declarative and deterministic. Normalize hard
requirements, targets, soft preferences, assumptions, external verification
and exact waivers into a constraint IR. Every hard/external constraint needs a
named consumer and verifier. Dependencies and profiles resolve only through a
content-addressed committed lockfile; generated geometry never becomes source.
Close one realistic acceptance board through source, dependency, logical,
library, process, placement, routing, DRC, export and independent CAM gates.

References: [CUE constraint semantics](https://cuelang.org/docs/reference/spec/),
[Starlark language rules](https://bazel.build/rules/language),
[SLSA provenance](https://slsa.dev/spec/v1.2/build-provenance), and
[SPDX package information](https://spdx.github.io/spdx-spec/v2.3.1/package-information/).
