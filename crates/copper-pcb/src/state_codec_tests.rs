//! Saved mechanical constraints must survive decoding without widening their allowance.
use super::*;

fn dc(name: &str, fields: Vec<(&str, StateValue)>) -> StateValue {
    StateValue::Dataclass {
        name: format!("pcbir.physical:{name}"),
        fields: fields.into_iter().map(|(name, value)| (name.to_owned(), value)).collect(),
    }
}

fn s(value: &str) -> StateValue {
    StateValue::Str(value.to_owned())
}
fn i(value: i64) -> StateValue {
    StateValue::Int(value.into())
}
fn p(x: i64, y: i64) -> StateValue {
    dc("Point", vec![("x_nm", i(x)), ("y_nm", i(y))])
}
fn tuple(values: Vec<StateValue>) -> StateValue {
    StateValue::Tuple(values)
}

fn fields(value: &mut StateValue) -> &mut IndexMap<String, StateValue> {
    let StateValue::Dataclass { fields, .. } = value else { panic!("fixture dataclass") };
    fields
}

fn saved_overhang() -> StateValue {
    dc(
        "BodyOverhang",
        vec![
            ("id", s("USB_NOSE")),
            ("reference", s("J_USB")),
            ("edge", s("USB_END")),
            ("start_nm", i(11_500_000)),
            ("end_nm", i(23_500_000)),
            ("distance_nm", i(2_000_000)),
            ("reason", s("  Manufacturer drawing permits body only; copper remains inside.  ")),
        ],
    )
}

fn saved_board() -> StateValue {
    dc(
        "PhysicalBoard",
        vec![
            ("name", s("Mechanical checkpoint")),
            (
                "outline",
                dc(
                    "BoardOutline",
                    vec![(
                        "vertices",
                        tuple(vec![p(0, 0), p(140_000_000, 0), p(140_000_000, 35_000_000), p(0, 35_000_000)]),
                    )],
                ),
            ),
            (
                "footprints",
                StateValue::Map {
                    entries: vec![(
                        s("connector"),
                        dc(
                            "PhysicalFootprint",
                            vec![
                                ("name", s("connector")),
                                ("pads", tuple(vec![])),
                                (
                                    "body_size",
                                    dc("Size", vec![("width_nm", i(1_000_000)), ("height_nm", i(1_000_000))]),
                                ),
                            ],
                        ),
                    )],
                    proxy: true,
                },
            ),
            (
                "placements",
                tuple(vec![dc(
                    "Placement",
                    vec![
                        ("reference", s("J_USB")),
                        ("footprint", s("connector")),
                        ("position", p(139_000_000, 17_500_000)),
                    ],
                )]),
            ),
            ("nets", tuple(vec![])),
            (
                "boundary_edges",
                tuple(vec![dc(
                    "BoardEdge",
                    vec![("id", s("USB_END")), ("start", p(140_000_000, 0)), ("end", p(140_000_000, 35_000_000))],
                )]),
            ),
            ("body_overhangs", tuple(vec![saved_overhang()])),
        ],
    )
}

#[test]
fn saved_body_overhang_preserves_exact_fields_and_board_boundary() {
    let board = board(&saved_board()).unwrap();
    assert_eq!(board.body_overhangs, vec![body_overhang(&saved_overhang()).unwrap()]);
    let allowance = &board.body_overhangs[0];
    assert_eq!((allowance.start_nm, allowance.end_nm, allowance.distance_nm), (11_500_000, 23_500_000, 2_000_000));
    assert_eq!(allowance.reason, "  Manufacturer drawing permits body only; copper remains inside.  ");
    assert_eq!(
        board.outline.vertices,
        vec![
            Point::new(0, 0),
            Point::new(140_000_000, 0),
            Point::new(140_000_000, 35_000_000),
            Point::new(0, 35_000_000),
        ]
    );
    assert!(
        body_outline(&board.outline, &board.boundary_edges, allowance)
            .unwrap()
            .vertices
            .iter()
            .any(|point| point.x_nm == 142_000_000)
    );
}

#[test]
fn saved_body_overhang_rejects_missing_unknown_and_mistyped_fields() {
    for name in ["id", "reference", "edge", "start_nm", "end_nm", "distance_nm", "reason"] {
        let mut saved = saved_overhang();
        fields(&mut saved).shift_remove(name);
        assert!(body_overhang(&saved).is_err(), "missing {name}");
        let mut saved = saved_overhang();
        fields(&mut saved).insert(name.to_owned(), StateValue::Null);
        assert!(body_overhang(&saved).is_err(), "mistyped {name}");
    }
    let mut saved = saved_overhang();
    fields(&mut saved).insert("extra".to_owned(), i(1));
    assert!(body_overhang(&saved).is_err());
}

#[test]
fn saved_body_overhang_rejects_blank_identities_and_invalid_ranges() {
    for name in ["id", "reference", "edge", "reason"] {
        let mut saved = saved_overhang();
        fields(&mut saved).insert(name.to_owned(), s(" \t\u{00a0}"));
        assert!(body_overhang(&saved).is_err(), "blank {name}");
    }
    for (name, value) in
        [("start_nm", -1), ("start_nm", 23_500_000), ("end_nm", 11_000_000), ("distance_nm", 0), ("distance_nm", -1)]
    {
        let mut saved = saved_overhang();
        fields(&mut saved).insert(name.to_owned(), i(value));
        assert!(body_overhang(&saved).is_err(), "{name}={value}");
    }
}

#[test]
fn saved_body_overhang_rejects_unknown_owner_edge_and_excess_interval() {
    for (name, value) in [("reference", s("MISSING")), ("edge", s("MISSING")), ("end_nm", i(35_000_001))] {
        let mut saved = saved_board();
        let mut allowance = saved_overhang();
        fields(&mut allowance).insert(name.to_owned(), value);
        fields(&mut saved).insert("body_overhangs".to_owned(), tuple(vec![allowance]));
        assert!(board(&saved).is_err(), "invalid {name}");
    }
    let mut saved = saved_board();
    fields(&mut saved).insert("body_overhangs".to_owned(), tuple(vec![saved_overhang(), saved_overhang()]));
    assert!(board(&saved).unwrap_err().to_string().contains("duplicate body_overhangs"));
    let mut saved = saved_board();
    fields(&mut saved).insert("body_overhangs".to_owned(), s("not a sequence"));
    assert!(board(&saved).is_err());
}

#[test]
fn saved_body_overhang_rejects_nonboundary_and_diagonal_edges() {
    for end in [p(140_000_000, 34_000_000), p(139_000_000, 35_000_000)] {
        let mut saved = saved_board();
        fields(&mut saved).insert(
            "boundary_edges".to_owned(),
            tuple(vec![dc("BoardEdge", vec![("id", s("USB_END")), ("start", p(140_000_000, 0)), ("end", end)])]),
        );
        assert!(board(&saved).is_err());
    }
}

#[test]
fn unsupported_mechanical_constraints_are_still_rejected() {
    for name in ["mechanical_references", "zone_fills"] {
        let mut saved = saved_board();
        fields(&mut saved).insert(name.to_owned(), tuple(vec![StateValue::Null]));
        assert!(matches!(board(&saved), Err(Error::NotImplemented(_))), "{name}");
        for malformed in [StateValue::Null, s("not a sequence"), i(0)] {
            fields(&mut saved).insert(name.to_owned(), malformed);
            assert!(board(&saved).is_err(), "malformed {name}");
        }
    }
}
