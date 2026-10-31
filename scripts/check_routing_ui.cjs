#!/usr/bin/env node
/* DOM/state tests with the real WASM router. This does not test browser layout. */
"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path");
const {pathToFileURL} = require("node:url"), {createRequire} = require("node:module");
const root = path.resolve(__dirname, "..");
const tools = createRequire(path.join(root, "tests/browser-demo/package.json"));
(async () => {
  const {Window} = await import(pathToFileURL(tools.resolve("happy-dom")));
  const window = new Window({url:"https://example.test/CopperScript/demo/routing/", settings:{enableJavaScriptEvaluation:false,disableJavaScriptFileLoading:true,disableCSSFileLoading:true}});
  const {default:init,route} = await import(pathToFileURL(path.join(root,"site/demo/routing/pkg/copper_web_route.js")));
  await init({module_or_path:fs.readFileSync(path.join(root,"site/demo/routing/pkg/copper_web_route_bg.wasm"))});
  window.document.write(fs.readFileSync(path.join(root,"site/demo/routing/index.html"),"utf8"));
  globalThis.document = window.document;
  globalThis.ResizeObserver = class {observe() {}};
  globalThis.Worker = class {
    constructor(){queueMicrotask(()=>{if(!this.stopped)this.onmessage?.({data:{type:"ready"}});});}
    postMessage({id,revision,source}){setTimeout(()=>{if(!this.stopped)this.onmessage?.({data:{type:"result",id,revision,result:JSON.parse(route(source)),milliseconds:0}});},20);}
    terminate(){this.stopped=true;}
  };
  globalThis.fetch = async url => new Response(fs.readFileSync(new URL(url)));
  const $ = id => window.document.getElementById(id);
  const click = selector => {const button=window.document.querySelector(selector);assert.ok(button,selector);button.click();};
  const change = id => $(id).dispatchEvent(new window.Event("change",{bubbles:true}));
  const waitFor = async predicate => {const end=Date.now()+15000;while(!predicate()){if(Date.now()>end)throw new Error("UI state timed out");await new Promise(resolve=>setTimeout(resolve,5));}};
  await import(pathToFileURL(path.join(root,"site/demo/routing/routing.js")));
  await waitFor(() => $("route-status").textContent.includes("8 of 8"));
  assert.equal(window.document.querySelector('[data-case="weave"]').getAttribute("aria-pressed"),"true");
  assert.match($("source").value,/board SignalWeave/);
  assert.equal($("net").options.length,9);
  assert.equal($("board").hasAttribute("hidden"),false,"SVG must actually be visible, not merely have a JS property");
  assert.ok($("board").querySelectorAll("line[data-layer]").length>0);
  click('[data-stage="before"]');assert.equal($("board").querySelectorAll("line[data-layer]").length,0);
  assert.ok($("board").querySelectorAll("line[stroke-dasharray]").length>0);
  click('[data-stage="after"]');assert.ok($("board").querySelectorAll("line[data-layer]").length>0);
  $("net").value="SIGNAL_1";change("net");
  assert.ok($("board").querySelectorAll('[data-net="SIGNAL_2"]').length>0);
  for(const track of $("board").querySelectorAll('[data-net="SIGNAL_2"]'))assert.equal(Number(track.getAttribute("opacity")),.12);
  $("net").value="all";change("net");
  click('[data-case="layers"]');await waitFor(()=>$("route-status").textContent.includes("2 of 2"));
  assert.ok($("board").querySelectorAll('line[data-layer="B.Cu"]').length>0);
  $("layer").value="B.Cu";change("layer");
  assert.equal($("board").querySelectorAll('polygon[fill="url(#keepout-hatch)"]').length,0,"Front-only keepout must hide on the back layer");
  $("net").value="SIGNAL_A";change("net");
  for(const track of $("board").querySelectorAll('[data-net="SIGNAL_B"]'))assert.equal(Number(track.getAttribute("opacity")),.12);
  $("layer").value="all";change("layer");
  click('[data-case="blocked"]');await waitFor(()=>$("route-status").textContent.includes("2 signals remain open"));
  assert.equal($("board").querySelectorAll("line[data-layer]").length,0);
  assert.ok($("route-findings").children.length>0);
  click('[data-case="detour"]');
  const edited=$("source").value+"\n// Changed while routing\n";
  $("source").value=edited;$("source").dispatchEvent(new window.Event("input",{bubbles:true}));
  await waitFor(()=>$("stop").hidden);
  assert.equal($("board").hasAttribute("hidden"),true,"Old revisions must never restore their geometry");
  assert.equal($("route-metrics").children.length,0);
  click('#route');await waitFor(()=>$("route-status").textContent.includes("2 of 2"));
  click('#route');click('#stop');
  assert.equal($("source").value,edited);assert.equal($("board").hasAttribute("hidden"),true);
  click('#route');await waitFor(()=>$("route-status").textContent.includes("2 of 2"));
  await window.happyDOM.abort();
  console.log("Routing DOM/state checks passed: default eight-signal weave, visible SVG, before/after, layer/net filters, blocked outcome, stale revisions, stop/restart.");
})().catch(error=>{console.error(error);process.exitCode=1;});
