export const scenarios = {
  detour: {description: "Both signals stay on the front layer and find a path around the keepout.", source: text => text},
  layers: {description: "The keepout blocks front copper. Both layers are allowed, so the router can use vias and back copper.", source: text => text
    .replaceAll('allowed_layers = "F.Cu"; max_vias = 0;', 'allowed_layers = "F.Cu,B.Cu"; max_vias = 2;')
    .replace('            layers = "F.Cu,B.Cu";', '            layers = "F.Cu";')
    .replace('origin = (12 mm, 5 mm); width = 8 mm; height = 6 mm;', 'origin = (12 mm, 0 mm); width = 8 mm; height = 16 mm;')},
  blocked: {description: "The keepout spans the board on both layers. The run retains its best result and reports open nets.", source: text => text
    .replace('origin = (12 mm, 5 mm); width = 8 mm; height = 6 mm;', 'origin = (12 mm, 0 mm); width = 8 mm; height = 16 mm;')},
};
