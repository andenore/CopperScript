"use strict";

// The links still open the native illustrations when JavaScript is unavailable.
const views = {
  all: { image: "assets/routing-all.svg", alt: "All six copper layers of CopperAssetTracker with tracks, vias and gold exposed copper, drawn without zone fills", caption: "All 6 copper layers · tracks, vias and exposed copper in gold, from a plot made before zone fill. Viewed from above." },
  top: { image: "assets/routing-top.svg", alt: "F.Cu top copper and silkscreen of CopperAssetTracker rendered from the native plot, exposed copper gold", caption: "F.Cu · top copper, zone fill and silkscreen from the native plot; exposed copper in gold. native-showcase-20261107; unapproved routing review." },
  inner: { image: "assets/routing-inner.svg", alt: "In2.Cu inner supply and signal routing of CopperAssetTracker from the native plot, viewed from above", caption: "In2.Cu · supply and signal routing from the same route. Viewed from above without mirroring; return-path qualification remains open." },
  bottom: { image: "assets/routing-bottom.svg", alt: "B.Cu bottom copper of CopperAssetTracker from the native plot, exposed copper gold, viewed from above", caption: "B.Cu · bottom copper from the same route, exposed copper in gold. Viewed from above without mirroring. A single layer does not show every connection." },
  placement: { image: "assets/placement.svg", alt: "Front silkscreen and gold solder-mask openings of the 87 CopperAssetTracker footprints at their routed poses", caption: "Placement · front silkscreen and solder-mask openings (gold) of 87 footprints at their routed poses, from the native plot." }
};

// Keep browser caches from mixing previous artwork with a new snapshot.
for (const view of Object.values(views)) view.image += "?v=20261107-native";

const image = document.getElementById("board-view");
const caption = document.getElementById("board-caption");
const fullsize = document.getElementById("board-fullsize");
const links = document.querySelectorAll("[data-view]");
links.forEach(link => {
  link.addEventListener("click", event => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
    const view = views[link.dataset.view];
    if (!view || !image || !caption) return;
    event.preventDefault();
    image.src = view.image;
    image.alt = view.alt;
    caption.textContent = view.caption;
    if (fullsize) fullsize.href = view.image;
    links.forEach(item => item.removeAttribute("aria-current"));
    link.setAttribute("aria-current", "true");
  });
});

// CopperReview screenshots: the links open each image without JavaScript.
const shots = {
  layout: { alt: "CopperReview layout mode showing all copper layers of CopperAssetTracker, the component list and the note form", caption: "Layout · select components, nets or a region of the routed board, write what you observe and what should improve, and export the notes for the next solver iteration." },
  schematic: { alt: "CopperReview schematic sheet of the CopperAssetTracker MCU with the modem UART interface selected and its endpoints listed", caption: "Schematic · generated sheets, power hierarchy and buses from the compiled IR. Here the modem UART is selected, with its endpoints and the source documents of each part." },
  "3d": { alt: "CopperReview 3D view of CopperAssetTracker with component bodies on a green solder-mask board", caption: "3D · the board with KiCad's 3D models and generated bodies, orbit, pan and orthographic views." },
  assembly: { alt: "CopperReview Assembly mode listing CopperAssetTracker parts by assembly line, with the next part to place highlighted", caption: "Assembly · walk a manual build line by line from assembly.lock: what to place next, where, its supplier, and what is placed and inspected." }
};
const shotImage = document.getElementById("review-shot");
const shotCaption = document.getElementById("review-caption");
const shotLinks = document.querySelectorAll("[data-shot]");
shotLinks.forEach(link => {
  link.addEventListener("click", event => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
    const shot = shots[link.dataset.shot];
    if (!shot || !shotImage || !shotCaption) return;
    event.preventDefault();
    shotImage.src = link.getAttribute("href");
    shotImage.alt = shot.alt;
    shotCaption.textContent = shot.caption;
    shotLinks.forEach(item => item.removeAttribute("aria-current"));
    link.setAttribute("aria-current", "true");
  });
});

if (navigator.clipboard && window.isSecureContext) {
  document.querySelectorAll("[data-copy]").forEach(button => {
    button.hidden = false;
    button.addEventListener("click", async () => {
      const source = document.getElementById(button.dataset.copy);
      if (!source) return;
      try {
        await navigator.clipboard.writeText(source.textContent.trim());
        button.textContent = "Copied";
      } catch {
        button.textContent = "Select text to copy";
      }
      window.setTimeout(() => { button.textContent = "Copy commands"; }, 2500);
    });
  });
}

// Antenna synthesis figure: recorded copper and return-loss curves from
// assets/antenna-synthesis.json (see scripts/update_site_antenna.cjs).
(() => {
  const root = document.getElementById("antenna-figure");
  if (!root || !window.fetch) return;
  const NS = "http://www.w3.org/2000/svg";
  const MATCHED = "#ad4b28", BARE = "#1f7fb0";
  const el = (name, attrs = {}, parent) => {
    const node = document.createElementNS(NS, name);
    for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
    if (parent) parent.appendChild(node);
    return node;
  };
  const text = (parent, x, y, content, attrs = {}) => { const t = el("text", {x, y, ...attrs}, parent); t.textContent = content; return t; };
  const ghz = hz => `${(hz / 1e9).toFixed(2)} GHz`;
  const db = value => `${value < 0 ? "−" : ""}${Math.abs(value).toFixed(1)} dB`;
  const polygon = points => points.map(p => p.join(",")).join(" ");

  function drawBoard(design, ghost) {
    const host = document.getElementById("antenna-board-svg");
    const b = design.board, [x0, y0] = [12, -1.5], [x1, y1] = [b.outline[2] + 1.5, 17.5];
    const svg = el("svg", {viewBox: `${x0} ${y0} ${x1 - x0} ${y1 - y0}`, role: "img", style: "overflow:hidden",
      "aria-label": `Top copper of the antenna corner: ${design.label.toLowerCase()}, ${design.geometry}`});
    el("rect", {x: b.outline[0], y: b.outline[1], width: b.outline[2] - b.outline[0], height: b.outline[3] - b.outline[1], fill: "#2f4a3f"}, svg);
    for (const fill of b.fills.filter(f => f.layer === "F.Cu")) el("polygon", {points: polygon(fill.points), fill: "#5f7d60"}, svg);
    for (const keepout of b.keepouts) el("polygon", {points: polygon(keepout), fill: "none", stroke: "#e8d9b8", "stroke-width": .15, "stroke-dasharray": ".6 .4"}, svg);
    if (ghost) {
      const arm = ghost.board.tracks.filter(t => t.locked);
      for (const t of arm)
        el("line", {x1: t.from[0], y1: t.from[1], x2: t.to[0], y2: t.to[1], stroke: "#f2e6cc", "stroke-width": .18, "stroke-dasharray": ".5 .35", "stroke-linecap": "round"}, svg);
      const end = arm.at(-1)?.to;
      if (end) text(svg, end[0] + .6, end[1] + .35, "template", {"font-family": "Consolas, monospace", "font-size": .95, fill: "#f2e6cc"});
    }
    for (const t of b.tracks.filter(t => t.layer === "F.Cu"))
      el("line", {x1: t.from[0], y1: t.from[1], x2: t.to[0], y2: t.to[1], stroke: t.locked ? "#d9a441" : "#c7b48a", "stroke-width": t.width, "stroke-linecap": "round"}, svg);
    for (const pad of b.pads) el("polygon", {points: polygon(pad.points), fill: "#d9a441"}, svg);
    for (const via of b.vias) el("circle", {cx: via.at[0], cy: via.at[1], r: via.size / 2, fill: "#d9a441", stroke: "#2f4a3f", "stroke-width": .1}, svg);
    const label = {"font-family": "Consolas, monospace", "font-size": 1.05, fill: "#f3ecdc"};
    const keep = b.keepouts[0];
    if (keep) text(svg, Math.max(...keep.map(p => p[0])) - .5, Math.max(...keep.map(p => p[1])) + 1.5, "antenna region", {...label, "text-anchor": "end", fill: "#e8d9b8"});
    for (const pad of b.pads.filter(p => p.pad === "1")) {
      const [x, y] = pad.points[2];
      text(svg, pad.points[0][0] - .5, y + .9, pad.reference || "", {...label, "text-anchor": "end"});
    }
    host.replaceChildren(svg);
  }

  function drawChart(design, band) {
    const host = document.getElementById("antenna-chart-svg");
    const W = 440, H = 300, m = {l: 40, r: 70, t: 12, b: 34};
    const curve = design.curve, fx = curve.map(p => p[0]);
    const [fmin, fmax] = [Math.min(...fx), Math.max(...fx)];
    // A perfect match reads as a very deep dip; the axis stops at -30 dB.
    const ymin = -30;
    const X = f => m.l + (f - fmin) / (fmax - fmin) * (W - m.l - m.r), Y = v => m.t + (0 - v) / (0 - ymin) * (H - m.t - m.b);
    const svg = el("svg", {viewBox: `0 0 ${W} ${H}`, role: "img",
      "aria-label": `Return loss from ${ghz(fmin)} to ${ghz(fmax)}: ${design.label}. Table view below.`});
    const font = {"font-family": "Consolas, monospace", "font-size": 10, fill: "#626962"};
    el("rect", {x: X(band[0]), y: m.t, width: X(band[1]) - X(band[0]), height: H - m.t - m.b, fill: "#e0d6c2"}, svg);
    text(svg, (X(band[0]) + X(band[1])) / 2, m.t + 11, "band", {...font, "text-anchor": "middle", fill: "#7a5a3a"});
    for (let v = 0; v >= ymin; v -= 5) {
      el("line", {x1: m.l, x2: W - m.r, y1: Y(v), y2: Y(v), stroke: v === -10 ? "#7a5a3a" : "#d6d8cc", "stroke-width": 1, "stroke-dasharray": v === -10 ? "4 3" : ""}, svg);
      text(svg, m.l - 6, Y(v) + 3.5, v === 0 ? "0" : `−${-v}`, {...font, "text-anchor": "end"});
    }
    const step = (fmax - fmin) > 1.2e9 ? 0.5e9 : 0.2e9;
    for (let f = Math.ceil(fmin / step) * step; f <= fmax + 1; f += step) {
      el("line", {x1: X(f), x2: X(f), y1: H - m.b, y2: H - m.b + 4, stroke: "#9aa094"}, svg);
      text(svg, X(f), H - m.b + 16, (f / 1e9).toFixed(1), {...font, "text-anchor": "middle"});
    }
    text(svg, (m.l + W - m.r) / 2, H - 3, "GHz", {...font, "text-anchor": "middle"});
    const clip = `antenna-clip-${Math.random().toString(36).slice(2)}`;
    el("rect", {x: m.l, y: m.t - 2, width: W - m.l - m.r, height: H - m.t - m.b + 2}, el("clipPath", {id: clip}, el("defs", {}, svg)));
    const plot = el("g", {"clip-path": `url(#${clip})`}, svg);
    const path = index => curve.map((p, i) => `${i ? "L" : "M"}${X(p[0]).toFixed(1)},${Y(Math.max(p[index], ymin - 2)).toFixed(1)}`).join("");
    el("path", {d: path(1), fill: "none", stroke: BARE, "stroke-width": 2, "stroke-dasharray": "5 3", "stroke-linejoin": "round"}, plot);
    el("path", {d: path(2), fill: "none", stroke: MATCHED, "stroke-width": 2, "stroke-linejoin": "round"}, plot);
    // Direct labels at the right end, spread so they never overlap.
    const last = curve.at(-1);
    const labels = [[Y(last[2]), "matched"], [Y(last[1]), "bare"], [Y(-10), "target"]].sort((a, b) => a[0] - b[0]);
    for (let i = 1; i < labels.length; i++) labels[i][0] = Math.max(labels[i][0], labels[i - 1][0] + 12);
    for (const [y, name] of labels) text(svg, W - m.r + 6, y + 3.5, name, {...font, fill: name === "target" ? "#7a5a3a" : "#252b29"});
    if (design.resonance_hz) {
      const r = design.resonance_hz;
      el("line", {x1: X(r), x2: X(r), y1: m.t, y2: H - m.b, stroke: "#252b29", "stroke-width": 1, opacity: .35}, svg);
      text(svg, X(r) - 5, H - m.b - 6, `resonance ${ghz(r)}`, {...font, fill: "#252b29", "text-anchor": "end"});
    }
    // Hover: a crosshair at the nearest recorded frequency.
    const cross = el("g", {visibility: "hidden"}, svg);
    const vline = el("line", {y1: m.t, y2: H - m.b, stroke: "#252b29", "stroke-width": 1}, cross);
    const dots = [MATCHED, BARE].map(color => el("circle", {r: 4, fill: color, stroke: "#ebece3", "stroke-width": 2}, cross));
    const box = el("rect", {width: 128, height: 46, fill: "#fcfcfb", stroke: "#cbd0c0"}, cross);
    const lines = [0, 1, 2].map(() => el("text", {class: "chart-tip"}, cross));
    const hit = el("rect", {x: m.l, y: m.t, width: W - m.l - m.r, height: H - m.t - m.b, fill: "transparent"}, svg);
    const move = event => {
      const point = svg.createSVGPoint(); point.x = event.clientX; point.y = event.clientY;
      const local = point.matrixTransform(svg.getScreenCTM().inverse());
      let best = curve[0];
      for (const p of curve) if (Math.abs(X(p[0]) - local.x) < Math.abs(X(best[0]) - local.x)) best = p;
      const x = X(best[0]);
      vline.setAttribute("x1", x); vline.setAttribute("x2", x);
      dots[0].setAttribute("cx", x); dots[0].setAttribute("cy", Y(best[2]));
      dots[1].setAttribute("cx", x); dots[1].setAttribute("cy", Y(best[1]));
      const bx = x + 136 > W ? x - 136 : x + 8, by = m.t + 18;
      box.setAttribute("x", bx); box.setAttribute("y", by);
      [`${(best[0] / 1e9).toFixed(3)} GHz`, `matched ${db(best[2])}`, `bare    ${db(best[1])}`].forEach((content, i) => {
        lines[i].textContent = content; lines[i].setAttribute("x", bx + 8); lines[i].setAttribute("y", by + 14 + i * 13);
      });
      cross.setAttribute("visibility", "visible");
    };
    hit.addEventListener("pointermove", move);
    hit.addEventListener("pointerdown", move);
    hit.addEventListener("pointerleave", () => cross.setAttribute("visibility", "hidden"));
    host.replaceChildren(svg);
  }

  function drawTable(design, band) {
    const table = document.createElement("table");
    table.innerHTML = "<thead><tr><th scope=\"col\">Frequency</th><th scope=\"col\">Bare</th><th scope=\"col\">Matched</th></tr></thead>";
    const body = document.createElement("tbody");
    for (const [f, bare, matched] of design.curve) {
      const row = body.insertRow();
      if (f >= band[0] && f <= band[1]) row.className = "in-band";
      for (const value of [`${(f / 1e9).toFixed(3)} GHz`, db(bare), db(matched)]) row.insertCell().textContent = value;
    }
    table.appendChild(body);
    document.getElementById("antenna-table").replaceChildren(table);
  }

  function show(data, key) {
    const design = data.designs[key], other = data.designs[key === "synthesized" ? "template" : "synthesized"];
    const bandText = design.bands[0].band, band = bandText.match(/[\d.]+/g).map(v => Number(v) * 1e6);
    document.getElementById("antenna-geometry").textContent = design.geometry;
    document.getElementById("antenna-resonance").textContent = design.resonance_hz ? ghz(design.resonance_hz) : "none in sweep";
    const worst = Math.max(...design.bands.map(b => b.matched_worst_db));
    document.getElementById("antenna-worst").textContent = db(worst);
    document.getElementById("antenna-match").textContent = design.matching_network.map(e => `${e.value.toFixed(2)} ${e.unit}`).join(" · ") || "none";
    document.getElementById("antenna-match-label").textContent = design.matching_network
      .map(e => `${e.position} ${e.kind === "inductor" ? "L" : "C"}`).join(", then ") || "no match fitted";
    const efficiency = design.radiation_efficiency;
    document.getElementById("antenna-efficiency").textContent = efficiency.length
      ? `${Math.round(100 * efficiency.reduce((s, e) => s + e.radiation_efficiency, 0) / efficiency.length)} %` : "not computed";
    drawBoard(design, key === "synthesized" ? other : null);
    drawChart(design, band);
    drawTable(design, band);
    root.querySelectorAll("[data-design]").forEach(button => button.setAttribute("aria-pressed", String(button.dataset.design === key)));
  }

  fetch("assets/antenna-synthesis.json?v=20261110-rev6").then(r => r.ok ? r.json() : Promise.reject(r.status)).then(data => {
    const controls = document.getElementById("antenna-controls");
    controls.hidden = false;
    controls.querySelectorAll("[data-design]").forEach(button => button.addEventListener("click", () => show(data, button.dataset.design)));
    show(data, "synthesized");
  }).catch(() => {});
})();

// EMI screening pages: copper emi-check --plot-summary output, copied byte for
// byte (scripts/update_site_analysis.cjs). The links open each SVG without JavaScript.
(() => {
  const pages = {
    bare: { alt: "copper emi-check summary of the buck converter without an input filter: hot loop 4.97 nH, conducted emission 49.6 dB over the average limit with 55 of 60 harmonics over, near-field map and conducted spectrum", caption: "No input filter, worst case at the 4.5 A current limit · 95.6 dBµV at 500 kHz, 49.6 dB over the CISPR 32 class B average limit; 55 of 60 harmonics over." },
    filter: { alt: "copper emi-check summary of the same buck converter with a 4.7 µH and 10 µF input filter at a 2 A load: conducted emission 6.4 dB under the average limit, no harmonic over, near-field map, conducted spectrum and input-network impedance", caption: "4.7 µH and 10 µF input filter, declared 2 A load · 39.6 dBµV at 500 kHz, 6.4 dB under the average limit; 0 of 60 harmonics over. Same placement and hot loop." }
  };
  const image = document.getElementById("emi-shot"), caption = document.getElementById("emi-caption"), fullsize = document.getElementById("emi-fullsize");
  const links = document.querySelectorAll("[data-emi]");
  links.forEach(link => link.addEventListener("click", event => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
    const page = pages[link.dataset.emi];
    if (!page || !image || !caption) return;
    event.preventDefault();
    image.src = link.getAttribute("href");
    image.alt = page.alt;
    caption.textContent = page.caption;
    if (fullsize) fullsize.href = link.getAttribute("href");
    links.forEach(item => item.removeAttribute("aria-current"));
    link.setAttribute("aria-current", "true");
  }));
})();

// Noise density of the preamp: the recorded curves of a copper sim run in
// assets/simulation-noise.json (see scripts/update_site_analysis.cjs).
(() => {
  const host = document.getElementById("sim-chart-svg");
  if (!host || !window.fetch) return;
  const NS = "http://www.w3.org/2000/svg";
  const el = (name, attrs = {}, parent) => {
    const node = document.createElementNS(NS, name);
    for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
    if (parent) parent.appendChild(node);
    return node;
  };
  const text = (parent, x, y, content, attrs = {}) => { const t = el("text", {x, y, ...attrs}, parent); t.textContent = content; return t; };
  const hz = f => f >= 1e6 ? `${+(f / 1e6).toPrecision(3)} MHz` : f >= 1e3 ? `${+(f / 1e3).toPrecision(3)} kHz` : `${+f.toPrecision(3)} Hz`;
  const nv = v => v >= 1e-6 ? `${(v * 1e6).toPrecision(3)} µV/√Hz` : `${(v * 1e9).toPrecision(3)} nV/√Hz`;
  const SERIES = [
    {index: 1, name: "total", color: "#ad4b28", width: 2.5, dash: ""},
    {index: 2, name: "sensor", color: "#1f7fb0", width: 1.6, dash: "5 3"},
    {index: 3, name: "R_G", color: "#5f7d60", width: 1.6, dash: "5 3"},
    {index: 4, name: "op-amp", color: "#6b5aa6", width: 1.6, dash: "2 3"}
  ];

  function draw(data) {
    const curve = data.curve.filter(p => p[0] >= 10);
    const W = 520, H = 300, m = {l: 48, r: 64, t: 12, b: 34};
    const [fmin, fmax] = [curve[0][0], curve.at(-1)[0]];
    const [vmin, vmax] = [1e-8, 1e-6];
    const lx = Math.log10, X = f => m.l + (lx(f) - lx(fmin)) / (lx(fmax) - lx(fmin)) * (W - m.l - m.r);
    const Y = v => m.t + (lx(vmax) - lx(Math.min(Math.max(v, vmin / 2), vmax * 2))) / (lx(vmax) - lx(vmin)) * (H - m.t - m.b);
    const svg = el("svg", {viewBox: `0 0 ${W} ${H}`, role: "img",
      "aria-label": `Output noise density of the preamp from ${hz(fmin)} to ${hz(fmax)}, total and the sensor, R_G and op-amp contributions. The key values are listed below the chart.`});
    const font = {"font-family": "Consolas, monospace", "font-size": 10, fill: "#626962"};
    for (const v of [1e-8, 3e-8, 1e-7, 3e-7, 1e-6]) {
      el("line", {x1: m.l, x2: W - m.r, y1: Y(v), y2: Y(v), stroke: "#d6d8cc"}, svg);
      text(svg, m.l - 6, Y(v) + 3.5, v >= 1e-6 ? "1 µ" : `${Math.round(v * 1e9)} n`, {...font, "text-anchor": "end"});
    }
    for (let f = 10; f <= fmax * 1.01; f *= 10) {
      el("line", {x1: X(f), x2: X(f), y1: m.t, y2: H - m.b, stroke: "#e1e2d8"}, svg);
      text(svg, X(f), H - m.b + 16, hz(f).replace(" ", ""), {...font, "text-anchor": "middle"});
    }
    // The limit of en_out_1khz, drawn where the bench measures it.
    const limit = 75e-9;
    el("line", {x1: X(1000) - 16, x2: X(1000) + 16, y1: Y(limit), y2: Y(limit), stroke: "#7a5a3a", "stroke-width": 1.5, "stroke-dasharray": "4 3"}, svg);
    text(svg, X(1000), Y(limit) - 6, "limit at 1 kHz", {...font, "text-anchor": "middle", fill: "#7a5a3a"});
    for (const s of SERIES) {
      const d = curve.map((p, i) => `${i ? "L" : "M"}${X(p[0]).toFixed(1)},${Y(p[s.index]).toFixed(1)}`).join("");
      el("path", {d, fill: "none", stroke: s.color, "stroke-width": s.width, "stroke-dasharray": s.dash, "stroke-linejoin": "round"}, svg);
    }
    // Direct labels at the 1 kHz to 10 kHz plateau, spread so they never overlap.
    const anchor = curve.find(p => p[0] >= 3000) || curve.at(-1);
    const labels = SERIES.map(s => [Y(anchor[s.index]), s]).sort((a, b) => a[0] - b[0]);
    for (let i = 1; i < labels.length; i++) labels[i][0] = Math.max(labels[i][0], labels[i - 1][0] + 12);
    for (const [y, s] of labels) text(svg, W - m.r + 6, y + 3.5, s.name, {...font, fill: s.color});
    const cross = el("g", {visibility: "hidden"}, svg);
    const vline = el("line", {y1: m.t, y2: H - m.b, stroke: "#252b29"}, cross);
    const dots = SERIES.map(s => el("circle", {r: 3.5, fill: s.color, stroke: "#ebece3", "stroke-width": 1.5}, cross));
    const box = el("rect", {width: 150, height: 72, fill: "#fcfcfb", stroke: "#cbd0c0"}, cross);
    const lines = [0, 1, 2, 3, 4].map(() => el("text", {class: "chart-tip"}, cross));
    const hit = el("rect", {x: m.l, y: m.t, width: W - m.l - m.r, height: H - m.t - m.b, fill: "transparent"}, svg);
    const move = event => {
      const point = svg.createSVGPoint(); point.x = event.clientX; point.y = event.clientY;
      const local = point.matrixTransform(svg.getScreenCTM().inverse());
      let best = curve[0];
      for (const p of curve) if (Math.abs(X(p[0]) - local.x) < Math.abs(X(best[0]) - local.x)) best = p;
      const x = X(best[0]);
      vline.setAttribute("x1", x); vline.setAttribute("x2", x);
      SERIES.forEach((s, i) => { dots[i].setAttribute("cx", x); dots[i].setAttribute("cy", Y(best[s.index])); });
      const bx = x + 158 > W ? x - 158 : x + 8, by = m.t + 6;
      box.setAttribute("x", bx); box.setAttribute("y", by);
      [hz(best[0]), ...SERIES.map(s => `${s.name.padEnd(7)}${nv(best[s.index])}`)].forEach((content, i) => {
        lines[i].textContent = content; lines[i].setAttribute("x", bx + 8); lines[i].setAttribute("y", by + 14 + i * 13);
      });
      cross.setAttribute("visibility", "visible");
    };
    hit.addEventListener("pointermove", move);
    hit.addEventListener("pointerdown", move);
    hit.addEventListener("pointerleave", () => cross.setAttribute("visibility", "hidden"));
    text(svg, (m.l + W - m.r) / 2, H - 3, "frequency", {...font, "text-anchor": "middle"});
    host.replaceChildren(svg);
  }

  fetch("assets/simulation-noise.json?v=20261123").then(r => r.ok ? r.json() : Promise.reject(r.status)).then(draw).catch(() => {});
})();
