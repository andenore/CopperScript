"""Pure, revision-bound source patching; no filesystem or editor save endpoint.

Character spans refer to decoded UTF-8 (without a BOM). Replacing only scalar
tokens preserves comments/trivia, including comments *inside* a quantity.
This first slice supports board-owned direct components, not imported files or
hierarchical instance target translation. Candidate bytes still need compiler,
electrical-equality and physical validation before any future save operation.
"""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from hashlib import sha256
from math import isfinite

from ..lexer import Token, TokenKind, tokenize
from ..parser import parse
from ..syntax import ComponentDecl, ConstraintDecl, MechanicalDecl, MechanicalProfileUseDecl


class SourceEditError(ValueError):
    """Unsafe, ambiguous or stale source edit."""


@dataclass(frozen=True, slots=True)
class SourceSpan:
    start: int
    end: int


@dataclass(frozen=True, slots=True)
class TextEdit:
    span: SourceSpan
    replacement: str


@dataclass(frozen=True, slots=True)
class SourceSnapshot:
    raw: bytes
    filename: str = "<memory>"

    @property
    def text(self) -> str:
        return self.raw.decode("utf-8-sig")

    @property
    def revision(self) -> str:
        return sha256(self.raw).hexdigest()

    @property
    def newline(self) -> str:
        text = self.text
        index = text.find("\n")
        return "\r\n" if index > 0 and text[index - 1] == "\r" else "\n"


@dataclass(frozen=True, slots=True)
class SourcePatch:
    revision: str
    edits: tuple[TextEdit, ...]

    def apply(self, snapshot: SourceSnapshot) -> bytes:
        """Return candidate bytes; reject stale and overlapping/out-of-range edits."""
        if self.revision != snapshot.revision:
            raise SourceEditError("source changed since the patch was prepared")
        text = snapshot.text
        edits = sorted(self.edits, key=lambda edit: (edit.span.start, edit.span.end))
        previous = None
        for edit in edits:
            start, end = edit.span.start, edit.span.end
            if type(start) is not int or type(end) is not int or not 0 <= start <= end <= len(text):
                raise SourceEditError("source span is outside the snapshot")
            if previous and (start < previous.end or start == previous.start):
                raise SourceEditError("source edits overlap or share an insertion point")
            previous = edit.span
        for edit in reversed(edits):
            text = text[:edit.span.start] + edit.replacement + text[edit.span.end:]
        bom = b"\xef\xbb\xbf" if snapshot.raw.startswith(b"\xef\xbb\xbf") else b""
        return bom + text.encode("utf-8")


@dataclass(frozen=True, slots=True)
class ConstraintSpans:
    declaration: SourceSpan
    closing_brace: int
    # Property tokens omit surrounding and inter-token trivia intentionally.
    properties: tuple[tuple[str, tuple[SourceSpan, ...]], ...]


def _span(token: Token) -> SourceSpan:
    if token.end_offset is None:
        raise SourceEditError("token has no original source end offset")
    return SourceSpan(token.location.offset, token.end_offset)


def _constraint_spans(tokens: tuple[Token, ...], declaration: ConstraintDecl) -> ConstraintSpans:
    i = next(i for i, token in enumerate(tokens) if token.location.offset == declaration.location.offset)
    beginning = _span(tokens[i - 1]).start  # The preceding 'constraint' keyword.
    while not (tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == "{"):
        i += 1
    i += 1
    properties = []
    while not (tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == "}"):
        name = tokens[i].text
        i += 2  # property, '='
        values = []
        while not (tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == ";"):
            values.append(_span(tokens[i]))
            i += 1
        properties.append((name, tuple(values)))
        i += 1
    return ConstraintSpans(SourceSpan(beginning, _span(tokens[i]).end),
                           tokens[i].location.offset, tuple(properties))


def fixed_placement_patch(snapshot: SourceSnapshot, reference: str, *,
                          x_nm: int | None = None, y_nm: int | None = None,
                          rotation: float | None = None, side: str | None = None) -> SourcePatch:
    """Update/add a single board-owned fixed placement without rewriting the file.

    Unspecified properties are retained. Position must be supplied as a pair.
    Multi-target/duplicate constraints are rejected instead of editing ownership
    implicitly. This function validates syntax, not fabrication legality.
    """
    document = parse(snapshot.text, snapshot.filename)
    if document.kind != "board":
        raise SourceEditError("placement edits require a board source")
    if reference not in {d.ref for d in document.declarations if isinstance(d, ComponentDecl)}:
        raise SourceEditError("target must be a direct component owned by this board source")
    if any(isinstance(item, MechanicalProfileUseDecl) and reference in item.bindings.values()
           for d in document.declarations if isinstance(d, MechanicalDecl) for item in d.items):
        raise SourceEditError("component pose is owned by an imported board profile")
    if (x_nm is None) != (y_nm is None):
        raise SourceEditError("position requires both x_nm and y_nm")
    values = {}
    for name, value in (("x", x_nm), ("y", y_nm)):
        if value is not None:
            if type(value) is not int or abs(value) > 10**12:
                raise SourceEditError("position must be integer nanometres within the editor range")
            values[name] = f"{format(Decimal(value) / Decimal(1000000), 'f')}mm"
    if rotation is not None:
        if type(rotation) not in (int, float) or abs(rotation) > 3600 or not isfinite(rotation):
            raise SourceEditError("rotation must be finite degrees within the editor range")
        values["rotation"] = format(Decimal(str(rotation)), "f")
    if side is not None:
        if side not in ("front", "back"):
            raise SourceEditError("side must be front or back")
        values["side"] = side
    if not values:
        raise SourceEditError("no placement properties supplied")
    matching = [d for d in document.declarations if isinstance(d, ConstraintDecl)
                and d.kind == "fixed_placement" and reference in d.targets]
    if len(matching) > 1 or (matching and matching[0].targets != (reference,)):
        raise SourceEditError("ambiguous or shared fixed-placement constraint; resolve ownership first")
    tokens = tokenize(snapshot.text, snapshot.filename)
    edits = []
    if matching:
        spans = _constraint_spans(tokens, matching[0])
        remaining = dict(values)
        for name, value_spans in spans.properties:
            if name not in remaining:
                continue
            value = remaining.pop(name)
            # Replace the first scalar token and erase others, leaving every
            # original comment and whitespace byte between them untouched.
            edits.extend(TextEdit(span, value if i == 0 else "") for i, span in enumerate(value_spans))
        if remaining:
            insertion = " " + " ".join(f"{key} = {value};" for key, value in remaining.items()) + " "
            edits.append(TextEdit(SourceSpan(spans.closing_brace, spans.closing_brace), insertion))
    else:
        # Valid parse guarantees the final non-EOF token closes the root board.
        closing = tokens[-2].location.offset
        line_start = snapshot.text.rfind("\n", 0, closing) + 1
        own_line = not snapshot.text[line_start:closing].strip()
        offset = line_start if own_line else closing
        assignments = " ".join(f"{key} = {value};" for key, value in values.items())
        insertion = ("" if own_line else snapshot.newline) + f"    constraint fixed_placement({reference}) {{ {assignments} }}" + snapshot.newline
        edits.append(TextEdit(SourceSpan(offset, offset), insertion))
    patch = SourcePatch(snapshot.revision, tuple(edits))
    parse(SourceSnapshot(patch.apply(snapshot), snapshot.filename).text, snapshot.filename)
    return patch
