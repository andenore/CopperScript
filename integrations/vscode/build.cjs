/* Reproducible staging of the extension for vsce packaging. */
const fs=require("node:fs"),path=require("node:path");
const root=path.resolve(__dirname,"../.."),output=path.join(root,"build/vscode-extension/extension");
fs.rmSync(output,{recursive:true,force:true});
fs.mkdirSync(path.join(output,"media"),{recursive:true});
for(const file of ["package.json","extension.cjs","lsp-client.cjs","compiler.cjs","setup.cjs","language-configuration.json","README.md","CHANGELOG.md","LICENSE"])fs.copyFileSync(path.join(__dirname,file),path.join(output,file));
fs.mkdirSync(path.join(output,"syntaxes"),{recursive:true});
fs.copyFileSync(path.join(__dirname,"syntaxes/copper.tmLanguage.json"),path.join(output,"syntaxes/copper.tmLanguage.json"));
// Only the Marketplace icon is shared with the browser editor's assets.
fs.copyFileSync(path.join(root,"crates/copper-editor/assets/copper-logo.png"),path.join(output,"media/copper-logo.png"));
console.log(output);
