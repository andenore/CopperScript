module github.com/anden/CopperScript/examples/fx10_bridge

require github.com/copperscript/examples v0.1.0
require github.com/KiCad/kicad-footprints 7ebfa6b23cc292a56f751b7b5f4a0e12eeef69dd

replace github.com/copperscript/examples => ../packages

footprint-library Capacitor_SMD github.com/KiCad/kicad-footprints/Capacitor_SMD.pretty
footprint-library Connector_PinHeader_1.27mm github.com/KiCad/kicad-footprints/Connector_PinHeader_1.27mm.pretty
footprint-library Connector_PinHeader_2.54mm github.com/KiCad/kicad-footprints/Connector_PinHeader_2.54mm.pretty
footprint-library Connector_USB github.com/KiCad/kicad-footprints/Connector_USB.pretty
footprint-library Crystal github.com/KiCad/kicad-footprints/Crystal.pretty
footprint-library Inductor_SMD github.com/KiCad/kicad-footprints/Inductor_SMD.pretty
footprint-library Resistor_SMD github.com/KiCad/kicad-footprints/Resistor_SMD.pretty
