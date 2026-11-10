//! Critical-routing and differential-pair behaviour specifications ported
//! from the former Python suite (d39067a `tests/`); see
//! `tests/PORTED-critical.md` for the file-by-file tracking table.

mod bundle_tuning;
mod collinear_pair_ports;
mod critical_bundles;
mod critical_diagnostics;
mod critical_lane_review;
mod critical_replay;
mod critical_return_preparation;
mod critical_routing;
mod critical_search_order;
mod critical_stage_planning;
mod local_critical;
mod loose_pair_repair;
mod pair_approach_ranking;
mod pair_budget_escalation;
mod pair_collinear_fork;
mod pair_converging_ports;
mod pair_departure;
mod pair_hybrid_contact_anchor;
mod pair_joint_supports;
mod pair_lateral_ports;
mod pair_obstacle_tuning_fallback;
mod pair_offset_ports;
mod pair_opposite_faces;
mod pair_outer_transitions;
mod pair_recessed_via_ports;
mod pair_refine;
mod pair_return_boundaries;
mod pair_search_resume;
mod pair_staggered_contacts;
mod pair_taper;
mod pair_terminal_fallback;
mod pair_through_hole_contacts;
mod pair_transition_diagnostics;
mod pair_transition_families;
mod pair_vias;
mod partial_pair_macro_ports;
mod planned_crossings;
mod planned_escape_copper;
mod recessed_pair_ports;
mod single_ended_return_vias;
