module github.com/anden/CopperScript/examples/nrf52_coin_cell

require github.com/copperscript/examples v0.1.0
require github.com/andenore/CopperLib 1c1699881d7cc1e63ce64312f09e5a266375b8e0
require github.com/KiCad/kicad-footprints 7ebfa6b23cc292a56f751b7b5f4a0e12eeef69dd

replace github.com/copperscript/examples => ../packages

// Keep the hard-macro members on the source-compatible geometry used by its
// digest-bound reference; unrelated parts use the pinned KiCad repository.
footprint-library Button_Switch_SMD github.com/KiCad/kicad-footprints/Button_Switch_SMD.pretty
footprint-library Capacitor_SMD github.com/KiCad/kicad-footprints/Capacitor_SMD.pretty
footprint-library Connector_Debug github.com/andenore/CopperLib/packages/parts/samtec/ftsh-105-01-l-dv-007-k/footprints
footprint-library Crystal github.com/KiCad/kicad-footprints/Crystal.pretty
footprint-library Inductor_SMD github.com/KiCad/kicad-footprints/Inductor_SMD.pretty
footprint-library LED_SMD github.com/KiCad/kicad-footprints/LED_SMD.pretty
footprint-library Package_DFN_QFN github.com/andenore/CopperLib/packages/parts/nordic/nrf52832/footprints/Package_DFN_QFN.pretty
footprint-library RF_Antenna github.com/andenore/CopperLib/packages/parts/johanson/2450at18a0100001e/footprints/RF_Antenna.pretty
footprint-library Resistor_SMD github.com/KiCad/kicad-footprints/Resistor_SMD.pretty
