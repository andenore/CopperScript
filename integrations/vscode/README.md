# CopperScript for VS Code

The regular `.copper` text editor includes theme-aware syntax highlighting for
declarations, types, properties, comments, strings, numbers and engineering units.
Highlighting works without starting the language server or installing a compiler.
Semantic token highlighting remains a planned LSP feature.

The regular `.copper` text editor also includes completion, hover, go to definition,
document symbols and live syntax/compiler/ERC diagnostics. Language features use
the standard [CopperScript LSP](https://github.com/andenore/CopperScript/blob/main/docs/language-server.md) and activate only
in trusted local workspaces. The existing mechanical editor remains optional.

Python **3.11 or newer** is supported, without an upper version limit. Install
your project's declared CopperScript dependencies first, then open its folder.
The extension discovers `.venv/Scripts/python.exe` on Windows or
`.venv/bin/python` on Linux/macOS. If no `.venv` exists, a project with both
`pyproject.toml` and `uv.lock` uses
`["uv", "run", "--locked", "--offline", "--no-sync", "python"]` to discover an
existing project environment. Discovery does not synchronize or install packages
and rejects a system-interpreter fallback. The resolved interpreter then runs
the language server directly.

Both `compilerCommand` settings default to `[]` for automatic discovery. An
explicit argv overrides discovery, for example:

```json
{
  "copperscript.lsp.compilerCommand": ["/absolute/path/to/.venv/bin/python"]
}
```

The selected interpreter must contain `pcbir.lsp.server`. Startup checks the
actual feature module and accepts newer Python versions. An incompatible project
environment reports a setup error; it does not switch to another compiler.
Projects pinned to an older compiler can explicitly select a newer interpreter
for editor guidance without changing their build pins.

Run **CopperScript: Show Setup** to inspect the interpreter, compiler version,
feature modules and mechanical fingerprint command in the Output panel. It does
not install packages or change project settings. Run **CopperScript: Restart
Language Server** after changing the environment. Interpreter settings changes
restart the LSP automatically. `copperscript.lsp.enabled` disables language
features. Logs appear in **CopperScript Language Server**.

The client sends unsaved documents, handles native completions/navigation and
invalidates on `.copper`/manifest/lock changes. Dependency resolution remains
locked/offline. The server does not write source, fetch packages or route boards.
Both the Python server and the VS Code client have no added runtime dependencies.

Optional custom text editor for `.copper` boards. Source stays authoritative;
reviewed edits use native WorkspaceEdit, document Undo/Redo, encoding and Save.
Unsaved text edits are compiled in memory. No backend source writes, downloads,
shell invocation or public server. A trusted **local workspace** is required.

Pin CopperScript in your project dependencies/lock first, run its normal fetch
workflow, and compute the exact compiler content identity:

```sh
uv run --locked --offline python -m pcbir.editor.document --fingerprint
```

Set `copperscript.mechanical.compilerDigest` to that 64-character result and
`copperscript.mechanical.footprintRoots` to installed KiCad footprint directories.
The mechanical editor uses the same automatic interpreter discovery, with its
own `copperscript.mechanical.compilerCommand` override. Its interpreter must
contain `pcbir.editor.document`. Show Setup prints the fingerprint argv for the
actual selected environment; use that command if you are not using uv.
Commands are argv arrays, never shell strings. Compiler changes fail closed until
you intentionally update the digest. Layer/profile/template/macro settings must
match your build. Imports and footprint resolution remain locked/offline.

Build shared assets with `node integrations/vscode/build.cjs`, then launch an
Extension Development Host with `code --extensionDevelopmentPath=<absolute
build/vscode-extension/extension> <project-directory>`. On a `.copper` tab use
**Reopen Editor With → CopperScript Mechanical / Floorplan**. It is not the default
text editor. Select a component and **Show component source declaration** to open
its root declaration (or hierarchical instance) beside the board view.

Use **CopperScript: Open Mechanical Editor** from the Command Palette or the
context menu on a `.copper` file. Without an active `.copper` text tab, the command
opens a board file picker. The same trusted workspace and compiler pin checks apply.

Use the Components, Mechanics and View sidebar tabs. The file bar shows source
status and **Save reviewed changes**; **More** contains source reload/history.
Temporary placement has its own toolbar and history. **Open board…** opens the
native picker directly from the mechanical view.

Review source edits and choose **Save reviewed changes**. Native document changes
invalidate old previews; invalid text remains editable in the regular text view
and can recover on the next valid document version. Native Undo/Redo restores
source, while temporary placement history is separate. A failed save leaves the
reviewed native edit dirty and undoable, rather than writing around VS Code.

This host deliberately uses VS Code's [custom text editor API](https://code.visualstudio.com/api/extension-guides/custom-editors)
and [workspace trust](https://code.visualstudio.com/api/extension-guides/workspace-trust).
Routed-reference import is currently available in the standalone host; advanced
mechanical formats remain governed by the main editor checklist.

Offline installable artifact (no npm/marketplace build dependencies):

```sh
node integrations/vscode/build.cjs
python integrations/vscode/package.py -o build/copperscript-0.3.1.vsix
code --install-extension build/copperscript-0.3.1.vsix
```

Packaging uses fixed archive timestamps and whitelisted runtime assets. Existing
VSIX files are never overwritten. The extension identity is now
`andenore.copperscript`; uninstall the earlier development extension
`andenore.copperscript-mechanical` before installing it to avoid duplicate tools.

The separate extension CI checks Python 3.11 through 3.15 and packages with
Microsoft's locked `vsce` tooling. Marketplace publication is prepared but needs
publisher account configuration. The
[release plan](https://github.com/andenore/CopperScript/blob/main/docs/extension-release-plan.md)
describes one-time publisher/OIDC setup and the independent `extension-v<version>`
release tags. Users install a Marketplace release once and receive normal VS Code
extension updates; their project compiler stays separately selected.
