"use strict";
const {test} = require("node:test"), assert = require("node:assert/strict");
const {selectCommand, startError} = require("../integrations/vscode/compiler.cjs");
const {activateSetup} = require("../integrations/vscode/setup.cjs");
const {checkTag} = require("../integrations/vscode/release.cjs");
const manifest = require("../integrations/vscode/package.json");

test("release tags must match the packaged extension version", () => {
  assert.equal(checkTag("refs/tags/extension-v0.3.0", {version: "0.3.0"}), "refs/tags/extension-v0.3.0");
  for (const ref of [undefined, "refs/heads/main", "refs/tags/v0.3.0", "refs/tags/extension-v0.2.0"])
    assert.throws(() => checkTag(ref, {version: "0.3.0"}), /Extension release must use/);
  assert.equal(checkTag(`refs/tags/extension-v${manifest.version}`, manifest), `refs/tags/extension-v${manifest.version}`);
});

test("copper on PATH is the default and argv is never parsed as a shell", () => {
  assert.deepEqual(manifest.contributes.configuration.properties["copperscript.lsp.compilerCommand"].default, ["copper"]);
  for (const configured of [[], ["copper"]]) assert.deepEqual(selectCommand(configured), {command: ["copper"], source: "copper on PATH"});
  const command = ["C:/Program Files/copper/copper.exe"];
  assert.deepEqual(selectCommand(command), {command, source: "configured command"});
  assert.notEqual(selectCommand(command).command, command, "selection copies the configured argv");
  assert.throws(() => selectCommand("copper lsp"), /argv/);
  assert.throws(() => selectCommand([""]), /argv/);
});

test("a missing binary reports how to configure it", () => {
  const missing = Object.assign(new Error("spawn copper ENOENT"), {code: "ENOENT"});
  assert.match(startError(["copper"], missing).message, /"copper": not found[\s\S]*compilerCommand/);
  const other = new Error("EACCES");
  assert.equal(startError(["copper"], other), other);
});

test("the extension no longer contributes the removed mechanical editor", () => {
  const text = JSON.stringify(manifest);
  assert.doesNotMatch(text, /mechanical|onCustomEditor|customEditors/i);
  assert.deepEqual(manifest.contributes.commands.map(c => c.command), ["copperscript.showSetup", "copperscript.restartLanguageServer"]);
});

test("setup respects workspace trust and restart uses the real client callback", async () => {
  const commands = {}, messages = [];
  let restarts = 0;
  const vscode = {
    commands: {registerCommand(name, fn) {commands[name] = fn; return {dispose() {}};}},
    workspace: {isTrusted: false},
    window: {async showInformationMessage(message) {messages.push(message);}}
  };
  activateSetup(vscode, {subscriptions: []}, {restart() {restarts++;}});
  await commands["copperscript.showSetup"]();
  assert.match(messages[0], /Trust this workspace/);
  commands["copperscript.restartLanguageServer"]();
  assert.equal(restarts, 1);
});

test("setup reports the selected copper lsp command and the running server", async () => {
  const commands = {}, lines = [];
  const folder = {name: "board", uri: {scheme: "file", fsPath: "/project", toString: () => "file:///project"}};
  const vscode = {
    commands: {registerCommand(name, fn) {commands[name] = fn; return {dispose() {}};}, async executeCommand() {}},
    workspace: {isTrusted: true, workspaceFolders: [folder], getWorkspaceFolder: () => folder,
      getConfiguration: () => ({get: (_name, fallback) => fallback})},
    window: {async showInformationMessage() {}}
  };
  const clients = new Map([["file:///project", {started: true, server: "CopperScript 0.1.0"}]]);
  activateSetup(vscode, {subscriptions: []}, {output: {show() {}, appendLine(line) {lines.push(line);}}, clients});
  await commands["copperscript.showSetup"]();
  assert.ok(lines.includes('Selected command: ["copper","lsp"] (copper on PATH)'));
  assert.ok(lines.includes("Running: CopperScript 0.1.0"));
  assert.ok(lines.some(line => line.includes("copper edit-mechanical BOARD.copper")));
});
