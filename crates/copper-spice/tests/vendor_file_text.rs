//! Two things TI's regulator models need of the reader (plan F6): a file
//! that is not UTF-8 (a Windows-1252 en dash in a comment) and braces nested
//! inside an expression. The netlists are written for these tests.

use copper_sim::{Circuit, NodeId, SourceValue};
use copper_spice::{Dialect, Library};

#[test]
fn a_single_byte_character_in_a_comment_is_read() {
    let directory = std::env::temp_dir().join(format!("copper-spice-latin1-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("divider.lib");
    let mut bytes = b"* Data sheet SBVS000 ".to_vec();
    bytes.push(0x96); // A Windows-1252 en dash: not UTF-8.
    bytes.extend_from_slice(b" October 2017\n.SUBCKT DIV IN OUT\nR1 IN OUT 1k\nR2 OUT 0 1k\n.ENDS\n");
    std::fs::write(&path, &bytes).unwrap();
    let lib = Library::load(&path, Dialect::Pspice, false).unwrap_or_else(|e| panic!("{e}"));
    let mut c = Circuit::new();
    let (input, out) = (c.node("in"), c.node("out"));
    c.voltage_source("V", input, NodeId::GROUND, SourceValue::dc(2.0)).unwrap();
    lib.instantiate(&mut c, "X", "DIV", &[input, out], &[]).unwrap();
    let op = copper_sim::operating_point(&c).unwrap();
    assert!((op.voltage("out").unwrap() - 1.0).abs() < 1e-12);
    // Binary data is still refused.
    std::fs::write(&path, b".SUBCKT A 1\n\0\0\n.ENDS\n").unwrap();
    let message = Library::load(&path, Dialect::Pspice, false).unwrap_err().to_string();
    assert!(message.contains("binary"), "{message}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn braces_nested_in_an_expression_group_like_parentheses() {
    let text = "\
.SUBCKT CMP IN OUT PARAMS: VTH=0.6 HIGH=5
E1 OUT 0 VALUE = { IF(V(IN) > {VTH}, {HIGH}, 0) }
.ENDS
";
    let lib = Library::parse(text, "cmp.lib", Dialect::Pspice).unwrap_or_else(|e| panic!("{e}"));
    for (input, expected) in [(1.0, 5.0), (0.2, 0.0)] {
        let mut c = Circuit::new();
        let (a, out) = (c.node("in"), c.node("out"));
        c.voltage_source("V", a, NodeId::GROUND, SourceValue::dc(input)).unwrap();
        c.resistor("RL", out, NodeId::GROUND, 1e3).unwrap();
        lib.instantiate(&mut c, "X", "CMP", &[a, out], &[]).unwrap();
        let op = copper_sim::operating_point(&c).unwrap();
        assert!((op.voltage("out").unwrap() - expected).abs() < 1e-9, "{input} V in");
    }
}
