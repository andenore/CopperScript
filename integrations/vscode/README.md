# CopperScript mechanical editor for VS Code

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
The default `compilerCommand` is `["uv", "run", "--locked", "--offline", "python"]`.
You may explicitly configure a pinned virtual environment's interpreter instead.
Commands are argv arrays, never shell strings. Compiler changes fail closed until
you intentionally update the digest. Layer/profile/template/macro settings must
match your build. Imports and footprint resolution remain locked/offline.

Build shared assets with `node integrations/vscode/build.cjs`, then launch an
Extension Development Host with `code --extensionDevelopmentPath=<absolute
build/vscode-extension/extension> <project-directory>`. On a `.copper` tab use
**Reopen Editor With → CopperScript Mechanical / Floorplan**. It is not the default
text editor. Select a component and **Show component source declaration** to open
its root declaration (or hierarchical instance) beside the board view.

Review source edits and choose Save reviewed source. Native document changes
invalidate old previews; invalid text remains editable in the regular text view
and can recover on the next valid document version. Native Undo/Redo restores
source, while temporary placement history is separate. A failed save leaves the
reviewed native edit dirty and undoable, rather than writing around VS Code.

This host deliberately uses VS Code's [custom text editor API](https://code.visualstudio.com/api/extension-guides/custom-editors)
and [workspace trust](https://code.visualstudio.com/api/extension-guides/workspace-trust).
Routed-reference import is currently available in the standalone host; advanced
mechanical formats remain governed by the main editor checklist.
