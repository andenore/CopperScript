# Agent bootstrap through the CLI

Run `copper agent-guide` (or `python -m copperscript agent-guide`) in the active
CopperScript environment. Guidance ships with the CLI and needs no manifest,
network or repository checkout. Retrieve one topic only when needed:
`syntax`, `libraries`, `verification` or `recovery`; use `--json` for the versioned
`copperscript.agent-guide/v1` response with package version and topic digest.
The digest identifies guidance, not a Git revision or qualification certificate.

For report behavior, use [LLM workflow](llm-workflow.md); for dependency discovery,
use [library context](library-context.md). The CLI topics supply workflow templates,
which must be bound to the actual project configuration and selected identities.

## Project pointer

Put this small hint in a consuming project's `AGENTS.md`, or in its host's startup
instructions if that host does not load the file:

> Start with `copper agent-guide` in the active CopperScript environment. Retrieve
> individual topics and selected library contracts as needed. Follow this project's
> README and build targets. If the pinned compiler lacks the guide, explicitly
> select a current compiler for guidance/review while retaining project build pins.

The host must supply/load the hint; the CLI cannot discover itself for an
unfamiliar agent. Keep shared workflow instructions in the CLI rather than
copying them into each project.
