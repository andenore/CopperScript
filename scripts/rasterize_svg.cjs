// Rasterize native CAD SVG, optionally cropping in its viewBox coordinates.
// Supply the installed Sharp module path; no runtime download is performed.
const fs = require('fs');
const sharp = require(process.argv[4]);
const input = process.argv[2];
const output = process.argv[3];
(async () => {
  let svg = fs.readFileSync(input, 'utf8');
  const view = svg.match(/viewBox="([^"]+)"/)[1].trim().split(/\s+/).map(Number);
  // Explicit pixel viewport avoids renderer-dependent interpretation of mm
  // dimensions/density. The native CAD viewBox and geometry remain unchanged.
  svg = svg.replace(/width="[^"]+" height="[^"]+"/,
    `width="3200" height="${Math.round(3200*view[3]/view[2])}"`);
  // Normalize native two-point stroked paths to equivalent SVG line objects.
  // Some lightweight rasterizers omit subsequent degenerate-width path boxes.
  svg = svg.replace(/<path d="M([\d.-]+) ([\d.-]+)\s+L([\d.-]+) ([\d.-]+)\s*"\s*\/>/g,
    '<line x1="$1" y1="$2" x2="$3" y2="$4"/>');
  if (process.argv[6]) {
    const overlay = fs.readFileSync(process.argv[6], 'utf8').replace(/^<svg[^>]*>/,'').replace(/<\/svg>\s*$/,'');
    svg = svg.replace(/<\/svg>\s*$/,overlay+'</svg>');
  }
  const bytes = await sharp(Buffer.from(svg), {density: 72}).flatten({background: 'white'}).png().toBuffer();
  const metadata = await sharp(bytes).metadata();
  let result = sharp(bytes);
  if (process.argv[5]) {
    const [x,y,w,h] = process.argv[5].split(',').map(Number);
    const left = Math.round((x-view[0])/view[2]*metadata.width);
    const top = Math.round((y-view[1])/view[3]*metadata.height);
    const width = Math.min(metadata.width-left, Math.round(w/view[2]*metadata.width));
    const height = Math.min(metadata.height-top, Math.round(h/view[3]*metadata.height));
    result = result.extract({left,top,width,height});
  }
  await result.resize({width:1600}).png().toFile(output);
})();
