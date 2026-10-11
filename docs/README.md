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
| Generate device models | [Device generation](device-generation.md); lab-specific rules live in `tools/device-model-lab/` |
| Plan unfinished work | [Current backlog](../TODO.md) |

Specifications define current contracts; dated observations establish evidence
only for their recorded inputs. Completed plans and review passes are in
`history/`; consult its index only for a specific historical investigation.
Do not treat old test counts, commands or publication notes as current status.
