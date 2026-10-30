#!/usr/bin/env node
/* Check the public static site without installing packages or building boards:
   local URLs and anchors, page metadata, the sitemap, JSON and SVG syntax, the
   recorded showcase hashes and the shared logo copies. */
"use strict";
const crypto = require("node:crypto"), fs = require("node:fs"), path = require("node:path");

const ROOT = path.resolve(__dirname, "..");
const SITE = path.join(ROOT, "site");
const LOGOS = path.join(ROOT, "crates/copper-editor/assets");
const BASE_URL = "https://andenore.github.io/CopperScript/";
const SNAPSHOT_ASSETS = ["routing-top.svg", "routing-inner.svg", "routing-bottom.svg", "routing-all.svg", "placement.svg",
  "rendering.json", "electrical-ir.excerpt.json", "physical-ir.excerpt.json"];

const sha256 = file => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const decode = text => text.replace(/&(#x[0-9a-f]+|#[0-9]+|amp|lt|gt|quot|apos);/gi, (_, entity) => {
  const named = {amp: "&", lt: "<", gt: ">", quot: "\"", apos: "'"}[entity.toLowerCase()];
  if (named) return named;
  return String.fromCodePoint(entity[1].toLowerCase() === "x" ? parseInt(entity.slice(2), 16) : parseInt(entity.slice(1), 10));
});

function walk(directory, extension, found = []) {
  for (const entry of fs.readdirSync(directory, {withFileTypes: true})) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) walk(full, extension, found);
    else if (entry.name.endsWith(extension)) found.push(full);
  }
  return found;
}

/* The start tags of an HTML page, without comments, scripts' bodies or styles. */
function startTags(html) {
  const text = html.replace(/<!--[\s\S]*?-->/g, "").replace(/<(script|style)\b([^>]*)>[\s\S]*?<\/\1>/gi, "<$1$2>");
  const tags = [];
  for (const match of text.matchAll(/<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[^\s=>/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>/g)) {
    const attrs = {};
    for (const a of match[2].matchAll(/([^\s=>/]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g))
      attrs[a[1].toLowerCase()] = decode(a[2] ?? a[3] ?? a[4] ?? "");
    tags.push({tag: match[1].toLowerCase(), attrs});
  }
  return tags;
}

function page(file) {
  const result = {ids: new Set(), duplicateIds: [], references: [], imagesWithoutAlt: 0, h1: 0, canonical: null, description: null};
  for (const {tag, attrs} of startTags(fs.readFileSync(file, "utf8"))) {
    if (attrs.id) { if (result.ids.has(attrs.id)) result.duplicateIds.push(attrs.id); result.ids.add(attrs.id); }
    if (tag === "h1") result.h1++;
    if (tag === "img" && !attrs.alt) result.imagesWithoutAlt++;
    if (tag === "link" && attrs.rel === "canonical") result.canonical = attrs.href;
    if (tag === "meta" && attrs.name === "description") result.description = attrs.content;
    if (tag === "meta" && attrs.property === "og:image" && attrs.content) result.references.push(attrs.content);
    for (const key of ["href", "src"]) if (attrs[key]) result.references.push(attrs[key]);
  }
  return result;
}

/* XML well-formedness, as far as balanced and properly nested elements go. */
function checkXml(file) {
  const text = fs.readFileSync(file, "utf8").replace(/<!--[\s\S]*?-->/g, "").replace(/<!\[CDATA\[[\s\S]*?\]\]>/g, "")
    .replace(/<\?[\s\S]*?\?>/g, "").replace(/<!DOCTYPE[^>]*>/gi, "");
  const stack = [];
  for (const match of text.matchAll(/<(\/?)([^\s/>]+)[^>]*?(\/?)>/g)) {
    if (match[3]) continue;
    if (!match[1]) stack.push(match[2]);
    else if (stack.pop() !== match[2]) throw new Error(`${path.basename(file)}: mismatched </${match[2]}>`);
  }
  if (stack.length) throw new Error(`${path.basename(file)}: unclosed <${stack.at(-1)}>`);
  return text;
}

function checkSite(site = SITE) {
  const errors = [], pages = new Map();
  for (const file of walk(site, ".html")) {
    const p = page(file);
    pages.set(path.resolve(file), p);
    const name = path.basename(file);
    if (p.h1 !== 1) errors.push(`${name}: expected one h1, found ${p.h1}`);
    if (p.duplicateIds.length || p.imagesWithoutAlt) errors.push(`${name}: duplicate IDs or missing image descriptions`);
    if (!p.description) errors.push(`${name}: missing description`);
  }
  const index = path.resolve(site, "index.html");
  if (!pages.has(index)) errors.push("Missing index.html");
  else if (pages.get(index).canonical !== BASE_URL) errors.push("Canonical URL does not match the intended GitHub Pages address");
  const root = path.resolve(site);
  for (const [file, p] of pages) {
    const name = path.basename(file);
    for (const reference of p.references) {
      let url, target;
      if (/^[a-z][a-z0-9+.-]*:|^\/\//i.test(reference)) {
        // Verify same-site absolute metadata URLs too, including the share image.
        if (!reference.startsWith(BASE_URL)) continue;
        url = new URL(reference.slice(BASE_URL.length), "file:///");
        target = path.join(site, decodeURIComponent(url.pathname));
      } else {
        url = new URL(reference, "file:///x/");
        if (reference.startsWith("/")) { errors.push(`${name}: root-relative URL breaks project hosting: ${reference}`); continue; }
        const local = reference.split(/[?#]/)[0];
        target = local ? path.join(path.dirname(file), decodeURIComponent(local)) : file;
      }
      if (fs.existsSync(target) && fs.statSync(target).isDirectory()) target = path.join(target, "index.html");
      target = path.resolve(target);
      const fragment = url.hash ? decodeURIComponent(url.hash.slice(1)) : "";
      if (!(target === root || target.startsWith(root + path.sep)) || !fs.existsSync(target))
        errors.push(`${name}: missing or out-of-site target: ${reference}`);
      else if (fragment && pages.has(target) && !pages.get(target).ids.has(fragment))
        errors.push(`${name}: missing anchor: ${reference}`);
    }
  }
  try {
    const sitemap = checkXml(path.join(site, "sitemap.xml"));
    const locations = [...sitemap.matchAll(/<loc>\s*([^<]*?)\s*<\/loc>/g)].map(m => decode(m[1]));
    if (locations.length !== 1 || locations[0] !== BASE_URL) errors.push("Sitemap does not match the canonical site URL");
    const assets = path.join(site, "assets");
    const snapshot = JSON.parse(fs.readFileSync(path.join(assets, "snapshot.json"), "utf8"));
    for (const name of SNAPSHOT_ASSETS)
      if (sha256(path.join(assets, name)) !== snapshot.rendered_assets[name])
        errors.push(`${name}: bytes do not match the recorded showcase snapshot`);
    const rendering = JSON.parse(fs.readFileSync(path.join(assets, "rendering.json"), "utf8"));
    if (rendering.pcb_sha256 !== snapshot.pcb_sha256) errors.push("Rendering metadata describes a different PCB");
    if (JSON.stringify(rendering.all_layers) !== JSON.stringify(snapshot.copper_layers) || rendering.views["routing-all.svg"].copper_fills)
      errors.push("Combined board view must show the full copper stack with fills hidden");
    if (rendering.native_geometry.footprints !== snapshot.component_count || !(rendering.native_geometry.pads > 0))
      errors.push("Rendering metadata is missing the recorded footprints/pads");
    for (const file of fs.readdirSync(assets)) {
      if (file.endsWith(".svg")) checkXml(path.join(assets, file));
      if (file.endsWith(".json")) JSON.parse(fs.readFileSync(path.join(assets, file), "utf8"));
    }
    const html = fs.readFileSync(index, "utf8");
    for (const match of html.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)) JSON.parse(match[1]);
    const png = fs.readFileSync(path.join(assets, "social-preview.png"));
    if (!png.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) errors.push("Social preview is not a PNG");
  } catch (error) {
    errors.push(error.message);
  }
  if (root === path.resolve(SITE)) {
    // The site's Cu mark copies the one the editor ships.
    for (const [copy, source] of [["copper-logo.svg", "copper-logo.svg"], ["favicon.svg", "copper-logo.svg"], ["copper-logo.png", "copper-logo.png"]]) {
      const target = path.join(SITE, "assets", copy);
      if (!fs.existsSync(target) || !fs.readFileSync(target).equals(fs.readFileSync(path.join(LOGOS, source))))
        errors.push(`Logo copy differs: ${target}`);
    }
  }
  return errors;
}

const failures = checkSite(process.argv[2] ? path.resolve(process.argv[2]) : SITE);
if (failures.length) {
  console.log(failures.map(failure => `ERROR: ${failure}`).join("\n"));
  process.exit(1);
}
console.log("Static site OK: local URLs, anchors, metadata, sitemap, JSON, SVG and snapshot hashes.");
