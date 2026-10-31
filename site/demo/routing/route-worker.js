import init, {route} from "./pkg/copper_web_route.js";
try {await init(); self.postMessage({type: "ready"});}
catch (error) {self.postMessage({type: "fatal", error: `Could not load the router: ${error.message}`});}
self.onmessage = event => {
  const {id, revision, source} = event.data;
  try {
    const started = performance.now();
    const result = JSON.parse(route(source));
    self.postMessage({type: "result", id, revision, result, milliseconds: performance.now() - started});
  } catch (error) {self.postMessage({type: "fatal", id, revision, error: `Router stopped: ${error.message}`});}
};
