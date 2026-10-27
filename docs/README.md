# Current documentation by task

Start with `copper agent-guide`; read one relevant reference below, then follow
only the sections required by the task. Do not load this entire directory.
The installed CLI guide is the common workflow bootstrap across repositories.

| Task | Current reference |
| --- | --- |
| Write or change `.copper` | [Language reference](language-reference.md) |
| Author board silkscreen or change default branding | [Silkscreen](silkscreen.md) |
| Run check, compile, placement or export | [CLI workflow](cli-workflow.md) |
| Configure a board build | [Make builds](make-builds.md), [examples](../examples/README.md) |
| Find parts across dependencies | [Library context](library-context.md) |
| Resolve or pin footprints | [Footprint dependencies](footprint-dependencies.md) |
| Read compact reports or requirement coverage | [LLM workflow](llm-workflow.md) |
| Iterate on routing or placement failures | [LLM workflow: iteration](llm-workflow.md#routing-and-placement-iteration); `copper agent-guide iteration` |
| Change routing or native repair | [Filled-copper feedback](filled-copper-feedback.md); follow linked specialized references only as needed |
| Plan or implement joint escape planning (side, layer, order per package) | [Escape planning plan](escape-planning-plan.md); implemented contract: [escape planning](escape-planning.md) |
| Investigate blocked package exits | [Required reference-layout comparison](component-integration-guidelines.md#package-escape-faults-reference-comparison-is-required) |
| Profile or compare routing | [Routing performance](routing-performance.md) |
| Plan the Rust port | [Rust port plan](rust-port-plan.md) |
| Add components or reusable circuits | [Integration guidelines](component-integration-guidelines.md) |
| Author or bind physical macros | [Physical hard macros](physical-hard-macros.md); [definition syntax](macro-definitions.md); [binding syntax](physical-macro-syntax-plan.md); [definition requirements](macro-definition-plan.md) |
| Edit mechanics or floorplan | [Mechanical editor](mechanical-editor.md) |
| Author mechanical geometry or profiles | [Mechanical language](mechanical-language.md), [profiles](mechanical-profiles.md) |
| Assess engineering or release evidence | [Engineering qualification](engineering-qualification.md) |
| Generate manufacturing or assembly files | [Manufacturing files](manufacturing-files.md), [assembly pinning](assembly-pinning.md) |
| Share KiCad outputs | [Project export](kicad-project-export.md) |
| Import placed KiCad boards for routing comparison | [KiCad board import](kicad-board-import.md) |
| Score router changes on many third-party boards | [Multi-board routing benchmark](routing-benchmark.md) |
| Use or change editor language features | [Language server and implementation plan](language-server.md) |
| Package or publish the VS Code extension | [Extension release plan](extension-release-plan.md) |
| Work on compiler internals or ERC | [Compiler architecture](compiler-architecture.md) |
| Simulate analog or power behavior | [Simulation](simulation.md), [model/numerical review](simulation-qualification.md) |
| Generate device models | [Device generation](device-generation.md); lab-specific rules live in `tools/device-model-lab/` |
| Plan unfinished work | [Current backlog](../TODO.md) |



Current contracts describe supported behavior. Board guides keep their latest
documented status; old routing/bug narratives are recoverable from Git rather
than duplicated in the checkout. Read accepted [design decisions](design-decisions.md)
only for a specific architectural change. Contributor tasks live in the
[routing backlog](routing-backlog.md), [optimization backlog](routing-optimization-todo.md),
[qualification backlog](qualification-todo.md) and [other contributor backlog](contributor-backlog.md).
The [device model](device-model.md) and [placement contracts](placement-internals.md)
are focused internal references. Simulation numerical/model review is in
[simulation qualification](simulation-qualification.md).
