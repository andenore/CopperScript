"""LSP UTF-16 coordinates, preserving Unicode and CRLF source bytes."""
from bisect import bisect_right
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import url2pathname


def path_from_uri(uri: str) -> Path:
    parsed = urlsplit(uri)
    if parsed.scheme != 'file' or parsed.query or parsed.fragment:
        raise ValueError('a local file URI is required')
    if parsed.netloc not in {'', 'localhost'}:
        raise ValueError('remote file hosts are unsupported')
    return Path(url2pathname(parsed.path)).resolve()


class Text:
    def __init__(self, source: str):
        self.source = source
        self.starts = [0, *(i + 1 for i, c in enumerate(source) if c == '\n')]

    def position(self, offset: int) -> dict:
        offset = max(0, min(len(self.source), offset))
        line = bisect_right(self.starts, offset) - 1
        return {'line': line, 'character': len(self.source[self.starts[line]:offset].encode('utf-16-le')) // 2}

    def offset(self, position: dict) -> int:
        line, character = position['line'], position['character']
        if type(line) is not int or type(character) is not int or not 0 <= line < len(self.starts) or character < 0:
            raise ValueError('invalid text position')
        start = self.starts[line]
        end = self.starts[line + 1] if line + 1 < len(self.starts) else len(self.source)
        content = self.source[start:end].rstrip('\r\n')
        units = 0
        for i, c in enumerate(content):
            if units == character:
                return start + i
            units += 2 if ord(c) > 0xffff else 1
            if units > character:
                raise ValueError('position splits a UTF-16 surrogate pair')
        if units == character:
            return start + len(content)
        raise ValueError('position is past end of line')

    def range(self, start: int, end: int) -> dict:
        return {'start': self.position(start), 'end': self.position(end)}

    def change(self, changes: list[dict]) -> str:
        source = self.source
        for change in changes:
            if not isinstance(change.get('text'), str):
                raise ValueError('change text must be a string')
            if 'range' not in change:
                source = change['text']
            else:
                current = Text(source)
                start = current.offset(change['range']['start'])
                end = current.offset(change['range']['end'])
                if end < start:
                    raise ValueError('reversed change range')
                source = source[:start] + change['text'] + source[end:]
        return source
