module github.com/anden/CopperScript

require github.com/copperscript/examples v0.1.0
require github.com/andenore/CopperLib bcc158d

footprint-library Connector_Debug github.com/andenore/CopperLib/packages/parts/samtec/ftsh-105-01-l-dv-007-k/footprints
footprint-library EG800G github.com/andenore/CopperLib/packages/parts/quectel/eg800g-eu/footprints
footprint-library MAX_M10S github.com/andenore/CopperLib/packages/parts/u-blox/max-m10s/footprints
footprint-library KENTO github.com/andenore/CopperLib/packages/parts/kento/kt0603r/footprints

// The example dependency is kept in this repository for offline development.
replace github.com/copperscript/examples => ./examples/packages
