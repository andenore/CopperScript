"use strict";
const fs = require("node:fs"), path = require("node:path");

function checkTag(ref, manifest) {
  const expected = `refs/tags/extension-v${manifest.version}`;
  if (ref !== expected) throw new Error(`Extension release must use ${expected}; received ${ref || "no tag"}`);
  return expected;
}

if (require.main === module) {
  try {
    if (process.argv[2] !== "check-tag") throw new Error("Usage: node integrations/vscode/release.cjs check-tag");
    console.log(checkTag(process.env.GITHUB_REF, JSON.parse(fs.readFileSync(path.join(__dirname, "package.json"), "utf8"))));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
module.exports = {checkTag};
