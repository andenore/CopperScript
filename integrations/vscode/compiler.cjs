"use strict";
const cp = require("node:child_process"), fs = require("node:fs"), path = require("node:path");

function validateCommand(command) {
  if (!Array.isArray(command) || command.length > 32 ||
      command.some(s => typeof s !== "string" || !s || s.length > 8192))
    throw new Error("The compiler command must be an interpreter argv array, or [] for automatic discovery.");
}

function selectCommand(configured, cwd, platform = process.platform, exists = fs.existsSync) {
  validateCommand(configured);
  if (configured.length) return {command: [...configured], source: "configured interpreter"};
  const local = path.join(cwd, ".venv", ...(platform === "win32" ? ["Scripts", "python.exe"] : ["bin", "python"]));
  if (exists(local)) return {command: [local], source: "project .venv"};
  if (exists(path.join(cwd, "pyproject.toml")) && exists(path.join(cwd, "uv.lock")))
    return {command: ["uv", "run", "--locked", "--offline", "--no-sync", "python"], source: "locked uv project"};
  throw new Error("No project Python environment found. Install Python 3.11 or newer and set up this project's CopperScript environment, or select an interpreter in CopperScript settings.");
}

// Only a lower Python bound: newer Python 3 releases are accepted. Import the
// actual feature module so a version label cannot hide an incompatible compiler.
const PROBE = `import importlib, importlib.metadata, json, sys
if sys.version_info < (3, 11):
    raise RuntimeError("CopperScript requires Python 3.11 or newer")
if sys.argv[2] == "project" and sys.prefix == sys.base_prefix:
    raise RuntimeError("The project Python environment is not set up; install the project's declared dependencies first")
module = importlib.import_module(sys.argv[1])
try:
    version = importlib.metadata.version("copperscript")
except importlib.metadata.PackageNotFoundError:
    version = "checkout"
print(json.dumps({"python": list(sys.version_info[:3]), "executable": sys.executable, "compilerVersion": version, "module": module.__file__}))
`;

function runProbe(command, cwd, feature, requireEnvironment = false) {
  const module = feature === "mechanical" ? "pcbir.editor.document" : "pcbir.lsp.server";
  return new Promise((resolve, reject) => {
    cp.execFile(command[0], [...command.slice(1), "-c", PROBE, module, requireEnvironment ? "project" : "explicit"],
      {cwd, shell: false, windowsHide: true, timeout: 12000, maxBuffer: 65536}, (error, stdout, stderr) => {
        if (error) {
          const detail = (stderr.trim() || error.message).slice(-3000);
          reject(new Error(`Cannot start CopperScript ${feature}: ${detail}\nUse Python 3.11 or newer with a compiler containing ${module}. Run CopperScript: Show Setup to inspect the selected environment.`));
          return;
        }
        try {
          const info = JSON.parse(stdout.trim());
          if (!Array.isArray(info.python) || info.python.length < 2 || info.python[0] < 3 ||
              (info.python[0] === 3 && info.python[1] < 11) || typeof info.executable !== "string")
            throw new Error("Invalid or unsupported interpreter response");
          resolve(info);
        } catch (error) { reject(new Error(`Cannot inspect CopperScript interpreter: ${error.message}`)); }
      });
  });
}

async function resolveCompiler(configured, cwd, feature = "lsp", probe = runProbe) {
  const selected = selectCommand(configured, cwd);
  // A selected but incompatible environment never falls through to another
  // compiler; the user must choose it explicitly to preserve project intent.
  const info = await probe(selected.command, cwd, feature, selected.source === "locked uv project");
  return {...selected, ...info, command: selected.source === "locked uv project" ? [info.executable] : selected.command};
}

module.exports = {validateCommand, selectCommand, resolveCompiler, runProbe};
