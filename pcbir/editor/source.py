"""Pure, revision-bound source patching; no filesystem operations.

Character spans refer to decoded UTF-8 (without a BOM). Replacing only scalar
tokens preserves comments/trivia, including comments *inside* a quantity.
Imported bytes are never edited. Resolved hierarchical targets require an
explicit compiler-provided inventory. Candidates need semantic/physical validation.
"""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from hashlib import sha256
from math import isfinite

from ..lexer import Token, TokenKind, tokenize
from ..parser import parse
from ..syntax import (ComponentDecl, ConstraintDecl, MechanicalDecl, MechanicalItemDecl,
    MechanicalProfileUseDecl, ModuleInstanceDecl)


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
    assignments: tuple[tuple[str, tuple[SourceSpan, ...]], ...] = ()


def _span(token: Token) -> SourceSpan:
    if token.end_offset is None:
        raise SourceEditError("token has no original source end offset")
    return SourceSpan(token.location.offset, token.end_offset)


def declaration_spans(tokens: tuple[Token, ...], declaration) -> ConstraintSpans:
    """Index one assignment block, including its exact scalar/compound tokens.

    Trivia is the untouched gap between tokens. Nested coordinate arrays and
    strings containing delimiters do not confuse this index.
    """
    i = next(i for i, token in enumerate(tokens) if token.location.offset == declaration.location.offset)
    beginning = _span(tokens[i - 1] if isinstance(declaration, ConstraintDecl) else tokens[i]).start
    while not (tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == "{"):
        i += 1
    i += 1
    properties, assignments = [], []
    while not (tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == "}"):
        name = tokens[i].text
        start = i
        i += 2  # property, '='
        values = []
        while not (tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == ";"):
            values.append(_span(tokens[i]))
            i += 1
        properties.append((name, tuple(values)))
        assignments.append((name, tuple(_span(t) for t in tokens[start:i + 1])))
        i += 1
    return ConstraintSpans(SourceSpan(beginning, _span(tokens[i]).end),
                           tokens[i].location.offset, tuple(properties), tuple(assignments))


# Kept private alias for the placement patch's block format.
_constraint_spans = declaration_spans


@dataclass(frozen=True, slots=True)
class DeclarationSpan:
    """Generic token-bounded source node; surrounding trivia stays source-owned."""
    span: SourceSpan
    header: tuple[str, ...]
    children: tuple[DeclarationSpan, ...] = ()


def declaration_index(snapshot: SourceSnapshot) -> DeclarationSpan:
    """Index declarations recursively without reading imports or losing bytes."""
    parse(snapshot.text, snapshot.filename)
    tokens = tokenize(snapshot.text, snapshot.filename)

    def read(start):
        i = start
        while tokens[i].text not in {"{", ";"} or tokens[i].kind is not TokenKind.SYMBOL:
            i += 1
        header = tuple(t.text for t in tokens[start:i])
        children = []
        if tokens[i].text == "{":
            i += 1
            while not (tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == "}"):
                child, i = read(i)
                children.append(child)
        result = DeclarationSpan(SourceSpan(_span(tokens[start]).start, _span(tokens[i]).end),
                                 header, tuple(children))
        return result, i + 1

    return read(0)[0]


def _erase_tokens(tokens, span):
    # Comments/trivia survive deletion as well as replacement.
    return [TextEdit(_span(t), "") for t in tokens if t.kind is not TokenKind.EOF
            and span.start <= t.location.offset < span.end]


def fixed_placement_patch(snapshot: SourceSnapshot, reference: str, *,
                          x_nm: int | None = None, y_nm: int | None = None,
                          rotation: float | None = None, side: str | None = None,
                          clear: tuple[str, ...] = (),
                          resolved_references: frozenset[str] = frozenset()) -> SourcePatch:
    """Update/add a single board-owned fixed placement without rewriting the file.

    Unspecified properties are retained. Position must be supplied as a pair.
    Multi-target/duplicate constraints are rejected instead of editing ownership
    implicitly. This function validates syntax, not fabrication legality.
    """
    document = parse(snapshot.text, snapshot.filename)
    if document.kind != "board":
        raise SourceEditError("placement edits require a board source")
    direct = {d.ref for d in document.declarations if isinstance(d, ComponentDecl)}
    modules = {d.ref for d in document.declarations if isinstance(d, ModuleInstanceDecl)}
    if reference not in direct and not (reference in resolved_references and "/" in reference
                                       and reference.split("/")[0] in modules):
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
    if set(clear) - {"x", "y", "rotation", "side"} or ("x" in clear) != ("y" in clear):
        raise SourceEditError("clear requires known properties and a complete position pair")
    if set(clear) & set(values):
        raise SourceEditError("cannot set and clear the same property")
    if not values and not clear:
        raise SourceEditError("no placement properties supplied")
    matching = [d for d in document.declarations if isinstance(d, ConstraintDecl)
                and d.kind == "fixed_placement" and reference in d.targets]
    if len(matching) > 1 or (matching and matching[0].targets != (reference,)):
        raise SourceEditError("ambiguous or shared fixed-placement constraint; resolve ownership first")
    tokens = tokenize(snapshot.text, snapshot.filename)
    edits = []
    if matching:
        spans = _constraint_spans(tokens, matching[0])
        if not (set(matching[0].parameters) - set(clear)) and not values:
            patch = SourcePatch(snapshot.revision, tuple(_erase_tokens(tokens, spans.declaration)))
            parse(SourceSnapshot(patch.apply(snapshot), snapshot.filename).text, snapshot.filename)
            return patch
        edits.extend(TextEdit(span, "") for name, assignment in spans.assignments if name in clear
                     for span in assignment)
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
    elif values:
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


def mechanical_patch(snapshot: SourceSnapshot, *, kind: str, name: str = "", shape: str = "",
                     parameters: dict[str, str] | None = None, remove: bool = False) -> SourcePatch:
    """Target one project-owned feature; never expand/rewrite an imported profile.

    Values are source literals, not expressions. A scratch parse enforces exactly
    one intended feature and property inventory before patching token spans.
    """
    parameters = parameters or {}
    if kind not in {"outline", "hole", "cutout", "keepout", "copper_keepout", "rules", "datum", "edge", "attach", "overhang", "component_height", "enclosure", "assembly_access", "slot", "boundary", "reference"}:
        raise SourceEditError("unsupported mechanical feature kind")
    if type(remove) is not bool:
        raise SourceEditError("remove must be boolean")
    header = " ".join(x for x in (kind, name, shape) if x)
    body = " ".join(f"{key} = {value};" for key, value in parameters.items())
    literal = f"{header} {{ {body} }}"
    scratch = parse(f"board Candidate {{ mechanical {{ {literal} }} }}")
    if len(scratch.declarations) != 1 or not isinstance(scratch.declarations[0], MechanicalDecl):
        raise SourceEditError("mechanical patch must contain exactly one intended feature")
    items = scratch.declarations[0].items
    if (len(items) != 1 or not isinstance(items[0], MechanicalItemDecl)
            or (items[0].kind, items[0].name, items[0].shape) != (kind, name, shape)
            or set(items[0].parameters) != set(parameters)):
        raise SourceEditError("mechanical patch must contain exactly one intended feature")
    document = parse(snapshot.text, snapshot.filename)
    if document.kind != "board":
        raise SourceEditError("mechanical edits require a board source")
    blocks = [d for d in document.declarations if isinstance(d, MechanicalDecl)]
    if len(blocks) > 1:
        raise SourceEditError("ambiguous mechanical block ownership")
    tokens = tokenize(snapshot.text, snapshot.filename)
    edits = []
    if blocks:
        block = blocks[0]
        if kind == "outline" and any(isinstance(i, MechanicalProfileUseDecl) for i in block.items):
            raise SourceEditError("outline may be owned by an imported board profile")
        matches = [i for i in block.items if isinstance(i, MechanicalItemDecl)
                   and i.kind == kind and i.name == name]
        if len(matches) > 1:
            raise SourceEditError("ambiguous mechanical feature ownership")
        if matches:
            item = matches[0]
            spans = declaration_spans(tokens, item)
            if remove:
                if kind == "outline":
                    raise SourceEditError("cannot remove the board outline")
                edits = _erase_tokens(tokens, spans.declaration)
            elif item.shape != shape:
                edits = _erase_tokens(tokens, spans.declaration)
                # Replace the first token rather than inserting at its offset.
                edits[0] = TextEdit(edits[0].span, literal)
            else:
                remaining = dict(parameters)
                for key, values in spans.properties:
                    if key in remaining:
                        value = remaining.pop(key)
                        edits.extend(TextEdit(s, value if n == 0 else "") for n, s in enumerate(values))
                    else:
                        edits.extend(TextEdit(s, "") for k, assignment in spans.assignments
                                     if k == key for s in assignment)
                if remaining:
                    text = " " + " ".join(f"{k} = {v};" for k, v in remaining.items()) + " "
                    edits.append(TextEdit(SourceSpan(spans.closing_brace, spans.closing_brace), text))
        elif remove:
            raise SourceEditError("feature is not owned by this source")
        else:
            node = next(n for n in declaration_index(snapshot).children if n.header == ("mechanical",))
            offset = node.span.end - 1
            edits.append(TextEdit(SourceSpan(offset, offset), snapshot.newline + "        " + literal + snapshot.newline + "    "))
    elif remove:
        raise SourceEditError("feature is not owned by this source")
    else:
        offset = tokens[-2].location.offset
        text = snapshot.newline + "    mechanical { " + literal + " }" + snapshot.newline
        edits.append(TextEdit(SourceSpan(offset, offset), text))
    patch = SourcePatch(snapshot.revision, tuple(edits))
    parse(SourceSnapshot(patch.apply(snapshot), snapshot.filename).text, snapshot.filename)
    return patch
