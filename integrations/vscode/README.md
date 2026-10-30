# CopperScript for VS Code

The regular `.copper` text editor includes theme-aware syntax highlighting for
declarations, types, properties, comments, strings, numbers and engineering units.
Highlighting works without starting the language server or installing a compiler.
Semantic token highlighting remains a planned LSP feature.

The regular `.copper` text editor also includes completion, hover, go to definition,
document symbols, live syntax/compiler/ERC diagnostics and quick fixes (suggested
replacements, declaring `no_connect` for unconnected pins). Language features use
the standard [CopperScript LSP](https://github.com/andenore/CopperScript/blob/main/docs/language-server.md) and activate only
in trusted local workspaces.

The language server is the `copper` binary's `copper lsp` command; no Python
installation is needed. Install `copper` on your `PATH` (for example with
`cargo install --locked --path crates/copper` from a CopperScript checkout), then open
your project folder. The extension starts `copper lsp` in that folder.

`copperscript.lsp.compilerCommand` is an argv array, never a shell string, and
defaults to `["copper"]`. The extension appends `lsp`. Select another binary
explicitly, for example:

```json
{
  "copperscript.lsp.compilerCommand": ["/absolute/path/to/copper"]
}
```

Run **CopperScript: Show Setup** to see the selected command and the running
server in the Output panel. It does not install anything or change project
settings. Run **CopperScript: Restart Language Server** after replacing the
binary. Changes to `copperscript.lsp.*` settings restart the server
automatically, and a crashed server restarts with backoff.
`copperscript.lsp.enabled` disables language features. Logs appear in
**CopperScript Language Server**.

The client sends unsaved documents, handles native completions/navigation and
invalidates on `.copper`/manifest/lock changes. Dependency resolution remains
locked/offline. The server does not write source, fetch packages or route boards.
The VS Code client has no npm runtime dependencies.

## Mechanical editor

The mechanical / floorplan editor is no longer a VS Code custom editor. It runs
in your browser from the `copper` binary:

```sh
copper edit-mechanical BOARD.copper
```

## Development and packaging

Stage the extension with `node integrations/vscode/build.cjs`, then launch an
Extension Development Host with `code --extensionDevelopmentPath=<absolute
build/vscode-extension/extension> <project-directory>`.

Package a VSIX with Microsoft's locked `vsce` tooling:

```sh
npm ci --prefix integrations/vscode/tools --ignore-scripts --no-audit --no-fund
node integrations/vscode/build.cjs
cd build/vscode-extension/extension
../../../integrations/vscode/tools/node_modules/.bin/vsce package --no-dependencies --out ../../copperscript-0.4.0.vsix
code --install-extension ../../copperscript-0.4.0.vsix
```

The extension identity is `andenore.copperscript`; uninstall the earlier
development extension `andenore.copperscript-mechanical` before installing it
to avoid duplicate tools.

The separate extension CI builds `copper` and runs the client tests against it
on Linux, Windows and macOS, then packages with `vsce`. Marketplace publication
is prepared but needs publisher account configuration. The
[release plan](https://github.com/andenore/CopperScript/blob/main/docs/extension-release-plan.md)
describes one-time publisher/OIDC setup and the independent `extension-v<version>`
release tags. Users install a Marketplace release once and receive normal VS Code
extension updates; their `copper` binary stays separately installed.
