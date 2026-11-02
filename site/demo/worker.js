import init, { analyze } from "./pkg/copper_web.js";

try {
  await init();
  self.postMessage({type: "ready"});
} catch (error) {
  self.postMessage({type: "fatal", error: `Could not load the compiler: ${error.message}`});
}

self.onmessage = event => {
  const {id, revision, source} = event.data;
  try {
    const started = performance.now();
    const result = JSON.parse(analyze(source));
    self.postMessage({type: "result", id, revision, result, milliseconds: performance.now() - started});
  } catch (error) {
    // A trapped WASM instance must be discarded. The editor stays in the page.
    self.postMessage({type: "fatal", id, revision, error: `Compiler stopped: ${error.message}`});
  }
};
