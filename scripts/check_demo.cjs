#!/usr/bin/env node
/* Run the shipped WASM, not a JS substitute. Optional native CLI parity check. */
"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), {pathToFileURL} = require("node:url");
const {spawnSync} = require("node:child_process");
const {Worker} = require("node:worker_threads");
const root = path.resolve(__dirname, "..");

// Exercise the actual browser worker module with Node's message/fetch adapters.
// This checks the protocol and WASM loading; it does not replace browser UI QA.
async function checkWorker(source) {
  const url = pathToFileURL(path.join(root, "site/demo/worker.js")).href;
  const worker = new Worker(`
    (async () => {
      const {parentPort} = await import('node:worker_threads');
      const fs = await import('node:fs');
      globalThis.self = {postMessage: message => parentPort.postMessage(message)};
      globalThis.fetch = async url => new Response(fs.readFileSync(new URL(url)), {headers: {'Content-Type': 'application/wasm'}});
      await import(${JSON.stringify(url)});
      parentPort.on('message', data => self.onmessage({data}));
    })().catch(error => {throw error;});
  `, {eval: true});
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("Worker protocol test timed out")), 15000);
      const fail = error => {clearTimeout(timer); reject(error);};
      worker.on("error", fail);
      worker.on("message", message => {
        try {
          if (message.type === "fatal") throw new Error(message.error);
          if (message.type === "ready") worker.postMessage({id: 10, revision: 4, source: "board {"});
          else if (message.id === 10) {
            assert.equal(message.revision, 4); assert.equal(message.result.check.passed, false);
            worker.postMessage({id: 11, revision: 5, source});
          } else {
            assert.equal(message.id, 11); assert.equal(message.revision, 5);
            assert.equal(message.result.check.passed, true); assert.ok(message.milliseconds >= 0);
            clearTimeout(timer); resolve();
          }
        } catch (error) {fail(error);}
      });
    });
  } finally {await worker.terminate();}
}
(async () => {
  const {default: init, analyze} = await import(pathToFileURL(path.join(root, "site/demo/pkg/copper_web.js")));
  const {sourceOffset} = await import(pathToFileURL(path.join(root, "site/demo/source-location.js")));
  await init({module_or_path: fs.readFileSync(path.join(root, "site/demo/pkg/copper_web_bg.wasm"))});
  const original = fs.readFileSync(path.join(root, "site/demo/sensor.copper"), "utf8").replace(/\r\n?/g, "\n");
  const check = source => JSON.parse(analyze(source));
  const codes = result => result.check.diagnostics.map(item => item.code);
  const good = check(original);
  assert.equal(good.check.passed, true); assert.deepEqual(codes(good), []);
  assert.equal(good.nets.find(item => item.net.net === "I2C_SDA").net.members.length, 3);
  const pullup = check(original.replace("U2.SDA; R1.2;", "U2.SDA;"));
  assert.equal(pullup.check.passed, false); assert.ok(codes(pullup).some(code => code.startsWith("I2C_")));
  assert.equal(pullup.nets.find(item => item.net.net === "I2C_SDA").net.members.length, 2);
  const voltage = check(original.replace("voltage = 3.3 V;", "voltage = 5 V;"));
  assert.equal(voltage.check.passed, false); assert.ok(codes(voltage).some(code => code.includes("VOLTAGE")));
  const typo = check(original.replace("U1.PB7; U2.SDA;", "U1.PB8; U2.SDA;"));
  assert.equal(typo.check.passed, false);
  assert.ok(typo.check.diagnostics.some(item => item.fix?.replacement === "U1.PB7" || item.fix?.replacement === "PB7"), JSON.stringify(typo.check));
  const malformed = check("board {");
  assert.equal(malformed.check.passed, false); assert.equal(malformed.nets.length, 0);
  const imports = check(original.replace("use library", "import x \"https://example.com/parts\";\n    use library"));
  assert.deepEqual(codes(imports), ["WEB_IMPORT"]);
  assert.deepEqual(codes(check(" ".repeat(32769))), ["WEB_SIZE"]);
  assert.deepEqual(check(original.replace(/\n/g, "\r\n")), good);
  assert.deepEqual(check(original), good, "Recover to a clean result after failures");
  const unicodeSource = "// 🟠\n" + original.replace("U1.PB7; U2.SDA;", "U1.PB8; U2.SDA;");
  const unicode = check(unicodeSource);
  const before = typo.check.diagnostics.find(item => item.fix);
  const after = unicode.check.diagnostics.find(item => item.code === before.code);
  assert.equal(after.location.line, before.location.line + 1);
  const range = after.fix.range;
  const start = sourceOffset(unicodeSource, range.line, range.column);
  const end = sourceOffset(unicodeSource, range.end_line, range.end_column);
  assert.equal(unicodeSource.slice(start, end), before.subject);
  assert.equal(sourceOffset("🟠 pin", 1, 3), 3, "Convert code-point columns to UTF-16 offsets");
  if (process.argv[2]) {
    const cli = spawnSync(path.resolve(process.argv[2]), ["check", "site/demo/sensor.copper", "--json", "--locked", "--offline"], {cwd: root, encoding: "utf8", windowsHide: true});
    if (cli.error) throw cli.error;
    assert.equal(cli.status, 0, cli.stdout + cli.stderr);
    assert.deepEqual(JSON.parse(cli.stdout).diagnostics, good.check.diagnostics);
    const query = spawnSync(path.resolve(process.argv[2]), ["query", "site/demo/sensor.copper", "net", "I2C_SDA", "--json", "--locked", "--offline"], {cwd: root, encoding: "utf8", windowsHide: true});
    assert.equal(query.status, 0, query.stdout + query.stderr);
    assert.deepEqual(JSON.parse(query.stdout), good.nets.find(item => item.net.net === "I2C_SDA"));
    console.log("Native CLI and browser WASM agree on the example checks and SDA query.");
  }
  await checkWorker(original);
  console.log("Worker loads the shipped WASM, echoes revisions and recovers from syntax errors.");
  const started = performance.now();
  for (let i = 0; i < 50; i++) check(original);
  console.log(`WASM scenarios passed; ${((performance.now() - started) / 50).toFixed(2)} ms mean warm analysis in this Node runtime (50 runs).`);
})().catch(error => {console.error(error); process.exitCode = 1;});
