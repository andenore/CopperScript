"use strict";
const {test} = require("node:test"), assert = require("node:assert/strict"), cp = require("node:child_process");
const {EventEmitter} = require("node:events"), Module = require("node:module"), path = require("node:path");
const {pathToFileURL, fileURLToPath} = require("node:url");
const {Connection, activateLsp} = require("../integrations/vscode/lsp-client.cjs");
const python = process.env.COPPER_TEST_PYTHON;
const root = path.resolve(__dirname, "..");

test("actual stdio client initializes, queries, and shuts down", {skip: !python}, async () => {
  const errors = [], c = new Connection([python], root, () => {}, e => errors.push(e));
  try {
  const init = await c.request("initialize", {capabilities: {}});
  assert.equal(init.capabilities.positionEncoding, "utf-16");
  c.notify("initialized", {});
  const uri = pathToFileURL(path.join(root, "build/client-unsaved.copper")).href;
  const text = 'board D { use library "tiny"; component R1: RESISTOR; net N { R1.; } }';
  c.notify("textDocument/didOpen", {textDocument: {uri, version: 1, text}});
  const completions = await c.request("textDocument/completion", {textDocument: {uri}, position: {line: 0, character: text.indexOf("R1.;") + 3}});
  assert.ok(completions.some(c => c.label === "1"));
  const symbols = await c.request("textDocument/documentSymbol", {textDocument: {uri}});
  assert.equal(symbols[0].name, "R1");
  } finally {
  const exited = c.child.exitCode === null ? new Promise(resolve => c.child.once("exit", resolve)) : Promise.resolve(c.child.exitCode);
  await c.stop();
  assert.equal(await exited, 0);
  }
  assert.deepEqual(errors, []);
});

test("client parses fragmented multibyte and concatenated messages", () => {
  const messages = [], c = Object.create(Connection.prototype);
  c.buffer = Buffer.alloc(0); c.pending = new Map(); c.onNotification = (method, params) => messages.push({method, params});
  const frame = message => { const body = Buffer.from(JSON.stringify(message)); return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]); };
  const bytes = Buffer.concat([frame({jsonrpc: "2.0", method: "test", params: "😀"}), frame({jsonrpc: "2.0", method: "test", params: "next"})]);
  for (let i = 0; i < bytes.length; i += 3) c.receive(bytes.subarray(i, i + 3));
  assert.deepEqual(messages.map(m => m.params), ["😀", "next"]);
  assert.throws(() => c.receive(Buffer.from("Content-Length: 999999999\r\n\r\n")), /Oversized/);
});

test("workspace providers use actual server with unsaved versions", {skip: !python}, async () => {
  const providers = {}, listeners = {}, subscriptions = [], notices = [];
  const disposable = {dispose() {}};
  const uri = s => ({scheme: "file", fsPath: fileURLToPath(s), toString: () => s});
  const folder = {uri: uri(pathToFileURL(root).href)};
  let text = 'board D { use library "tiny"; component R1: RESISTOR { value = 100ohm; } net N { R1.1; } }';
  let command = [path.join(root, "nonexistent-python")];
  const warnings = [];
  const doc = {uri: uri(pathToFileURL(path.join(root, "build/provider-unsaved.copper")).href), languageId: "copper", version: 1, getText: () => text};
  class Range {constructor(...args) {this.args = args;}}
  class Item {constructor(...args) {this.args = args;}}
  class Markdown {appendText(value) {this.value = value;}}
  const vscode = {
    Uri: {parse: uri}, Range, CompletionItem: Item, Diagnostic: Item, DocumentSymbol: Item, Hover: Item,
    MarkdownString: Markdown, Location: Item, DiagnosticRelatedInformation: Item,
    DiagnosticSeverity: {Error: 0, Warning: 1},
    languages: {
      createDiagnosticCollection: () => ({...disposable, clear() {}, delete() {}, set(u, d) {notices.push(d);}}),
      ...Object.fromEntries(["CompletionItem", "Hover", "Definition", "DocumentSymbol"].map(kind =>
        [`register${kind}Provider`, (selector, provider) => {providers[kind] = provider; return disposable;}]))
    },
    window: {createOutputChannel: () => ({...disposable, appendLine() {}}),
      async showWarningMessage(message) {warnings.push(message);}},
    workspace: {
      isTrusted: true, textDocuments: [doc], getWorkspaceFolder: () => folder,
      getConfiguration: () => ({get: name => name === "compilerCommand" ? command : true}),
      ...Object.fromEntries(["OpenTextDocument", "ChangeTextDocument", "SaveTextDocument", "CloseTextDocument", "ChangeConfiguration"].map(kind =>
        [`onDid${kind}`, listener => {listeners[kind] = listener; return disposable;}])),
      createFileSystemWatcher: () => ({...disposable, onDidCreate: () => disposable, onDidChange: () => disposable, onDidDelete: () => disposable})
    }
  };
  const api = activateLsp(vscode, {subscriptions});
  try {
    assert.equal(await providers.Hover.provideHover(doc, {line: 0, character: 0}), undefined);
    assert.equal(await providers.Hover.provideHover(doc, {line: 0, character: 0}), undefined);
    assert.equal(warnings.length, 1, "failed setup should not warn for every keystroke");
    command = [python];
    api.restart();
    const item = await providers.Hover.provideHover(doc, {line: 0, character: text.indexOf("R1.1")});
    assert.match(item.args[0].value, /100 ohm/);
    text = text.replaceAll("R1", "R_NEW"); doc.version++;
    listeners.ChangeTextDocument({document: doc});
    const symbols = await providers.DocumentSymbol.provideDocumentSymbols(doc);
    assert.equal(symbols[0].args[0], "R_NEW");
    const c = [...api.clients.values()][0];
    // Published diagnostics for an older buffer must be ignored by the client.
    c.connection.onNotification("textDocument/publishDiagnostics", {uri: doc.uri.toString(), version: 1, diagnostics: [{message: "stale"}]});
    assert.ok(notices.every(items => items.every(item => !item.args?.includes("stale"))));
    await c.connection.stop();
  } finally {for (const d of subscriptions) d.dispose();}
});

test("untrusted workspaces never start an interpreter", () => {
  const subscriptions = [], disposable = {dispose() {}};
  const original = cp.spawn; cp.spawn = () => { throw new Error("must not spawn"); };
  const doc = {languageId: "copper", uri: {scheme: "file"}};
  const vscode = {
    languages: {createDiagnosticCollection: () => ({...disposable, clear() {}}),
      ...Object.fromEntries(["CompletionItem", "Hover", "Definition", "DocumentSymbol"].map(k => [`register${k}Provider`, () => disposable]))},
    window: {createOutputChannel: () => disposable},
    workspace: {isTrusted: false, textDocuments: [doc],
      ...Object.fromEntries(["OpenTextDocument", "ChangeTextDocument", "SaveTextDocument", "CloseTextDocument", "ChangeConfiguration"].map(k => [`onDid${k}`, () => disposable])),
      createFileSystemWatcher: () => ({...disposable, onDidCreate: () => disposable, onDidChange: () => disposable, onDidDelete: () => disposable})}
  };
  try {assert.equal(activateLsp(vscode, {subscriptions}).clients.size, 0);}
  finally {cp.spawn = original; for (const s of subscriptions) s.dispose();}
});
