"use strict";
const $ = id => document.getElementById(id);
const token = new URLSearchParams(location.hash.slice(1)).get("token");
const NS = "http://www.w3.org/2000/svg";
let accepted, preview = null, sourcePreview = null, selected = "", viewbox = null, drag = null, pan = null, busy = false;
let measurePoints=[], vertexPoints=[];
let jobPoll=null;
const mm = n => n / 1000000;
const scene = () => sourcePreview || preview || accepted;
function featureLabel(kind,name="") {
  const source=scene()?.mechanical_provenance?.features?.find(f=>f.kind===kind && f.name===name);
  return source ? `${source.profile ? "Imported (read-only): "+source.profile : "Project-owned"}; ${source.source.filename}:${source.source.line}` : "";
}
function status(message, error = false) { $("status").textContent = message; $("status").classList.toggle("error", error); }
async function api(path, body) {
  const response = await fetch(path, {method: body ? "POST" : "GET", headers: {
    "X-Copper-Token": token || "", ...(body ? {"Content-Type": "application/json"} : {})},
    ...(body ? {body: JSON.stringify(body)} : {})});
  const result = await response.json();
  if (!response.ok) {
    const error = new Error(result.error || "Editor request failed");
    error.status = response.status;
    throw error;
  }
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
  if (!scene()) return;
  const b = scene().bounds.map(mm), margin = 4;
  viewbox = [b[0]-margin,b[1]-margin,b[2]-b[0]+2*margin,b[3]-b[1]+2*margin];
  $("board").setAttribute("viewBox",viewbox.join(" "));
}
function render() {
  const s = scene(), svg = $("board"); if (!s) return; svg.replaceChildren();
  const c = s.outline.circle;
  const outline=c ? node("circle",{cx:mm(c.center[0]),cy:mm(c.center[1]),r:mm(c.radius_nm),class:"outline"}) :
    node("polygon",{points:points(s.outline.vertices),class:"outline"});
  outline.append(node("title",{},featureLabel("outline")));svg.append(outline);
  for (const cutout of s.outline.cutouts) {
    const n=node("polygon",{points:points(cutout.vertices),class:"cutout"});
    n.append(node("title",{},`${cutout.id}. ${featureLabel("cutout",cutout.id)}`));svg.append(n);
  }
  for (const h of s.holes) {
    if (h.head_clearance_radius_nm) svg.append(node("circle",{cx:mm(h.position[0]),cy:mm(h.position[1]),r:mm(h.head_clearance_radius_nm),class:"head-clearance"}));
    const n = node("circle",{cx:mm(h.position[0]),cy:mm(h.position[1]),r:mm(h.diameter_nm)/2,class:"hole"});
    n.append(node("title",{},`${h.id}: ${mm(h.diameter_nm)} mm NPTH. ${featureLabel("hole",h.id)}`)); svg.append(n);
  }
  for (const [items,cls] of [[s.regions,"region"],[s.keepouts,"keepout"]]) for (const k of items) {
    const n=node("polygon",{points:points(k.vertices),class:cls});
    n.append(node("title",{},`${k.name}. ${featureLabel(cls,k.name)}`));svg.append(n);
  }
  for (const k of s.copper_keepouts||[]) {
    const ring=vertices=>`M ${points(vertices).replaceAll(" "," L ")} Z`;
    const n=node("path",{d:[ring(k.vertices),...(k.holes||[]).map(ring)].join(" "),class:"copper-keepout","fill-rule":"evenodd"});
    n.append(node("title",{},`${k.name}: copper keepout on ${k.layers.join(", ")}. ${featureLabel("copper_keepout",k.name)}`));svg.append(n);
  }
  const visible = new Set(s.components.filter(c => $("side").value === "both" || c.side === $("side").value).map(c => c.reference));
  const relevant = new Set(s.components.find(c=>c.reference===selected)?.pads.map(p=>p.net).filter(Boolean)||[]);
  if ($("airwires").checked) for (const [edgeIndex,edge] of s.ratsnest.entries()) {
    if ($("net").value && edge.net !== $("net").value) continue;
    if (!$("zone-nets").checked && s.zone_nets.includes(edge.net)) continue;
    if ($("selected-only").checked && !relevant.has(edge.net)) continue;
    if ((edge.from.reference && !visible.has(edge.from.reference)) || (edge.to.reference && !visible.has(edge.to.reference))) continue;
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
  if (vertexPoints.length) svg.append(node("polyline",{points:vertexPoints.map(p=>p.join(",")).join(" "),class:"vertex-preview"}));
  if (measurePoints.length===2) {
    const [a,b]=measurePoints, distance=Math.hypot(b[0]-a[0],b[1]-a[1]);
    svg.append(node("line",{x1:a[0],y1:a[1],x2:b[0],y2:b[1],class:"measure-line"}),
      node("text",{x:(a[0]+b[0])/2,y:(a[1]+b[1])/2,class:"measure-text"},`${distance.toFixed(3)} mm`));
    $("measurement").textContent=`${distance.toFixed(6)} mm; ΔX ${(b[0]-a[0]).toFixed(6)}, ΔY ${(b[1]-a[1]).toFixed(6)} mm`;
  }
  if (!viewbox) resetFit(); else svg.setAttribute("viewBox",viewbox.join(" "));
  $("warnings").textContent = Object.entries(s.warnings).map(([key,value])=>`${key}: ${value}`).join("\n\n") || "No recorded footprint/omission warnings.";
  updateControls();
}
function updateControls() {
  const s = scene(), component = s?.components.find(c => c.reference===selected);
  const sourceReview=!!accepted?.source_review;
  const blocked = busy || !s || !!accepted?.source_stale || sourceReview;
  $("notice").textContent=accepted?.notice || "Loading source workspace…";
  $("reference").value = selected;
  $("apply").disabled = !preview || blocked; $("discard").disabled = !preview || blocked;
  const job=accepted?.placement_job;
  $("auto").disabled = blocked || !!preview || job?.status==="running";
  $("cancel-auto").disabled=busy || job?.status!=="running";
  $("job-status").hidden=!job;
  $("job-status").textContent=job ? `Auto-placement ${job.status}: ${job.progress.phase}, candidate ${Math.min(job.progress.candidate+1,job.progress.candidates)}/${job.progress.candidates}; ${job.elapsed_seconds}s / ${job.budget_seconds}s${job.error?" — "+job.error:""}` : "";
  $("job-profile").hidden=!job?.profile;$("profile-text").textContent=job?.profile?.top_cumulative||"";
  if (job?.status==="running" && jobPoll===null) jobPoll=setTimeout(pollJob,250);
  $("reload").disabled = busy;
  $("reload-source").disabled=busy || !accepted?.source_writable;
  $("save-source").disabled=busy || !sourceReview || !!accepted?.source_stale;
  $("discard-source").disabled=busy || !sourceReview || !!accepted?.source_stale;
  $("undo-source").disabled=blocked || !accepted?.can_source_undo;
  $("redo-source").disabled=blocked || !accepted?.can_source_redo;
  $("source-review").hidden=!sourceReview;
  $("source-diff").textContent=accepted?.source_review?.diff || "";
  $("persistent-controls").disabled=blocked || !!preview || !accepted?.source_writable;
  $("prepare-lock").disabled=blocked || !!preview || !component || !$("edit-locks").checked || !!component.profile_role;
  $("mechanical-controls").disabled=blocked || !!preview || !accepted?.capabilities.mechanical_edit;
  $("undo").disabled = blocked || !accepted?.can_undo; $("redo").disabled = blocked || !accepted?.can_redo;
  $("pending").hidden = !preview;
  $("move").disabled = !component || blocked || !!preview || component.session_locked || component.source_position_locked || component.source_rotation_locked;
  $("lock").disabled = !component || blocked || !!preview;
  if (component) {
    $("x").value = mm(component.position[0]); $("y").value = mm(component.position[1]);
    $("rotation").value = component.rotation; $("pose-side").value = component.side;
    $("lock").textContent = component.session_locked ? "Unlock temporary pose" : "Lock temporary pose";
    $("details").textContent = `${component.footprint}; ${component.value}. ${component.source_position_locked||component.source_rotation_locked ? "SOURCE LOCK. Enable explicit source-lock editing to change it. " : ""}${component.profile_role ? "Imported profile role: "+component.profile_role+" (read-only). " : ""}${component.macro ? "Rigid unit: "+component.macro+". " : ""}Allowed angles: ${component.allowed_orientations.join(", ")}.`;
  }
}
function populate() {
  for (const [id,values,label] of [["reference",accepted.components.map(c=>c.reference),"Select a component"],["net",accepted.nets,"All nets"]]) {
    const element = $(id), old = element.value; element.replaceChildren(new Option(label,""));
    for (const value of values) element.add(new Option(value,value)); element.value=old;
  }
  const features=$("feature"),old=features.value;features.replaceChildren(new Option("New feature",""));
  for (const [index,f] of (accepted.mechanical_features||[]).entries()) features.add(new Option(`${f.kind} ${f.name||f.shape} (line ${f.line})`,String(index)));
  features.value=old;
}
async function operation(action, fields={}, applyImmediately=false) {
  if (busy || !accepted) return; busy=true; updateControls(); status(`${action}: working…`);
  try {
    let result = await api("/api/operation",{action,revision:accepted.revision,...fields});
    if (result.preview && applyImmediately) {
      // The existing two-phase API validates both operations. Never apply using
      // an old revision: another editor may have changed the pending pose.
      accepted.revision=result.preview.revision;
      result=await api("/api/operation",{action:"apply",revision:result.preview.revision});
    }
    if (result.preview) {preview=result.preview; accepted.revision=preview.revision; status("Preview — inspect, then apply or discard. Source remains unchanged.");}
    else {accepted=result.scene;preview=null;sourcePreview=result.source_preview||null;populate();
      status(accepted.source_review ? "Source preview — inspect the exact diff, then Save or Discard." :
        accepted.outputs_stale ? "Source/session updated. Route, fill and manufacturing outputs are stale; rebuild them." : "Temporary session updated. Source remains unchanged.");}
  } catch (error) {
    preview=null;sourcePreview=null;
    // Recover the authoritative revision after conflicts and rejected poses.
    // Otherwise every subsequent edit can fail until the page is reloaded.
    try {accepted=await api("/api/scene");populate();} catch (_) { /* Keep the original failure visible. */ }
    status(`${error.message}${error.status===409 && !accepted?.source_stale ? " Scene reloaded; retry your edit." : ""}`,true);
  }
  finally {busy=false;render();}
}
function svgPoint(event) {
  const p=new DOMPoint(event.clientX,event.clientY).matrixTransform($("board").getScreenCTM().inverse());
  return [p.x,p.y];
}
async function pollJob() {
  jobPoll=null;
  if (busy) {jobPoll=setTimeout(pollJob,250);return;}
  try {
    accepted=await api("/api/scene");preview=accepted.pending_preview;
    if(accepted.placement_job?.status==="completed") status("Auto-placement preview ready — apply or discard.");
    if(accepted.placement_job?.error) status(accepted.placement_job.error,true);
    render();
  } catch(error) {status(error.message,true);}
}
function startDrag(event,component,g) {
  if (event.button!==0 || busy || drag || pan || accepted?.source_stale || accepted?.source_review || $("measure").checked || $("draw-vertices").checked) return;
  selected=component.reference; $("reference").value=selected; syncLockChecks(); updateControls();
  if (preview) {status("Apply or discard the pending preview before moving another component.");return;}
  const start=svgPoint(event);
  if (component.session_locked || component.source_position_locked || component.source_rotation_locked) {render();return;}
  const members=scene().components.filter(c=>c.reference===component.reference || (component.macro && c.macro===component.macro));
  drag={component,g,start,last:start,pointerId:event.pointerId,members:new Set(members.map(c=>c.reference))}; g.classList.add("dragging"); $("board").setPointerCapture(event.pointerId);
}
$("board").addEventListener("contextmenu",event=>event.preventDefault());
$("board").addEventListener("pointerdown",event=>{
  if (event.button!==2 || !viewbox || drag || pan) return;
  event.preventDefault();
  const inverse=$("board").getScreenCTM().inverse();
  const start=new DOMPoint(event.clientX,event.clientY).matrixTransform(inverse);
  pan={start,inverse,viewbox:[...viewbox],pointerId:event.pointerId};
  $("board").classList.add("panning");
  $("board").setPointerCapture(event.pointerId);
});
$("board").addEventListener("pointermove",event=>{
  if (pan && event.pointerId===pan.pointerId) {
    const p=new DOMPoint(event.clientX,event.clientY).matrixTransform(pan.inverse);
    viewbox=[pan.viewbox[0]+pan.start.x-p.x,pan.viewbox[1]+pan.start.y-p.y,...pan.viewbox.slice(2)];
    $("board").setAttribute("viewBox",viewbox.join(" "));return;
  }
  if (drag && event.pointerId!==drag.pointerId) return;
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
  if (pan && event.pointerId===pan.pointerId) {endPan();return;}
  if (!drag || event.pointerId!==drag.pointerId) return; const d=drag;drag=null;
  if ($("board").hasPointerCapture(event.pointerId)) $("board").releasePointerCapture(event.pointerId);
  if (Math.hypot(d.last[0]-d.start[0],d.last[1]-d.start[1])<.01) {render();return;}
  const snap=Number($("snap").value); if (!Number.isFinite(snap)||snap<=0) {render();status("Snap must be positive",true);return;}
  const x=Math.round((mm(d.component.position[0])+d.last[0]-d.start[0])/snap)*snap;
  const y=Math.round((mm(d.component.position[1])+d.last[1]-d.start[1])/snap)*snap;
  operation("move",{reference:d.component.reference,x_nm:Math.round(x*1e6),y_nm:Math.round(y*1e6),rotation:d.component.rotation,side:d.component.side},!$("preview-drags").checked);
});
function endPan() {
  const id=pan?.pointerId;pan=null;$("board").classList.remove("panning");
  if (id!==undefined && $("board").hasPointerCapture(id)) $("board").releasePointerCapture(id);
}
function cancelGesture() {endPan();drag=null;render();}
$("board").addEventListener("pointercancel",cancelGesture);
$("board").addEventListener("lostpointercapture",()=>{if(drag||pan) cancelGesture();});
$("reference").addEventListener("change",()=>{selected=$("reference").value;render();});
$("reference").addEventListener("change",syncLockChecks);
$("edit-locks").onchange=()=>{syncLockChecks();updateControls();};
for (const id of ["side","net","airwires","selected-only","zone-nets"]) $(id).addEventListener("change",render);
for (const [id,action] of [["apply","apply"],["discard","discard"],["undo","undo"],["redo","redo"]]) $(id).onclick=()=>operation(action);
$("auto").onclick=()=>operation("start_auto_place",{budget_seconds:Number($("placement-budget").value)});
$("cancel-auto").onclick=()=>operation("cancel_auto_place");
$("lock").onclick=()=>operation("lock",{reference:selected,locked:!scene().components.find(c=>c.reference===selected).session_locked});
$("move").onclick=()=>operation("move",{reference:selected,x_nm:Math.round(Number($("x").value)*1e6),y_nm:Math.round(Number($("y").value)*1e6),rotation:$("rotation").value,side:$("pose-side").value});
$("fit").onclick=()=>{resetFit();render();};
function zoom(factor,anchor) {
  if (!viewbox || drag || pan) return;
  const [x,y,w,h]=viewbox;
  // Keep the cursor's world coordinate fixed and bound the zoom scale.
  factor=Math.max(0.1/w,Math.min(100000/w,factor));
  const [ax,ay]=anchor||[x+w/2,y+h/2];
  viewbox=[ax+(x-ax)*factor,ay+(y-ay)*factor,w*factor,h*factor];
  $("board").setAttribute("viewBox",viewbox.join(" "));
}
$("board").addEventListener("wheel",event=>{
  event.preventDefault();
  const unit=event.deltaMode===1 ? 16 : event.deltaMode===2 ? $("board").clientHeight : 1;
  zoom(Math.exp(Math.max(-1,Math.min(1,event.deltaY*unit*.0015))),viewbox ? svgPoint(event) : undefined);
},{passive:false});
$("zoom-in").onclick=()=>zoom(.8); $("zoom-out").onclick=()=>zoom(1.25);
async function reloadScene() {
  if (busy) return;busy=true;updateControls();
  try {
    const s=await api("/api/scene");preview=null;sourcePreview=null;
    accepted=s;populate();render();
    status(s.source_stale ? "Source changed externally; use Reload source (discard session) before editing." :
      `${s.board}: ${s.components.length} components; ${s.ratsnest.length} airwires. ${s.placement_legal ? "Placement legal." : "Initial inspection placement needs legalization."} ${Object.keys(s.warnings).length ? "Footprint/omission warnings are present in scene data." : ""}`,s.source_stale);
  } catch(error) {status(error.message,true);}
  finally {busy=false;updateControls();}
}
$("reload").onclick=reloadScene;
function syncLockChecks() {
  const c=scene()?.components.find(c=>c.reference===selected);
  for (const [id,key] of [["position-lock","source_position_locked"],["rotation-lock","source_rotation_locked"],["side-lock","source_side_locked"]]) $(id).checked=!!c?.[key];
}
$("prepare-lock").onclick=()=>operation("prepare_lock",{reference:selected,
  x_nm:Math.round(Number($("x").value)*1e6),y_nm:Math.round(Number($("y").value)*1e6),
  rotation:Number($("rotation").value),side:$("pose-side").value,
  locks:["position","rotation","side"].filter(k=>$(k+"-lock").checked)});
$("save-source").onclick=()=>operation("save_source",{review_id:accepted.source_review.id});
for (const [id,action] of [["discard-source","discard_source"],["undo-source","undo_source"],["redo-source","redo_source"]]) $(id).onclick=()=>operation(action);
$("reload-source").onclick=()=>{if (confirm("Discard temporary placement, previews and history, and reload the current source?")) operation("reload_source");};
const featureDefaults={
  "outline:rectangle":{width:"40mm",height:"30mm",origin:"(0mm,0mm)"},
  "outline:circle":{diameter:"50mm",center:"(25mm,25mm)"},
  "outline:polygon":{vertices:"[(0mm,0mm), (40mm,0mm), (40mm,30mm), (0mm,30mm)]"},
  "hole:":{position:"(3mm,3mm)",diameter:"2.2mm",head_clearance_radius:"0mm"},
  "cutout:polygon":{vertices:"[(27mm,18mm), (33mm,18mm), (33mm,23mm), (27mm,23mm)]"},
  "keepout:rectangle":{width:"4mm",height:"4mm",origin:"(10mm,10mm)",side:"front"},
  "keepout:polygon":{vertices:"[(10mm,10mm), (14mm,10mm), (14mm,14mm), (10mm,14mm)]",side:"front"},
  "copper_keepout:rectangle":{width:"4mm",height:"4mm",origin:"(10mm,10mm)",layers:'"F.Cu"',block_tracks:"true",block_vias:"true",block_pads:"true",block_zones:"true",block_footprints:"false"},
  "copper_keepout:polygon":{vertices:"[(10mm,10mm), (14mm,10mm), (14mm,14mm), (10mm,14mm)]",layers:'"F.Cu"'},
  "rules:":{minimum_clearance:"0.2mm",minimum_track_width:"0.2mm"}
};
function featureInputs(parameters) {
  parameters={...parameters};
  if ($("feature-kind").value==="keepout" && !("maximum_height" in parameters)) parameters.maximum_height="";
  const container=$("feature-parameters");container.replaceChildren();
  for (const [key,value] of Object.entries(parameters)) {
    const label=document.createElement("label");label.textContent=key;
    const input=document.createElement(key==="vertices"?"textarea":"input");input.id="feature-param-"+key;input.dataset.property=key;input.value=value;label.append(input);container.append(label);
  }
}
function featureMode() {
  const kind=$("feature-kind").value;
  if (["hole","rules"].includes(kind)) $("feature-shape").value="";
  else if (kind==="cutout") $("feature-shape").value="polygon";
  else if (kind.includes("keepout") && $("feature-shape").value==="circle") $("feature-shape").value="rectangle";
  $("feature-name").disabled=["outline","rules"].includes(kind);
  if ($("feature-name").disabled) $("feature-name").value="";
  featureInputs(featureDefaults[kind+":"+$("feature-shape").value]||{});vertexPoints=[];render();
}
$("feature-kind").onchange=featureMode;$("feature-shape").onchange=featureMode;
$("feature").onchange=()=>{
  const f=accepted.mechanical_features?.[Number($("feature").value)];
  if ($("feature").value==="" || !f) {featureMode();return;}
  $("feature-kind").value=f.kind;$("feature-name").value=f.name;$("feature-shape").value=f.shape;
  $("feature-name").disabled=["outline","rules"].includes(f.kind);
  featureInputs(f.parameters);vertexPoints=[];render();
};
function featureRequest(remove=false) {
  const parameters=Object.fromEntries([...$("feature-parameters").querySelectorAll("[data-property]")].filter(n=>n.value.trim()).map(n=>[n.dataset.property,n.value.trim()]));
  operation("prepare_mechanical",{kind:$("feature-kind").value,name:$("feature-name").value,shape:$("feature-shape").value,parameters:remove?{}:parameters,remove});
}
$("prepare-feature").onclick=()=>featureRequest();$("remove-feature").onclick=()=>featureRequest(true);
function snappedPoint(event) {
  const step=Number($("snap").value),p=svgPoint(event);
  if (!Number.isFinite(step)||step<=0) throw new Error("Snap must be positive");
  return p.map(v=>Math.round(Math.round(v/step)*step*1e6)/1e6);
}
$("board").addEventListener("click",event=>{
  if (busy || preview || accepted?.source_review) return;
  if (!$("measure").checked && !$("draw-vertices").checked) return;
  try {
    const p=snappedPoint(event);
    if ($("measure").checked) {if(measurePoints.length===2) measurePoints=[];measurePoints.push(p);}
    else {
      const field=$("feature-param-vertices");
      if (!field) throw new Error("Choose a polygon feature before adding vertices");
      vertexPoints.push(p);field.value="["+vertexPoints.map(v=>`(${v[0]}mm,${v[1]}mm)`).join(", ")+"]";
    }
    render();
  } catch(error) {status(error.message,true);}
});
$("board").addEventListener("keydown",event=>{
  if (event.key==="Escape") {cancelGesture();measurePoints=[];vertexPoints=[];render();return;}
  const delta={ArrowLeft:[-1,0],ArrowRight:[1,0],ArrowUp:[0,-1],ArrowDown:[0,1]}[event.key];
  if (!delta || busy || preview || accepted?.source_review) return;
  const c=scene()?.components.find(c=>c.reference===selected);
  if (!c || c.session_locked || c.source_position_locked) return;
  event.preventDefault();const step=Math.round(Number($("snap").value)*1e6)*(event.shiftKey?10:1);
  operation("move",{reference:selected,x_nm:c.position[0]+delta[0]*step,y_nm:c.position[1]+delta[1]*step,rotation:c.rotation,side:c.side},true);
});
featureMode();
updateControls();reloadScene();
