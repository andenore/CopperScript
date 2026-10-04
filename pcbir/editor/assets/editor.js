"use strict";
const $ = id => document.getElementById(id);
const token = new URLSearchParams(location.hash.slice(1)).get("token");
const NS = "http://www.w3.org/2000/svg";
let accepted, preview = null, selected = "", viewbox = null, drag = null, busy = false;
const mm = n => n / 1000000;
const scene = () => preview || accepted;
function status(message, error = false) { $("status").textContent = message; $("status").classList.toggle("error", error); }
async function api(path, body) {
  const response = await fetch(path, {method: body ? "POST" : "GET", headers: {
    "X-Copper-Token": token || "", ...(body ? {"Content-Type": "application/json"} : {})},
    ...(body ? {body: JSON.stringify(body)} : {})});
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || "Editor request failed");
  return result;
}
function node(tag, attrs = {}, text) {
  const n = document.createElementNS(NS, tag);
  for (const [key, value] of Object.entries(attrs)) n.setAttribute(key, value);
  if (text !== undefined) n.textContent = text;
  return n;
}
const points = values => values.map(p => `${mm(p[0])},${mm(p[1])}`).join(" ");
function shape(data, cls) {
  const {spine, radius_nm} = data, r = mm(radius_nm);
  if (spine.length === 1) return node("circle", {cx:mm(spine[0][0]),cy:mm(spine[0][1]),r,class:cls});
  const attrs = {points:points(spine),class:cls,"stroke-width":2*r,"stroke-linejoin":"round","stroke-linecap":"round"};
  if (spine.length === 2) {attrs.fill="none"; return node("polyline",attrs);}
  return node("polygon",attrs);
}
function resetFit() {
  const b = scene().bounds.map(mm), margin = 4;
  viewbox = [b[0]-margin,b[1]-margin,b[2]-b[0]+2*margin,b[3]-b[1]+2*margin];
  $("board").setAttribute("viewBox",viewbox.join(" "));
}
function render() {
  const s = scene(), svg = $("board"); svg.replaceChildren();
  const c = s.outline.circle;
  svg.append(c ? node("circle",{cx:mm(c.center[0]),cy:mm(c.center[1]),r:mm(c.radius_nm),class:"outline"}) :
    node("polygon",{points:points(s.outline.vertices),class:"outline"}));
  for (const cutout of s.outline.cutouts) svg.append(node("polygon",{points:points(cutout.vertices),class:"cutout"}));
  for (const h of s.holes) {
    if (h.head_clearance_radius_nm) svg.append(node("circle",{cx:mm(h.position[0]),cy:mm(h.position[1]),r:mm(h.head_clearance_radius_nm),class:"head-clearance"}));
    const n = node("circle",{cx:mm(h.position[0]),cy:mm(h.position[1]),r:mm(h.diameter_nm)/2,class:"hole"});
    n.append(node("title",{},`${h.id}: ${mm(h.diameter_nm)} mm NPTH`)); svg.append(n);
  }
  for (const [items,cls] of [[s.regions,"region"],[s.keepouts,"keepout"]]) for (const k of items)
    svg.append(node("polygon",{points:points(k.vertices),class:cls}));
  const visible = new Set(s.components.filter(c => $("side").value === "both" || c.side === $("side").value).map(c => c.reference));
  const relevant = new Set(s.ratsnest.filter(e => e.from.reference===selected || e.to.reference===selected).map(e=>e.net));
  if ($("airwires").checked) for (const [edgeIndex,edge] of s.ratsnest.entries()) {
    if ($("net").value && edge.net !== $("net").value) continue;
    if (!$("zone-nets").checked && s.zone_nets.includes(edge.net)) continue;
    if ($("selected-only").checked && !relevant.has(edge.net)) continue;
    if (!visible.has(edge.from.reference) || !visible.has(edge.to.reference)) continue;
    const n = node("line",{x1:mm(edge.from.position[0]),y1:mm(edge.from.position[1]),x2:mm(edge.to.position[0]),y2:mm(edge.to.position[1]),class:"airwire","data-edge":edgeIndex});
    n.append(node("title",{},edge.net)); svg.append(n);
  }
  for (const component of s.components) {
    if (!visible.has(component.reference)) continue;
    const locked = component.source_position_locked || component.source_rotation_locked || component.session_locked;
    const g = node("g",{class:`component ${component.side}${selected === component.reference ? " selected" : ""}${locked ? " locked" : ""}`,"data-reference":component.reference});
    g.append(node("polygon",{points:points(component.courtyard),class:"courtyard"}),node("polygon",{points:points(component.body),class:"body"}));
    for (const pad of component.pads) {
      if (pad.kind !== "non_plated_through_hole") {
        const n = shape(pad.shape,"pad");n.append(node("title",{},`${component.reference}.${pad.number}: ${pad.net || "no net"}`));g.append(n);
      }
      if (pad.drill) g.append(shape(pad.drill,"drill"));
    }
    g.append(node("text",{x:mm(component.position[0]),y:mm(component.position[1])-1.4,class:"ref"},component.reference));
    g.addEventListener("pointerdown",event=>startDrag(event,component,g)); svg.append(g);
  }
  if (!viewbox) resetFit(); else svg.setAttribute("viewBox",viewbox.join(" "));
  $("warnings").textContent = Object.entries(s.warnings).map(([key,value])=>`${key}: ${value}`).join("\n\n") || "No recorded footprint/omission warnings.";
  updateControls();
}
function updateControls() {
  const s = scene(), component = s.components.find(c => c.reference===selected);
  $("reference").value = selected;
  $("apply").disabled = !preview || busy; $("discard").disabled = !preview || busy;
  $("auto").disabled = busy; $("undo").disabled = busy || !accepted.can_undo; $("redo").disabled = busy || !accepted.can_redo;
  $("move").disabled = !component || busy || !!preview || component.session_locked || component.source_position_locked || component.source_rotation_locked;
  $("lock").disabled = !component || busy || !!preview;
  if (component) {
    $("x").value = mm(component.position[0]); $("y").value = mm(component.position[1]);
    $("rotation").value = component.rotation; $("pose-side").value = component.side;
    $("lock").textContent = component.session_locked ? "Unlock temporary pose" : "Lock temporary pose";
    $("details").textContent = `${component.footprint}; ${component.value}. ${component.source_position_locked||component.source_rotation_locked ? "SOURCE LOCK (read-only). " : ""}${component.macro ? "Rigid unit: "+component.macro+". " : ""}Allowed angles: ${component.allowed_orientations.join(", ")}.`;
  }
}
function populate() {
  for (const [id,values,label] of [["reference",accepted.components.map(c=>c.reference),"Select a component"],["net",accepted.nets,"All nets"]]) {
    const element = $(id), old = element.value; element.replaceChildren(new Option(label,""));
    for (const value of values) element.add(new Option(value,value)); element.value=old;
  }
}
async function operation(action, fields={}) {
  if (busy) return; busy=true; updateControls(); status(`${action}: working…`);
  try {
    const result = await api("/api/operation",{action,revision:accepted.revision,...fields});
    if (result.preview) {preview=result.preview; accepted.revision=preview.revision; status("Preview — inspect, then apply or discard. Source remains unchanged.");}
    else {accepted=result.scene;preview=null;status("Temporary session updated. Source remains unchanged.");}
  } catch (error) {preview=null;status(error.message,true);}
  finally {busy=false;render();}
}
function svgPoint(event) {
  const p=new DOMPoint(event.clientX,event.clientY).matrixTransform($("board").getScreenCTM().inverse());
  return [p.x,p.y];
}
function startDrag(event,component,g) {
  if (busy || preview) return;
  selected=component.reference; $("reference").value=selected; updateControls();
  const start=svgPoint(event);
  if (component.session_locked || component.source_position_locked || component.source_rotation_locked) {render();return;}
  const members=scene().components.filter(c=>c.reference===component.reference || (component.macro && c.macro===component.macro));
  drag={component,g,start,last:start,members:new Set(members.map(c=>c.reference))}; g.classList.add("dragging"); $("board").setPointerCapture(event.pointerId);
}
$("board").addEventListener("pointermove",event=>{
  if (!drag) return; drag.last=svgPoint(event);
  const dx=drag.last[0]-drag.start[0],dy=drag.last[1]-drag.start[1];
  for (const g of $("board").querySelectorAll(".component")) if (drag.members.has(g.dataset.reference)) g.setAttribute("transform",`translate(${dx} ${dy})`);
  // Live airwire geometry during drag; exact island/MST recomputation is server-side on release.
  const wires=$("board").querySelectorAll(".airwire"),s=scene();
  for (const n of wires) {
    const e=s.ratsnest[Number(n.dataset.edge)];
    if (!e) continue;
    for (const [end,key] of [["from","1"],["to","2"]]) {
      const shift=drag.members.has(e[end].reference);
      n.setAttribute("x"+key,mm(e[end].position[0])+(shift?dx:0));n.setAttribute("y"+key,mm(e[end].position[1])+(shift?dy:0));
    }
  }
});
$("board").addEventListener("pointerup",event=>{
  if (!drag) return; const d=drag;drag=null;
  if (Math.hypot(d.last[0]-d.start[0],d.last[1]-d.start[1])<.01) {render();return;}
  const snap=Number($("snap").value); if (!Number.isFinite(snap)||snap<=0) {render();status("Snap must be positive",true);return;}
  const x=Math.round((mm(d.component.position[0])+d.last[0]-d.start[0])/snap)*snap;
  const y=Math.round((mm(d.component.position[1])+d.last[1]-d.start[1])/snap)*snap;
  operation("move",{reference:d.component.reference,x_nm:Math.round(x*1e6),y_nm:Math.round(y*1e6),rotation:d.component.rotation,side:d.component.side});
});
$("board").addEventListener("pointercancel",()=>{drag=null;render();});
$("reference").addEventListener("change",()=>{selected=$("reference").value;render();});
for (const id of ["side","net","airwires","selected-only","zone-nets"]) $(id).addEventListener("change",render);
for (const [id,action] of [["auto","auto_place"],["apply","apply"],["discard","discard"],["undo","undo"],["redo","redo"]]) $(id).onclick=()=>operation(action);
$("lock").onclick=()=>operation("lock",{reference:selected,locked:!scene().components.find(c=>c.reference===selected).session_locked});
$("move").onclick=()=>operation("move",{reference:selected,x_nm:Math.round(Number($("x").value)*1e6),y_nm:Math.round(Number($("y").value)*1e6),rotation:$("rotation").value,side:$("pose-side").value});
$("fit").onclick=()=>{resetFit();render();};
function zoom(factor) {const [x,y,w,h]=viewbox;viewbox=[x+w*(1-factor)/2,y+h*(1-factor)/2,w*factor,h*factor];render();}
$("zoom-in").onclick=()=>zoom(.8); $("zoom-out").onclick=()=>zoom(1.25);
api("/api/scene").then(s=>{
  accepted=s;populate();render();status(`${s.board}: ${s.components.length} components; ${s.ratsnest.length} airwires. ${s.placement_legal ? "Placement legal." : "Initial inspection placement needs legalization."} ${Object.keys(s.warnings).length ? "Footprint/omission warnings are present in scene data." : ""}`);
}).catch(error=>status(error.message,true));
