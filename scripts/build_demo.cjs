#!/usr/bin/env node
/* Build the real Rust frontend for the static browser playground. */
"use strict";
const fs = require("node:fs"), path = require("node:path"), {spawnSync} = require("node:child_process");
const root = path.resolve(__dirname, "..");
function run(command, args, capture = false) {
  const result = spawnSync(command, args, {cwd: root, encoding: "utf8", stdio: capture ? "pipe" : "inherit", windowsHide: true});
  if (result.error) throw new Error(`${command}: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}${result.stderr ? `: ${result.stderr}` : ""}`);
  return result.stdout;
}
try {
  const bindgen = process.env.WASM_BINDGEN || "wasm-bindgen";
  const expected = "wasm-bindgen 0.2.130";
  if (run(bindgen, ["--version"], true).trim() !== expected) throw new Error(`Install ${expected}; it must match copper-web's pinned dependency.`);
  run("cargo", ["build", "--locked", "--release", "--target", "wasm32-unknown-unknown", "-p", "copper-web", "-p", "copper-web-route"]);
  const metadata = JSON.parse(run("cargo", ["metadata", "--no-deps", "--format-version", "1"], true));
  for (const [name, directory] of [["copper_web", "site/demo/pkg"], ["copper_web_route", "site/demo/routing/pkg"]]) {
    const input = path.join(metadata.target_directory, `wasm32-unknown-unknown/release/${name}.wasm`);
    const output = path.join(root, directory);
    fs.mkdirSync(output, {recursive: true});
    run(bindgen, [input, "--target", "web", "--out-dir", output, "--out-name", name, "--no-typescript"]);
    console.log(`${name}: ${fs.statSync(path.join(output, `${name}_bg.wasm`)).size.toLocaleString()} bytes (uncompressed).`);
  }
} catch (error) {console.error(error.message); process.exitCode = 1;}
