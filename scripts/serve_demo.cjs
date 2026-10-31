#!/usr/bin/env node
/* Local static preview, including project-prefix paths used by GitHub Pages. */
"use strict";
const fs = require("node:fs"), http = require("node:http"), path = require("node:path");
const root = path.resolve(__dirname, "../site");
const port = Number(process.env.PORT || 4173);
const types = {".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".wasm": "application/wasm", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".copper": "text/plain; charset=utf-8", ".xml": "application/xml"};
const server = http.createServer((request, response) => {
  try {
    if (request.method !== "GET" && request.method !== "HEAD") {response.writeHead(405).end(); return;}
    let name = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    if (name.startsWith("/CopperScript/")) name = name.slice("/CopperScript".length);
    let file = path.resolve(root, `.${name}`);
    if (!(file === root || file.startsWith(root + path.sep))) {response.writeHead(403).end(); return;}
    if (fs.existsSync(file) && fs.statSync(file).isDirectory()) file = path.join(file, "index.html");
    if (!fs.existsSync(file) || !fs.statSync(file).isFile()) {response.writeHead(404).end("Not found"); return;}
    response.writeHead(200, {"Content-Type": types[path.extname(file)] || "application/octet-stream", "Cache-Control": "no-store"});
    if (request.method === "HEAD") response.end(); else fs.createReadStream(file).pipe(response);
  } catch {response.writeHead(400).end("Bad request");}
});
server.listen(port, "127.0.0.1", () => console.log(`CopperScript demo: http://127.0.0.1:${port}/CopperScript/demo/`));
process.on("SIGINT", () => server.close());
