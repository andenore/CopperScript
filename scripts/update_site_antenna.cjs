#!/usr/bin/env node
/* Write site/assets/antenna-synthesis.json, the data behind the promo site's
   antenna figure, from recorded `copper synthesize-antenna` reports and the
   natively filled boards of the same example. Nothing is simulated here: the
   curves, matching networks and copper are copied from those files, rounded
   for size, with the SHA-256 of every input.

   node scripts/update_site_antenna.cjs \
     --template-report build/promo/antenna-template.json --template-pcb build/promo/template.filled.kicad_pcb \
     --synthesized-report build/promo/antenna-verify.json --synthesized-pcb build/promo/synth.filled.kicad_pcb \
     [-o site/assets/antenna-synthesis.json]

   See site/README.md for the commands that produce the inputs. */
"use strict";
const crypto = require("node:crypto"), fs = require("node:fs"), path = require("node:path");

const ROOT = path.resolve(__dirname, "..");
const USAGE = "usage: update_site_antenna.cjs --template-report R --template-pcb P --synthesized-report R " +
  "--synthesized-pcb P [-o OUTPUT]";

function options(argv) {
  const known = ["--template-report", "--template-pcb", "--synthesized-report", "--synthesized-pcb", "-o"];
  const result = {"-o": path.join(ROOT, "site/assets/antenna-synthesis.json")};
  for (let i = 0; i < argv.length; i += 2) {
    if (!known.includes(argv[i]) || argv[i + 1] === undefined) throw new Error(USAGE);
    result[argv[i]] = argv[i + 1];
  }
  for (const required of known.slice(0, 4)) if (!result[required]) throw new Error(USAGE);
  return result;
}

/* A KiCad s-expression as nested arrays of atoms. */
function parse(text) {
  const stack = [[]];
  for (const token of text.matchAll(/"((?:[^"\\]|\\.)*)"|([()])|([^\s()"]+)/g)) {
    if (token[2] === "(") stack.push([]);
    else if (token[2] === ")") { const list = stack.pop(); stack.at(-1).push(list); }
    else stack.at(-1).push(token[1] !== undefined ? token[1] : token[3]);
  }
  return stack[0][0];
}

const children = (node, name) => node.filter(c => Array.isArray(c) && c[0] === name);
const child = (node, name) => children(node, name)[0];
const num = value => Number(value);
const round = value => Math.round(value * 1000) / 1000;
const points = node => children(child(node, "pts"), "xy").map(xy => [round(num(xy[1])), round(num(xy[2]))]);

function board(file) {
  const pcb = parse(fs.readFileSync(file, "utf8"));
  const edges = children(pcb, "gr_line").filter(l => child(l, "layer")[1] === "Edge.Cuts")
    .flatMap(l => [child(l, "start"), child(l, "end")]).map(p => [num(p[1]), num(p[2])]);
  const xs = edges.map(p => p[0]), ys = edges.map(p => p[1]);
  const fills = [], keepouts = [];
  for (const zone of children(pcb, "zone")) {
    if (child(zone, "keepout")) { keepouts.push(points(child(zone, "polygon"))); continue; }
    for (const filled of children(zone, "filled_polygon"))
      fills.push({layer: child(filled, "layer")[1], points: points(filled)});
  }
  const tracks = children(pcb, "segment").map(s => ({
    from: [round(num(child(s, "start")[1])), round(num(child(s, "start")[2]))],
    to: [round(num(child(s, "end")[1])), round(num(child(s, "end")[2]))],
    width: num(child(s, "width")[1]), layer: child(s, "layer")[1], locked: Boolean(child(s, "locked"))
  }));
  const vias = children(pcb, "via").map(v => ({at: [num(child(v, "at")[1]), num(child(v, "at")[2])], size: num(child(v, "size")[1])}));
  const pads = [];
  for (const footprint of children(pcb, "footprint")) {
    const at = child(footprint, "at"), [fx, fy, fr] = [num(at[1]), num(at[2]), num(at[3] || 0)];
    const reference = children(footprint, "property").find(p => p[1] === "Reference")?.[2] ?? "";
    for (const pad of children(footprint, "pad")) {
      const pat = child(pad, "at"), size = child(pad, "size");
      const [px, py] = [num(pat[1]), num(pat[2])], [w, h] = [num(size[1]), num(size[2] ?? size[1])];
      const angle = -(fr + num(pat[3] || 0)) * Math.PI / 180, cos = Math.cos(angle), sin = Math.sin(angle);
      const fa = -fr * Math.PI / 180, cx = fx + px * Math.cos(fa) - py * Math.sin(fa), cy = fy + px * Math.sin(fa) + py * Math.cos(fa);
      const corners = [[-w / 2, -h / 2], [w / 2, -h / 2], [w / 2, h / 2], [-w / 2, h / 2]]
        .map(([u, v]) => [round(cx + u * cos - v * sin), round(cy + u * sin + v * cos)]);
      pads.push({reference, pad: pad[1], points: corners});
    }
  }
  return {outline: [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)], fills, keepouts, tracks, vias, pads};
}

/* The report writes fixed-decimal numbers as strings; the site wants numbers. */
const n = value => (value === null || value === undefined ? null : Number(value));

function report(file) {
  const r = JSON.parse(fs.readFileSync(file, "utf8"));
  if (r.schema !== "copperscript-antenna-synthesis/v0.1") throw new Error(`${file}: not an antenna synthesis report`);
  return {
    geometry: r.geometry, method: r.method, final_fidelity: r.final_fidelity, converged: r.converged,
    resonance_hz: n(r.resonance_hz),
    bands: r.bands.map(b => ({band: b.band, bare_worst_db: n(b.bare_worst_db), matched_worst_db: n(b.matched_worst_db),
      target_db: n(b.target_db), meets_target: b.meets_target})),
    matching_network: r.matching_network.map(e => ({position: e.position, kind: e.kind, value: n(e.value), unit: e.unit})),
    radiation_efficiency: r.radiation_efficiency.map(e => ({frequency_hz: n(e.frequency_hz), radiation_efficiency: n(e.radiation_efficiency)})),
    curve: r.curve.map(p => [n(p.frequency_hz), n(p.s11_db), n(p.matched_s11_db)]),
    solver_warnings: r.solver_warnings.length, site_digest: r.site_digest, generator: r.generator, solver: r.solver,
    assumptions: r.assumptions, unresolved: r.unresolved
  };
}

const sha256 = file => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const relative = file => path.relative(ROOT, path.resolve(file)).split(path.sep).join("/");

function main() {
  const o = options(process.argv.slice(2));
  const inputs = {};
  for (const key of ["--template-report", "--template-pcb", "--synthesized-report", "--synthesized-pcb"])
    inputs[relative(o[key])] = sha256(o[key]);
  const document = {
    schema: "copperscript-site-antenna/v1",
    example: "examples/antenna_synthesis/board.copper",
    note: "Simulation evidence for one model of a synthetic two-component probe board. Not a qualified radio design.",
    inputs,
    designs: {
      template: {label: "First-order template", ...report(o["--template-report"]), board: board(o["--template-pcb"])},
      synthesized: {label: "Full-wave synthesis", ...report(o["--synthesized-report"]), board: board(o["--synthesized-pcb"])}
    }
  };
  fs.writeFileSync(o["-o"], JSON.stringify(document) + "\n");
  console.log(`wrote ${relative(o["-o"])}`);
}

try { main(); } catch (error) { console.error(error.message); process.exit(2); }
