import {sourceOffset} from "../source-location.js";

// Fast compiler checks use their own worker, so a route cannot delay feedback.
export function sourceChecks(source, summary, findings, onChecked) {
  let worker = null, ready = false, active = null, revision = 0, sequence = 0;
  let debounce = null, deadline = null, loading = null;
  let routingError = null;
  function message(text, state = "stale") {
    summary.textContent = text;
    summary.className = `source-check-summary ${state}`;
  }
  function fail(text) {
    clearTimeout(deadline); clearTimeout(loading);
    worker?.terminate(); worker = null; ready = false; active = null;
    findings.replaceChildren(); message(text, "fail");
    onChecked(true, text);
  }
  function display(check) {
    findings.replaceChildren();
    const errors = check.diagnostics.filter(d => d.severity === "error").length;
    const warnings = check.diagnostics.length - errors;
    const text = errors ? `${errors} source error${errors === 1 ? "" : "s"}` : routingError ? `Routing error: ${routingError}`
      : `Syntax and electrical checks passed${warnings ? ` · ${warnings} warning${warnings === 1 ? "" : "s"}` : ""}. Route board to check geometry.`;
    message(text, errors || routingError ? "fail" : "pass");
    source.setAttribute("aria-invalid", String(errors > 0));
    for (const diagnostic of check.diagnostics) {
      const item = document.createElement("li"), jump = document.createElement("button");
      const location = diagnostic.location;
      jump.type = "button"; jump.disabled = !location;
      jump.textContent = `${diagnostic.severity.toUpperCase()} · ${diagnostic.code}${location ? ` · line ${location.line}, column ${location.column}` : ""}: ${diagnostic.message}`;
      jump.addEventListener("click", () => {
        source.focus();
        source.setSelectionRange(sourceOffset(source.value, location.line, location.column), sourceOffset(source.value, location.end_line, location.end_column));
        source.scrollTop = Math.max(0, (location.line - 4) * parseFloat(getComputedStyle(source).lineHeight));
      });
      item.append(jump);
      if (diagnostic.hint) {const hint = document.createElement("p"); hint.textContent = diagnostic.hint; item.append(hint);}
      findings.append(item);
    }
    onChecked(errors > 0 || !!routingError, text);
  }
  function start() {
    const own = new Worker(new URL("../worker.js", import.meta.url), {type:"module"}); worker = own;
    loading = setTimeout(() => {if (worker === own && !ready) fail("Source checker loading timed out. Edit again to retry.");}, 20000);
    own.onerror = event => {event.preventDefault(); if (worker === own) fail("Could not load source checks. Edit again to retry, or use Route board.");};
    own.onmessage = event => {
      if (worker !== own) return;
      const data = event.data;
      if (data.type === "ready") {clearTimeout(loading); ready = true; run();}
      else if (data.type === "fatal") fail(data.error);
      else if (data.type === "result" && data.id === active?.id) {
        clearTimeout(deadline); active = null;
        if (data.revision === revision) display(data.result.check);
        else if (!debounce) run();
      }
    };
  }
  function run() {
    clearTimeout(debounce); debounce = null;
    if (new TextEncoder().encode(source.value).length > 8192) {
      message("The routing lab accepts at most 8 KiB of source.", "fail");
      source.setAttribute("aria-invalid", "true");
      onChecked(true, summary.textContent); return;
    }
    if (!worker) {start(); return;}
    if (!ready || active) return;
    active = {id:++sequence, revision};
    worker.postMessage({...active, source:source.value});
    deadline = setTimeout(() => fail("Source checks reached the 5-second limit. Edit again to retry."), 5000);
  }
  return {
    schedule(delay = 450) {
      revision++; routingError = null; clearTimeout(debounce); findings.replaceChildren();
      source.removeAttribute("aria-invalid"); message("Checking current source…");
      debounce = setTimeout(run, delay);
    },
    routeError(error) {
      routingError = error;
      // Preserve richer compiler diagnostics if those have already arrived.
      if (source.getAttribute("aria-invalid") === "true") return;
      message(error, "fail");
    },
  };
}
