# CopperScript

**PCB design as code. Hardware intent, written in code.**

CopperScript is a strongly typed language and compiler for PCB connectivity,
electrical intent, mechanical geometry and physical constraints. Describe
components, peripheral selection, nets, supply domains, board shape and placement
intent in readable `.copper` source. Check electrical rules, resolve versioned
library parts, edit mechanics and floorplans visually, and inspect experimental
routing and native KiCad exports.

[Project website](https://andenore.github.io/CopperScript/) ·
[Language reference](docs/language-reference.md) ·
[Examples](examples/README.md) ·
[CopperAssetTracker](https://github.com/andenore/CopperAssetTracker)

LLM agents: start with `copper agent-guide`, then request only the topics and
library contracts needed for the task. Contributors also follow [AGENTS.md](AGENTS.md).
The [documentation index](docs/README.md) routes tasks to current references;
[TODO.md](TODO.md) tracks unfinished work.

## More than connections

A capacitor protects a particular pin. A switching converter has a critical
current loop. A radio needs reference geometry and a return path. CopperScript
makes supported intent explicit alongside the connections, with reusable
engineering definitions and inspectable intermediate representations.

```copper
component C_MCU: CAPACITOR {
    value = 100nF;
    footprint = "Capacitor_SMD:C_0402_1005Metric";
    role = decoupling;
    decouples = "U_MCU.VDD";
}
```

This excerpt from the tracker composition associates a bypass capacitor with
its supply pin. The association survives compilation and informs generic
decoupling placement and surface-feed decisions. It is not a complete board.

## Mechanical design and constraints

Define board shape and fit in the same version-controlled source as the circuit:
rectangular, circular, polygonal and supported curved outlines; cutouts; mounting
holes and slots; placement and copper keepouts. Named datums and edge/pad
attachments locate connectors precisely. Declared component heights, enclosure
limits and assembly-access regions express supported fit requirements; pinned
DXF drawings provide read-only reference guides.

Placement constraints capture fixed position, rotation and side, allowed angles,
placement regions, distance limits, alignment and groups. Routing constraints
describe widths, clearances, permitted layers, differential pairs and length
matching. Supported constraints reach physical IR and their placement, routing
or verification consumers; reports retain unresolved requirements and external
evidence obligations.

This excerpt from [the mechanical editor demo](examples/mechanical_editor_demo/board.copper)
defines a board outline, a mounting hole and a component pose. The complete
example supplies the component declarations and nets:

```copper
mechanical {
    outline rectangle { width = 40mm; height = 30mm; }
    hole H1 {
        position = (3mm, 3mm); diameter = 2.2mm;
        head_clearance_radius = 2mm;
    }
}
constraint fixed_placement(R_ANCHOR) {
    x = 7mm; y = 8mm;
    rotation = 0; side = front;
}
```

The browser-based mechanical editor shows actual pads, courtyards, board material
and ratsnest guides. Preview automatic placement, drag or enter exact poses,
measure geometry and deliberately lock placement intent. Review the source diff
and explicitly save geometry or constraint changes back to `.copper`, with
undo/redo. The VS Code custom editor uses the same shared UI and source workflow.
Temporary placement previews stay temporary until you save explicit intent;
imported profile definitions retain their library ownership.

```console
uv run copper edit-mechanical examples/mechanical_editor_demo/board.copper
```

Resolve the example's real KiCad footprints before opening the editor. See
[mechanical source](docs/mechanical-language.md),
[the editor workflow](docs/mechanical-editor.md),
[reusable board profiles](docs/mechanical-profiles.md),
[constraint syntax and coverage](docs/language-reference.md#constraints) and
[VS Code setup](integrations/vscode/README.md). Supported mechanical checks do
not establish a full 3D enclosure fit or manufacturing qualification.

## See a complete design

![Native KiCad top copper and silkscreen of CopperAssetTracker](site/assets/routing-top.svg)

[CopperAssetTracker](https://github.com/andenore/CopperAssetTracker) combines
an STM32 host, cellular connectivity, GNSS, Bluetooth and motion sensing on a
six-layer board. The recorded 2026-10-11 modem-power review has 91 components and
75 physical nets, with zero native DRC violations and zero unconnected items.
These are results for that recorded candidate, not a success guarantee for other
boards or a production approval. Routing remains experimental; RF, power,
battery, assembly, CAM and prototype qualification remain open.

Inspect the [snapshot provenance](site/assets/snapshot.json),
[current board status](https://github.com/andenore/CopperAssetTracker/blob/main/docs/validation.md)
and [revision reproduction guide](https://github.com/andenore/CopperAssetTracker/blob/main/revisions/modem-power-v1/README.md).
The picture is native KiCad output, not a photograph of assembled hardware.

## Three repositories, one workflow

| Repository | Owns |
| --- | --- |
| CopperScript | Language/compiler, electrical and physical checks, mechanical editor, constraint-driven placement/routing and KiCad export |
| [CopperLib](https://github.com/andenore/CopperLib) | Versioned parts, footprints, reusable circuits, mechanical profiles and engineering contracts |
| [CopperAssetTracker](https://github.com/andenore/CopperAssetTracker) | Board composition, application configuration and board-specific evidence |

CopperLib is one library provider; independent libraries use the same interfaces.

## Quick start

Python 3.11 or newer is required. Install with uv from this repository:

```console
uv sync
uv run copper agent-guide
uv run copper check examples/valid_board/board.copper
uv run copper compile examples/valid_board/board.copper -o build/board.json
```

For a development installation with pip:

```console
python -m pip install -e ".[test]"
python -m pytest
```

To use every core, install `pytest-xdist` and run `python -m pytest -n auto`.
Each worker is a separate process; tests keep their files under `tmp_path`.

Populate pinned dependencies once, then use their verified cache offline:

```console
uv run copper check examples/full_vertical/board.copper --locked
uv run copper check examples/full_vertical/board.copper --locked --offline
```

Dependency versions and development replacements belong in `copper.mod`;
`copper.lock` binds supported package assets. Investigate mismatched inputs
instead of silently relocking. CopperLib is one provider; other library
repositories use the same interfaces. See [managed footprints](docs/footprint-dependencies.md)
and [selective library retrieval](docs/library-context.md).

## Design and review

Use the [language reference](docs/language-reference.md) for syntax and
[examples index](examples/README.md) for complete board inputs. The
[CLI workflow](docs/cli-workflow.md) covers compilation, placement and export.

```console
uv run copper library search "query" --project path/to/project
uv run copper library inspect "package/path::Symbol" --project path/to/project
uv run copper summarize build/route-report.json
```

Search declared dependencies first, then inspect selected interfaces,
requirements and evidence by section. Reports provide bounded summaries and
artifact-bound requirement coverage; see [LLM workflow](docs/llm-workflow.md).

For a complete registered board, use its shared Make targets:

```console
make EXAMPLE=full-vertical route
make EXAMPLE=cm4 route
```

The standard `route-board` pipeline owns bounded placement/package feedback,
ordinary routing and native-filled copper repair. Supply the board's actual
layers, fabrication profile, footprints and macro assets. See
[routing profiles and invariants](docs/filled-copper-feedback.md) and
[build configuration](docs/make-builds.md). KiCad 10 is needed for the qualified
native refill adapter. Outputs and reports go into fresh build directories;
failed runs remain review drafts.

## Qualification and outputs

ERC, successful routing and zero native DRC errors are distinct from electrical,
RF/power, assembly, independent CAM and prototype qualification. Missing evidence
remains incomplete. Use [engineering qualification](docs/engineering-qualification.md),
[manufacturing files](docs/manufacturing-files.md) and
[assembly selections](docs/assembly-pinning.md) for their respective gates.

PCB export writes a same-stem `.kicad_pro`, local footprint table and footprint
library. Keep the generated directory together when sharing it; see
[KiCad project export](docs/kicad-project-export.md). Placement-only exports have
no routing copper and must not be sent for fabrication.

The [GitHub board-build workflow](docs/github-board-builds.md) publishes
inspection artifacts with reports and checksums; their presence is not approval.
Contributor internals and ERC coverage are in [compiler architecture](docs/compiler-architecture.md).

## Editor language support

The [CopperScript language server](docs/language-server.md) provides live source
checks, completion, hover, definition navigation and document outlines for
`.copper` files. Start it through an LSP client with `copper lsp` or
`python -m pcbir.lsp.server`. The [VS Code extension](integrations/vscode/README.md)
includes the client alongside the mechanical editor and builds offline without
npm dependencies. It discovers the project's Python environment and accepts
Python 3.11 or newer, without an upper bound. The separate
[extension release plan](docs/extension-release-plan.md) covers Marketplace
packaging and automated releases. Analysis uses unsaved source and the project's locked/offline
imports; older compiler pins need an explicitly selected LSP-capable environment.

## Project website

The [static project website](https://andenore.github.io/CopperScript/) introduces
the language and the tracker showcase. Its source, deployment configuration and
[publishing plan](site/README.md) live in this repository. Website validation
does not compile boards or change compiler, library or release pins.
