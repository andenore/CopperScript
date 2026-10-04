"""Pure candidate source edits: ownership, exact bytes, comments and revisions."""
from dataclasses import replace

import pytest

from pcbir.compiler import compile_design_source
from pcbir.editor.source import (SourceEditError, SourcePatch, SourceSnapshot,
    SourceSpan, TextEdit, fixed_placement_patch)
from pcbir.lexer import TokenKind, tokenize
from pcbir.parser import parse
from pcbir.syntax import ConstraintDecl, RawQuantity


BASE = '''// Unicode Ω and a string containing braces must survive.
board SourcePatch {
    use library "standard";
    component R1: RESISTOR { value = 1kohm; footprint = "brace } and \\"quote\\""; }
    component R2: RESISTOR;
    net N { R1.1; R2.1; }
    mechanical { outline circle { diameter = 30mm; } }
    // Connector placement ownership remains here.
    constraint fixed_placement(R1) { x = - // Keep this sign comment.
        2 mm; y = 3mm; rotation = 90; side = front; } // End of placement.
    constraint allowed_orientations(R1) { values = "0,45,90,135,180,225,270,315"; }
} // Final comment.
'''


def parameters(candidate):
    doc = parse(SourceSnapshot(candidate).text)
    return next(d.parameters for d in doc.declarations if isinstance(d, ConstraintDecl)
                and d.kind == "fixed_placement" and d.targets == ("R1",))


@pytest.mark.parametrize("newline", ["\n", "\r\n"])
@pytest.mark.parametrize("bom", [b"", b"\xef\xbb\xbf"])
def test_patch_preserves_comments_encoding_newlines_and_other_declarations(newline, bom):
    raw = bom + BASE.replace("\n", newline).encode("utf-8")
    snapshot = SourceSnapshot(raw)
    patch = fixed_placement_patch(snapshot, "R1", x_nm=12500000, y_nm=4250000, rotation=45, side="back")
    candidate = patch.apply(snapshot)
    expected = raw.replace(b"x = -", b"x = 12.5mm").replace(b"2 mm; y = 3mm", b" ; y = 4.25mm").replace(
        b"rotation = 90; side = front", b"rotation = 45; side = back")
    assert candidate == expected
    assert snapshot.raw == raw  # No mutation or filesystem writes.
    assert parameters(candidate) == {"x": RawQuantity("12.5", "mm"), "y": RawQuantity("4.25", "mm"), "rotation": 45, "side": "back"}


def test_candidate_compile_preserves_electrical_ir():
    snapshot = SourceSnapshot(BASE.encode())
    candidate = fixed_placement_patch(snapshot, "R1", rotation=45).apply(snapshot)
    before = compile_design_source(snapshot.text)
    after = compile_design_source(SourceSnapshot(candidate).text)
    # Only constraints differ. All authoritative electrical objects are equal.
    assert replace(before.electrical, constraints=()) == replace(after.electrical, constraints=())
    assert before.mechanical == after.mechanical


@pytest.mark.parametrize("newline", ["\n", "\r\n"])
def test_insert_owned_constraint_deterministic_and_retains_trailing_text(newline):
    snapshot = SourceSnapshot(BASE.replace("\n", newline).encode())
    patch = fixed_placement_patch(snapshot, "R2", x_nm=-1, y_nm=1000001, side="back")
    assert patch == fixed_placement_patch(snapshot, "R2", x_nm=-1, y_nm=1000001, side="back")
    candidate = patch.apply(snapshot)
    insertion = f"    constraint fixed_placement(R2) {{ x = -0.000001mm; y = 1.000001mm; side = back; }}{newline}".encode()
    offset = snapshot.raw.index(b"} // Final comment.")
    assert candidate == snapshot.raw[:offset] + insertion + snapshot.raw[offset:]


def test_insert_in_single_line_board():
    snapshot = SourceSnapshot(b"board B { component R1: RESISTOR; }")
    candidate = fixed_placement_patch(snapshot, "R1", rotation=45).apply(snapshot)
    assert candidate == b"board B { component R1: RESISTOR; \n    constraint fixed_placement(R1) { rotation = 45; }\n}"


def test_add_missing_properties_keeps_unmodified_values():
    snapshot = SourceSnapshot(b"board B { component R1: RESISTOR; constraint fixed_placement(R1) { rotation = 90; // note\n} }")
    candidate = fixed_placement_patch(snapshot, "R1", x_nm=1000000, y_nm=2000000).apply(snapshot)
    assert b"rotation = 90; // note\n" in candidate
    assert parameters(candidate)["rotation"] == 90
    assert parameters(candidate)["x"] == RawQuantity("1", "mm")


def test_scalar_delimiter_strings_do_not_confuse_span_index():
    snapshot = SourceSnapshot(b'board B { component R1: RESISTOR; constraint fixed_placement(R1) { note = "; }"; rotation = 90; } }')
    candidate = fixed_placement_patch(snapshot, "R1", rotation=45).apply(snapshot)
    assert b'note = "; }";' in candidate
    assert parameters(candidate)["rotation"] == 45


@pytest.mark.parametrize("target", ["M/R1", "M.R1", "UNKNOWN"])
def test_rejects_unowned_and_hierarchical_targets(target):
    snapshot = SourceSnapshot(BASE.encode())
    with pytest.raises(SourceEditError, match="direct component"):
        fixed_placement_patch(snapshot, target, rotation=45)


@pytest.mark.parametrize("extra", [
    "constraint fixed_placement(R1) { rotation = 45; }",
    "constraint fixed_placement(R1, R2) { rotation = 45; }",
])
def test_duplicate_or_shared_constraints_rejected(extra):
    snapshot = SourceSnapshot(BASE.replace("} // Final comment.", extra + " } // Final comment.").encode())
    with pytest.raises(SourceEditError, match="ambiguous or shared"):
        fixed_placement_patch(snapshot, "R1", rotation=45)


def test_single_shared_constraint_rejected():
    snapshot = SourceSnapshot(b"board B { component R1: RESISTOR; component R2: RESISTOR; constraint fixed_placement(R1, R2) { rotation = 0; } }")
    with pytest.raises(SourceEditError, match="shared"):
        fixed_placement_patch(snapshot, "R1", rotation=45)


def test_imported_content_not_loaded_or_modified():
    snapshot = SourceSnapshot(b'board B { import remote "https://example.invalid/lib.git//part.copper"; component R1: remote; }')
    candidate = fixed_placement_patch(snapshot, "R1", rotation=45).apply(snapshot)
    assert b'import remote "https://example.invalid/lib.git//part.copper";' in candidate
    # Patching is pure, so it succeeds without contacting the deliberately invalid URL.


def test_stale_patch_rejected_even_for_trivia_only_change():
    snapshot = SourceSnapshot(BASE.encode())
    patch = fixed_placement_patch(snapshot, "R1", rotation=45)
    with pytest.raises(SourceEditError, match="source changed"):
        patch.apply(SourceSnapshot(snapshot.raw + b"\n"))


@pytest.mark.parametrize("spans", [((0, 3), (2, 4)), ((0, 0), (0, 0)), ((-1, 1),), ((0, 10000),), ((3, 1),)])
def test_invalid_patch_spans_rejected(spans):
    snapshot = SourceSnapshot(b"abcde")
    patch = SourcePatch(snapshot.revision, tuple(TextEdit(SourceSpan(*span), "x") for span in spans))
    with pytest.raises(SourceEditError):
        patch.apply(snapshot)


@pytest.mark.parametrize("values", [{}, {"x_nm": 1}, {"x_nm": True, "y_nm": 1},
    {"x_nm": 10**13, "y_nm": 1}, {"rotation": float("nan")}, {"rotation": float("inf")},
    {"rotation": True}, {"rotation": 4000}, {"rotation": 10**400}, {"side": "top"}])
def test_invalid_pose_values_rejected(values):
    with pytest.raises(SourceEditError):
        fixed_placement_patch(SourceSnapshot(BASE.encode()), "R1", **values)


def test_non_board_source_rejected():
    with pytest.raises(SourceEditError, match="board source"):
        fixed_placement_patch(SourceSnapshot(b"part Resistor { pin A {} }"), "R1", rotation=45)


def test_lexer_end_offsets_include_escaped_strings_and_unicode():
    source = 'board B { text = "Ω \\"quoted\\""; value = 1.2mm; }'
    tokens = tokenize(source)
    assert tokens[-1].end_offset == len(source)
    string = next(t for t in tokens if t.kind is TokenKind.STRING)
    assert source[string.location.offset:string.end_offset] == '"Ω \\"quoted\\""'
    assert string.text == 'Ω "quoted"'
    assert all(t.end_offset is not None and t.end_offset >= t.location.offset for t in tokens)
