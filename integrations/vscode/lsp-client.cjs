"use strict";
/* Small LSP client for the advertised CopperScript subset; no npm runtime. */
const cp = require("node:child_process"), path = require("node:path");
const {resolveCompiler} = require("./compiler.cjs");
const MAX_MESSAGE = 16 * 1024 * 1024, MAX_HEADER = 8192;

class Connection {
  constructor(command, cwd, onNotification, onError) {
    if (!Array.isArray(command) || !command.length || command.length > 32 ||
        command.some(s => typeof s !== "string" || !s || s.length > 8192))
      throw new Error("Configure copperscript.lsp.compilerCommand as an interpreter argv array");
    this.pending = new Map(); this.nextId = 0; this.buffer = Buffer.alloc(0); this.closed = false;
    this.onNotification = onNotification; this.onError = onError;
    this.child = cp.spawn(command[0], [...command.slice(1), "-u", "-m", "pcbir.lsp.server"],
      {cwd, shell: false, windowsHide: true, stdio: ["pipe", "pipe", "pipe"]});
    this.child.stdout.on("data", bytes => { try { this.receive(bytes); } catch (error) { this.fail(error); } });
    this.child.stderr.on("data", bytes => onNotification("window/logMessage", {type: 3, message: bytes.toString()}));
    this.child.stdin.on("error", error => this.fail(error));
    this.child.on("error", error => this.fail(error));
    this.child.on("exit", () => this.fail(new Error("CopperScript language server exited")));
  }
  send(message) {
    if (this.closed) throw new Error("Language server is closed");
    const body = Buffer.from(JSON.stringify(message), "utf8");
    if (body.length > MAX_MESSAGE) throw new Error("LSP message exceeds bounds");
    this.child.stdin.write(Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii"), body]));
  }
  notify(method, params) { this.send({jsonrpc: "2.0", method, params}); }
  request(method, params, token) {
    if (token?.isCancellationRequested) return Promise.resolve(undefined);
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const finish = (error, result) => {
        const p = this.pending.get(id); if (!p) return;
        this.pending.delete(id); clearTimeout(p.timer); p.cancel?.dispose();
        error ? reject(error) : resolve(result);
      };
      const timer = setTimeout(() => { finish(new Error("Language server timed out")); this.fail(new Error("Language server timed out")); }, 30000);
      this.pending.set(id, {finish, timer});
      try {
        this.send({jsonrpc: "2.0", id, method, params});
        const cancel = token?.onCancellationRequested(() => {
          if (!this.closed) this.notify("$/cancelRequest", {id});
          finish(null, undefined);
        });
        const p = this.pending.get(id); if (p) p.cancel = cancel; else cancel?.dispose();
      } catch (error) { finish(error); }
    });
  }
  receive(bytes) {
    this.buffer = Buffer.concat([this.buffer, bytes]);
    while (this.buffer.length) {
      const split = this.buffer.indexOf("\r\n\r\n");
      if (split < 0) {
        if (this.buffer.length > MAX_HEADER) throw new Error("Oversized LSP header");
        return;
      }
      if (split > MAX_HEADER) throw new Error("Oversized LSP header");
      const lengths = this.buffer.subarray(0, split).toString("ascii").split("\r\n")
        .filter(line => /^Content-Length:/i.test(line));
      if (lengths.length !== 1 || !/^Content-Length:\s*\d+\s*$/i.test(lengths[0])) throw new Error("Invalid LSP framing");
      const length = Number(lengths[0].split(":")[1]);
      if (!Number.isSafeInteger(length) || length < 1 || length > MAX_MESSAGE) throw new Error("Oversized LSP payload");
      if (this.buffer.length < split + 4 + length) return;
      const message = JSON.parse(this.buffer.subarray(split + 4, split + 4 + length).toString("utf8"));
      this.buffer = this.buffer.subarray(split + 4 + length);
      if (message.jsonrpc !== "2.0") throw new Error("Invalid JSON-RPC response");
      if (message.method) {
        if ("id" in message) this.send({jsonrpc: "2.0", id: message.id, error: {code: -32601, message: "Client method unsupported"}});
        else this.onNotification(message.method, message.params);
      } else {
        this.pending.get(message.id)?.finish(message.error ? new Error(message.error.message) : null, message.result);
      }
    }
  }
  fail(error) {
    if (this.closed) return;
    this.closed = true;
    for (const p of [...this.pending.values()]) p.finish(error);
    this.child.kill();
    this.onError(error);
  }
  async stop() {
    if (this.closed) return;
    try { await this.request("shutdown", {}); this.notify("exit", {}); } catch (_) {}
    this.closed = true;
    for (const p of [...this.pending.values()]) p.finish(new Error("Language server stopped"));
    this.child.stdin.end();
    const timer = setTimeout(() => this.child.kill(), 2000); timer.unref();
    this.child.once("exit", () => clearTimeout(timer));
  }
}

function activateLsp(vscode, context) {
  const clients = new Map(), diagnostics = vscode.languages.createDiagnosticCollection("CopperScript");
  const output = vscode.window.createOutputChannel("CopperScript Language Server");
  const selector = {language: "copper", scheme: "file"};
  const position = p => ({line: p.line, character: p.character});
  const range = r => new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character);
  const key = uri => { const p = path.resolve(uri.fsPath); return process.platform === "win32" ? p.toLowerCase() : p; };
  // Server diagnostics by document, so code actions can send back their `data` (fixes);
  // vscode.Diagnostic objects do not carry it across the extension host boundary.
  const published = new Map();
  const severity = s => [vscode.DiagnosticSeverity.Error, vscode.DiagnosticSeverity.Warning,
    vscode.DiagnosticSeverity.Information ?? vscode.DiagnosticSeverity.Warning,
    vscode.DiagnosticSeverity.Hint ?? vscode.DiagnosticSeverity.Warning][(s || 1) - 1] ?? vscode.DiagnosticSeverity.Warning;
  // Automatic restart after a crash or timeout of a server that had started.
  const restarts = new Map(), restartAt = new Map(), RESTART_LIMIT = 5, RESTART_WINDOW = 5 * 60 * 1000;
  const status = vscode.window.createStatusBarItem?.(vscode.StatusBarAlignment?.Left ?? 1, 0);
  if (status) { status.command = "copperscript.restartLanguageServer"; context.subscriptions.push(status); }
  const showStatus = (state, detail) => {
    if (!status) return;
    status.text = {starting: "$(sync~spin) Copper", ready: "$(check) Copper", restarting: "$(sync~spin) Copper",
      failed: "$(error) Copper"}[state] || "Copper";
    status.tooltip = `CopperScript language server: ${detail || state}. Click to restart.`;
    status.show();
  };
  let disposed = false;
  function retire() {
    for (const c of clients.values()) { c.retired = true; if (c.connection) void c.connection.stop(); }
    clients.clear(); diagnostics.clear(); published.clear(); restarts.clear(); restartAt.clear();
  }
  function restart() {
    retire();
    for (const doc of vscode.workspace.textDocuments) client(doc);
  }
  context.subscriptions.push(diagnostics, output, {dispose() { disposed = true; retire(); }});
  function eligible(document) {
    return !disposed && vscode.workspace.isTrusted && document.languageId === "copper" && document.uri.scheme === "file" &&
      vscode.workspace.getConfiguration("copperscript.lsp", document.uri).get("enabled", true);
  }
  function notify(entry, method, params) {
    if (entry.retired || disposed) return;
    if (method === "window/logMessage") { output.appendLine(params.message); return; }
    if (method !== "textDocument/publishDiagnostics") return;
    const uri = vscode.Uri.parse(params.uri);
    const document = vscode.workspace.textDocuments.find(d => key(d.uri) === key(uri));
    if (document && vscode.workspace.getWorkspaceFolder(document.uri)?.uri.toString() !== entry.folderId) return;
    if (params.version !== undefined && (!document || document.version !== params.version)) return;
    published.set(key(uri), params.diagnostics);
    diagnostics.set(uri, params.diagnostics.map(item => {
      const d = new vscode.Diagnostic(range(item.range), item.message, severity(item.severity));
      d.code = item.code; d.source = item.source;
      if (item.relatedInformation) d.relatedInformation = item.relatedInformation.map(info =>
        new vscode.DiagnosticRelatedInformation(new vscode.Location(vscode.Uri.parse(info.location.uri), range(info.location.range)), info.message));
      return d;
    }));
  }
  function sync(entry, document) {
    if (entry.retired || !eligible(document) || !entry.connection || entry.connection.closed) return;
    const uri = document.uri.toString(), previous = entry.versions.get(uri);
    if (previous === document.version) return;
    if (previous === undefined) entry.connection.notify("textDocument/didOpen", {textDocument: {uri, languageId: "copper", version: document.version, text: document.getText()}});
    else entry.connection.notify("textDocument/didChange", {textDocument: {uri, version: document.version}, contentChanges: [{text: document.getText()}]});
    entry.versions.set(uri, document.version);
  }
  function client(document) {
    if (!eligible(document)) return null;
    const folder = vscode.workspace.getWorkspaceFolder(document.uri);
    if (!folder) return null;
    const id = folder.uri.toString();
    if (clients.has(id)) return clients.get(id);
    if ((restartAt.get(id) || 0) > Date.now()) return null;
    const entry = {versions: new Map(), folderId: id};
    const config = vscode.workspace.getConfiguration("copperscript.lsp", document.uri);
    const report = error => {
      if (entry.retired || disposed) return;
      output.appendLine(`${folder.uri.fsPath}: ${error.message}`);
      if (entry.started && !/timed out/.test(error.message)) {
        // A running server crashed: restart it with backoff instead of going silent. A timeout
        // is not retried automatically (a slow board would only time out again).
        const now = Date.now(), recent = (restarts.get(id) || []).filter(t => now - t < RESTART_WINDOW);
        if (recent.length < RESTART_LIMIT) {
          restarts.set(id, [...recent, now]);
          entry.retired = true; clients.delete(id);
          const delay = Math.min(30000, 1000 * 2 ** recent.length);
          restartAt.set(id, Date.now() + delay);  // client() waits until then, whatever the user does
          showStatus("restarting", `${error.message}; restarting in ${delay / 1000} s`);
          const timer = setTimeout(() => {
            restartAt.delete(id);
            if (disposed) return;
            for (const doc of vscode.workspace.textDocuments)
              if (vscode.workspace.getWorkspaceFolder(doc.uri)?.uri.toString() === id) client(doc);
          }, delay);
          timer.unref?.();
          return;
        }
        showStatus("failed", `stopped after ${RESTART_LIMIT} restarts in 5 minutes: ${error.message}`);
      } else showStatus("failed", error.message);
      for (const doc of vscode.workspace.textDocuments)
        if (vscode.workspace.getWorkspaceFolder(doc.uri)?.uri.toString() === id) diagnostics.delete(doc.uri);
      if (!entry.warned) {
        entry.warned = true;
        void vscode.window.showWarningMessage?.("CopperScript language tools could not start. Check the selected project environment.", "Show Setup")
          .then(choice => { if (choice === "Show Setup") return vscode.commands.executeCommand("copperscript.showSetup", folder); });
      }
    };
    clients.set(id, entry);
    showStatus("starting");
    entry.ready = resolveCompiler(config.get("compilerCommand", []), folder.uri.fsPath).then(async compiler => {
        if (entry.retired || disposed) return;
        entry.compiler = compiler;
        output.appendLine(`${folder.uri.fsPath}: ${compiler.source}; Python ${compiler.python.join(".")}; CopperScript ${compiler.compilerVersion}; ${compiler.module}`);
        entry.connection = new Connection(compiler.command, folder.uri.fsPath,
          (method, params) => notify(entry, method, params), report);
        await entry.connection.request("initialize", {processId: process.pid, rootUri: folder.uri.toString(),
          capabilities: {general: {positionEncodings: ["utf-16"]}, textDocument: {publishDiagnostics: {versionSupport: true}}}});
        if (entry.retired || disposed) return;
        entry.connection.notify("initialized", {});
        entry.started = true;
        showStatus("ready", `${compiler.source}, CopperScript ${compiler.compilerVersion}`);
        for (const doc of vscode.workspace.textDocuments) sync(entry, doc);
      });
    entry.ready.catch(report);
    return entry;
  }
  async function query(document, method, p, token) {
    const entry = client(document); if (!entry) return;
    try { await entry.ready; } catch (_) { return; }
    if (entry.retired || !entry.connection || entry.connection.closed || token?.isCancellationRequested || disposed) return;
    sync(entry, document);
    const version = document.version;
    let result;
    try { result = await entry.connection.request(method, {textDocument: {uri: document.uri.toString()}, ...(p ? {position: position(p)} : {})}, token); }
    catch (_) { return; }
    return document.version === version && !token?.isCancellationRequested ? result : undefined;
  }
  function syncAll(doc) {
    client(doc);
    for (const c of clients.values()) c.ready.then(() => sync(c, doc)).catch(error => output.appendLine(error.message));
  }
  context.subscriptions.push(
    vscode.languages.registerCompletionItemProvider(selector, {async provideCompletionItems(doc, p, token) {
      const items = await query(doc, "textDocument/completion", p, token);
      return items?.map(item => { const c = new vscode.CompletionItem(item.label, item.kind - 1); c.detail = item.detail;
        if (item.textEdit) { c.range = range(item.textEdit.range); c.insertText = item.textEdit.newText; } return c; });
    }}, "."),
    vscode.languages.registerHoverProvider(selector, {async provideHover(doc, p, token) {
      const item = await query(doc, "textDocument/hover", p, token);
      if (!item) return;
      const content = new vscode.MarkdownString(); content.appendText(item.contents.value); content.isTrusted = false;
      return new vscode.Hover(content, range(item.range));
    }}),
    vscode.languages.registerDefinitionProvider(selector, {async provideDefinition(doc, p, token) {
      const item = await query(doc, "textDocument/definition", p, token);
      return item ? new vscode.Location(vscode.Uri.parse(item.uri), range(item.range)) : undefined;
    }}),
    vscode.languages.registerDocumentSymbolProvider(selector, {async provideDocumentSymbols(doc, token) {
      const items = await query(doc, "textDocument/documentSymbol", null, token);
      return items?.map(item => new vscode.DocumentSymbol(item.name, item.detail || "", item.kind - 1, range(item.range), range(item.selectionRange)));
    }}),
    ...(vscode.languages.registerCodeActionsProvider ? [vscode.languages.registerCodeActionsProvider(selector, {
      async provideCodeActions(doc, _range, context, token) {
        const raw = published.get(key(doc.uri)) || [];
        // Match VS Code's copies to the server's diagnostics by code, message and range.
        const same = (d, item) => String(d.code) === String(item.code) && d.message === item.message &&
          d.range?.start?.line === item.range.start.line && d.range?.start?.character === item.range.start.character;
        const chosen = raw.filter(item => item.data && (context.diagnostics || []).some(d => same(d, item)));
        if (!chosen.length) return [];
        const entry = client(doc); if (!entry) return [];
        try { await entry.ready; } catch (_) { return []; }
        if (!entry.connection || entry.connection.closed || token?.isCancellationRequested) return [];
        sync(entry, doc);
        let actions;
        try { actions = await entry.connection.request("textDocument/codeAction", {textDocument: {uri: doc.uri.toString()},
          range: chosen[0].range, context: {diagnostics: chosen}}, token); } catch (_) { return []; }
        return (actions || []).map(item => {
          const action = new vscode.CodeAction(item.title, vscode.CodeActionKind.QuickFix);
          action.isPreferred = !!item.isPreferred;
          action.edit = new vscode.WorkspaceEdit();
          for (const [target, edits] of Object.entries(item.edit?.changes || {}))
            for (const e of edits) action.edit.replace(vscode.Uri.parse(target), range(e.range), e.newText);
          action.diagnostics = (context.diagnostics || []).filter(d => (item.diagnostics || []).some(x => same(d, x)));
          return action;
        });
      }}, {providedCodeActionKinds: [vscode.CodeActionKind.QuickFix]})] : []),
    vscode.workspace.onDidOpenTextDocument(syncAll),
    vscode.workspace.onDidChangeTextDocument(event => syncAll(event.document)),
    vscode.workspace.onDidSaveTextDocument(doc => {
      if (!eligible(doc)) return;
      client(doc);
      for (const c of clients.values()) c.ready.then(() => {
        sync(c, doc);
        if (!c.retired && c.connection && !c.connection.closed) c.connection.notify("textDocument/didSave", {textDocument: {uri: doc.uri.toString()}});
      }).catch(error => output.appendLine(error.message));
    }),
    vscode.workspace.onDidCloseTextDocument(doc => {
      diagnostics.delete(doc.uri); published.delete(key(doc.uri));
      for (const c of clients.values()) c.ready.then(() => { const uri = doc.uri.toString(); if (!c.retired && c.versions.delete(uri) && c.connection && !c.connection.closed) c.connection.notify("textDocument/didClose", {textDocument: {uri}}); }).catch(error => output.appendLine(error.message));
    }),
    vscode.workspace.onDidChangeConfiguration(event => {
      if (!event.affectsConfiguration("copperscript.lsp")) return;
      restart();
    })
  );
  const watcher = vscode.workspace.createFileSystemWatcher("**/{*.copper,copper.mod,copper.lock}");
  // LSP FileChangeType: 1 created, 2 changed, 3 deleted.
  const changed = type => uri => { for (const c of clients.values()) c.ready.then(() => {
    if (!c.retired && c.connection && !c.connection.closed) c.connection.notify("workspace/didChangeWatchedFiles", {changes: [{uri: uri.toString(), type}]});
  }).catch(error => output.appendLine(error.message)); };
  context.subscriptions.push(watcher, watcher.onDidCreate(changed(1)), watcher.onDidChange(changed(2)), watcher.onDidDelete(changed(3)));
  for (const doc of vscode.workspace.textDocuments) client(doc);
  return {clients, diagnostics, output, restart};
}

module.exports = {Connection, activateLsp};
