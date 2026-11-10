# Current documentation by task

Start with `copper agent-guide`; read one relevant reference below, then follow
only the sections required by the task. Do not load this entire directory.
The installed CLI guide is the common workflow bootstrap across repositories.

| Task | Current reference |
| --- | --- |
| Write or change `.copper` | [Language reference](language-reference.md) |
| Author board silkscreen or change default branding | [Silkscreen](silkscreen.md) |
| Arch the corners of RF, analog or tuned pair nets | [Arched corners](language-reference.md#arched-corners) |
| Run check, compile, placement or export | [CLI workflow](cli-workflow.md) |
| Configure a board build | [Board configuration](board-configuration.md), [Make builds](make-builds.md), [examples](../examples/README.md) |
| Find parts across dependencies | [Library context](library-context.md) |
| Resolve or pin footprints | [Footprint dependencies](footprint-dependencies.md) |
| Read compact reports or requirement coverage | [LLM workflow](llm-workflow.md) |
| Iterate on routing or placement failures | [LLM workflow: iteration](llm-workflow.md#routing-and-placement-iteration); `copper agent-guide iteration` |
| Plan closure of the measured Rust CopperProbe routing failures | [Rust placement and routing closure plan](rust-routing-closure-plan.md), subordinate to the routing refactor and escape plans |
| Change routing or native repair | [Filled-copper feedback](filled-copper-feedback.md); follow linked specialized references only as needed |
| Plan or implement joint escape planning (side, layer, order per package) | [Escape planning plan](escape-planning-plan.md); implemented contract: [escape planning](escape-planning.md) |
| Investigate blocked package exits | [Required reference-layout comparison](component-integration-guidelines.md#package-escape-faults-reference-comparison-is-required) |
| Profile or compare routing | [Routing performance](routing-performance.md) |
| Build, install or test `copper` | [Rust workspace](../crates/README.md) |
| Add components or reusable circuits | [Integration guidelines](component-integration-guidelines.md) |
| Author or bind physical macros | [Physical hard macros](physical-hard-macros.md); [definition syntax](macro-definitions.md); [binding syntax](physical-macro-syntax-plan.md); [definition requirements](macro-definition-plan.md) |
| Synthesize a trace antenna in a reserved region | [Trace antenna synthesis](language-reference.md#trace-antenna-synthesis); design and solver tiers: [research note](pcb-antenna-synthesis-research.md) |
| Edit mechanics or floorplan | [Mechanical editor](mechanical-editor.md) |
| Run the physical editor in the browser (WebAssembly) | [Editor WebAssembly plan](editor-wasm-plan.md) |
| Author mechanical geometry or profiles | [Mechanical language](mechanical-language.md), [profiles](mechanical-profiles.md) |
| Assess engineering or release evidence | [Engineering qualification](engineering-qualification.md) |
| Generate manufacturing or assembly files | [Manufacturing files](manufacturing-files.md), [native plotting](native-plotting.md), [native zone filling](native-filling.md), [native DRC](native-drc.md), [assembly pinning](assembly-pinning.md) |
| Share KiCad outputs | [Project export](kicad-project-export.md) |
| Panelize routed boards for fabrication | [Panelization](panelization.md); design and remaining steps: [plan](panelization-plan.md) |
| Give parts 3D models or fallback bodies | [3D models](3d-models.md) |
| Import placed KiCad boards for routing comparison | [KiCad board import](kicad-board-import.md) |
| Use or change editor language features | [Language server and implementation plan](language-server.md) |
| Package or publish the VS Code extension | [Extension release plan](extension-release-plan.md) |
| Work on compiler internals or ERC | [Compiler architecture](compiler-architecture.md) |
| Plan unfinished work | [Current backlog](../TODO.md) |



Current contracts describe supported behavior. Board guides keep their latest
documented status; old routing/bug narratives are recoverable from Git rather
than duplicated in the checkout. Read accepted [design decisions](design-decisions.md)
only for a specific architectural change. Contributor tasks live in the
[routing backlog](routing-backlog.md), [optimization backlog](routing-optimization-todo.md),
[qualification backlog](qualification-todo.md) and [other contributor backlog](contributor-backlog.md).
The [device model](device-model.md) and [placement contracts](placement-internals.md)
are focused internal references.
