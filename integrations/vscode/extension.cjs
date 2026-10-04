"use strict";
const vscode=require("vscode"), cp=require("node:child_process"), path=require("node:path"), fs=require("node:fs"), crypto=require("node:crypto"), readline=require("node:readline");
const escape=s=>s.replaceAll("&","&amp;").replaceAll("<","&lt;").replaceAll('"',"&quot;");
class Backend {
  constructor(command,args,cwd) {
    this.pending=new Map();this.id=0;this.closed=false;this.errors="";
    this.child=cp.spawn(command[0],[...command.slice(1),"-u","-m","pcbir.editor.document",...args],{cwd,shell:false,windowsHide:true,stdio:["pipe","pipe","pipe"]});
    this.lines=readline.createInterface({input:this.child.stdout});
    this.lines.on("line",line=>{
      try {
        if(line.length>64*1024*1024) throw new Error("Compiler response exceeds bounds");
        const message=JSON.parse(line),p=this.pending.get(message.id);if(!p)return;
        this.pending.delete(message.id);clearTimeout(p.timer);
        message.error?p.reject(new Error(message.error)):p.resolve(message.result);
      } catch(error) {this.fail(error);}
    });
    this.child.stderr.on("data",bytes=>{this.errors=(this.errors+bytes.toString()).slice(-8192);});
    this.child.stdin.on("error",e=>this.fail(e));
    this.child.on("error",e=>this.fail(e));this.child.on("exit",()=>this.fail(new Error("Compiler exited. "+this.errors)));
  }
  request(request) {
    if(this.closed)return Promise.reject(new Error("Compiler is closed"));
    return new Promise((resolve,reject)=>{
      const id=++this.id,timer=setTimeout(()=>{this.pending.delete(id);reject(new Error("Compiler response timed out"));this.dispose();},60000);
      this.pending.set(id,{resolve,reject,timer});this.child.stdin.write(JSON.stringify({id,request})+"\n");
    });
  }
  fail(error) {for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(error);}this.pending.clear();}
  dispose() {
    if(this.closed)return;this.closed=true;this.fail(new Error("Editor closed"));this.lines.close();
    // EOF lets the Python host terminate its placement worker in finally.
    this.child.stdin.end();const timer=setTimeout(()=>this.child.kill(),2500);timer.unref();
    this.child.once("exit",()=>clearTimeout(timer));
  }
}

class DocumentBridge {
  constructor(document,backend) {this.document=document;this.backend=backend;this.version=-1;this.tail=Promise.resolve();this.scene=null;}
  enqueue(work) {const result=this.tail.then(work);this.tail=result.catch(()=>{});return result;}
  async sync(force=false) {
    const doc=this.document;
    if(force || doc.version!==this.version) {
      // The backend requires monotonic host versions. A forced reload without
      // a source change restarts the session with a separate monotonic counter.
      this.hostVersion=(this.hostVersion||0)+1;
      const result=await this.backend.request({method:"open",version:this.hostVersion,text:doc.getText()});
      this.version=doc.version;this.scene=result.scene;
    }
    return this.scene;
  }
  async handle(message) {
    await this.sync();
    if(message.path==="/api/scene") {this.scene=(await this.backend.request({method:"scene"})).scene;return this.scene;}
    if(message.path==="/api/source-link") {
      const c=this.scene.components.find(c=>c.reference===message.body?.reference);
      if(!c?.source_link)throw new Error("No source declaration for this component");
      const editor=await vscode.window.showTextDocument(this.document,{viewColumn:vscode.ViewColumn.Beside,preserveFocus:false});
      const position=this.document.positionAt(c.source_link.start);
      editor.selection=new vscode.Selection(position,position);editor.revealRange(new vscode.Range(position,position));return {};
    }
    if(message.path!=="/api/operation" || typeof message.body!=="object")throw new Error("Unsupported editor message");
    const action=message.body.action;
    if(["undo_source","redo_source","reload_source"].includes(action)) {
      if(action!=="reload_source") await vscode.commands.executeCommand(action==="undo_source"?"undo":"redo");
      await this.sync(true);return {scene:this.scene};
    }
    const version=this.document.version,before=this.document.getText();
    const result=await this.backend.request({method:"operation",version:this.hostVersion,operation:message.body});
    if(!result.document_edit) {if(result.scene)this.scene=result.scene;return result;}
    const patch=result.document_edit;
    if(this.document.version!==version || patch.version!==this.hostVersion ||
       patch.source_revision!==crypto.createHash("sha256").update(before).digest("hex"))throw new Error("Document changed after review; reload and retry");
    const edit=new vscode.WorkspaceEdit();let expected=before;
    for(const item of [...patch.edits].sort((a,b)=>b.start-a.start)) {
      if(!Number.isInteger(item.start)||!Number.isInteger(item.end)||item.start<0||item.end<item.start||item.end>before.length||typeof item.text!=="string")throw new Error("Invalid compiler document edit");
      edit.replace(this.document.uri,new vscode.Range(this.document.positionAt(item.start),this.document.positionAt(item.end)),item.text);
      expected=expected.slice(0,item.start)+item.text+expected.slice(item.end);
    }
    if(!await vscode.workspace.applyEdit(edit))throw new Error("Native document edit was rejected");
    if(this.document.getText()!==expected)throw new Error("Concurrent document edit: changes remain unsaved; inspect native Undo");
    if(!await this.document.save())throw new Error("Save failed; reviewed changes remain in the dirty document and native Undo");
    await this.sync();return {scene:this.scene};
  }
}

function activate(context) {
  const active=new Map();
  context.subscriptions.push(vscode.window.registerCustomEditorProvider("copperscript.mechanical",{
    async resolveCustomTextEditor(document,panel) {
      if(!vscode.workspace.isTrusted || document.uri.scheme!=="file" || !vscode.workspace.getWorkspaceFolder(document.uri))throw new Error("Mechanical editing requires a trusted local workspace");
      const config=vscode.workspace.getConfiguration("copperscript.mechanical",document.uri);
      const command=config.get("compilerCommand"),digest=config.get("compilerDigest");
      if(!Array.isArray(command)||command.length<1||command.length>32||command.some(s=>typeof s!=="string"||!s||s.length>8192)||!/^([a-f0-9]{64})$/.test(digest||""))throw new Error("Configure compilerCommand and the exact compilerDigest before opening. See the extension README.");
      const cwd=vscode.workspace.getWorkspaceFolder(document.uri).uri.fsPath;
      const local=s=>path.resolve(cwd,s);
      const args=[document.uri.fsPath,"--layers",String(config.get("layers")),"--fab-profile",config.get("fabricationProfile")];
      for(const root of config.get("footprintRoots")) args.push("--footprint-root",local(root));
      if(config.get("placementTemplates"))args.push("--placement-templates",local(config.get("placementTemplates")));
      for(const macro of config.get("hardMacros"))args.push("--hard-macro",local(macro));
      const backend=new Backend(command,args,cwd),bridge=new DocumentBridge(document,backend);
      try {
        const hello=await backend.request({method:"hello"});
        if(hello.compiler_digest!==digest||hello.protocol!=="copperscript-editor-document/v0.1")throw new Error("Compiler pin mismatch; explicitly review/update compilerDigest");
        await bridge.sync();
      } catch(error) {backend.dispose();throw error;}
      active.set(document.uri.toString(),bridge);
      const media=vscode.Uri.file(path.join(__dirname,"media"));
      panel.webview.options={enableScripts:true,localResourceRoots:[media]};
      const nonce=crypto.randomBytes(24).toString("hex");
      let html=fs.readFileSync(path.join(media.fsPath,"index.html"),"utf8");
      html=html.replace("<head>",`<head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${panel.webview.cspSource}; script-src 'nonce-${nonce}';">`)
        .replace('href="/editor.css"',`href="${panel.webview.asWebviewUri(vscode.Uri.joinPath(media,"editor.css"))}"`)
        .replace('src="/editor.js"',`nonce="${nonce}" src="${panel.webview.asWebviewUri(vscode.Uri.joinPath(media,"editor.js"))}"`);
      panel.webview.html=html;
      const messages=panel.webview.onDidReceiveMessage(message=>{
        if(!Number.isInteger(message?.id))return;
        bridge.enqueue(()=>bridge.handle(message)).then(result=>panel.webview.postMessage({id:message.id,result}),error=>panel.webview.postMessage({id:message.id,error:error.message}));
      });
      const changes=vscode.workspace.onDidChangeTextDocument(event=>{
        if(event.document.uri.toString()!==document.uri.toString())return;
        bridge.enqueue(()=>bridge.sync()).then(()=>panel.webview.postMessage({type:"refresh"}),error=>panel.webview.postMessage({type:"refresh",error:error.message}));
      });
      const selections=vscode.window.onDidChangeTextEditorSelection(event=>{
        if(event.textEditor.document.uri.toString()!==document.uri.toString()||!bridge.scene)return;
        const offset=document.offsetAt(event.selections[0].active);
        const component=bridge.scene.components.find(c=>c.source_link && c.source_link.start<=offset && offset<c.source_link.end);
        if(component)panel.webview.postMessage({type:"selection",reference:component.reference});
      });
      panel.onDidDispose(()=>{messages.dispose();changes.dispose();selections.dispose();backend.dispose();active.delete(document.uri.toString());});
    }
  },{webviewOptions:{retainContextWhenHidden:true},supportsMultipleEditorsPerDocument:false}));
  // Test access to the same bridge used by real webview messages, not a second implementation.
  return {active,DocumentBridge,Backend};
}
module.exports={activate};
