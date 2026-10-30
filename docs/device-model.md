# Device and package semantics

The electrical model is implemented. Use [language reference](language-reference.md)
for source syntax (the Python device generator was removed with the Python
implementation); this guide explains the model's invariants and scope.

## Physical identities and semantic views

Device pads and package pins are physical identities. Functional units belong
to device definitions; packages bond physical pins to device pads. Unit terminals,
signal groups and interfaces provide semantic views that resolve to physical
identities. Source/provenance retains the semantic spelling; compiled endpoints
also retain the canonical physical identity used by ERC. Nets alone establish
connectivity. Shared unit supply terminals do not duplicate physical pins.

## Electrical profiles and package rules

Signal domain, direction, drive behavior, traits and typed limits are orthogonal.
Required peripheral profiles are checked against the selected physical pad.
Explicit connection policies distinguish required, optional and do-not-connect
pins; package bonds and required net traits remain authoritative. Differential
groups preserve member completeness, polarity and selected-mode availability.
Analog checks cover structure, connections, polarity and ranges, not stability,
converter behavior or signal integrity.

Category/interface names are open identifiers. Unknown names remain representable;
core checking never infers function solely from a taxonomy string. Device-specific
facts outside generic checking remain namespaced metadata.

## Modes and routable selections

Finite named mode groups and conjunctions of equality selections keep conditions
bounded and deterministic. Reject conflicting selections and unavailable pads,
groups or bonds. Explicit mux tables coexist with pad-set/route-rule selections,
so programmable GPIO routing does not require a Cartesian-product table.
Record a concrete selection and selector value; do not silently configure firmware.

## Checking and presentation boundaries

Canonical endpoint resolution enforces one physical pin per net, package policies,
typed operating ranges, required/provided profiles and trusted protocol validators
such as I2C pull-up checks. Firmware-managed sharing is an explicit ownership
waiver, not an electrical waiver. Behavioral analog/RF/power verification needs
separate evidence.

Backends own drawing coordinates and graphical multi-unit presentation. The
KiCad backend resolves semantic terminals to physical pins; richer native
multi-unit rendering is a [future contributor task](contributor-backlog.md).
