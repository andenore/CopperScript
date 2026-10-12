/* Reproducible staging of the extension with the wheel's shared web assets. */
const fs=require("node:fs"),path=require("node:path");
const root=path.resolve(__dirname,"../.."),output=path.join(root,"build/vscode-extension/extension");
fs.mkdirSync(path.join(output,"media"),{recursive:true});
for(const file of ["package.json","extension.cjs","lsp-client.cjs","compiler.cjs","setup.cjs","language-configuration.json","README.md","CHANGELOG.md"])fs.copyFileSync(path.join(__dirname,file),path.join(output,file));
fs.mkdirSync(path.join(output,"syntaxes"),{recursive:true});
fs.copyFileSync(path.join(__dirname,"syntaxes/copper.tmLanguage.json"),path.join(output,"syntaxes/copper.tmLanguage.json"));
fs.copyFileSync(path.join(__dirname,"LICENSE"),path.join(output,"LICENSE"));
for(const file of ["editor.js","editor.css","index.html","copper-logo.svg","copper-logo.png"])fs.copyFileSync(path.join(root,"pcbir/editor/assets",file),path.join(output,"media",file));
console.log(output);
