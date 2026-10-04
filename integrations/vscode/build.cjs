/* Reproducible staging of the extension with the wheel's shared web assets. */
const fs=require("node:fs"),path=require("node:path");
const root=path.resolve(__dirname,"../.."),output=path.join(root,"build/vscode-extension/extension");
fs.mkdirSync(path.join(output,"media"),{recursive:true});
for(const file of ["package.json","extension.cjs","README.md"])fs.copyFileSync(path.join(__dirname,file),path.join(output,file));
fs.copyFileSync(path.join(__dirname,"LICENSE"),path.join(output,"LICENSE"));
for(const file of ["editor.js","editor.css","index.html"])fs.copyFileSync(path.join(root,"pcbir/editor/assets",file),path.join(output,"media",file));
console.log(output);
