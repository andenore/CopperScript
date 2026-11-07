# Panel

`panel.copper` puts the two synthetic antenna example boards
(`../antenna_synthesis/board.copper` and `inverted_f.copper`) on one
fabrication panel. Not a qualified design; see
[panelization](../../docs/panelization.md).

From the repository root:

```sh
copper route-board examples/antenna_synthesis/board.copper -o build/panel-example/mono/mono.kicad_pcb --report build/panel-example/mono/route.json --locked --offline
copper route-board examples/antenna_synthesis/inverted_f.copper -o build/panel-example/ifa/ifa.kicad_pcb --report build/panel-example/ifa/route.json --locked --offline
copper panelize examples/panel/panel.copper -o build/panel-example/panel.kicad_pcb --locked --offline
copper fill build/panel-example/panel.kicad_pcb -o build/panel-example/filled.kicad_pcb
copper drc build/panel-example/filled.kicad_pcb --project build/panel-example/panel.kicad_pro --rules build/panel-example/panel.kicad_dru --expected-open-ends build/panel-example/panel.panel.json
```

Moving `mono_top` to `at = 25 mm` puts it on the monopole's radiator edge,
and panelize refuses it with both intervals.
