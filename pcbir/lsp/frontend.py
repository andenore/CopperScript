"""Recover complete declarations using the compiler's lexer and strict parser.

Recovery is editor-only: it never produces a buildable board or silently repairs
source. Original token offsets survive synthetic root delimiters.
"""
from dataclasses import dataclass

from ..lexer import Lexer, Token, TokenKind
from ..parser import Parser
from ..syntax import CopperScriptError, Document


@dataclass(frozen=True)
class Parsed:
    document: Document | None
    tokens: tuple[Token, ...]
    errors: tuple[CopperScriptError, ...]


def parse_recovering(text: str, filename: str) -> Parsed:
    lexer = Lexer(text, filename)
    tokens = lexer.tokenize(recover=True)
    errors = list(lexer.errors)
    eof = tokens[-1]
    close = Token(TokenKind.SYMBOL, '}', eof.location, eof.end_offset)
    try:
        empty = Parser((*tokens[:3], close, eof)).parse()
    except CopperScriptError as exc:
        return Parsed(None, tokens, tuple([*errors, exc]))
    declarations, imports, libraries = [], [], []
    i = 3
    closed = False
    starters = {'component', 'module', 'net', 'no_connect', 'supply', 'interface', 'constraint',
                'member', 'track', 'via', 'isolated', 'port', 'protected_region', 'keepout',
                'zone', 'polygon', 'plane_return', 'width_contract', 'edge_opening',
                'configure', 'power_state', 'port', 'import', 'use', 'mechanical', 'silkscreen', 'physical_macro',
                'pin', 'pad', 'peripheral', 'power_domain', 'unit', 'resource',
                'group', 'mode_group', 'pad_set', 'route'}
    while i < len(tokens) - 1:
        if tokens[i].kind is TokenKind.SYMBOL and tokens[i].text == '}':
            closed = True
            i += 1
            break
        start, depth = i, 0
        # A sibling starts at the indentation of the line the declaration
        # starts on. A block opened mid-line (``board B { silkscreen {``) keeps
        # its own indented items: nested polygon/track/via/zone/keepout items
        # share names with macro-definition declarations.
        indent = tokens[start].location.column
        for earlier in reversed(tokens[:start]):
            if earlier.location.line != tokens[start].location.line:
                break
            indent = earlier.location.column
        while i < len(tokens) - 1:
            t = tokens[i]
            # An unclosed declaration must not swallow subsequent siblings.
            if (i > start and t.kind is TokenKind.IDENTIFIER and t.text in starters
                    and t.location.line > tokens[start].location.line
                    and t.location.column <= indent):
                break
            if t.kind is TokenKind.SYMBOL:
                if t.text == '{':
                    depth += 1
                elif t.text == '}':
                    if depth == 0:
                        break
                    depth -= 1
                    if depth == 0:
                        i += 1
                        if (empty.kind == 'macro' and start + 1 < len(tokens)
                                and tokens[start + 1].text == '=' and tokens[i].text == ';'):
                            i += 1
                        break
                elif t.text == ';' and depth == 0:
                    i += 1
                    break
            i += 1
        if i == start:
            i += 1
        try:
            chunk = Parser((*tokens[:3], *tokens[start:i], close, eof)).parse()
            declarations.extend(chunk.declarations)
            imports.extend(chunk.imports)
            libraries.extend(chunk.libraries)
        except CopperScriptError as exc:
            errors.append(exc)
    if not closed:
        errors.append(CopperScriptError('PAR001', "expected '}' before end of file", eof.location))
    elif i < len(tokens) - 1:
        errors.append(CopperScriptError('PAR010', 'expected end of file', tokens[i].location))
    document = Document(empty.location, empty.kind, empty.name, tuple(libraries),
                        tuple(imports), tuple(declarations))
    return Parsed(document, tokens, tuple(errors))
