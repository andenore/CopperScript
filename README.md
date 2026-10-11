# CopperScript

CopperScript is a strongly typed language for PCB connectivity, electrical
intent and physical constraints. It compiles `.copper` source, checks electrical
rules, resolves versioned library parts and footprints, and provides placement,
routing and KiCad export workflows.

LLM agents: start with `copper agent-guide`, then request only the topics and
library contracts needed for the task. Contributors also follow [AGENTS.md](AGENTS.md).
The [documentation index](docs/README.md) routes tasks to current references;
[TODO.md](TODO.md) tracks unfinished work.

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

The [mechanical editor](docs/mechanical-editor.md) displays real footprint and
board geometry and supports checked placement and reviewed source edits:

```console
uv run copper edit-mechanical examples/mechanical_editor_demo/board.copper
```

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
