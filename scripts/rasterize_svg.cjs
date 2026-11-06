// Rasterize native CAD SVG, optionally cropping in its viewBox coordinates.
// Supply the installed Sharp module path; no runtime download is performed.
const fs = require('fs');
const sharp = require(process.argv[4]);
const input = process.argv[2];
const output = process.argv[3];
(async () => {
  let svg = fs.readFileSync(input, 'utf8');
  const view = svg.match(/viewBox="([^"]+)"/)[1].trim().split(/\s+/).map(Number);
  const viewport = process.argv[5] ? process.argv[5].split(',').map(Number) : view;
  if (viewport.length !== 4 || !viewport.every(Number.isFinite) || viewport[2] <= 0 || viewport[3] <= 0) {
    throw new Error('Viewport must be x,y,width,height with positive dimensions');
  }
  // Explicit pixel viewport avoids renderer-dependent interpretation of mm
  // dimensions/density. The native CAD geometry remains unchanged.
  // Crop the vector viewport before rasterizing so close-ups retain detail.
  svg = svg.replace(/<svg\b[^>]*>/, root =>
    root.replace(/\s+(?:width|height)="[^"]*"/g, '')
      .replace(/viewBox="[^"]*"/, `viewBox="${viewport.join(' ')}"`).replace(/>$/,
        ` width="3200" height="${Math.round(3200*viewport[3]/viewport[2])}">`));
  // Normalize native two-point stroked paths to equivalent SVG line objects.
  // Some lightweight rasterizers omit subsequent degenerate-width path boxes.
  svg = svg.replace(/<path d="M([\d.-]+) ([\d.-]+)\s+L([\d.-]+) ([\d.-]+)\s*"\s*\/>/g,
    '<line x1="$1" y1="$2" x2="$3" y2="$4"/>');
  if (process.argv[6]) {
    const overlay = fs.readFileSync(process.argv[6], 'utf8').replace(/^<svg[^>]*>/,'').replace(/<\/svg>\s*$/,'');
    svg = svg.replace(/<\/svg>\s*$/,overlay+'</svg>');
  }
  const bytes = await sharp(Buffer.from(svg), {density: 72}).flatten({background: 'white'}).png().toBuffer();
  const result = sharp(bytes);
  await result.resize({width:1600}).png().toFile(output);
})();
