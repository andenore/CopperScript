import {sourceOffset} from "./source-location.js";

const $ = id => document.getElementById(id);
const source = $("source");
const MAX_BYTES = 32 * 1024;
let original = "", worker = null, ready = false, revision = 0, requestId = 0;
let active = null, deadline = null, debounce = null, current = null, selectedNet = "I2C_SDA";

function runtime(title, detail, state = "") {
  $("runtime-status").textContent = title;
  $("runtime-detail").textContent = detail;
  $("runtime-status").parentElement.className = `runtime ${state}`;
}

function summary(text, state = "stale") {
  $("check-summary").textContent = text;
  $("check-summary").className = `check-summary ${state}`;
}

function clearResult(message = "Source changed · check again to update results.") {
  current = null;
  $("diagnostics").replaceChildren();
  $("connections").replaceChildren(Object.assign(document.createElement("p"), {textContent: "No current connection view. Check the source to refresh it."}));
  $("net").replaceChildren(Object.assign(document.createElement("option"), {textContent: "Waiting for analysis"}));
  $("net").disabled = $("locate-net").disabled = true;
  $("pin-detail").textContent = "Select a pin to inspect its declared type and source.";
  $("check-timing").textContent = "";
  summary(message);
}

function finishRequest() {
  clearTimeout(deadline); deadline = null; active = null;
  $("cancel").hidden = true;
  $("check").disabled = !ready;
}

function startWorker() {
  if (worker) worker.terminate();
  ready = false; finishRequest();
  runtime("Loading compiler…", "Rust + WebAssembly · runs on your device");
  const ownWorker = new Worker(new URL("./worker.js", import.meta.url), {type: "module"});
  worker = ownWorker;
  const loadingTimeout = setTimeout(() => {
    if (worker === ownWorker && !ready) stopWorker("Compiler loading timed out. Press Check circuit to retry.");
  }, 20000);
  ownWorker.onerror = event => {
    event.preventDefault(); clearTimeout(loadingTimeout);
    if (worker === ownWorker) stopWorker("Could not start the compiler. Build the demo assets, then press Check circuit to retry.");
  };
  ownWorker.onmessage = event => {
    if (worker !== ownWorker) return;
    const message = event.data;
    if (message.type === "ready") {
      clearTimeout(loadingTimeout); ready = true;
      runtime("Compiler ready", "Rust + WebAssembly · source stays on this device", "ready");
      $("check").disabled = !original;
      if (original) run();
    } else if (message.type === "fatal") {
      clearTimeout(loadingTimeout); stopWorker(message.error);
    } else if (message.type === "result" && message.id === active?.id) {
      finishRequest();
      if (message.revision === revision) display(message.result, message.milliseconds);
      else if (ready) run();
    }
  };
}

function stopWorker(message) {
  clearTimeout(debounce); debounce = null;
  if (worker) worker.terminate(); worker = null; ready = false;
  finishRequest(); clearResult(message);
  runtime("Compiler paused", "Your source is preserved · Check circuit restarts the compiler", "failed");
  $("check").disabled = !original;
}

function run() {
  clearTimeout(debounce);
  if (!original) return;
  if (!worker) { startWorker(); return; }
  if (!ready || active) return;
  if (new TextEncoder().encode(source.value).length > MAX_BYTES) {
    clearResult("Source exceeds the prototype's 32 KiB limit."); return;
  }
  clearResult("Checking source…");
  $("source-state").textContent = "Checking current revision";
  active = {id: ++requestId, revision};
  $("check").disabled = true; $("cancel").hidden = false;
  worker.postMessage({...active, source: source.value});
  deadline = setTimeout(() => stopWorker("Analysis reached the 5-second limit. Simplify the source and check again."), 5000);
}

function edited() {
  revision++;
  $("source-state").textContent = "Source changed";
  clearResult();
  clearTimeout(debounce);
  debounce = setTimeout(run, 450);
}

// Compiler locations count Unicode code points; textarea offsets count UTF-16.
function offset(line, column) {
  return sourceOffset(source.value, line, column);
}

function locate(location) {
  if (!location) return;
  source.focus();
  const start = offset(location.line, location.column), end = offset(location.end_line, location.end_column);
  source.setSelectionRange(start, end);
  const lineHeight = parseFloat(getComputedStyle(source).lineHeight);
  source.scrollTop = Math.max(0, (location.line - 4) * lineHeight);
}

function locateText(text, net = false) {
  // Escape derived names before searching. This is navigation, never analysis.
  const escaped = text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(net ? `\\bnet\\s+(${escaped})(?=\\s*\\{)` : `(?<![\\w/.])${escaped}(?![\\w/.])`);
  const match = pattern.exec(source.value);
  if (!match) return;
  const start = match.index + (net ? match[0].lastIndexOf(text) : 0);
  const prefix = source.value.slice(0, start), line = prefix.split("\n").length;
  const column = Array.from(prefix.slice(prefix.lastIndexOf("\n") + 1)).length + 1;
  locate({line, column, end_line: line, end_column: column + Array.from(text).length});
}

function element(tag, className, text) {
  return Object.assign(document.createElement(tag), {className, textContent: text});
}

function display(result, milliseconds) {
  current = result;
  $("source-state").textContent = "Results match current source";
  $("check-timing").textContent = `${milliseconds.toFixed(1)} ms · analysis`;
  const {check} = result;
  const errors = check.counts.error, warnings = check.counts.warning;
  summary(errors ? `${errors} error${errors === 1 ? "" : "s"}${warnings ? ` · ${warnings} warning${warnings === 1 ? "" : "s"}` : ""}` : warnings ? `ERC passed · ${warnings} warning${warnings === 1 ? "" : "s"}` : "ERC passed · no findings", errors ? "fail" : "pass");
  for (const diagnostic of check.diagnostics) {
    const li = element("li", "", "");
    li.append(element("p", "diagnostic-code", `${diagnostic.severity.toUpperCase()} · ${diagnostic.code}${diagnostic.location ? ` · line ${diagnostic.location.line}` : ""}`));
    const jump = element("button", "diagnostic-jump", diagnostic.message);
    jump.type = "button"; jump.disabled = !diagnostic.location;
    jump.addEventListener("click", () => locate(diagnostic.location)); li.append(jump);
    if (diagnostic.hint) li.append(element("p", "diagnostic-hint", diagnostic.hint));
    if (diagnostic.fix) {
      const fix = element("button", "diagnostic-fix", "Apply suggested fix"); fix.type = "button";
      fix.addEventListener("click", () => {
        const {range, replacement} = diagnostic.fix;
        source.setRangeText(replacement, offset(range.line, range.column), offset(range.end_line, range.end_column), "select");
        edited(); run();
      }); li.append(fix);
    }
    $("diagnostics").append(li);
  }
  const nets = result.nets.map(query => query.net);
  $("net").replaceChildren(...nets.map(net => Object.assign(document.createElement("option"), {value: net.net, textContent: net.net})));
  if (nets.some(net => net.net === selectedNet)) $("net").value = selectedNet;
  $("net").disabled = $("locate-net").disabled = !nets.length;
  if (nets.length) showNet();
  else $("connections").replaceChildren(element("p", "", "Compilation did not produce a design. Correct the source to restore the connection view."));
}

function showNet() {
  selectedNet = $("net").value;
  const net = current?.nets.find(query => query.net.net === selectedNet)?.net;
  if (!net) return;
  const grid = element("div", "endpoint-grid", "");
  for (const member of net.members) {
    const button = element("button", "endpoint", ""); button.type = "button"; button.setAttribute("aria-pressed", "false");
    const part = member.part || "Unknown component", pinType = member.type || "Unresolved pin";
    button.append(element("strong", "", member.endpoint), element("span", "", part), element("span", "", pinType));
    button.addEventListener("click", () => {
      grid.querySelectorAll("button").forEach(item => item.setAttribute("aria-pressed", String(item === button)));
      $("pin-detail").textContent = `${member.endpoint} · ${part} · ${pinType} · on ${net.net}`;
      locateText(member.endpoint);
    }); grid.append(button);
  }
  $("connections").replaceChildren(element("div", "net-line", `${net.net} · ${net.members.length} pin${net.members.length === 1 ? "" : "s"}`), grid);
  $("pin-detail").textContent = "Select a pin to inspect its declared type and source.";
}

const scenarios = {
  healthy: [text => text, "The starting circuit includes both I²C pull-ups and a 3.3 V supply."],
  pullup: [text => text.replace("U2.SDA; R1.2;", "U2.SDA;"), "R1.2 is disconnected. Inspect I2C_SDA and follow the real ERC findings."],
  voltage: [text => text.replace("voltage = 3.3 V;", "voltage = 5 V;"), "The declared supply is now 5 V. Check the voltage limits of the demonstration parts."],
  typo: [text => text.replace("U1.PB7; U2.SDA;", "U1.PB8; U2.SDA;"), "PB8 is not a declared pin on this part. Follow the diagnostic and its suggested correction."],
};

document.querySelectorAll("[data-example]").forEach(button => button.addEventListener("click", () => {
  if (!original) return;
  const [transform, description] = scenarios[button.dataset.example];
  source.value = transform(original);
  $("scenario-description").textContent = description;
  document.querySelectorAll("[data-example]").forEach(item => item.setAttribute("aria-pressed", String(item === button)));
  edited(); run();
}));
source.addEventListener("input", () => {
  document.querySelectorAll("[data-example]").forEach(button => button.setAttribute("aria-pressed", "false"));
  $("scenario-description").textContent = "Your edits are checked automatically after a short pause.";
  edited();
});
source.addEventListener("keydown", event => {
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {event.preventDefault(); run();}
});
$("check").addEventListener("click", run);
$("cancel").addEventListener("click", () => stopWorker("Analysis stopped. Your source is preserved."));
$("net").addEventListener("change", showNet);
$("locate-net").addEventListener("click", () => locateText(selectedNet, true));
$("download").addEventListener("click", () => {
  const url = URL.createObjectURL(new Blob([source.value], {type: "text/plain;charset=utf-8"}));
  const link = Object.assign(document.createElement("a"), {href: url, download: "sensor.copper"});
  link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
});
async function boot() {
  try {
    const response = await fetch(new URL("./sensor.copper", import.meta.url));
    if (!response.ok) throw new Error(`Example request failed (${response.status})`);
    original = (await response.text()).replace(/\r\n?/g, "\n");
    source.value = original; source.disabled = false; $("download").disabled = false;
    startWorker();
  } catch (error) {
    summary(`Could not load the example: ${error.message}`, "fail");
    runtime("Example unavailable", "Reload the page to retry", "failed");
  }
}
boot();
