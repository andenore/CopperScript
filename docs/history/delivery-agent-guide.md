# Agent bootstrap through the CLI

> Historical record, archived 2026-10-11. Claims and commands below apply to
> their recorded revisions, not current usage or approval. Read only when
> investigating that history; use the current reference guides for new work.

Run `copper agent-guide` or `python -m copperscript agent-guide` in the selected
CopperScript environment. It returns a compact starting workflow without a
project manifest, network access or file writes. The guide ships as Python data
with the compiler, so an installed CLI can teach its workflow without a compiler
checkout or the full documentation.

Retrieve a topic only when needed:

```console
copper agent-guide syntax
copper agent-guide libraries
copper agent-guide verification
copper agent-guide recovery
copper agent-guide --json
```

The default `start` topic introduces project configuration, provider-independent
library retrieval, composition and the verification loop. `syntax` includes an
offline-compilable electrical example. `libraries` explains selective sections,
member filters, source identity and omissions. `verification` distinguishes
routing, requirement coverage and hardware qualification. `recovery` covers
older compiler pins, cache/lock errors, stale selections and engineering blockers.
Command templates require the actual project configuration and selected identities.

JSON uses `copperscript.agent-guide/v1` and includes `guide_version`, the installed
package version, selected topic, available topics and a content digest. The
digest binds that topic's text/templates; the package version alone is not a Git
revision or qualification certificate. Both text and JSON default to one topic.

## Small project pointer

Put this in a consuming project's `AGENTS.md`, or provide it through the agent's
startup instructions when its host does not load that file:

> Start with `copper agent-guide` (or `python -m copperscript agent-guide`) in the
> active CopperScript environment. Retrieve individual topics and selected library
> contracts as needed. Follow this project's README and build targets for its
> configuration. If its pinned compiler lacks the guide, explicitly select a
> current compiler for guidance/review while retaining project build pins.

CopperAssetTracker and CopperLedRing now carry this pointer, with the latter's
template guidance asking new projects to retain it. CopperLib and CopperScript
also point to the guide from their existing contributor instructions. These are
documentation changes only in the consumer/provider projects; no components,
dependency revisions or build artifacts were changed.

The bundled guide is the common workflow entry point. Project pointers carry
local configuration ownership without copying the whole guide into each repo.
Agent hosts must supply or load the pointer; the CLI cannot make an unfamiliar
LLM discover itself automatically.

Validation: **66 tests passed**, with one Windows symlink-fixture skip, across
the guide, library-context and existing CLI suites. The guide runs from an empty
directory without creating files; every command template is accepted by the
current parser, and the bundled syntax example compiles and passes ERC offline.
The default text guide is about 1,500 characters; detailed topics are retrieved
separately. These character counts are not measured model-token savings.
