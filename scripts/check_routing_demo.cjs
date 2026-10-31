#!/usr/bin/env node
"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), {pathToFileURL} = require("node:url");
const {Worker} = require("node:worker_threads");
const root = path.resolve(__dirname, "..");
async function workerCheck(source) {
  const url = pathToFileURL(path.join(root, "site/demo/routing/route-worker.js")).href;
  const worker = new Worker(`(async()=>{
    const {parentPort}=await import('node:worker_threads');const fs=await import('node:fs');
    globalThis.self={postMessage:message=>parentPort.postMessage(message)};
    globalThis.fetch=async url=>new Response(fs.readFileSync(new URL(url)),{headers:{'Content-Type':'application/wasm'}});
    await import(${JSON.stringify(url)});parentPort.on('message',data=>self.onmessage({data}));
  })().catch(error=>{throw error;});`, {eval:true});
  try {
    await new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(new Error("Routing worker timed out")),30000);
      const fail=error=>{clearTimeout(timer);reject(error);};worker.on("error",fail);
      worker.on("message",message=>{try{
        if(message.type==="fatal")throw new Error(message.error);
        if(message.type==="ready")worker.postMessage({id:1,revision:3,source:"board {"});
        else if(message.id===1){assert.ok(message.result.error);worker.postMessage({id:2,revision:4,source});}
        else{assert.equal(message.id,2);assert.equal(message.revision,4);assert.equal(message.result.error,null);clearTimeout(timer);resolve();}
      }catch(error){fail(error);}});
    });
  } finally {await worker.terminate();}
}
(async()=>{
  const {default:init,route}=await import(pathToFileURL(path.join(root,"site/demo/routing/pkg/copper_web_route.js")));
  const {scenarios}=await import(pathToFileURL(path.join(root,"site/demo/routing/routing-cases.js")));
  await init({module_or_path:fs.readFileSync(path.join(root,"site/demo/routing/pkg/copper_web_route_bg.wasm"))});
  const original=fs.readFileSync(path.join(root,"site/demo/routing/board.copper"),"utf8").replace(/\r\n?/g,"\n");
  const run=source=>JSON.parse(route(source));
  const records=[];
  for(const name of ["detour","layers","blocked"]){
    const started=performance.now(),result=run(scenarios[name].source(original));
    assert.equal(result.error,null,JSON.stringify(result));
    assert.equal(result.native_verified,false);assert.equal(result.qualified_release,false);
    assert.equal(result.before.tracks.length,0);
    assert.deepEqual(result.before.footprints,result.after.footprints,"Fixed component poses must survive routing");
    const metrics=result.routing.metrics;
    if(name==="blocked")assert.equal(metrics.unrouted_net_count,2);
    else{
      assert.equal(metrics.routed_net_count,2,JSON.stringify(result.routing));assert.equal(metrics.unrouted_net_count,0);
      assert.ok(result.after.tracks.length>0);assert.equal(result.drc.findings.filter(f=>f.severity==="error").length,0,JSON.stringify(result.drc));
    }
    if(name==="layers"){assert.ok(metrics.via_count>0,"Layer-change case should produce actual vias");assert.ok(result.after.tracks.some(t=>t.layer==="B.Cu"));}
    if(name==="detour"){
      assert.ok(result.after.tracks.every(t=>t.layer==="F.Cu"));
      const benchmarkFile=path.join(root,"build/bench/browser-routing-before/browser-routing-demo/route-report.json");
      if(fs.existsSync(benchmarkFile)){
        const native=JSON.parse(fs.readFileSync(benchmarkFile,"utf8"));
        assert.deepEqual(result.routing.metrics,native.detailed.metrics,"Native CLI and WASM routing metrics must agree");
      }
    }
    records.push({case:name,connected:metrics.routed_net_count,open:metrics.unrouted_net_count,vias:metrics.via_count,length_mm:metrics.total_length_nm/1e6,milliseconds:Math.round(performance.now()-started)});
  }
  assert.ok(run("board {").error);assert.ok(run(" ".repeat(8193)).error);
  assert.ok(run(original.replace('use library','import x "./parts"; use library')).error);
  assert.ok(run(original.replace('width = 32 mm;','width = 100 mm;')).error);
  assert.ok(run(original.replace('constraint fixed_placement(R1)', 'constraint placement_region(R1)')).error);
  const omitted=run(original.replace('component R1:', 'component PS1: VOLTAGE_SOURCE; no_connect PS1.OUT, PS1.GND { reason = "No physical footprint"; } component R1:'));
  assert.match(omitted.error,/fixed component/,"Unphysicalized components must be rejected, never silently omitted");
  assert.equal(run(original).error,null,"Router must remain usable after rejected input");
  await workerCheck(original);
  console.log(JSON.stringify({wasm_routing_cases:records,worker:"passed",limits_and_recovery:"passed"}));
})().catch(error=>{console.error(error);process.exitCode=1;});
