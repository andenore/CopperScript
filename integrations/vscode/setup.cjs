"use strict";
const {selectCommand} = require("./compiler.cjs");

function activateSetup(vscode, context, languageTools) {
  async function showSetup(selected) {
    if (!vscode.workspace.isTrusted) {
      await vscode.window.showInformationMessage("Trust this workspace before running the CopperScript language server.");
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
    output.appendLine("The language server is the copper binary's `copper lsp` command. By default `copper` is found on PATH; an explicit copperscript.lsp.compilerCommand argv takes priority.");
    try {
      const compiler = selectCommand(vscode.workspace.getConfiguration("copperscript.lsp", folder.uri).get("compilerCommand", ["copper"]));
      output.appendLine(`Selected command: ${JSON.stringify([...compiler.command, "lsp"])} (${compiler.source})`);
      const running = languageTools.clients?.get(folder.uri.toString());
      output.appendLine(running?.started ? `Running: ${running.server}` : "Language server is not running for this folder; see the log above for startup errors.");
    } catch (error) { output.appendLine(error.message); }
    output.appendLine("The mechanical editor runs in the browser: copper edit-mechanical BOARD.copper");
    output.appendLine("After fixing the command, run CopperScript: Restart Language Server. Show Setup does not install anything or change project pins.");
    const choice = await vscode.window.showInformationMessage("CopperScript setup details are in the Output panel.", "Open Settings");
    if (choice === "Open Settings") await vscode.commands.executeCommand("workbench.action.openWorkspaceSettings", "copperscript");
  }
  context.subscriptions.push(
    vscode.commands.registerCommand("copperscript.showSetup", showSetup),
    vscode.commands.registerCommand("copperscript.restartLanguageServer", () => languageTools.restart())
  );
  return {showSetup};
}
module.exports = {activateSetup};
