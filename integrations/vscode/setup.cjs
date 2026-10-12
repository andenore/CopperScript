"use strict";
const {resolveCompiler} = require("./compiler.cjs");

function activateSetup(vscode, context, languageTools) {
  async function showSetup(selected) {
    if (!vscode.workspace.isTrusted) {
      await vscode.window.showInformationMessage("Trust this workspace before running the CopperScript project interpreter.");
      return;
    }
    const folders = (vscode.workspace.workspaceFolders || []).filter(f => f.uri.scheme === "file");
    let folder = selected && folders.find(f => f.uri.toString() === selected.uri?.toString());
    if (!folder && vscode.window.activeTextEditor) folder = vscode.workspace.getWorkspaceFolder(vscode.window.activeTextEditor.document.uri);
    if (!folder && folders.length === 1) folder = folders[0];
    if (!folder && folders.length > 1) {
      const pick = await vscode.window.showQuickPick(folders.map(f => ({label: f.name, description: f.uri.fsPath, folder: f})), {placeHolder: "Choose the CopperScript project"});
      folder = pick?.folder;
    }
    if (!folder) { await vscode.window.showInformationMessage("Open a local project folder to set up CopperScript."); return; }
    const output = languageTools.output;
    output.show(true);
    output.appendLine(`\nCopperScript setup: ${folder.uri.fsPath}`);
    output.appendLine("Python 3.11 or newer is accepted, with no upper version limit. Install the project's declared CopperScript dependencies first.");
    output.appendLine("Automatic discovery uses .venv, then an existing uv.lock with uv run --locked --offline --no-sync python. Explicit compilerCommand settings take priority.");
    for (const feature of ["lsp", "mechanical"]) {
      try {
        const config = vscode.workspace.getConfiguration(`copperscript.${feature}`, folder.uri);
        const compiler = await resolveCompiler(config.get("compilerCommand", []), folder.uri.fsPath, feature);
        output.appendLine(`${feature}: Python ${compiler.python.join(".")}; CopperScript ${compiler.compilerVersion}; ${compiler.executable}`);
        output.appendLine(`Selected command: ${JSON.stringify(compiler.command)} (${compiler.source})`);
        output.appendLine(`Feature module: ${compiler.module}`);
        if (feature === "mechanical") {
          output.appendLine(`Fingerprint command (run argv in the project): ${JSON.stringify([...compiler.command, "-m", "pcbir.editor.document", "--fingerprint"])}`);
          output.appendLine("Review that fingerprint and set copperscript.mechanical.compilerDigest before editing.");
        }
      } catch (error) { output.appendLine(error.message); }
    }
    output.appendLine("After fixing the environment, run CopperScript: Restart Language Server. Show Setup does not install packages or change project pins.");
    const choice = await vscode.window.showInformationMessage("CopperScript environment details are in the Output panel.", "Open Settings");
    if (choice === "Open Settings") await vscode.commands.executeCommand("workbench.action.openWorkspaceSettings", "copperscript");
  }
  context.subscriptions.push(
    vscode.commands.registerCommand("copperscript.showSetup", showSetup),
    vscode.commands.registerCommand("copperscript.restartLanguageServer", () => languageTools.restart())
  );
  return {showSetup};
}
module.exports = {activateSetup};
