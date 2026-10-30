# CopperScript extension distribution plan

## Decisions

Keep the language server client in CopperScript's `integrations/vscode`
extension. Use the permanent Marketplace identity `andenore.copperscript`,
displayed as **CopperScript**. Extension versions and release tags are
independent of compiler and board qualification releases.

The extension runs the language server as `copper lsp` from the `copper`
binary (version 0.4.0 onward). It needs no Python. The mechanical custom
editor was removed in 0.4.0; the browser editor is
`copper edit-mechanical BOARD.copper`.

## Implementation and acceptance

1. **Compiler setup — implemented.** `copperscript.lsp.compilerCommand` is the
   `copper` binary's argv and defaults to `["copper"]` on PATH. An explicit
   argv takes priority. Never fall back from a failing selected binary to a
   different compiler. Report the selected command in Output; provide Show
   Setup and Restart Language Server commands. Do not install anything,
   upgrade build pins or alter source/locks.
2. **Marketplace package — implemented.** Rename the extension before its first
   publication, add repository/support metadata, a changelog and PNG branding.
   Package with Microsoft's pinned `vsce` and a separate npm tooling lock
   (`integrations/vscode/tools`). No npm runtime dependencies are added.
3. **Compatibility/release CI — implemented.** `vscode-extension.yml` builds
   `copper` and runs the compiler and LSP client tests against it on Linux,
   Windows and macOS, tests the grammar, then packages with `vsce`. Keep board
   toolchain/evidence pins unchanged.
4. **Release automation — implemented, account configuration pending.** Build
   and retain a VSIX after the matrix succeeds. An `extension-v<version>` tag must
   match the extension manifest. Publish that exact artifact using GitHub OIDC
   and Microsoft Entra authentication. Only the upstream repo's enabled release
   job receives publishing identity access.
5. **First publication — pending.** Register/verify the publisher, configure the
   identity, execute CI and review the resulting artifact before enabling release
   publication. Install from Marketplace once; subsequent extension versions use
   VS Code's normal update behavior. Existing development installs of
   `andenore.copperscript-mechanical` must be uninstalled when moving identities.

## Publisher configuration

Follow the current [VS Code publishing guide](https://code.visualstudio.com/api/working-with-extensions/publishing-extension)
and [Azure Login OIDC setup](https://github.com/Azure/login#login-with-openid-connect-oidc-recommended).

1. Register the `andenore` Marketplace publisher, or confirm its ownership. If a
   different publisher is needed, update the manifest and documented extension
   identity together before the first release.
2. Create an Entra application/service principal or user-assigned managed identity
   and authorize it as a Contributor of that Marketplace publisher, following
   the publishing guide's identity registration steps. The Azure identity must
   be recognized by Marketplace; Azure subscription permissions alone do not
   grant extension publishing access.
3. Configure a federated credential with issuer
   `https://token.actions.githubusercontent.com`, audience
   `api://AzureADTokenExchange`, and subject
   `repo:andenore/CopperScript:environment:vscode-marketplace`.
4. Create the GitHub environment `vscode-marketplace`, limit deployment branches
   and tags to extension release tags, and add environment variables
   `AZURE_CLIENT_ID` and `AZURE_TENANT_ID`. Require review there if desired.
   `allow-no-subscriptions` is used because publishing is a tenant service.
5. Run the **CopperScript VS Code extension** workflow to review its package.
   Set repository variable `COPPER_MARKETPLACE_ENABLED=true` when ready to enable
   tag publication. No access-token secrets are stored by this workflow.
6. Commit the extension version and changelog, then create and push the matching
   `extension-v0.4.0` tag for this version. Ordinary compiler `v*` tags and manual
   package checks do not publish the extension. An existing Marketplace version
   is not silently replaced; use a new extension version for the next release.

## Local checks

Use Node 22 or newer:

```console
cargo build --release -p copper
COPPER_TEST_BIN=target/release/copper node --test tests/compiler-client.cjs tests/lsp-client.cjs
npm ci --prefix integrations/vscode/tools --ignore-scripts --no-audit --no-fund
node --test tests/syntax-highlighting.cjs
node integrations/vscode/build.cjs
cd build/vscode-extension/extension
# Unix shell; on Windows use the corresponding vsce.cmd launcher.
../../../integrations/vscode/tools/node_modules/.bin/vsce package --no-dependencies --out ../../copperscript-0.4.0.vsix
```

Bundled runtimes, downloads and automatic compiler upgrades are deferred until
compiler compatibility and release provenance have a defined policy. A project
can explicitly select a newer editor compiler without changing its build pins;
its diagnostics are guidance from that selected version.

## Local verification, 2026-10-12

Version 0.3.2 (Python language server) passed its focused suite locally on
Windows and the Linux/macOS/Windows CI matrix and Marketplace packaging for
commit `a8f5680`. Authenticated publication remains unverified. Version 0.4.0
replaces the Python server with `copper lsp`.

Version 0.4.0 runs `copper lsp`, removes the mechanical custom editor and
packages only with `vsce`. Version 0.3.2 added quick fixes, automatic language-server restarts and Windows
URI fixes. Version 0.3.1 added compiler-independent syntax highlighting. The release workflow
tests the grammar with VS Code's actual TextMate/Oniguruma tokenizer before
packaging; the VSIX must include the registered grammar.
