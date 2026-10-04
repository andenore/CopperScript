"use strict";
const $ = id => document.getElementById(id);
const token = new URLSearchParams(location.hash.slice(1)).get("token");
const vscodeHost=typeof acquireVsCodeApi==="function" ? acquireVsCodeApi() : null;
const hostRequests=new Map();let hostRequestId=0;
if(vscodeHost) window.addEventListener("message",event=>{
  const message=event.data;
  if(message?.type==="refresh") {reloadScene();return;}
  if(message?.type==="selection" && scene()?.components.some(c=>c.reference===message.reference)) {
    selected=message.reference;render();syncLockChecks();return;
  }
  const pending=hostRequests.get(message?.id);if(!pending) return;
  hostRequests.delete(message.id);clearTimeout(pending.timeout);
  if(message.error) pending.reject(new Error(message.error));else pending.resolve(message.result);
});
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
  if(vscodeHost) return new Promise((resolve,reject)=>{
    const id=++hostRequestId;
    const timeout=setTimeout(()=>{hostRequests.delete(id);reject(new Error("Document host timed out"));},60000);
    hostRequests.set(id,{resolve,reject,timeout});vscodeHost.postMessage({id,path,body});
  });
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
  const s=scene(),liveSvg=$("board");if(!s)return;
  const focused=document.activeElement?.closest?.(".component")?.dataset.reference;
  const svg=node("g"); // Build off-document; one DOM replacement avoids layout churn.
  const c = s.outline.circle;
  let pathData="";
  if(s.outline.path) {
    pathData=`M ${s.outline.path[0].start.map(mm).join(" ")}`;
    for(const segment of s.outline.path) pathData+=segment.kind==="line" ? ` L ${segment.end.map(mm).join(" ")}` :
      ` A ${mm(segment.radius_nm)} ${mm(segment.radius_nm)} 0 0 ${segment.sweep?1:0} ${segment.end.map(mm).join(" ")}`;
    pathData+=" Z";
  }
  const outline=s.outline.path ? node("path",{d:pathData,class:"outline"}) : c ? node("circle",{cx:mm(c.center[0]),cy:mm(c.center[1]),r:mm(c.radius_nm),class:"outline"}) :
    node("polygon",{points:points(s.outline.vertices),class:"outline"});
  outline.append(node("title",{},featureLabel("outline")));svg.append(outline);
  for(const d of s.datums||[]) {
    const [x,y]=d.position.map(mm),g=node("g",{class:"datum","pointer-events":"none"});
    g.append(node("line",{x1:x-.4,y1:y,x2:x+.4,y2:y}),node("line",{x1:x,y1:y-.4,x2:x,y2:y+.4}),node("text",{x:x+.5,y:y-.3},d.id),node("title",{},featureLabel("datum",d.id)));svg.append(g);
  }
  for(const e of s.boundary_edges||[]) {
    const g=node("g",{class:"named-edge","pointer-events":"none"});
    g.append(node("line",{x1:mm(e.start[0]),y1:mm(e.start[1]),x2:mm(e.end[0]),y2:mm(e.end[1])}),
      node("text",{x:mm(e.start[0]+e.end[0])/2,y:mm(e.start[1]+e.end[1])/2},e.id));svg.append(g);
  }
  for(const a of s.attachments||[]) svg.append(node("circle",{cx:mm(a.position[0]),cy:mm(a.position[1]),r:.4,class:"attachment-anchor","pointer-events":"none"}));
  for(const [key,label] of [["body_overhangs","BODY ONLY"],["assembly_envelopes","HEIGHT LIMIT"],["assembly_access","TOOL ACCESS"]]) for(const region of s[key]||[]) {
    if(region.side && $("side").value!=="both" && region.side!==$("side").value)continue;
    const n=node("polygon",{points:points(region.vertices),class:key});
    n.append(node("title",{},`${label}: ${region.id}; ${region.reason||region.purpose||mm(region.maximum_height_nm)+" mm"}`));svg.append(n);
  }
  for (const cutout of s.outline.cutouts) {
    const n=node("polygon",{points:points(cutout.vertices),class:"cutout"});
    n.append(node("title",{},`${cutout.id}. ${featureLabel("cutout",cutout.id)}`));svg.append(n);
  }
  for (const h of s.holes) {
    if (h.head_clearance_radius_nm) svg.append(node("circle",{cx:mm(h.position[0]),cy:mm(h.position[1]),r:mm(h.head_clearance_radius_nm),class:"head-clearance"}));
    const n = node("circle",{cx:mm(h.position[0]),cy:mm(h.position[1]),r:mm(h.diameter_nm)/2,class:"hole"});
    n.append(node("title",{},`${h.id}: ${mm(h.diameter_nm)} mm NPTH. ${featureLabel("hole",h.id)}`)); svg.append(n);
  }
  for(const slot of s.slots||[]) {
    const n=shape({spine:[slot.start,slot.end],radius_nm:slot.width_nm/2},"mechanical-slot");
    n.append(node("title",{},`NPTH slot ${slot.id}; tool width ${mm(slot.width_nm)} mm`));svg.append(n);
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
  const netVisible = net => (!$("net").value || net === $("net").value) &&
    (!$("selected-only").checked || relevant.has(net)) &&
    ($("power-filter").value==="all" || (s.power_nets||[]).includes(net) === ($("power-filter").value==="power"));
  const overlay=s.routed_overlay;
  $("overlay-controls").hidden=!overlay;
  if (overlay) {
    $("overlay-status").textContent=`${overlay.notice}. ${overlay.connectivity_credit ? "Matching native connectivity evidence." : "No plane connectivity credit."}`;
    $("overlay-status").classList.toggle("error",overlay.stale);
    const layerVisible = layer => (!$("copper-layer").value || layer===$("copper-layer").value) &&
      ($("side").value==="both" || layer!==($("side").value==="front"?"B.Cu":"F.Cu"));
    const group=node("g",{class:`routed-overlay${overlay.stale?" stale":""}`,"pointer-events":"none"});
    const colors={"F.Cu":"#ef7777","B.Cu":"#699eed","In1.Cu":"#d4bd69","In2.Cu":"#bb80dc","In3.Cu":"#72bda0","In4.Cu":"#df8db2"};
    if ($("filled-zones").checked) for (const f of overlay.native.fills) {
      if (!netVisible(f.net) || !layerVisible(f.layer)) continue;
      const ring=r=>`M ${points(r).replaceAll(" "," L ")} Z`;
      const n=node("path",{d:[ring(f.outer),...f.holes.map(ring)].join(" "),"fill-rule":"evenodd",fill:colors[f.layer]||"#a5b9c7",class:"zone-fill"});
      n.append(node("title",{},`${f.net} actual filled copper on ${f.layer}`));group.append(n);
    }
    if ($("routed-tracks").checked) {
      for (const t of overlay.native.tracks) {
        if (!netVisible(t.net) || !layerVisible(t.layer)) continue;
        const n=node("line",{x1:mm(t.start[0]),y1:mm(t.start[1]),x2:mm(t.end[0]),y2:mm(t.end[1]),stroke:colors[t.layer]||"#a5b9c7","stroke-width":mm(t.width_nm),class:"routed-track"});
        n.append(node("title",{},`${t.net} on ${t.layer}`));group.append(n);
      }
      for (const v of overlay.native.vias) {
        const layers=overlay.native.layers, a=layers.indexOf(v.from_layer),b=layers.indexOf(v.to_layer);
        if (!netVisible(v.net) || !layers.slice(Math.min(a,b),Math.max(a,b)+1).some(layerVisible)) continue;
        group.append(node("circle",{cx:mm(v.position[0]),cy:mm(v.position[1]),r:mm(v.size_nm)/2,class:"routed-via"}),
                     node("circle",{cx:mm(v.position[0]),cy:mm(v.position[1]),r:mm(v.drill_nm)/2,class:"drill"}));
      }
    }
    if ($("native-opens").checked) for (const item of overlay.evidence.remaining) {
      const positions=(item.items||[]).map(i=>i.pos).filter(p=>p && Number.isFinite(p.x) && Number.isFinite(p.y));
      for (const p of positions) group.append(node("circle",{cx:p.x,cy:p.y,r:.6,class:"native-open"}));
      if (positions.length===2) group.append(node("line",{x1:positions[0].x,y1:positions[0].y,x2:positions[1].x,y2:positions[1].y,class:"native-open"}));
    }
    svg.append(group);
  }
  if ($("airwires").checked) for (const [edgeIndex,edge] of s.ratsnest.entries()) {
    if (!netVisible(edge.net)) continue;
    if (!$("zone-nets").checked && s.zone_nets.includes(edge.net)) continue;
    if ($("selected-only").checked && !relevant.has(edge.net)) continue;
    if ((edge.from.reference && !visible.has(edge.from.reference)) || (edge.to.reference && !visible.has(edge.to.reference))) continue;
    const n = node("line",{x1:mm(edge.from.position[0]),y1:mm(edge.from.position[1]),x2:mm(edge.to.position[0]),y2:mm(edge.to.position[1]),class:"airwire","data-edge":edgeIndex});
    n.append(node("title",{},edge.net)); svg.append(n);
    if ($("net-labels").checked) svg.append(node("text",{x:mm(edge.from.position[0]+edge.to.position[0])/2,y:mm(edge.from.position[1]+edge.to.position[1])/2,class:"net-label"},`${edge.net} ${(Math.sqrt(Number(edge.distance_squared_nm))/1e6).toFixed(2)} mm`));
  }
  $("net-cost-summary").hidden=!$("net-costs").checked;
  $("net-cost-summary").textContent=Object.entries(s.net_costs||{}).filter(([net])=>netVisible(net)).map(([net,cost])=>`${net}: ${cost.airwires} airwires, ${mm(cost.length_nm).toFixed(2)} mm MST estimate`).join("\n")||"No remaining airwires in this filter.";
  for (const component of s.components) {
    if (!visible.has(component.reference)) continue;
    const locked = component.source_position_locked || component.source_rotation_locked || component.session_locked;
    const g = node("g",{class:`component ${component.side}${selected === component.reference ? " selected" : ""}${locked ? " locked" : ""}`,"data-reference":component.reference,
      role:"button",tabindex:selected===component.reference || (!selected && component.reference===s.components[0]?.reference)?"0":"-1",
      "aria-pressed":selected===component.reference?"true":"false",
      "aria-label":`${component.reference}, ${component.value||component.footprint}, ${component.side}, X ${mm(component.position[0])} mm, Y ${mm(component.position[1])} mm${locked?", pose constraints present":""}`});
    g.append(node("polygon",{points:points(component.courtyard),class:"courtyard"}),node("polygon",{points:points(component.body),class:"body"}));
    for (const pad of component.pads) {
      if (pad.kind !== "non_plated_through_hole") {
        const n = shape(pad.shape,"pad");n.append(node("title",{},`${component.reference}.${pad.number}: ${pad.net || "no net"}`));g.append(n);
      }
      if (pad.drill) g.append(shape(pad.drill,"drill"));
    }
    g.append(node("text",{x:mm(component.position[0]),y:mm(component.position[1])-1.4,class:"ref"},component.reference));
    g.addEventListener("pointerdown",event=>startDrag(event,component,g)); svg.append(g);
    g.addEventListener("keydown",event=>{if(["Enter"," "].includes(event.key)){event.preventDefault();selected=component.reference;render();syncLockChecks();}});
  }
  if (vertexPoints.length) svg.append(node("polyline",{points:vertexPoints.map(p=>p.join(",")).join(" "),class:"vertex-preview"}));
  if (measurePoints.length===2) {
    const [a,b]=measurePoints, distance=Math.hypot(b[0]-a[0],b[1]-a[1]);
    svg.append(node("line",{x1:a[0],y1:a[1],x2:b[0],y2:b[1],class:"measure-line"}),
      node("text",{x:(a[0]+b[0])/2,y:(a[1]+b[1])/2,class:"measure-text"},`${distance.toFixed(3)} mm`));
    $("measurement").textContent=`${distance.toFixed(6)} mm; ΔX ${(b[0]-a[0]).toFixed(6)}, ΔY ${(b[1]-a[1]).toFixed(6)} mm`;
  } else $("measurement").textContent=measurePoints.length ? "Select the second measurement point" : "No measurement";
  liveSvg.replaceChildren(svg);
  if (!viewbox) resetFit(); else liveSvg.setAttribute("viewBox",viewbox.join(" "));
  if(focused) liveSvg.querySelector(`.component[data-reference="${CSS.escape(focused)}"]`)?.focus();
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
  $("undo-source").disabled=blocked || (!accepted?.can_source_undo && !accepted?.document_host);
  $("redo-source").disabled=blocked || (!accepted?.can_source_redo && !accepted?.document_host);
  $("source-link").hidden=!accepted?.document_host;
  $("source-link").disabled=busy || !component?.source_link;
  $("source-review").hidden=!sourceReview;
  $("source-diff").textContent=accepted?.source_review?.diff || "";
  $("persistent-controls").disabled=blocked || !!preview || !accepted?.source_writable;
  $("prepare-lock").disabled=blocked || !!preview || !component || !$("edit-locks").checked || !!component.profile_role || !!component.source_attachment;
  $("mechanical-controls").disabled=blocked || !!preview || !accepted?.capabilities.mechanical_edit;
  $("undo").disabled = blocked || !accepted?.can_undo; $("redo").disabled = blocked || !accepted?.can_redo;
  $("pending").hidden = !preview;
  $("move").disabled = !component || blocked || !!preview || component.session_locked ||
    (component.source_position_locked && component.source_rotation_locked && component.source_side_locked);
  $("lock").disabled = !component || blocked || !!preview;
  if (component) {
    if(component.source_attachment && !sourceReview && !preview && !busy && !accepted?.source_stale) $("notice").textContent=`Pose owned by attachment ${component.source_attachment}; edit the attachment or its datum in Mechanical features.`;
    $("x").disabled=$("y").disabled=!!component.source_position_locked && !$("edit-locks").checked;
    $("rotation").disabled=!!component.source_rotation_locked && !$("edit-locks").checked;
    $("pose-side").disabled=!!component.source_side_locked && !$("edit-locks").checked;
    $("x").value = mm(component.position[0]); $("y").value = mm(component.position[1]);
    $("rotation").value = component.rotation; $("pose-side").value = component.side;
    $("lock").textContent = component.session_locked ? "Unlock temporary pose" : "Lock temporary pose";
    $("details").textContent = `${component.footprint}; ${component.value}. ${component.source_position_locked||component.source_rotation_locked ? "SOURCE LOCK. Enable explicit source-lock editing to change it. " : ""}${component.profile_role ? "Imported profile role: "+component.profile_role+" (read-only). " : ""}${component.macro ? "Rigid unit: "+component.macro+". " : ""}Allowed angles: ${component.allowed_orientations.join(", ")}.`;
    $("details").textContent+=` Height: ${component.height_nm==null ? "unknown" : mm(component.height_nm)+" mm"}.`;
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
  const existing=(accepted.mechanical_features||[]).filter(f=>f.kind==="boundary");
  $("path-segments").value=JSON.stringify(existing.length ? existing.map(f=>({id:f.name,kind:f.shape,...f.parameters})) :
    [{id:"TOP",kind:"line",start:"(0mm,0mm)",end:"(40mm,0mm)"},
     {id:"RIGHT",kind:"line",start:"(40mm,0mm)",end:"(40mm,30mm)"},
     {id:"BOTTOM",kind:"line",start:"(40mm,30mm)",end:"(0mm,30mm)"},
     {id:"LEFT",kind:"line",start:"(0mm,30mm)",end:"(0mm,0mm)"}],null,2);
  const layer=$("copper-layer"),previous=layer.value;layer.replaceChildren(new Option("All copper layers",""));
  for (const name of accepted.routed_overlay?.native.layers||[]) layer.add(new Option(name,name));layer.value=previous;
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
  if (component.session_locked || component.source_position_locked) {render();return;}
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
for (const id of ["side","net","airwires","selected-only","zone-nets","power-filter","net-labels","net-costs","copper-layer","routed-tracks","filled-zones","native-opens"]) $(id).addEventListener("change",render);
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
$("source-link").onclick=()=>api("/api/source-link",{reference:selected}).catch(e=>status(e.message,true));
const featureDefaults={
  "outline:rectangle":{width:"40mm",height:"30mm",origin:"(0mm,0mm)"},
  "outline:rounded_rectangle":{width:"40mm",height:"30mm",origin:"(0mm,0mm)",corner_radius:"3mm",maximum_chord_error:"0.01mm"},
  "outline:path":{maximum_chord_error:"0.01mm"},
  "outline:circle":{diameter:"50mm",center:"(25mm,25mm)"},
  "outline:polygon":{vertices:"[(0mm,0mm), (40mm,0mm), (40mm,30mm), (0mm,30mm)]"},
  "hole:":{position:"(3mm,3mm)",diameter:"2.2mm",head_clearance_radius:"0mm"},
  "slot:":{start:"(17mm,10mm)",end:"(23mm,10mm)",width:"1.5mm"},
  "boundary:line":{start:"(0mm,0mm)",end:"(40mm,0mm)"},
  "boundary:arc":{start:"(0mm,20mm)",mid:"(10mm,10mm)",end:"(20mm,20mm)"},
  "cutout:polygon":{vertices:"[(27mm,18mm), (33mm,18mm), (33mm,23mm), (27mm,23mm)]"},
  "keepout:rectangle":{width:"4mm",height:"4mm",origin:"(10mm,10mm)",side:"front"},
  "keepout:polygon":{vertices:"[(10mm,10mm), (14mm,10mm), (14mm,14mm), (10mm,14mm)]",side:"front"},
  "copper_keepout:rectangle":{width:"4mm",height:"4mm",origin:"(10mm,10mm)",layers:'"F.Cu"',block_tracks:"true",block_vias:"true",block_pads:"true",block_zones:"true",block_footprints:"false"},
  "copper_keepout:polygon":{vertices:"[(10mm,10mm), (14mm,10mm), (14mm,14mm), (10mm,14mm)]",layers:'"F.Cu"'},
  "rules:":{minimum_clearance:"0.2mm",minimum_track_width:"0.2mm"},
  "datum:":{position:"(5mm,5mm)"},
  "edge:":{start:"(0mm,0mm)",end:"(40mm,0mm)"},
  "attach:":{component:"J1",target:"DATUM",offset:"(0mm,0mm)",anchor:"origin",rotation:"0",side:"front"},
  "overhang:":{component:"J1",edge:"TOP",start:"10mm",end:"20mm",distance:"2mm",reason:'"Audited connector body protrusion; copper remains on board"'},
  "component_height:":{component:"J1",height:"3mm"},
  "enclosure:rectangle":{origin:"(0mm,0mm)",width:"40mm",height:"30mm",side:"front",maximum_height:"4mm"},
  "enclosure:polygon":{vertices:"[(0mm,0mm),(40mm,0mm),(40mm,30mm),(0mm,30mm)]",side:"front",maximum_height:"4mm"},
  "assembly_access:rectangle":{component:"J1",origin:"(-3mm,-5mm)",width:"6mm",height:"3mm",side:"component",purpose:'"Connector insertion clearance"'},
  "assembly_access:polygon":{component:"J1",vertices:"[(-3mm,-5mm),(3mm,-5mm),(3mm,-2mm),(-3mm,-2mm)]",side:"component",purpose:'"Connector insertion clearance"'}
};
function featureInputs(parameters) {
  parameters={...parameters};
  if ($("feature-kind").value==="keepout" && !("maximum_height" in parameters)) parameters.maximum_height="";
  if ($("feature-kind").value==="datum") for(const key of ["position","relative_to","offset"]) if(!(key in parameters))parameters[key]="";
  if ($("feature-kind").value==="attach") for(const key of ["target","position","offset","anchor_pad","anchor_point"]) if(!(key in parameters))parameters[key]="";
  const container=$("feature-parameters");container.replaceChildren();
  for (const [key,value] of Object.entries(parameters)) {
    const label=document.createElement("label");label.textContent=key;
    const input=document.createElement(key==="vertices"?"textarea":"input");input.id="feature-param-"+key;input.dataset.property=key;input.value=value;label.append(input);container.append(label);
  }
}
function featureMode() {
  const kind=$("feature-kind").value;
  if (["hole","slot","rules","datum","edge","attach","overhang","component_height"].includes(kind)) $("feature-shape").value="";
  else if(kind==="boundary" && !["line","arc"].includes($("feature-shape").value)) $("feature-shape").value="line";
  else if (kind==="cutout") $("feature-shape").value="polygon";
  else if (kind.includes("keepout") && $("feature-shape").value==="circle") $("feature-shape").value="rectangle";
  else if (["enclosure","assembly_access"].includes(kind) && !["rectangle","polygon"].includes($("feature-shape").value)) $("feature-shape").value="rectangle";
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
  if(!remove && $("feature-kind").value==="outline" && $("feature-shape").value==="path") {reviewPath(parameters.maximum_chord_error);return;}
  if(!remove && $("feature-kind").value==="outline" && accepted.mechanical_features?.some(f=>f.kind==="boundary")) {
    operation("prepare_mechanical_batch",{features:[{kind:"outline",name:"",shape:$("feature-shape").value,parameters,remove:false},
      ...accepted.mechanical_features.filter(f=>f.kind==="boundary").map(f=>({kind:"boundary",name:f.name,shape:f.shape,parameters:{},remove:true}))]});return;
  }
  operation("prepare_mechanical",{kind:$("feature-kind").value,name:$("feature-name").value,shape:$("feature-shape").value,parameters:remove?{}:parameters,remove});
}
$("prepare-feature").onclick=()=>featureRequest();$("remove-feature").onclick=()=>featureRequest(true);
function reviewPath(error="0.01mm") {
  try {
    const segments=JSON.parse($("path-segments").value);
    if(!Array.isArray(segments) || segments.length<2 || segments.length>256)throw new Error("Path requires 2–256 primitives");
    const previous=scene().mechanical_features.find(f=>f.kind==="outline" && f.shape==="path");
    const features=[{kind:"outline",name:"",shape:"path",parameters:{maximum_chord_error:error||previous?.parameters.maximum_chord_error||"0.01mm"},remove:false}];
    for(const segment of segments) {
      const {id,kind,...parameters}=segment;
      if(!["line","arc"].includes(kind) || typeof id!=="string")throw new Error("Each primitive requires an id and line/arc kind");
      features.push({kind:"boundary",name:id,shape:kind,parameters,remove:false});
    }
    const names=new Set(segments.map(s=>s.id));
    for(const old of accepted.mechanical_features||[]) if(old.kind==="boundary" && !names.has(old.name))
      features.push({kind:"boundary",name:old.name,shape:old.shape,parameters:{},remove:true});
    operation("prepare_mechanical_batch",{features});
  } catch(e){status(e.message,true);}
}
$("prepare-path").onclick=()=>reviewPath();
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
  if(event.key==="+" || event.key==="="){event.preventDefault();zoom(.8);return;}
  if(event.key==="-"){event.preventDefault();zoom(1.25);return;}
  if(event.key.toLowerCase()==="f"){event.preventDefault();resetFit();render();return;}
  const c=scene()?.components.find(c=>c.reference===selected);
  if(event.key.toLowerCase()==="r" && c && !busy && !preview && !accepted?.source_review && !c.session_locked && !c.source_rotation_locked) {
    event.preventDefault();const angles=c.allowed_orientations.map(Number),current=((Number(c.rotation)%360)+360)%360;
    const next=angles.find(a=>a>current)??angles[0];
    if(next!==undefined)operation("move",{reference:selected,x_nm:c.position[0],y_nm:c.position[1],rotation:next,side:c.side},true);
    return;
  }
  const delta={ArrowLeft:[-1,0],ArrowRight:[1,0],ArrowUp:[0,-1],ArrowDown:[0,1]}[event.key];
  if (!delta || busy || preview || accepted?.source_review) return;
  if (!c || c.session_locked || c.source_position_locked) return;
  event.preventDefault();const step=Math.round(Number($("snap").value)*1e6)*(event.shiftKey?10:1);
  operation("move",{reference:selected,x_nm:c.position[0]+delta[0]*step,y_nm:c.position[1]+delta[1]*step,rotation:c.rotation,side:c.side},true);
});
featureMode();
updateControls();reloadScene();
