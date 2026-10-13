"use strict";
const {test}=require("node:test"),assert=require("node:assert/strict");
const fs=require("node:fs"),path=require("node:path"),vm=require("node:vm");

function harness() {
  const commands={},calls=[],disposable={dispose() {}};
  const vscode={
    commands:{registerCommand(name,fn) {commands[name]=fn;return disposable;},
      async executeCommand(...args) {calls.push(args);}},
    window:{registerCustomEditorProvider() {return disposable;},
      async showOpenDialog() {return [];}}
  };
  const module={exports:{}};
  const filename=path.resolve(__dirname,"../integrations/vscode/extension.cjs");
  vm.runInNewContext(fs.readFileSync(filename,"utf8"),{module,__dirname:path.dirname(filename),
    setTimeout,clearTimeout,require(name) {
      if(name==="vscode")return vscode;
      if(name==="./lsp-client.cjs")return {activateLsp() {return {};}};
      if(name==="./setup.cjs")return {activateSetup() {}};
      if(name==="./compiler.cjs")return require("../integrations/vscode/compiler.cjs");
      return require(name);
    }});
  return {vscode,commands,calls,...module.exports.activate({subscriptions:[]})};
}

test("document changes during compilation are synchronized before returning a scene",async()=>{
  const {DocumentBridge}=harness();
  let text="old",release;
  const document={version:1,getText:()=>text},requests=[];
  const backend={async request(request) {
    requests.push(request);
    if(requests.length===1)await new Promise(resolve=>{release=resolve;});
    return {scene:{text:request.text}};
  }};
  const bridge=new DocumentBridge(document,backend),pending=bridge.sync();
  document.version=2;text="new";release();
  assert.equal((await pending).text,"new");
  assert.equal(bridge.version,2);
  assert.deepEqual(requests.map(r=>[r.version,r.text]),[[1,"old"],[2,"new"]]);
  await bridge.sync();assert.equal(requests.length,2);
  await bridge.sync(true);assert.equal(requests[2].version,3);
});

test("invalid buffers clear the cached scene and recover without falling back to disk",async()=>{
  const {DocumentBridge}=harness();
  let text="valid";
  const document={version:1,getText:()=>text};
  const backend={async request(request) {
    if(request.text==="invalid")throw new Error("invalid source");
    return {scene:{text:request.text}};
  }};
  const bridge=new DocumentBridge(document,backend);
  await bridge.sync();document.version=2;text="invalid";
  await assert.rejects(bridge.sync(),/invalid source/);
  assert.equal(bridge.scene,null);
  document.version=3;text="recovered";
  assert.equal((await bridge.sync()).text,"recovered");
});

test("a source review cannot commit while the native document changes",async()=>{
  const {DocumentBridge}=harness();
  let text="before";
  const document={version:1,getText:()=>text};
  const backend={async request(request) {
    if(request.method==="open")return {scene:{}};
    document.version++;text="external edit";
    return {document_edit:{version:request.version,source_revision:"old",edits:[]}};
  }};
  const bridge=new DocumentBridge(document,backend);
  await assert.rejects(bridge.handle({path:"/api/operation",body:{action:"save_source"}}),/Document changed after review/);
  assert.equal(text,"external edit");
});

test("open command uses a selected board, active text board, or file picker",async()=>{
  const {vscode,commands,calls}=harness();
  const uri={scheme:"file",path:"/project/board.copper"};
  const open=commands["copperscript.openMechanical"];
  await open(uri);assert.equal(calls[0][1],uri);
  vscode.window.activeTextEditor={document:{uri}};
  await open();assert.equal(calls[1][1],uri);
  vscode.window.activeTextEditor=null;
  let options;
  vscode.window.showOpenDialog=async value=>{options=value;return [uri];};
  await open();assert.equal(calls[2][1],uri);
  assert.equal(options.canSelectMany,false);
  assert.deepEqual(Array.from(options.filters["CopperScript boards"]),["copper"]);
  vscode.window.showOpenDialog=async()=>[];
  await open();assert.equal(calls.length,3);
});

test("webview Open always uses a native picker and works with an invalid current buffer",async()=>{
  const {vscode,calls,DocumentBridge}=harness();
  const active={scheme:"file",path:"/project/current.copper"},picked={scheme:"file",path:"/project/next.copper"};
  vscode.window.activeTextEditor={document:{uri:active}};
  vscode.window.showOpenDialog=async()=>[picked];
  const bridge=new DocumentBridge({}, {request() {throw new Error("invalid buffer must not prevent opening another board");}});
  assert.equal((await bridge.handle({path:"/api/open-board"})).opened,true);
  assert.equal(calls[0][1],picked);
});
