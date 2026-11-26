#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;

use boards::{board, footprint, pad, placement};
use copper_drc::placement_copper_findings;

#[test]
fn overlapping_copper_of_one_unconnected_terminal_passes_placement_drc() {
    let footprint =
        footprint("compound", vec![pad("7", 0.725, 0.875, 0.25, 0.65), pad("7", 0.9, 0.7, 0.6, 0.3)], 3.0, 3.0);
    let placed = placement("U1", &footprint.name, 10.0, 10.0);
    let mut board = board("compound-pad", 30.0, 30.0, vec![footprint], vec![placed], vec![]);
    assert!(placement_copper_findings(&board).unwrap().is_empty());

    let pad = &mut board.footprints.get_mut("compound").unwrap().pads[1];
    pad.number = "8".to_owned();
    let findings = placement_copper_findings(&board).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "DRC-SHORT");
    assert_eq!(findings[0].objects, ["pad:U1.7:0", "pad:U1.8:1"]);
}

#[test]
fn disjoint_or_unnumbered_lands_and_other_footprints_keep_clearance_checks() {
    for (second_number, x, second_footprint) in [("7", 0.55, false), ("", 0.0, false), ("7", 0.0, true)] {
        let first_number = if second_number.is_empty() { "" } else { "7" };
        let mut pads = vec![pad(first_number, 0.0, 0.0, 0.5, 0.5)];
        let mut placements = vec![placement("U1", "pad-test", 10.0, 10.0)];
        if second_footprint {
            placements.push(placement("U2", "pad-test", 10.0, 10.0));
        } else {
            pads.push(pad(second_number, x, 0.0, 0.5, 0.5));
        }
        let board = board("pad-clearance", 30.0, 30.0, vec![footprint("pad-test", pads, 3.0, 3.0)], placements, vec![]);
        let findings = placement_copper_findings(&board).unwrap();
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, if x > 0.0 { "DRC-CLEARANCE" } else { "DRC-SHORT" });
    }
}
