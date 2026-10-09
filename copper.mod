module github.com/anden/CopperScript

require github.com/copperscript/examples v0.1.0
require github.com/andenore/CopperLib da8d764a96948e3e61dd3e4b8f5c7acc9949c3b3
require github.com/KiCad/kicad-footprints 7ebfa6b23cc292a56f751b7b5f4a0e12eeef69dd

footprint-library Connector_Debug github.com/andenore/CopperLib/packages/parts/samtec/ftsh-105-01-l-dv-007-k/footprints
footprint-library EG800G github.com/andenore/CopperLib/packages/parts/quectel/eg800g-eu/footprints
footprint-library MAX_M10S github.com/andenore/CopperLib/packages/parts/u-blox/max-m10s/footprints
footprint-library KENTO github.com/andenore/CopperLib/packages/parts/kento/kt0603r/footprints
footprint-library Button_Switch_SMD github.com/KiCad/kicad-footprints/Button_Switch_SMD.pretty
footprint-library Capacitor_SMD github.com/KiCad/kicad-footprints/Capacitor_SMD.pretty
footprint-library Connector_Coaxial github.com/KiCad/kicad-footprints/Connector_Coaxial.pretty
footprint-library Connector_PinHeader_2.54mm github.com/KiCad/kicad-footprints/Connector_PinHeader_2.54mm.pretty
footprint-library Crystal github.com/KiCad/kicad-footprints/Crystal.pretty
footprint-library Inductor_SMD github.com/KiCad/kicad-footprints/Inductor_SMD.pretty
footprint-library LED_SMD github.com/KiCad/kicad-footprints/LED_SMD.pretty
footprint-library Package_DFN_QFN github.com/KiCad/kicad-footprints/Package_DFN_QFN.pretty
footprint-library Package_LGA github.com/KiCad/kicad-footprints/Package_LGA.pretty
footprint-library Package_QFP github.com/KiCad/kicad-footprints/Package_QFP.pretty
footprint-library Package_SO github.com/KiCad/kicad-footprints/Package_SO.pretty
footprint-library Package_TO_SOT_SMD github.com/KiCad/kicad-footprints/Package_TO_SOT_SMD.pretty
footprint-library RF_Antenna github.com/KiCad/kicad-footprints/RF_Antenna.pretty
footprint-library Resistor_SMD github.com/KiCad/kicad-footprints/Resistor_SMD.pretty

// The example dependency is kept in this repository for offline development.
replace github.com/copperscript/examples => ./examples/packages
