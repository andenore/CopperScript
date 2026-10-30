//! Crate-private behaviour tests ported from the former Python suite
//! (planes, stitching, fanout, vias, tuning and cleanup). The board
//! builders are shared with the integration tests in `tests/`.

#[path = "../tests/support_planes/mod.rs"]
mod support_planes;

/// `tests/test_length_match_tuning.py`.
mod length_match_tuning {
    use super::support_planes::lanes::*;
    use super::support_planes::*;
    use crate::critical::{CriticalOptions, route_critical_nets, validate_candidate};

    fn lane(tracks: &[copper_pcb::board::TrackSegment], name: &str) -> Vec<copper_pcb::board::TrackSegment> {
        let prefix = format!("{name}_");
        tracks.iter().filter(|track| track.net.starts_with(&prefix)).cloned().collect()
    }

    #[test]
    fn candidates_completing_an_over_skew_group_are_not_rejected_during_routing() {
        let board = Lanes { amplitude: None, ..Lanes::new(&TWO_LANES) }.build();
        let routed = route_critical_nets(board.clone(), &global(&board), &CriticalOptions::default(), None).unwrap();
        let second = routed.nets[1].clone();
        assert!(routed.nets[0].connected && second.connected);
        let tracks_a = lane(&routed.locked_tracks, "LANE_A");
        let tracks_b = lane(&routed.locked_tracks, "LANE_B");
        // Together the two pairs violate the group skew, yet the candidate
        // that completes the group passes validation: the tuning pass judges
        // the group.
        let (accepted, accepted_tracks, _) =
            validate_candidate(&board, second.clone(), tracks_b.clone(), Vec::new(), &tracks_a, &[], None).unwrap();
        assert!(accepted.connected && accepted.diagnostics.is_empty());
        assert_eq!(accepted_tracks, tracks_b);
        // Other hard findings still reject it.
        let mut blocked = board.clone();
        blocked.copper_keepouts = vec![lane_keepout("on-lane", 14.0, 13.0, 16.0, 15.0)];
        let (rejected, rejected_tracks, _) =
            validate_candidate(&blocked, second, tracks_b, Vec::new(), &tracks_a, &[], None).unwrap();
        assert!(!rejected.connected && rejected_tracks.is_empty());
        assert!(!rejected.diagnostics.iter().any(|message| message.starts_with("DRC-LENGTH-MATCH")));
    }
}
