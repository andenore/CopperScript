# Changelog

## 0.4.0

- Run the language server as `copper lsp` from the Rust `copper` binary; Python is no longer required.
- `copperscript.lsp.compilerCommand` is now the `copper` binary's argv and defaults to `["copper"]` on PATH. The `.venv`/`uv` discovery and Python version checks are removed; Python interpreter settings must be replaced.
- Remove the VS Code mechanical custom editor (`copperscript.mechanical`), the Open Mechanical Editor command and the `copperscript.mechanical.*` settings. Use the browser editor instead: `copper edit-mechanical BOARD.copper`.
- Package only with `vsce`; the Python offline packager is removed.

## 0.3.2

- Quick fixes: apply suggested replacements for misspelled pins, components, keywords and constraint targets, and declare `no_connect` for unconnected pins.
- Fix diagnostics on Windows, where VS Code encodes the drive colon in file URIs, and for files opened through symlinks, junctions or `subst` drives.
- Restart the language server automatically (with backoff) after a crash or timeout, and show its state in the status bar.
- Stop the mechanical editor backend when its tab is closed while it is still preparing.
- Highlight `no_connect`; report information/hint severities and created/deleted files correctly; treat `-` as part of identifiers.

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
