/* Runs inside a disposable installed VS Code Extension Development Host. */
const assert=require("node:assert/strict"),fs=require("node:fs"),path=require("node:path"),cp=require("node:child_process"),vscode=require("vscode");
async function waitUntil(predicate,timeout=30000) {
  const end=Date.now()+timeout;
  while(Date.now()<end) {if(await predicate())return;await new Promise(r=>setTimeout(r,100));}
  throw new Error("VS Code test timed out");
}
exports.run=async()=>{
  const root=process.env.COPPER_TEST_ROOT,python=path.join(root,".venv/Scripts/python.exe");
  const folder=vscode.workspace.workspaceFolders[0],source=vscode.Uri.joinPath(folder.uri,"board.copper");
  const original=fs.readFileSync(source.fsPath);
  assert(source.fsPath.includes("build"),"Only test disposable sources");
  const digest=cp.execFileSync(python,["-m","pcbir.editor.document","--fingerprint"],{cwd:root,encoding:"utf8",windowsHide:true}).trim();
  const config=vscode.workspace.getConfiguration("copperscript.mechanical",source);
  await config.update("compilerCommand",[python],vscode.ConfigurationTarget.WorkspaceFolder);
  await config.update("compilerDigest",digest,vscode.ConfigurationTarget.WorkspaceFolder);
  await config.update("footprintRoots",[process.env.COPPER_TEST_FOOTPRINTS],vscode.ConfigurationTarget.WorkspaceFolder);
  const extension=vscode.extensions.getExtension("andenore.copperscript-mechanical");assert(extension);
  const api=await extension.activate();
  await vscode.commands.executeCommand("vscode.openWith",source,"copperscript.mechanical");
  await waitUntil(()=>api.active.has(source.toString()));
  const bridge=api.active.get(source.toString()),document=bridge.document;
  const scene=()=>bridge.enqueue(()=>bridge.handle({path:"/api/scene"}));
  const operation=async(action,fields={})=>{
    const s=await scene();return bridge.enqueue(()=>bridge.handle({path:"/api/operation",body:{action,revision:s.revision,...fields}}));
  };
  assert.equal((await scene()).components.length,4);
  await operation("start_auto_place",{budget_seconds:30});
  await waitUntil(async()=>!(await scene()).placement_job || (await scene()).placement_job.status!=="running");
  assert((await scene()).pending_preview,"Placement worker must complete");await operation("apply");
  await operation("prepare_lock",{reference:"R_ANCHOR",x_nm:8000000,y_nm:8000000,rotation:0,side:"back",locks:["position","rotation","side"]});
  const review=(await scene()).source_review;assert(review);
  assert.deepEqual(fs.readFileSync(source.fsPath),original,"Review cannot write disk");
  await operation("save_source",{review_id:review.id});
  assert.match(document.getText(),/x = 8mm; y = 8mm/);
  assert.equal(document.isDirty,false,"Native Save owns persistence");
  assert.match(fs.readFileSync(source.fsPath,"utf8"),/x = 8mm; y = 8mm/);
  await operation("undo_source");
  await waitUntil(()=>/x = 7mm; y = 8mm/.test(document.getText()));
  await bridge.enqueue(()=>bridge.sync());
  assert.equal((await scene()).components.find(c=>c.reference==="R_ANCHOR").position[0],7000000);
  await document.save();assert.deepEqual(fs.readFileSync(source.fsPath),original);
  // Invalid unsaved buffers must not fall back to old on-disk source.
  await vscode.window.showTextDocument(document);
  const edit=new vscode.WorkspaceEdit();edit.insert(source,new vscode.Position(0,0),"invalid\n");await vscode.workspace.applyEdit(edit);
  await assert.rejects(()=>bridge.enqueue(()=>bridge.sync()));
  await vscode.commands.executeCommand("undo");await waitUntil(()=>!document.getText().startsWith("invalid"));
  await bridge.enqueue(()=>bridge.sync());assert.equal((await scene()).components.length,4);
  // Native component declaration navigation uses compiler locations, not webview filenames.
  await bridge.enqueue(()=>bridge.handle({path:"/api/source-link",body:{reference:"R_ANCHOR"}}));
  assert.equal(vscode.window.activeTextEditor.selection.start.line,3);
  await document.save();assert.deepEqual(fs.readFileSync(source.fsPath),original);
  console.log("PASS: actual VS Code custom editor, pinned compiler, rough placement, reviewed WorkspaceEdit/Save, native Undo, invalid-buffer recovery and source navigation");
};
