//! Critical-routing and differential-pair behaviour specifications ported
//! from the former Python suite (d39067a `tests/`); see
//! `tests/PORTED-critical.md` for the file-by-file tracking table.

mod critical_bundles;
mod critical_diagnostics;
mod critical_lane_review;
mod critical_replay;
mod critical_routing;
mod critical_search_order;
mod local_critical;
mod pair_budget_escalation;
mod pair_collinear_fork;
mod pair_converging_ports;
mod pair_hybrid_contact_anchor;
mod pair_joint_supports;
mod pair_opposite_faces;
mod pair_outer_transitions;
mod pair_recessed_via_ports;
mod pair_refine;
mod pair_return_boundaries;
mod pair_search_resume;
mod pair_taper;
mod pair_terminal_fallback;
mod pair_through_hole_contacts;
mod pair_tuning_spans;
mod pair_vias;
mod planned_crossings;
mod planned_escape_copper;
mod recessed_pair_ports;
mod single_ended_return_vias;
