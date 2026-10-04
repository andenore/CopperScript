# Work packets

Each packet is a small, bounded extraction unit. A low-cost Codex agent should:

1. Read the packet JSON and only the cited official source pages.
2. Extract exact field values and a locator (table, page, or section).
3. Append one JSON object per fact to `data/evidence.jsonl` using statuses `verified`, `inferred`, `unresolved`, or `illustrative`.
4. Use `?` for unknown values; never fill gaps from a similar package or family member.
5. Run `python -m copperscript_stm32g0 validate` before handing the packet back.

No API key or model call is required by the repository. The packet/result boundary makes model-assisted extraction replaceable and auditable.
