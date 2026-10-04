"""Dependency-free lexer for CopperScript."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

from .syntax import CopperScriptError, SourceLocation


class TokenKind(str, Enum):
    IDENTIFIER = "identifier"
    NUMBER = "number"
    STRING = "string"
    SYMBOL = "symbol"
    EOF = "end of file"


@dataclass(frozen=True, slots=True)
class Token:
    kind: TokenKind
    text: str
    location: SourceLocation
    # Exclusive character offset in the original source, including quotes/escapes.
    # Optional only for callers constructing synthetic tokens.
    end_offset: int | None = None


class Lexer:
    def __init__(self, source: str, filename: str = "<memory>"):
        self.source = source
        self.filename = filename
        self.offset = 0
        self.line = 1
        self.column = 1

    def tokenize(self) -> tuple[Token, ...]:
        tokens: list[Token] = []
        while self.offset < len(self.source):
            character = self._peek()
            if character.isspace():
                self._advance()
            elif character == "#" or (character == "/" and self._peek(1) == "/"):
                self._skip_comment()
            elif character.isalpha() or character == "_":
                tokens.append(self._identifier())
            elif character.isdigit():
                tokens.append(self._number())
            elif character == '"':
                tokens.append(self._string())
            elif character in "{}[]():;=,.+-":
                location = self._location()
                tokens.append(Token(TokenKind.SYMBOL, self._advance(), location, self.offset))
            else:
                raise CopperScriptError(
                    "LEX001", f"unexpected character {character!r}", self._location()
                )
        tokens.append(Token(TokenKind.EOF, "", self._location(), self.offset))
        return tuple(tokens)

    def _identifier(self) -> Token:
        location = self._location()
        start = self.offset
        while self._peek().isalnum() or self._peek() in "_-":
            self._advance()
        return Token(TokenKind.IDENTIFIER, self.source[start : self.offset], location, self.offset)

    def _number(self) -> Token:
        location = self._location()
        start = self.offset
        while self._peek().isdigit():
            self._advance()
        if self._peek() == "." and self._peek(1).isdigit():
            self._advance()
            while self._peek().isdigit():
                self._advance()
        return Token(TokenKind.NUMBER, self.source[start : self.offset], location, self.offset)

    def _string(self) -> Token:
        location = self._location()
        self._advance()
        characters: list[str] = []
        escapes = {'"': '"', "\\": "\\", "n": "\n", "t": "\t"}
        while self.offset < len(self.source) and self._peek() != '"':
            if self._peek() == "\\":
                self._advance()
                escaped = self._peek()
                if escaped not in escapes:
                    raise CopperScriptError(
                        "LEX002", f"unsupported escape sequence \\{escaped}", self._location()
                    )
                characters.append(escapes[escaped])
                self._advance()
            else:
                characters.append(self._advance())
        if self.offset >= len(self.source):
            raise CopperScriptError("LEX003", "unterminated string", location)
        self._advance()
        return Token(TokenKind.STRING, "".join(characters), location, self.offset)

    def _skip_comment(self) -> None:
        while self.offset < len(self.source) and self._peek() not in "\r\n":
            self._advance()

    def _peek(self, distance: int = 0) -> str:
        index = self.offset + distance
        return self.source[index] if index < len(self.source) else "\0"

    def _advance(self) -> str:
        character = self.source[self.offset]
        self.offset += 1
        if character == "\n":
            self.line += 1
            self.column = 1
        else:
            self.column += 1
        return character

    def _location(self) -> SourceLocation:
        return SourceLocation(self.filename, self.offset, self.line, self.column)


def tokenize(source: str, filename: str = "<memory>") -> tuple[Token, ...]:
    return Lexer(source, filename).tokenize()
