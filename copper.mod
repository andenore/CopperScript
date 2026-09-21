module github.com/anden/CopperScript

require github.com/copperscript/examples v0.1.0
require github.com/andenore/CopperLib v0.1.0

// The example dependency is kept in this repository for offline development.
replace github.com/copperscript/examples => ./examples/packages

// Reusable part and device definitions are developed in the sibling CopperLib repository.
replace github.com/andenore/CopperLib => ../CopperLib
