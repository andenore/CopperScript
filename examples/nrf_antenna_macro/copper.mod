module github.com/anden/CopperScript/examples/nrf_antenna_macro

require github.com/andenore/CopperLib 1c1699881d7cc1e63ce64312f09e5a266375b8e0
require github.com/KiCad/kicad-footprints 7ebfa6b23cc292a56f751b7b5f4a0e12eeef69dd

// The RF trial is bound to the source-compatible package geometry used by its
// digest-bound hard macro. Other examples use the current KiCad provider.
footprint-library Capacitor_SMD github.com/andenore/CopperLib/packages/circuits/nordic/nrf52832-johanson-reference/footprints/Capacitor_SMD.pretty
footprint-library Inductor_SMD github.com/andenore/CopperLib/packages/circuits/nordic/nrf52832-johanson-reference/footprints/Inductor_SMD.pretty
footprint-library Package_DFN_QFN github.com/andenore/CopperLib/packages/parts/nordic/nrf52832/footprints/Package_DFN_QFN.pretty
footprint-library RF_Antenna github.com/andenore/CopperLib/packages/parts/johanson/2450at18a0100001e/footprints/RF_Antenna.pretty
