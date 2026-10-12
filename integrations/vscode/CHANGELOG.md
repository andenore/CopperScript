# Changelog

## 0.3.1

- Add theme-aware syntax highlighting for declarations, types, properties, comments, strings, numbers and engineering units.
- Highlight text without starting the language server or requiring an installed compiler.
- Include the grammar in both offline and Marketplace packages and test it with VS Code's TextMate tokenizer.

## 0.3.0

- Prepare the combined language and mechanical tools as `andenore.copperscript`.
- Discover project Python environments automatically, with explicit interpreter overrides.
- Accept Python 3.11 and newer without an upper bound; check installed compiler capabilities before starting.
- Add Show Setup and Restart Language Server commands, with actionable startup errors.
- Add independent extension tests, packaging and Marketplace release automation.

Development VSIX users should uninstall `andenore.copperscript-mechanical` before installing the new identity.
