//! Fabrication panels: already-routed KiCad boards composed into one panel
//! board with rails, mouse-bite tabs, fiducials and tooling holes, checked
//! against each board's own intent (see docs/panelization-plan.md).

pub mod compose;
pub mod rules;
pub mod sexpr;
