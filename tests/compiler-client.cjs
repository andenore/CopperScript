"use strict";
const {test} = require("node:test"), assert = require("node:assert/strict");
const fs = require("node:fs"), os = require("node:os"), path = require("node:path");
const cp = require("node:child_process");
const {selectCommand, resolveCompiler, runProbe} = require("../integrations/vscode/compiler.cjs");
const {activateSetup} = require("../integrations/vscode/setup.cjs");
const {checkTag} = require("../integrations/vscode/release.cjs");
const python = process.env.COPPER_TEST_PYTHON;
const root = path.resolve(__dirname, "..");

test("release tags must match the packaged extension version", () => {
  assert.equal(checkTag("refs/tags/extension-v0.3.0", {version: "0.3.0"}), "refs/tags/extension-v0.3.0");
  for (const ref of [undefined, "refs/heads/main", "refs/tags/v0.3.0", "refs/tags/extension-v0.2.0"])
    assert.throws(() => checkTag(ref, {version: "0.3.0"}), /Extension release must use/);
});

test("explicit interpreter wins and argv is never parsed as a shell", () => {
  const command = ["C:/Python with spaces/python.exe", "-I"];
  assert.deepEqual(selectCommand(command, root, "win32", () => true).command, command);
  assert.throws(() => selectCommand("python", root), /argv/);
  assert.throws(() => selectCommand([""], root), /argv/);
});

test("automatic discovery finds native venvs and existing locked uv projects", () => {
  for (const platform of ["win32", "linux", "darwin"]) {
    const expected = path.join(root, ".venv", ...(platform === "win32" ? ["Scripts", "python.exe"] : ["bin", "python"]));
    assert.deepEqual(selectCommand([], root, platform, p => p === expected).command, [expected]);
  }
  const locked = new Set([path.join(root, "pyproject.toml"), path.join(root, "uv.lock")]);
  assert.deepEqual(selectCommand([], root, "linux", p => locked.has(p)).command, ["uv", "run", "--locked", "--offline", "--no-sync", "python"]);
  assert.throws(() => selectCommand([], root, "linux", () => false), /No project Python environment/);
});

test("incompatible selected compiler cannot silently fall back", async () => {
  const calls = [];
  await assert.rejects(resolveCompiler(["selected-python"], root, "lsp", async (...args) => {
    calls.push(args); throw new Error("missing LSP module");
  }), /missing LSP/);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0][0], ["selected-python"]);
});

test("capability probe loads real language and mechanical modules", {skip: !python}, async () => {
  for (const feature of ["lsp", "mechanical"]) {
    const info = await runProbe([python], root, feature);
    assert.ok(info.python[0] > 3 || info.python[1] >= 11);
    assert.match(info.module, /pcbir/);
    assert.ok(info.executable);
  }
});

test("missing feature reports setup instructions without modifying the project", {skip: !python}, async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "copper-missing-lsp-"));
  try {
    fs.mkdirSync(path.join(dir, "pcbir"));
    fs.writeFileSync(path.join(dir, "pcbir/__init__.py"), "");
    await assert.rejects(runProbe([python, "-S"], dir, "lsp"), /pcbir.lsp.server[\s\S]*Show Setup/);
    assert.deepEqual(fs.readdirSync(dir), ["pcbir"]);
  } finally { fs.rmSync(dir, {recursive: true, force: true}); }
});

test("automatic uv discovery rejects a system interpreter before loading its compiler", {skip: !python}, async () => {
  const base = cp.execFileSync(python, ["-c", 'import os,sys; print(os.path.join(sys.base_prefix, "python.exe") if sys.platform == "win32" else os.path.join(sys.base_prefix, "bin", "python3"))'], {encoding: "utf8", windowsHide: true}).trim();
  await assert.rejects(runProbe([base], root, "lsp", true), /project Python environment is not set up/);
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
