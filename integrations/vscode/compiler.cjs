"use strict";
/* Selects the `copper` command that runs `copper lsp`; argv only, never a shell string. */
const DEFAULT_COMMAND = ["copper"];

function validateCommand(command) {
  if (!Array.isArray(command) || command.length > 32 ||
      command.some(s => typeof s !== "string" || !s || s.length > 8192))
    throw new Error("copperscript.lsp.compilerCommand must be an argv array for the copper binary, for example [\"copper\"].");
}

function selectCommand(configured) {
  validateCommand(configured);
  const fallback = !configured.length || (configured.length === 1 && configured[0] === DEFAULT_COMMAND[0]);
  return fallback ? {command: [...DEFAULT_COMMAND], source: "copper on PATH"}
    : {command: [...configured], source: "configured command"};
}

// Actionable text for a command that could not be spawned at all.
function startError(command, error) {
  if (error?.code !== "ENOENT") return error;
  return new Error(`Cannot start ${JSON.stringify(command[0])}: not found. Install the copper binary on PATH, or set copperscript.lsp.compilerCommand to its argv (for example ["/absolute/path/to/copper"]).`);
}

module.exports = {DEFAULT_COMMAND, validateCommand, selectCommand, startError};
