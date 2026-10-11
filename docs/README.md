# Current documentation by task

Start with `copper agent-guide`; read one relevant reference below, then follow
only the sections required by the task. Do not load this entire directory.
The installed CLI guide is the common workflow bootstrap across repositories.

| Task | Current reference |
| --- | --- |
| Write or change `.copper` | [Language reference](language-reference.md) |
| Run check, compile, placement or export | [CLI workflow](cli-workflow.md) |
| Configure a board build | [Make builds](make-builds.md), [examples](../examples/README.md) |
| Find parts across dependencies | [Library context](library-context.md) |
| Resolve or pin footprints | [Footprint dependencies](footprint-dependencies.md) |
| Read compact reports or requirement coverage | [LLM workflow](llm-workflow.md) |
| Change routing or native repair | [Filled-copper feedback](filled-copper-feedback.md); follow linked specialized references only as needed |
| Profile or compare routing | [Routing performance](routing-performance.md) |
| Add components or reusable circuits | [Integration guidelines](component-integration-guidelines.md) |
| Bind physical macros | [Physical hard macros](physical-hard-macros.md) |
| Edit mechanics or floorplan | [Mechanical editor](mechanical-editor.md) |
| Author mechanical geometry or profiles | [Mechanical language](mechanical-language.md), [profiles](mechanical-profiles.md) |
| Assess engineering or release evidence | [Engineering qualification](engineering-qualification.md) |
| Generate manufacturing or assembly files | [Manufacturing files](manufacturing-files.md), [assembly pinning](assembly-pinning.md) |
| Share KiCad outputs | [Project export](kicad-project-export.md) |
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
