import {scenarios} from "./routing-cases.js";
import {sourceChecks} from "./source-checks.js";
const $ = id => document.getElementById(id);
const NS = "http://www.w3.org/2000/svg";
let original = "", weave = "", worker = null, ready = false, revision = 0, sequence = 0, active = null, timer = null;
let result = null, stage = "after";
const checks = sourceChecks($("source"), $("source-check-status"), $("source-findings"), (errors, message) => {
  if (!result && (!active || active.revision !== revision)) status(message, errors ? "fail" : "stale");
});

function node(tag, attributes = {}, text = "") {
  const el = document.createElementNS(NS, tag);
  for (const [key, value] of Object.entries(attributes)) el.setAttribute(key, value);
  if (text) el.textContent = text;
  return el;
}
function html(tag, text, className = "") {return Object.assign(document.createElement(tag), {textContent: text, className});}
function status(message, state = "stale") {
  $("route-status").textContent = message;
  $("route-status").className = `check-summary ${state}`;
}
function runtime(message, state = "") {
  $("runtime-status").textContent = message;
  $("runtime-status").parentElement.className = `runtime ${state}`;
}
function finish() {
  clearTimeout(timer); timer = null; active = null;
  $("stop").hidden = true; $("route").disabled = !original || (!ready && !!worker);
}
function clear(message = "Source changed · route again to update the geometry.") {
  result = null; $("board").replaceChildren(); $("board").toggleAttribute("hidden", true);
  $("board-empty").hidden = false; $("board-empty").textContent = "No current routing result.";
  $("route-metrics").replaceChildren(); $("route-findings").replaceChildren(); $("route-timing").textContent = "";
  $("net").replaceChildren(Object.assign(document.createElement("option"), {value:"all",textContent:"All signals"})); $("net").disabled = true;
  $("board-state").textContent = "Result is stale"; status(message);
}
function stop(message, state = "stale") {
  if (worker) worker.terminate(); worker = null; ready = false; finish(); clear(message);
  status(message, state); if (state === "fail") checks.routeError(message);
  runtime("Router paused", "failed"); $("route").disabled = !original;
}
function start() {
  if (worker) worker.terminate(); ready = false; finish();
  runtime("Loading router…");
  const own = new Worker(new URL("./route-worker.js", import.meta.url), {type: "module"}); worker = own;
  const loading = setTimeout(() => {if (worker === own && !ready) stop("Router loading timed out. Press Route board to retry.", "fail");}, 30000);
  own.onerror = event => {event.preventDefault(); clearTimeout(loading); if (worker === own) stop("Could not load the router. Check your connection, then press Route board to retry.", "fail");};
  own.onmessage = event => {
    if (worker !== own) return;
    const message = event.data;
    if (message.type === "ready") {
      clearTimeout(loading); ready = true; runtime("Router ready", "ready"); finish(); run();
    } else if (message.type === "fatal") {
      clearTimeout(loading); stop(message.error, "fail");
    } else if (message.type === "result" && active?.id === message.id) {
      finish();
      if (revision !== message.revision) return;
      if (message.result.error) {clear(message.result.error); status(message.result.error,"fail"); checks.routeError(message.result.error); return;}
      display(message.result, message.milliseconds);
    }
  };
}
function run() {
  if (!original) return;
  if (!worker) {start(); return;}
  if (!ready || active) return;
  checks.schedule(0);
  if (new TextEncoder().encode($("source").value).length > 8192) {clear(); status("The routing lab accepts at most 8 KiB of source.","fail"); checks.schedule(0); return;}
  clear("Routing with bounded search…"); active = {id: ++sequence, revision};
  $("route").disabled = true; $("stop").hidden = false;
  worker.postMessage({...active, source: $("source").value});
  timer = setTimeout(() => stop("The 12-second browser limit was reached. Source is preserved; simplify it and retry.", "fail"), 12000);
}
function display(value, milliseconds) {
  result = value; $("board-state").textContent = "Computed from current source";
  $("board-title").textContent = value.after.name;
  $("route-timing").textContent = `${milliseconds.toFixed(0)} ms · pipeline`;
  const metrics = value.routing.metrics, total = metrics.routed_net_count + metrics.unrouted_net_count;
  status(metrics.unrouted_net_count ? `${metrics.unrouted_net_count} signal${metrics.unrouted_net_count === 1 ? " remains" : "s remain"} open` : `${metrics.routed_net_count} of ${total} signals connected`, metrics.unrouted_net_count ? "fail" : "pass");
  for (const [label, number] of [["Track length", `${(metrics.total_length_nm / 1e6).toFixed(2)} mm`], ["Vias", metrics.via_count], ["Routing passes", metrics.passes], ["Internal DRC findings", value.drc.findings.length]]) {
    $("route-metrics").append(html("dt", label), html("dd", String(number)));
  }
  for (const net of value.routing.nets.filter(net => !net.connected)) {
    const li = html("li", `${net.net}: ${net.diagnostics.join("; ") || "The bounded search did not complete this connection."}`);
    $("route-findings").append(li);
  }
  for (const finding of value.drc.findings) {
    $("route-findings").append(html("li", `${finding.code}: ${finding.message}`));
  }
  $("net").replaceChildren(Object.assign(document.createElement("option"), {value: "all", textContent: "All signals"}), ...value.routing.nets.map(net => Object.assign(document.createElement("option"), {value: net.net, textContent: net.net})));
  $("net").disabled = false;
  stage = "after"; syncStage(); draw();
}
function syncStage() {document.querySelectorAll("[data-stage]").forEach(button => button.setAttribute("aria-pressed", String(button.dataset.stage === stage)));}
function draw() {
  if (!result) return;
  const svg = $("board"), scene = result[stage], layer = $("layer").value, selected = $("net").value;
  svg.toggleAttribute("hidden", false); $("board-empty").hidden = true;
  const coordinates = scene.edges.flat();
  const minX = Math.min(...coordinates.map(p => p[0])), maxX = Math.max(...coordinates.map(p => p[0]));
  const minY = Math.min(...coordinates.map(p => p[1])), maxY = Math.max(...coordinates.map(p => p[1]));
  const margin = 2, width = maxX - minX + margin * 2, height = maxY - minY + margin * 2;
  svg.setAttribute("viewBox", `${minX - margin} ${minY - margin} ${width} ${height}`);
  svg.style.aspectRatio = `${width}/${height}`;
  const font = 11 * width / Math.max(260, svg.clientWidth);
  const visible = layers => layer === "all" || layers.includes(layer);
  const opacity = net => selected === "all" || selected === net ? 1 : .12;
  const color = name => name === "B.Cu" ? "#2f6d85" : "#ad4b28";
  const title = node("title", {}, `${scene.name}: ${stage === "before" ? "unrouted placement" : "computed routing"}. ${result.routing.metrics.unrouted_net_count} open signals in the result.`);
  const defs = node("defs"), pattern = node("pattern", {id: "keepout-hatch", width: 1, height: 1, patternUnits: "userSpaceOnUse", patternTransform: "rotate(45)"});
  pattern.append(node("line", {x1:0,y1:0,x2:0,y2:1,stroke:"#b3afa3","stroke-width":.12})); defs.append(pattern);
  svg.replaceChildren(title, defs);
  for (const [a,b] of scene.edges) svg.append(node("line", {x1:a[0],y1:a[1],x2:b[0],y2:b[1],stroke:"#737b70","stroke-width":.12}));
  for (const keepout of result.keepouts.filter(k => visible(k.layers))) {
    svg.append(node("polygon", {points:keepout.vertices.map(p=>p.join(",")).join(" "),fill:"url(#keepout-hatch)",stroke:"#77796f","stroke-width":.12,"stroke-dasharray":".4 .3"}));
  }
  const open = new Set(result.routing.nets.filter(n => !n.connected).map(n => n.net));
  const groups = new Map();
  for (const pad of scene.pads) {
    if (!pad.net) continue;
    if (!groups.has(pad.net)) groups.set(pad.net, []);
    groups.get(pad.net).push(pad);
  }
  for (const [name, pads] of groups) if (stage === "before" || open.has(name)) {
    for (const target of pads.slice(1)) svg.append(node("line", {x1:pads[0].center[0],y1:pads[0].center[1],x2:target.center[0],y2:target.center[1],stroke:"#77796f","stroke-width":.09,"stroke-dasharray":".45 .4",opacity:opacity(name)}));
  }
  for (const track of scene.tracks.filter(t=>visible([t.layer]))) svg.append(node("line", {x1:track.start[0],y1:track.start[1],x2:track.end[0],y2:track.end[1],stroke:color(track.layer),"stroke-width":track.width,"stroke-linecap":"round",opacity:opacity(track.net),"data-net":track.net,"data-layer":track.layer}));
  for (const pad of scene.pads.filter(p=>visible(p.layers))) {
    const polygon = node("polygon", {points:pad.polygon.map(p=>p.join(",")).join(" "),fill:"#252b29",opacity:opacity(pad.net)});
    polygon.append(node("title", {}, `${pad.reference}.${pad.pad} · ${pad.net}`)); svg.append(polygon);
  }
  for (const via of scene.vias.filter(v=>visible(v.layers))) {
    svg.append(node("circle", {cx:via.center[0],cy:via.center[1],r:via.size/2,fill:color("B.Cu"),opacity:opacity(via.net)}));
    svg.append(node("circle", {cx:via.center[0],cy:via.center[1],r:via.drill/2,fill:"#fffefa",opacity:opacity(via.net)}));
  }
  for (const footprint of scene.footprints) svg.append(node("text", {x:footprint.center[0],y:footprint.center[1]-2,"text-anchor":"middle",fill:"#252b29","font-size":font,"font-family":"Segoe UI,Arial,sans-serif"},footprint.reference));
  svg.append(node("text", {x:minX,y:maxY+1.4,fill:"#626962","font-size":font,"font-family":"Segoe UI,Arial,sans-serif"},`${maxX-minX} × ${maxY-minY} mm · viewed from above`));
}

$("route").addEventListener("click", run); $("stop").addEventListener("click", ()=>stop("Routing stopped. Source is preserved."));
$("layer").addEventListener("change", draw); $("net").addEventListener("change", draw);
document.querySelectorAll("[data-stage]").forEach(button=>button.addEventListener("click",()=>{stage=button.dataset.stage;syncStage();draw();}));
document.querySelectorAll("[data-case]").forEach(button=>button.addEventListener("click",()=>{
  if (!original) return;
  if (active) stop("Scenario changed. Route again.");
  $("source").value = scenarios[button.dataset.case].source(original, weave); revision++; clear();
  checks.schedule(0);
  $("scenario-description").textContent = scenarios[button.dataset.case].description;
  document.querySelectorAll("[data-case]").forEach(b=>b.setAttribute("aria-pressed",String(b===button)));
  run();
}));
$("source").addEventListener("input",()=>{revision++;clear();checks.schedule();document.querySelectorAll("[data-case]").forEach(b=>b.setAttribute("aria-pressed","false"));});
$("source").addEventListener("keydown",event=>{if((event.ctrlKey||event.metaKey)&&event.key==="Enter"){event.preventDefault();run();}});
new ResizeObserver(draw).observe($("board"));
try {
  const sources = await Promise.all(["board.copper", "weave.copper"].map(async name => {
    const response = await fetch(new URL(name, import.meta.url));
    if (!response.ok) throw new Error(`Example request failed (${response.status})`);
    return (await response.text()).replace(/\r\n?/g, "\n");
  }));
  [original, weave] = sources;
  $("source").value = scenarios.weave.source(original, weave); $("source").disabled=false; checks.schedule(0); start();
} catch(error){status(`Could not prepare the example: ${error.message}`,"fail");runtime("Example unavailable","failed");}
