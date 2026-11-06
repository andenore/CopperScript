use super::*;
use crate::fixed_prefix::{AdmittedFixedPrefix, FixedContribution, FixedPrefixRole};
use crate::test_support_routers as routers;

fn fixture() -> (PhysicalBoard, AdmittedFixedPrefix) {
    let source = routers::pad_board(&[("S", &[(2.0, 2.0), (18.0, 2.0)]), ("T", &[])], 2, Vec::new());
    let stem = routers::track("S", (2.0, 2.0), (3.0, 3.0), 0.15, CopperLayer::Front);
    let contributions = [
        FixedContribution {
            id: "ordinary".to_owned(),
            role: FixedPrefixRole::OrdinaryEscape,
            tracks: vec![stem.clone(), stem],
            vias: vec![routers::via("S", (3.0, 3.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back)],
        },
        FixedContribution {
            id: "lattice".to_owned(),
            role: FixedPrefixRole::ViaLattice,
            tracks: Vec::new(),
            vias: vec![routers::via("T", (8.0, 8.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back)],
        },
    ];
    let frame = AdmittedFixedPrefix::admit(&source, &contributions).expect("valid declared frame");
    (source, frame)
}

#[test]
fn declared_prefix_round_trip_preserves_source_order_roles_and_duplicate_occurrences() {
    let (source, frame) = fixture();
    let rebuilt = fixed_prefix_input(&encode_fixed_prefix(&frame)).expect("fresh reconstruction");
    assert_eq!(rebuilt.source(), &source);
    assert_eq!(rebuilt.board(), frame.board());
    assert_eq!(rebuilt.contributions(), frame.contributions());
    assert_eq!(rebuilt.ranges(), frame.ranges());
    assert_eq!(rebuilt.contributions()[0].tracks.len(), 2);
    let mut changed_source = source;
    changed_source.rules.minimum_clearance_nm += 1;
    assert!(
        rebuilt.revalidate_for(&changed_source).is_err(),
        "a different outer replay source cannot borrow admission"
    );
}

#[test]
fn prefix_replay_does_not_accept_serialized_admission_credentials() {
    let (_, frame) = fixture();
    for key in ["admitted", "observational_digest"] {
        let mut encoded = encode_fixed_prefix(&frame);
        let StateValue::Dataclass { fields, .. } = &mut encoded else { panic!("typed input") };
        fields.insert(key.to_owned(), StateValue::Bool(true));
        assert!(fixed_prefix_input(&encoded).is_err(), "unknown proof field {key} must not become authority");
    }
}

#[test]
fn prefix_replay_freshly_rejects_changed_geometry_that_collides() {
    let (_, frame) = fixture();
    let mut encoded = encode_fixed_prefix(&frame);
    let StateValue::Dataclass { fields, .. } = &mut encoded else { panic!("typed input") };
    let StateValue::Tuple(parts) = fields.get_mut("contributions").expect("parts") else { panic!("tuple") };
    let StateValue::Dataclass { fields, .. } = &mut parts[1] else { panic!("lattice contribution") };
    let StateValue::Tuple(vias) = fields.get_mut("vias").expect("vias") else { panic!("tuple") };
    let StateValue::Dataclass { fields, .. } = &mut vias[0] else { panic!("via") };
    fields.insert("position".to_owned(), e_point(&routers::pt(3.0, 3.0)));
    assert!(fixed_prefix_input(&encoded).is_err(), "foreign barrels cannot borrow a previous valid frame");
}
