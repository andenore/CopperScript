"use strict";
const $ = id => document.getElementById(id);
const token = new URLSearchParams(location.hash.slice(1)).get("token");
const vscodeHost=typeof acquireVsCodeApi==="function" ? acquireVsCodeApi() : null;
const hostRequests=new Map();let hostRequestId=0;
if(vscodeHost) window.addEventListener("message",event=>{
  const message=event.data;
  if(message?.type==="refresh") {reloadScene();return;}
  if(message?.type==="selection" && scene()?.components.some(c=>c.reference===message.reference)) {
    selectReferences([message.reference],message.reference);render();syncLockChecks();return;
  }
  const pending=hostRequests.get(message?.id);if(!pending) return;
  hostRequests.delete(message.id);clearTimeout(pending.timeout);
  if(message.error) pending.reject(new Error(message.error));else pending.resolve(message.result);
});
const NS = "http://www.w3.org/2000/svg";
let accepted, preview = null, sourcePreview = null, selected = "", selectedReferences = new Set(), viewbox = null, drag = null, pan = null, busy = false;
let measurePoints=[], vertexPoints=[];
let jobPoll=null;
let mechanicalFormRevision="";
const mm = n => n / 1000000;
const scene = () => sourcePreview || preview || accepted;
function acceptScene(value) {
  const initial=!accepted;
  accepted=value;
  preview=value.pending_preview || null;
  sourcePreview=value.source_preview || null;
  const definition=value.document_kind && value.document_kind!=="board";
  for(const tab of ["components","mechanics"])$("tab-"+tab).hidden=definition;
  if(initial && definition)switchTab("features");
}
const toolTabs=["components","mechanics","features","view"];
function switchTab(name,focus=false) {
  for(const tab of toolTabs) {
    const active=tab===name;
    $("tab-"+tab).setAttribute("aria-selected",String(active));
    $("tab-"+tab).tabIndex=active ? 0 : -1;
    $("panel-"+tab).hidden=!active;
  }
  if(name!=="mechanics") {$("measure").checked=false;$("draw-vertices").checked=false;}
  if(name!=="features") $("physical-pick-point").checked=false;
  if(focus)$("tab-"+name).focus();
}
for(const [index,name] of toolTabs.entries()) {
  $("tab-"+name).onclick=()=>switchTab(name);
  $("tab-"+name).onkeydown=event=>{
    const tabs=toolTabs.filter(tab=>!$("tab-"+tab).hidden),index=tabs.indexOf(name);
    const next=event.key==="ArrowRight" ? (index+1)%tabs.length : event.key==="ArrowLeft" ? (index+tabs.length-1)%tabs.length :
      event.key==="Home" ? 0 : event.key==="End" ? tabs.length-1 : null;
    if(next!==null) {event.preventDefault();switchTab(tabs[next],true);}
  };
}
document.addEventListener("keydown",event=>{
  if(event.key==="Escape")for(const menu of document.querySelectorAll(".toolbar-menu[open]")) {
    menu.open=false;menu.querySelector("summary").focus();
  }
});
for(const menu of document.querySelectorAll(".toolbar-menu"))menu.addEventListener("click",event=>{
  if(event.target.closest("button"))menu.open=false;
});
function selectReferences(references, primary="") {
  const available=new Set((scene()?.components||[]).map(component=>component.reference));
  selectedReferences=new Set(references.filter(reference=>available.has(reference)));
  selected=selectedReferences.has(primary) ? primary : [...selectedReferences][0] || "";
  if(selected)switchTab(scene()?.document_kind && scene().document_kind!=="board"?"features":"components");
}
function selectionSet() {
  return new Set(selectedReferences.size ? selectedReferences : (selected ? [selected] : []));
}
function expandedSelection() {
  const components=scene()?.components||[], selection=selectionSet(), macros=new Set(
    components.filter(component=>selection.has(component.reference) && component.macro)
      .map(component=>component.macro));
  return new Set(components.filter(component=>selection.has(component.reference) ||
    (component.macro && macros.has(component.macro))).map(component=>component.reference));
}
function pruneSelection() {
  if (!scene()) return;
  const available=new Set(scene().components.map(component=>component.reference));
  selectedReferences=new Set([...selectedReferences].filter(reference=>available.has(reference)));
  if (selected && !available.has(selected)) selected=[...selectedReferences][0] || "";
  if (!selected && selectedReferences.size) selected=[...selectedReferences][0];
}
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
function artworkShape(item) {
  const style={stroke:item.width_nm ? "currentColor" : "none","stroke-width":mm(item.width_nm),
    fill:item.filled ? "currentColor" : "none","stroke-linejoin":"round","stroke-linecap":"round"};
  if(item.kind==="text") {
    const text=node("text",{class:"artwork-text","font-size":mm(item.size_nm),
      transform:`translate(${item.position.map(mm).join(" ")}) rotate(${Number(item.rotation)})${item.mirrored ? " scale(-1 1)" : ""}`});
    const lines=item.text.split("\n");
    for(const [index,line] of lines.entries())text.append(node("tspan",{x:0,y:(index-(lines.length-1)/2)*mm(item.size_nm)*1.4},line));
    return text;
  }
  if(item.kind==="line")return node("line",{x1:mm(item.start[0]),y1:mm(item.start[1]),x2:mm(item.end[0]),y2:mm(item.end[1]),...style});
  if(item.kind==="polygon")return node("polygon",{points:points(item.vertices),...style});
  if(item.kind==="circle")return node("circle",{cx:mm(item.center[0]),cy:mm(item.center[1]),
    r:Math.hypot(item.end[0]-item.center[0],item.end[1]-item.center[1])/1e6,...style});
  if(item.kind==="arc")return node("path",{d:`M ${item.start.map(mm).join(" ")} A ${mm(item.radius_nm)} ${mm(item.radius_nm)} 0 ${item.large_arc?1:0} ${item.sweep?1:0} ${item.end.map(mm).join(" ")}`,...style});
}
function artworkGroup(items,side,material,cls) {
  const editable=cls==="board-artwork";
  const group=node("g",{class:`artwork artwork-${material} ${side} ${cls}`,"pointer-events":editable?"visiblePainted":"none"});
  for(const item of items) {
    const drawing=artworkShape(item);if(!drawing)continue;
    const feature=node("g",{"data-artwork-id":item.id});
    feature.append(node("title",{},`${item.id}: ${item.layer||side+" silkscreen"}${item.exposed ? "; exposed copper" : ""}${item.kind==="text" ? "; text uses a preview font" : ""}`),drawing);
    if(editable)feature.addEventListener("click",event=>{
      if($("physical-pick-point").checked)return;
      event.stopPropagation();
      const found=selectPhysicalFeature(f=>f.category==="artwork" && (f.name===item.id || (f.kind==="logo"&&item.id.startsWith(f.name+":")) || (f.kind==="branding" && item.id.startsWith("__copperscript"))));
      if(!found && item.id.startsWith("__copperscript")) {
        const index=(accepted.feature_catalog||[]).findIndex(f=>f.kind==="branding");
        if(index>=0){$("physical-feature").value="";$("physical-kind").value=String(index);loadPhysicalForm();switchTab("features");}
      }
    });
    group.append(feature);
  }
  return group;
}
function padLabelLayout(pad,label) {
  const spine=pad.shape?.spine||[], radius=mm(Number(pad.shape?.radius_nm)||0);
  const xs=spine.map(point=>mm(point[0])), ys=spine.map(point=>mm(point[1]));
  const width=Math.max(0.02,(Math.max(...xs)-Math.min(...xs))+2*radius);
  const height=Math.max(0.02,(Math.max(...ys)-Math.min(...ys))+2*radius);
  const inset=Math.min(0.04,width*.12,height*.12);
  const availableWidth=Math.max(0.01,width-2*inset), availableHeight=Math.max(0.01,height-2*inset);
  // SVG text without a unit is measured in the same user coordinates as the
  // pad geometry.  It therefore zooms with the pad instead of staying at a
  // browser/CSS-pixel size.  Reserve vertical room for glyph ascenders and
  // compress only long names horizontally when they would exceed the land.
  const fontSize=Math.max(0.01,Math.min(0.55,availableHeight*.72));
  const naturalWidth=label.length*fontSize*.62;
  const attrs={"font-size":fontSize.toFixed(3)};
  if(naturalWidth>availableWidth) {
    attrs.textLength=availableWidth.toFixed(3);
    attrs.lengthAdjust="spacingAndGlyphs";
  }
  return attrs;
}
function resetFit() {
  if (!scene()) return;
  const b = scene().bounds.map(mm), margin = 4;
  viewbox = [b[0]-margin,b[1]-margin,b[2]-b[0]+2*margin,b[3]-b[1]+2*margin];
  $("board").setAttribute("viewBox",viewbox.join(" "));
}
function render() {
  const s=scene(),liveSvg=$("board");if(!s)return;
  pruneSelection();
  const selectedSet=selectionSet();
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
  if($("reference-guides").checked) for(const guide of s.references||[]) {
    if(guide.side!=="both" && $("side").value!=="both" && guide.side!==$("side").value)continue;
    const g=node("g",{class:"reference-guide"+(s.references_stale?" stale":""),"pointer-events":"none"});
    g.append(node("title",{},`${guide.id}: ${guide.purpose}; locked ${guide.sha256}. ${featureLabel("reference",guide.id)}`));
    for(const e of guide.entities) {
      const p=e.points.map(p=>p.map(mm));let n;
      if(e.kind==="line")n=node("line",{x1:p[0][0],y1:p[0][1],x2:p[1][0],y2:p[1][1]});
      else if(e.kind==="circle")n=node("circle",{cx:p[0][0],cy:p[0][1],r:mm(e.radius_nm)});
      else if(e.kind==="arc")n=node("path",{d:`M ${e.start.map(mm).join(" ")} A ${mm(e.radius_nm)} ${mm(e.radius_nm)} 0 ${e.large?1:0} ${e.sweep?1:0} ${e.end.map(mm).join(" ")}`});
      else n=node(e.closed?"polygon":"polyline",{points:points(e.points)});
      g.append(n);
    }
    svg.append(g);
  }
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
  renderPhysicalGeometry(svg,s);
  const visible = new Set(s.components.filter(c => $("side").value === "both" || c.side === $("side").value).map(c => c.reference));
  const relevant = new Set(s.components.filter(component=>selectedSet.has(component.reference))
    .flatMap(component=>component.pads.map(pad=>pad.net).filter(Boolean)));
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
    const warning = !!component.constraint_warning;
    const invalid = !!component.pose_invalid;
    const isSelected=selectedSet.has(component.reference);
    const g = node("g",{class:`component ${component.side}${isSelected ? " selected" : ""}${locked ? " locked" : ""}${warning ? " constraint-warning" : ""}${invalid ? " pose-invalid" : ""}`,"data-reference":component.reference,
      role:"button",tabindex:isSelected || (!selected && component.reference===s.components[0]?.reference)?"0":"-1",
      "aria-pressed":isSelected?"true":"false",
      "aria-label":`${component.reference}, ${component.value||component.footprint}, ${component.side}, X ${mm(component.position[0])} mm, Y ${mm(component.position[1])} mm${locked?", pose constraints present":""}${warning?`, ${component.constraint_warning_count} relative placement warning${component.constraint_warning_count===1?"":"s"}`:""}`});
    g.append(node("polygon",{points:points(component.courtyard),class:"courtyard"}),node("polygon",{points:points(component.body),class:"body"}));
    if(invalid)g.append(node("title",{},(component.pose_issues||[]).join("\n")));
    if($("silkscreen").checked)g.append(artworkGroup(component.silkscreen||[],component.side,"silkscreen","footprint-artwork"));
    if (warning) {
      const marker=node("circle",{cx:mm(component.position[0]),cy:mm(component.position[1]),r:.65,class:"constraint-warning-marker"});
      marker.append(node("title",{},`${component.reference}: relative placement constraint warning`));g.append(marker);
    }
    for (const pad of component.pads) {
      const padGroup=node("g",{class:"pad-hit","data-pad-number":pad.number});
      if (pad.kind !== "non_plated_through_hole") {
        const n = shape(pad.shape,"pad");
        n.append(node("title",{},`${component.reference}.${pad.number}${pad.name ? ` (${pad.name})` : ""}: ${pad.net || "no net"}`));
        padGroup.append(n);
        if (pad.number) {
          const center=pad.shape.spine.reduce((sum,point)=>[sum[0]+point[0],sum[1]+point[1]],[0,0]);
          center[0]/=pad.shape.spine.length;center[1]/=pad.shape.spine.length;
          const label=pad.name || pad.number;
          padGroup.append(node("text",{x:mm(center[0]),y:mm(center[1]),class:"pad-label",
            ...padLabelLayout(pad,label)},label));
        }
      }
      if (pad.drill) padGroup.append(shape(pad.drill,"drill"));
      if (padGroup.childNodes.length) g.append(padGroup);
    }
    g.append(node("text",{x:mm(component.position[0]),y:mm(component.position[1])-1.4,class:"ref"},component.reference));
    g.addEventListener("pointerdown",event=>startDrag(event,component,g)); svg.append(g);
    g.addEventListener("keydown",event=>{if(["Enter"," "].includes(event.key)){event.preventDefault();selectReferences([component.reference],component.reference);render();syncLockChecks();}});
  }
  for(const side of ["front","back"])for(const material of ["silkscreen","copper"]) {
    if($("side").value!=="both" && $("side").value!==side)continue;
    if(!$(material==="copper" ? "copper-artwork" : "silkscreen").checked)continue;
    const items=(s.artwork||[]).filter(item=>item.side===side && item.material===material);
    if(items.length)svg.append(artworkGroup(items,side,material,"board-artwork"));
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
  const placementWarnings=s.placement_constraints?.violations||[];
  $("placement-constraint-warnings").hidden=!placementWarnings.length;
  $("constraint-warnings").textContent=placementWarnings.map(item=>item.message).join("\n")||"No relative placement warnings.";
  updateControls();
}
function updateControls() {
  const s = scene(), component = s?.components.find(c => c.reference===selected);
  const selectionCount=selectionSet().size, groupCount=expandedSelection().size;
  const sourceReview=!!accepted?.source_review;
  const placementUnsaved=!!(preview || accepted)?.placement_unsaved;
  const placementSave=!!accepted?.source_writable && placementUnsaved && (!preview || preview.placement_hard_legal);
  const editingPreview=!!preview?.preview_editable;
  const frozenPreview=!!preview && !editingPreview;
  const blocked = busy || !s || !!accepted?.source_stale || sourceReview;
  $("board-name").textContent=s?.board || "Loading board…";
  $("notice").textContent=accepted ? "Review and save source changes. Use Review placement to save the current layout as explicit source locks." : "Loading source workspace…";
  $("open-board").hidden=!vscodeHost;
  $("open-board").disabled=busy || !!preview || sourceReview;
  $("source-state").textContent=accepted?.source_stale ? "Source changed" : sourceReview ? "Review pending" :
    !accepted ? "Loading" : accepted.source_writable ? placementUnsaved ? "Placement unsaved" : "Source ready" : "Read only";
  $("source-state").classList.toggle("pending",sourceReview || placementUnsaved);
  $("source-state").classList.toggle("error",!!accepted?.source_stale);
  $("reference").value = selected;
  $("apply").disabled = !preview || blocked || !preview.placement_hard_legal; $("discard").disabled = !preview || blocked;
  const job=accepted?.placement_job;
  $("auto").disabled = blocked || !!preview || job?.status==="running" || !s?.capabilities.auto_place;
  $("auto").hidden = !s?.capabilities.auto_place;
  $("cancel-auto").disabled=busy || job?.status!=="running";
  $("cancel-auto").hidden=job?.status!=="running";
  $("auto").title="Preview automatic placement; existing locks and physical constraints are preserved.";
  $("job-status").hidden=!job;
  $("job-status").textContent=job ? `Auto-placement ${job.status}: ${job.progress.phase}, candidate ${Math.min(job.progress.candidate+1,job.progress.candidates)}/${job.progress.candidates}; ${job.elapsed_seconds}s elapsed${job.error?" — "+job.error:""}` : "";
  $("job-profile").hidden=!job?.profile;$("profile-text").textContent=job?.profile?.top_cumulative||"";
  if (job?.status==="running" && jobPoll===null) jobPoll=setTimeout(pollJob,250);
  $("reload").disabled = busy;
  $("reload-source").disabled=busy || !accepted?.source_writable;
  $("save-source").disabled=busy || (!sourceReview && !placementSave) || !!accepted?.source_stale || job?.status==="running";
  $("save-source").textContent=sourceReview ? "Save reviewed changes" : placementUnsaved ? "Review placement to save…" : "Save";
  $("save-help").textContent=accepted?.source_stale ? "Source changed externally. Reload source before reviewing changes." :
    sourceReview ? (accepted.source_review.summary || "Changes are reviewed but unsaved. Inspect the diff, then save or discard.") :
    placementUnsaved ? preview && !preview.placement_hard_legal ? "Fix the invalid placement before saving. The draft remains editable." :
    "Placement is unsaved. Review placement creates explicit position, rotation and side locks; inspect the diff, then Save." :
    accepted?.source_writable ? "Edit a feature in Properties or on the canvas, then review and save. Stage related changes to validate them together." :
    "This scene is read-only; open a .copper source board to save changes.";
  $("discard-source").disabled=busy || !sourceReview || !!accepted?.source_stale;
  $("undo-source").disabled=blocked || (!accepted?.can_source_undo && !accepted?.document_host);
  $("redo-source").disabled=blocked || (!accepted?.can_source_redo && !accepted?.document_host);
  $("source-link").hidden=!accepted?.document_host;
  $("source-link").disabled=busy || !component?.source_link;
  $("source-review").hidden=!sourceReview;
  $("source-diff").textContent=accepted?.source_review?.diff || "";
  $("persistent-controls").disabled=blocked || !!preview || !accepted?.source_writable;
  for(const dimension of ["position","rotation","side"])$(dimension+"-lock").disabled=!$("edit-locks").checked || !component;
  $("prepare-lock").disabled=blocked || !!preview || !component || !$("edit-locks").checked || !!component.profile_role || !!component.source_attachment;
  $("mechanical-controls").disabled=blocked || !!preview || !accepted?.capabilities.mechanical_edit;
  $("physical-fields").disabled=blocked || !!preview || !accepted?.source_writable || activePhysicalFeature()?.editable===false;
  $("physical-remove").disabled=!$("physical-feature").value || activePhysicalFeature()?.kind==="properties";
  $("physical-pick-point").disabled=blocked || activePhysicalFeature()?.editable===false;
  $("physical-polygon").disabled=blocked || activePhysicalFeature()?.editable===false;
  if(s?.document_kind && s.document_kind!=="board") {
    $("notice").textContent=`${s.document_kind === "macro" ? "Macro definition: local coordinates" : "Mechanical profile definition"}. Review and save source changes.`;
    $("persistent-controls").disabled=true;
    $("move").disabled=true;$("lock").disabled=true;
  }
  $("remove-feature").disabled=!$("feature").value;
  $("remove-feature").title=$("feature").value ? "Review removal of the selected board-owned feature." : "Select an existing feature to remove it.";
  $("undo").disabled = blocked || !accepted?.can_undo; $("redo").disabled = blocked || !accepted?.can_redo;
  $("pending").hidden = !preview;
  $("pending").classList.toggle("invalid",!!preview && !preview.placement_hard_legal);
  $("preview-message").textContent=preview && !preview.placement_hard_legal ? "Invalid temporary pose — drag, rotate or edit coordinates to fix it, or discard." :
    editingPreview ? "Temporary preview — keep adjusting, apply or discard." : "Temporary preview — apply or discard.";
  const poseIssues=s?.placement_validation?.issues||[];
  $("pose-validation").hidden=!preview || !poseIssues.length;
  $("pose-validation-reasons").textContent=poseIssues.map(issue=>issue.message).join("\n");
  $("move").disabled = !s?.capabilities.pose_preview || !component || blocked || frozenPreview || selectionCount>1 || component.session_locked ||
    (component.source_position_locked && component.source_rotation_locked && component.source_side_locked);
  $("lock").disabled = !s?.capabilities.session_locks || !component || blocked || !!preview;
  $("physical-review-staged").disabled=blocked || !physicalStaged.length;
  $("physical-clear-staged").disabled=busy;
  for(const id of ["rotate-left","rotate-right"])$(id).disabled=!s?.capabilities.pose_preview || !component || blocked || frozenPreview ||
    !!component.session_locked || !!component.source_rotation_locked || component.allowed_orientations.length<2;
  if (component) {
    if(component.source_attachment && !sourceReview && !preview && !busy && !accepted?.source_stale) $("notice").textContent=`Pose owned by attachment ${component.source_attachment}; edit the attachment or its datum in Mechanical features.`;
    $("x").disabled=$("y").disabled=blocked || frozenPreview || !!component.source_position_locked && !$("edit-locks").checked;
    $("rotation").disabled=blocked || frozenPreview || !!component.source_rotation_locked && !$("edit-locks").checked;
    $("pose-side").disabled=blocked || frozenPreview || !!component.source_side_locked && !$("edit-locks").checked;
    $("x").value = mm(component.position[0]); $("y").value = mm(component.position[1]);
    $("rotation").value = component.rotation; $("pose-side").value = component.side;
    $("lock").textContent = component.session_locked ? "Unlock for this session" : "Lock for this session";
    $("temporary-lock-state").textContent=component.session_locked ? "Temporarily locked. This lock will not survive reopening." : "Temporary pose is unlocked.";
    const sourceLocks=["position","rotation","side"].filter(d=>component["source_"+d+"_locked"]);
    $("source-lock-state").textContent=sourceLocks.length ? (sourceReview ? "Proposed source locks: " : "Source locks: ")+sourceLocks.join(", ")+"." :
      sourceReview ? "Proposed source pose has no locks." : "No saved source locks.";
    $("details").textContent = `${component.footprint}; ${component.value}. ${component.profile_role ? "Imported profile role: "+component.profile_role+" (read-only). " : ""}${component.macro ? "Rigid unit: "+component.macro+". " : ""}Allowed angles: ${component.allowed_orientations.join(", ")}.`;
    $("details").textContent+=` Height: ${component.height_nm==null ? "unknown" : mm(component.height_nm)+" mm"}.`;
    if(selectionCount>1) $("details").textContent+=` ${selectionCount} components selected (${groupCount} including rigid macro members); drag any highlighted component to move them together.`;
    if(component.constraint_warning) $("details").textContent+=` Relative placement warning (${component.constraint_warning_count}); inspect the highlighted components and warning panel.`;
  } else {
    $("temporary-lock-state").textContent="No component selected.";
    $("source-lock-state").textContent="Select a component to inspect its saved locks.";
    $("details").textContent="Click a footprint to inspect it.";
    for(const id of ["x","y","rotation","pose-side"])$(id).disabled=true;
  }
}
function populate() {
  populatePhysicalFeatures();
  if(mechanicalFormRevision!==accepted.source_revision)syncLockChecks();
  for (const [id,values,label] of [["reference",accepted.components.map(c=>c.reference),"Select a component"],["net",accepted.nets,"All nets"]]) {
    const element = $(id), old = element.value; element.replaceChildren(new Option(label,""));
    for (const value of values) element.add(new Option(value,value)); element.value=old;
  }
  const features=$("feature"),old=features.value;features.replaceChildren(new Option("New feature",""));
  for (const [index,f] of (accepted.mechanical_features||[]).entries()) features.add(new Option(`${f.kind} ${f.name||f.shape} (line ${f.line})`,String(index)));
  features.value=old;
  // Save/Undo/Reload must not leave stale literals in a selected feature form.
  if(old!=="" && mechanicalFormRevision!==accepted.source_revision)features.onchange?.();
  mechanicalFormRevision=accepted.source_revision;
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
    if (result.preview && applyImmediately && result.preview.placement_hard_legal) {
      // The existing two-phase API validates both operations. Never apply using
      // an old revision: another editor may have changed the pending pose.
      accepted.revision=result.preview.revision;
      result=await api("/api/operation",{action:"apply",revision:result.preview.revision});
    }
    if (result.preview) {preview=result.preview; accepted.revision=preview.revision;
      status(preview.placement_hard_legal ? "Preview — adjust, apply or discard. Source remains unchanged." :
        "Invalid temporary pose — adjust its position or rotation. " +(preview.placement_validation?.issues||[]).map(issue=>issue.message).join("; "),!preview.placement_hard_legal);}
    else {acceptScene(result.scene);sourcePreview=result.source_preview||sourcePreview;populate();
      status(accepted.source_review ? "Source preview — inspect the exact diff, then Save or Discard." :
        action==="save_source" ? "Source saved. " +(accepted.placement_unsaved ? "Placement remains unsaved; use Review placement to save it. " : "") +"Rebuild routing, fills and manufacturing outputs." :
        accepted.outputs_stale ? "Source/session updated. Route, fill and manufacturing outputs are stale; rebuild them." : "Temporary session updated. Source remains unchanged.");}
  } catch (error) {
    preview=null;sourcePreview=null;
    // Recover the authoritative revision after conflicts and rejected poses.
    // Otherwise every subsequent edit can fail until the page is reloaded.
    try {acceptScene(await api("/api/scene"));populate();} catch (_) { /* Keep the original failure visible. */ }
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
    acceptScene(await api("/api/scene"));
    if(accepted.placement_job?.status==="completed") status("Auto-placement preview ready — apply or discard.");
    if(accepted.placement_job?.error) status(accepted.placement_job.error,true);
    render();
  } catch(error) {status(error.message,true);}
}
function startDrag(event,component,g) {
  if (event.button!==0 || busy || drag || pan || accepted?.source_stale || accepted?.source_review || $("measure").checked || $("draw-vertices").checked || $("physical-pick-point").checked) return;
  if (event.shiftKey) {
    event.preventDefault();
    const next=selectionSet();
    if (next.has(component.reference)) next.delete(component.reference);
    else next.add(component.reference);
    selectReferences([...next],next.has(component.reference) ? component.reference : [...next][0] || "");
    $("reference").value=selected; syncLockChecks(); updateControls(); render();
    return;
  }
  if (!selectionSet().has(component.reference)) selectReferences([component.reference],component.reference);
  else selected=component.reference;
  $("reference").value=selected; syncLockChecks(); updateControls();
  if(scene().document_kind==="macro")selectPhysicalFeature(f=>f.kind==="member" && f.name===component.reference);
  if (preview && !preview.preview_editable) {status("Apply or discard the automatic-placement preview before moving a component.");return;}
  $("board").focus({preventScroll:true});
  const start=svgPoint(event);
  const members=expandedSelection();
  const lockedMember=scene().components.find(c=>members.has(c.reference) && (c.session_locked || c.source_position_locked));
  if (lockedMember) {status(`${lockedMember.reference} is locked and cannot be moved with this selection.`,true);render();return;}
  drag={component,g,start,last:start,pointerId:event.pointerId,members}; g.classList.add("dragging"); $("board").setPointerCapture(event.pointerId);
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
  const dx_nm=Math.round(x*1e6)-d.component.position[0],dy_nm=Math.round(y*1e6)-d.component.position[1];
  if(scene().document_kind==="macro") {
    const f=accepted.source_features.find(f=>f.kind==="member" && f.name===d.component.reference);
    if(f)operation("review_feature",{category:"macro",kind:"member",name:f.name,parameters:{...f.parameters,at:`(${mm(Math.round(x*1e6))} mm, ${mm(Math.round(y*1e6))} mm)`}});
    return;
  }
  if (d.members.size>1) operation("move_group",{references:[...d.members],dx_nm,dy_nm},!$("preview-drags").checked);
  else operation("move",{reference:d.component.reference,x_nm:Math.round(x*1e6),y_nm:Math.round(y*1e6),rotation:d.component.rotation,side:d.component.side},!$("preview-drags").checked);
});
function endPan() {
  const id=pan?.pointerId;pan=null;$("board").classList.remove("panning");
  if (id!==undefined && $("board").hasPointerCapture(id)) $("board").releasePointerCapture(id);
}
function cancelGesture() {endPan();drag=null;render();}
$("board").addEventListener("pointercancel",cancelGesture);
$("board").addEventListener("lostpointercapture",()=>{if(drag||pan) cancelGesture();});
$("reference").addEventListener("change",()=>{selectReferences($("reference").value ? [$("reference").value] : [],$("reference").value);render();});
$("reference").addEventListener("change",syncLockChecks);
$("edit-locks").onchange=()=>{syncLockChecks();updateControls();};
for (const id of ["side","net","airwires","selected-only","zone-nets","power-filter","net-labels","net-costs","copper-layer","routed-tracks","filled-zones","native-opens","reference-guides","silkscreen","copper-artwork"]) $(id).addEventListener("change",()=>{
  const draft=["x","y","rotation","pose-side"].map(id=>[id,$(id).value]);
  render();
  for(const [id,value] of draft)$(id).value=value;
});
for (const [id,action] of [["apply","apply"],["discard","discard"],["undo","undo"],["redo","redo"]]) $(id).onclick=()=>operation(action);
$("auto").onclick=()=>operation("start_auto_place");
$("cancel-auto").onclick=()=>operation("cancel_auto_place");
$("lock").onclick=()=>operation("lock",{reference:selected,locked:!scene().components.find(c=>c.reference===selected).session_locked});
$("move").onclick=()=>operation("move",{reference:selected,x_nm:Math.round(Number($("x").value)*1e6),y_nm:Math.round(Number($("y").value)*1e6),rotation:$("rotation").value,side:$("pose-side").value});
function rotateSelected(direction) {
  const c=scene()?.components.find(c=>c.reference===selected);
  if(!c || busy || accepted?.source_review || accepted?.source_stale || drag ||
      (preview && !preview.preview_editable) || c.session_locked || c.source_rotation_locked ||
      !scene().capabilities.pose_preview)return;
  const angles=[...new Set(c.allowed_orientations.map(Number))].sort((a,b)=>a-b);
  const current=((Number(c.rotation)%360)+360)%360;
  const next=direction<0 ? [...angles].reverse().find(a=>a<current) ?? angles.at(-1) : angles.find(a=>a>current) ?? angles[0];
  if(next!==undefined)operation("move",{reference:selected,x_nm:c.position[0],y_nm:c.position[1],rotation:next,side:c.side},!$("preview-drags").checked);
}
$("rotate-left").onclick=()=>rotateSelected(-1);
$("rotate-right").onclick=()=>rotateSelected(1);
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
    const s=await api("/api/scene");acceptScene(s);populate();render();
    status(s.source_stale ? "Source changed externally; use Reload source (discard session) before editing." :
      s.document_kind && s.document_kind!=="board" ? `${s.board}: ${s.document_kind} definition. Local validation only; consumer integration and qualification remain pending.` :
      `${s.board}: ${s.components.length} components; ${s.ratsnest.length} airwires. ${s.placement_legal ? "Placement legal." : s.placement_hard_legal && s.placement_constraints?.violations?.length ? "Relative placement warnings are present." : "Initial inspection placement needs legalization."} ${Object.keys(s.warnings).length ? "Footprint/omission warnings are present in scene data." : ""}`,s.source_stale);
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
$("save-source").onclick=()=>accepted.source_review ? operation("save_source",{review_id:accepted.source_review.id}) : operation("prepare_placement");
$("open-board").onclick=()=>api("/api/open-board").catch(e=>status(e.message,true));
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
  "reference:dxf":{file:'"assets/case.dxf"',sha256:'"REPLACE_WITH_FILE_SHA256"',units:"mm",frame:"cartesian",position:"(0mm,30mm)",rotation:"0",mirror_x:"false",side:"both",purpose:'"Enclosure reference only"'},
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
  const kind=$("feature-kind").value;
  if(["datum","attach"].includes(kind)) {
    const label=document.createElement("label");label.textContent="Position reference";
    const mode=document.createElement("select");mode.id="feature-frame";
    mode.add(new Option("Absolute position","absolute"));mode.add(new Option(kind==="datum" ? "Relative to another datum" : "Datum or named edge","relative"));
    mode.value=parameters.relative_to || parameters.target ? "relative" : "absolute";
    mode.onchange=updateFeatureFields;label.append(mode);container.append(label);
  }
  for (const [key,value] of Object.entries(parameters)) {
    const label=document.createElement("label");label.textContent=key.replaceAll("_"," ").replace(/^./,c=>c.toUpperCase());
    const input=document.createElement(key==="vertices"?"textarea":key==="anchor" ? "select" : "input");
    if(input.tagName==="INPUT")input.type="text";
    if(key==="anchor") {
      for(const [name,title] of [["origin","Footprint origin"],["pad","Physical pad"],["mating_face","Audited mating face"]])input.add(new Option(title,name));
      if(![...input.options].some(o=>o.value===value))input.add(new Option(value,value));
      input.onchange=updateFeatureFields;
    }
    input.id="feature-param-"+key;input.dataset.property=key;input.value=value;label.append(input);container.append(label);
  }
  updateFeatureFields();
}
function updateFeatureFields() {
  const kind=$("feature-kind").value,relative=$("feature-frame")?.value==="relative";
  const anchor=$("feature-param-anchor")?.value.replaceAll('"',"");
  for(const input of $("feature-parameters").querySelectorAll("[data-property]")) {
    const key=input.dataset.property;
    const visible=key==="anchor_pad" ? anchor==="pad" : key==="anchor_point" ? anchor==="mating_face" :
      ["datum","attach"].includes(kind) && key==="position" ? !relative :
      ["relative_to","target","offset"].includes(key) && ["datum","attach"].includes(kind) ? relative : true;
    input.disabled=!visible;input.closest("label").hidden=!visible;
  }
}
function featureShapeOptions(current="") {
  const kind=$("feature-kind").value,select=$("feature-shape");
  const shapes=Object.keys(featureDefaults).filter(key=>key.startsWith(kind+":")).map(key=>key.slice(kind.length+1));
  if(current && !shapes.includes(current))shapes.push(current);
  select.replaceChildren();
  for(const value of shapes)select.add(new Option(value ? value.replaceAll("_"," ") : "Not applicable",value));
  select.value=shapes.includes(current) ? current : shapes[0]||"";
  $("feature-shape-label").hidden=shapes.length<=1;
  $("feature-name-label").hidden=["outline","rules"].includes(kind);
  $("feature-name").disabled=$("feature-name-label").hidden;
  $("draw-vertices-label").hidden=select.value!=="polygon";
  if(select.value!=="polygon")$("draw-vertices").checked=false;
  $("path-editor").hidden=!["outline","boundary"].includes(kind);
}
function featureMode() {
  const kind=$("feature-kind").value;
  const current=$("feature-shape").value;
  featureShapeOptions(featureDefaults[kind+":"+current] ? current : "");
  if ($("feature-name").disabled) $("feature-name").value="";
  featureInputs(featureDefaults[kind+":"+$("feature-shape").value]||{});vertexPoints=[];render();
}
$("feature-kind").onchange=featureMode;$("feature-shape").onchange=featureMode;
$("feature").onchange=()=>{
  const f=accepted.mechanical_features?.[Number($("feature").value)];
  if ($("feature").value==="" || !f) {featureMode();return;}
  $("feature-kind").value=f.kind;$("feature-name").value=f.name;featureShapeOptions(f.shape||"");
  featureInputs(f.parameters);vertexPoints=[];render();
};
function featureRequest(remove=false) {
  const parameters=Object.fromEntries([...$("feature-parameters").querySelectorAll("[data-property]")].filter(n=>!n.disabled && n.value.trim()).map(n=>[n.dataset.property,n.value.trim()]));
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
  if (event.key==="Escape") {cancelGesture();measurePoints=[];vertexPoints=[];if(!busy && preview?.preview_editable)operation("discard");else render();return;}
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase()==="z") {
    event.preventDefault();
    operation(event.shiftKey ? "redo" : "undo");
    return;
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase()==="y") {
    event.preventDefault();operation("redo");return;
  }
  if(event.key==="+" || event.key==="="){event.preventDefault();zoom(.8);return;}
  if(event.key==="-"){event.preventDefault();zoom(1.25);return;}
  if(event.key.toLowerCase()==="f" || event.key==="Home"){event.preventDefault();resetFit();render();return;}
  if(scene()?.document_kind && scene().document_kind!=="board")return;
  const c=scene()?.components.find(c=>c.reference===selected);
  if((event.key.toLowerCase()==="l" || event.key.toLowerCase()==="u") && c && !busy && !preview && !accepted?.source_review) {
    const lock=event.key.toLowerCase()==="l" && !event.shiftKey;
    const unlock=event.key.toLowerCase()==="u" || (event.key.toLowerCase()==="l" && event.shiftKey);
    if((lock && c.session_locked) || (unlock && !c.session_locked)) return;
    event.preventDefault();operation("lock",{reference:selected,locked:lock},true);
    return;
  }
  if(event.key.toLowerCase()==="r" && c && !busy && (!preview || preview.preview_editable) && !accepted?.source_review && !c.session_locked && !c.source_rotation_locked) {
    event.preventDefault();rotateSelected(event.shiftKey?-1:1);
    return;
  }
  if(event.key.toLowerCase()==="s" && c && !busy && (!preview || preview.preview_editable) && !accepted?.source_review && !c.session_locked && !c.source_side_locked) {
    event.preventDefault();operation("move",{reference:selected,x_nm:c.position[0],y_nm:c.position[1],rotation:c.rotation,side:c.side==="front"?"back":"front"},true);
    return;
  }
  const delta={ArrowLeft:[-1,0],ArrowRight:[1,0],ArrowUp:[0,-1],ArrowDown:[0,1]}[event.key];
  if (!delta || busy || (preview && !preview.preview_editable) || accepted?.source_review) return;
  event.preventDefault();const step=Math.round(Number($("snap").value)*1e6)*(event.shiftKey?10:1);
  const group=expandedSelection();
  if (group.size>1) {
    operation("move_group",{references:[...group],dx_nm:delta[0]*step,dy_nm:delta[1]*step},true);
    return;
  }
  if (!c || c.session_locked || c.source_position_locked) return;
  operation("move",{reference:selected,x_nm:c.position[0]+delta[0]*step,y_nm:c.position[1]+delta[1]*step,rotation:c.rotation,side:c.side},true);
});
// All physical source forms use the same reviewed transaction controls.
let physicalFormRevision="", physicalStaged=[];
function renderPhysicalGeometry(svg,s) {
  const side=$("side").value, selectedNets=new Set(s.components.filter(c=>selectionSet().has(c.reference)).flatMap(c=>c.pads.map(p=>p.net)));
  for(const feature of s.physical_geometry||[]) {
    if(feature.layers && $("copper-layer").value && !feature.layers.includes($("copper-layer").value))continue;
    if(feature.side && feature.side!=="both" && side!=="both" && feature.side!==side)continue;
    if(side!=="both" && feature.layers?.length && feature.layers.every(layer=>layer===(side==="front"?"B.Cu":"F.Cu")))continue;
    const nets=feature.nets||[feature.net].filter(Boolean);
    if(nets.length && $("net").value && !nets.includes($("net").value))continue;
    if(nets.length && $("selected-only").checked && !nets.some(n=>selectedNets.has(n)))continue;
    if(nets.length && $("power-filter").value!=="all" && !nets.some(n=>(s.power_nets||[]).includes(n)===($("power-filter").value==="power")))continue;
    let drawing;
    const attrs={class:`physical-geometry physical-${feature.kind}`};
    if(feature.radius_nm!==undefined && feature.points)drawing=shape({spine:feature.points,radius_nm:feature.radius_nm},"physical-geometry physical-breakout");
    else if(feature.vertices) {
      const ring=vertices=>`M ${points(vertices).replaceAll(" "," L ")} Z`;
      drawing=node("path",{...attrs,d:[ring(feature.vertices),...(feature.holes||[]).map(ring)].join(" "),"fill-rule":"evenodd"});
    }
    else if(feature.points)drawing=node("polyline",{...attrs,points:points(feature.points),fill:"none","stroke-width":feature.width_nm?mm(feature.width_nm):.15});
    else if(feature.position)drawing=node("circle",{...attrs,cx:mm(feature.position[0]),cy:mm(feature.position[1]),r:.35});
    if(!drawing)continue;
    drawing.append(node("title",{},`${feature.kind} ${feature.name||""}${feature.net?": "+feature.net:""}; source intent`));
    drawing.dataset.featureName=feature.name||"";
    drawing.addEventListener("click",event=>{
      if($("physical-pick-point").checked)return;
      event.stopPropagation();
      if(!selectPhysicalFeature(f=>f.category===feature.category && f.kind===feature.kind && f.name===(feature.source_name||feature.name)))
        selectPhysicalFeature(f=>f.category===feature.category && f.kind===(["breakout","routing_corridor"].includes(feature.kind)?"routing":feature.kind) && f.targets?.some(t=>(feature.targets||[feature.net]).includes(t)));
    });svg.append(drawing);
    if(feature.measured_nm!==undefined && feature.points?.length>=2) {
      const [a,b]=feature.points;
      const label=node("text",{x:mm(a[0]+b[0])/2,y:mm(a[1]+b[1])/2-.3,class:"physical-dimension"},`${mm(feature.measured_nm)} mm / ${mm(feature.limit_nm)} mm${feature.satisfied?"":" !"}`);
      svg.append(label);
    }
  }
  for(const role of s.connector_roles||[]) {
    const [x,y]=role.position.map(mm),g=node("g",{class:"connector-role"});
    g.append(node("circle",{cx:x,cy:y,r:.5}),node("text",{x:x+.7,y},role.name));
    g.addEventListener("click",()=>selectPhysicalFeature(f=>f.kind==="connector"&&f.name===role.name));svg.append(g);
  }
}
function activePhysicalFeature() {
  const index=$("physical-feature").value;
  return index!=="" ? accepted?.source_features?.[Number(index)] : accepted?.feature_catalog?.[Number($("physical-kind").value)];
}
function addPhysicalInput(key,value="") {
  if(document.getElementById("physical-param-"+key)) return;
  const label=document.createElement("label");label.textContent=key.replaceAll("_"," ");
  const input=document.createElement(["declaration","points","vertices","source","unresolved"].includes(key) ? "textarea" : "input");
  input.id="physical-param-"+key;input.dataset.property=key;input.value=value;
  if(input.tagName==="TEXTAREA")input.rows=key==="declaration"?10:3;
  label.append(input);$("physical-parameters").append(label);
}
function loadPhysicalForm() {
  const f=activePhysicalFeature();$("physical-parameters").replaceChildren();
  $("physical-name").value=f?.name||"";
  $("physical-name").disabled=!!$("physical-feature").value;
  $("physical-name-label").hidden=["constraint","stackup"].includes(f?.category) || f?.kind==="properties";
  $("physical-targets-label").hidden=f?.category!=="constraint";
  $("physical-targets").value=(f?.targets||[]).join(", ");
  $("physical-targets").disabled=!!$("physical-feature").value;
  $("physical-parent-label").hidden=f?.kind!=="contact";
  $("physical-parent").value=f?.parent||"";
  $("physical-polygon").hidden=f?.kind!=="copper_zone";
  for(const [key,value] of Object.entries(f?.parameters||{})) addPhysicalInput(key,value);
  $("physical-owner").textContent=f?.editable===false ? `Read only. ${f.owner||f.source||"Imported definition"}` : "Changes update this document after review and save.";
  $("physical-help").textContent=f?.description || "Pick a canvas point to replace a position or a selected vertex. Numeric edits preserve exact source units.";
  $("physical-contracts").textContent=f?.contracts ? JSON.stringify(f.contracts,null,2) : "";
  $("physical-open-definition").hidden=!f?.definition_path;
  updateControls();
}
function populatePhysicalFeatures() {
  const features=$("physical-feature"),old=features.value, kinds=$("physical-kind"),kind=kinds.value;
  features.replaceChildren(new Option("New feature",""));
  for(const [i,f] of (accepted.source_features||[]).entries())features.add(new Option(`${f.category}: ${f.kind} ${f.name||""}${f.editable===false?" (read only)":""}`,String(i)));
  features.value=old;
  if(features.selectedIndex<0)features.value="";
  kinds.replaceChildren();
  for(const [i,f] of (accepted.feature_catalog||[]).entries())kinds.add(new Option(`${f.category}: ${f.kind}${f.shape?" / "+f.shape:""}`,String(i)));
  kinds.value=kind;if(kinds.selectedIndex<0)kinds.selectedIndex=0;
  if(physicalFormRevision!==accepted.source_revision) {physicalStaged=[];physicalFormRevision=accepted.source_revision;loadPhysicalForm();}
  showStagedFeatures();
  const stack=$("physical-stackup");stack.replaceChildren();
  for(const layer of accepted.stackup_layers||[]) {
    const row=document.createElement("div");row.className="stackup-layer stackup-"+layer.kind;
    row.textContent=`${layer.name}: ${mm(layer.thickness_nm)} mm${layer.material?" · "+layer.material:""}${layer.er?" · εr "+layer.er:""}`;
    stack.append(row);
  }
}
function physicalFields(remove=false) {
  const f=activePhysicalFeature();if(!f)throw new Error("Choose a physical feature type");
  const parameters={};
  for(const input of $("physical-parameters").querySelectorAll("[data-property]"))if(input.value.trim())parameters[input.dataset.property]=input.value.trim();
  const request={category:f.category,kind:f.kind,name:$("physical-name").value.trim(),parameters};
  if(remove)request.remove=true;
  if(f.shape)request.shape=f.shape;
  if(f.parent || f.kind==="contact")request.parent=$("physical-parent").value.trim();
  if(f.category==="constraint") {request.name=f.name||"";request.targets=$("physical-targets").value.split(",").map(v=>v.trim()).filter(Boolean);}
  return request;
}
function selectPhysicalFeature(predicate) {
  const index=(accepted?.source_features||[]).findIndex(predicate);if(index<0)return false;
  $("physical-feature").value=String(index);loadPhysicalForm();switchTab("features");return true;
}
$("physical-feature").onchange=loadPhysicalForm;
$("physical-kind").onchange=()=>{$("physical-feature").value="";loadPhysicalForm();};
$("physical-polygon").onclick=()=>{
  for(const key of ["x","y","width","height","region","inset"])$("physical-param-"+key)?.closest("label").remove();
  addPhysicalInput("polygon_mm",'""');$("physical-param-polygon_mm").value='""';
  $("physical-point-index").value=0;$("physical-pick-point").checked=true;
  status("Click at least three boundary vertices, then review the zone. This replaces its previous boundary mode.");
};
$("physical-add-property").onclick=()=>{
  const name=$("physical-property-name").value.trim();
  if(!/^[a-zA-Z_][a-zA-Z_0-9]*$/.test(name)){status("Enter a property name",true);return;}
  addPhysicalInput(name);$("physical-property-name").value="";
};
for(const [id,remove] of [["physical-review",false],["physical-remove",true]])$(id).onclick=()=>{
  try {operation("review_feature",physicalFields(remove));}catch(error){status(error.message,true);}
};
function showStagedFeatures() {
  $("physical-staged").hidden=!physicalStaged.length;
  $("physical-staged-count").textContent=physicalStaged.map((f,i)=>`${i+1}. ${f.remove?"Remove":"Update"} ${f.kind} ${f.name}`).join("; ");
}
for(const [id,remove] of [["physical-stage",false],["physical-stage-remove",true]])$(id).onclick=()=>{
  try {
    if(physicalStaged.length>=64)throw new Error("Review the current 64 staged changes first");
    physicalStaged.push(physicalFields(remove));showStagedFeatures();updateControls();
    status("Change staged. Stage any related changes, then review them together.");
  }catch(error){status(error.message,true);}
};
$("physical-review-staged").onclick=()=>operation("review_features",{features:physicalStaged});
$("physical-clear-staged").onclick=()=>{physicalStaged=[];showStagedFeatures();updateControls();};
$("physical-open-definition").onclick=async()=>{
  const f=activePhysicalFeature();if(!f?.definition_path)return;
  const view=vscodeHost?null:window.open("about:blank","_blank");
  if(view)view.opener=null;
  try {
    const result=await api("/api/open-definition",{feature_index:Number($("physical-feature").value)});
    if(view)view.location.href=result.url;
    else if(!vscodeHost)status("Allow popups to open the definition view.",true);
  }catch(error){view?.close();status(error.message,true);}
};
$("board").addEventListener("click",event=>{
  if(!$("physical-pick-point").checked || busy || accepted?.source_review || accepted?.source_stale)return;
  try {
    const p=snappedPoint(event), literal=`(${p[0]} mm, ${p[1]} mm)`;
    const single=["at","point","position","center","start","end"].map(k=>$("physical-param-"+k)).find(Boolean);
    const vertices=$("physical-param-vertices")||$("physical-param-points");
    const zone=$("physical-param-polygon_mm");
    if(zone) {
      const raw=JSON.parse(zone.value), entries=raw?raw.split(";"):[];
      const index=Number($("physical-point-index").value);
      if(!Number.isInteger(index)||index<0||index>entries.length)throw new Error("Choose an existing vertex or the next index to append");
      const append=index===entries.length;entries[index]=p.join(",");zone.value=JSON.stringify(entries.join("; "));
      if(append)$("physical-point-index").value=index+1;
    } else if(vertices) {
      // Split only at top-level commas, preserving pad references and exact literals.
      const value=vertices.value.trim();if(!value.startsWith("[")||!value.endsWith("]"))throw new Error("Expected a point list");
      const entries=[];let depth=0,quoted=false,escape=false,start=1;
      for(let i=1;i<value.length-1;i++) {
        const c=value[i];if(escape){escape=false;continue;}if(c==="\\"&&quoted){escape=true;continue;}
        if(c==='"'){quoted=!quoted;continue;}if(quoted)continue;
        if(c==="(")depth++;else if(c===")")depth--;else if(c===","&&depth===0){entries.push(value.slice(start,i).trim());start=i+1;}
      }
      if(value.slice(start,-1).trim())entries.push(value.slice(start,-1).trim());
      const index=Number($("physical-point-index").value);
      if(!Number.isInteger(index)||index<0||index>entries.length)throw new Error("Choose an existing point index, or the next index to append");
      if(entries[index]&&!entries[index].startsWith("("))throw new Error("This point is a pad reference. Edit its literal explicitly to detach it.");
      const append=index===entries.length;entries[index]=literal;vertices.value="["+entries.join(", ")+"]";
      if(append)$("physical-point-index").value=index+1;
    } else if(single) single.value=literal;
    else if($("physical-param-x") && $("physical-param-y")) {$("physical-param-x").value=`${p[0]} mm`;$("physical-param-y").value=`${p[1]} mm`;}
    else throw new Error("This feature has no point property; add one or use numeric fields");
    status("Point updated in the form. Review changes to validate the geometry.");
  } catch(error){status(error.message,true);}
});
featureMode();
updateControls();reloadScene();
