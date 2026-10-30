"use strict";
const vscode = require("vscode");

function activate(context) {
  const languageTools = require("./lsp-client.cjs").activateLsp(vscode, context);
  require("./setup.cjs").activateSetup(vscode, context, languageTools);
  return {languageTools};
}
module.exports = {activate};
