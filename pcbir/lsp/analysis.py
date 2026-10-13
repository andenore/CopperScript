"""Source-derived symbols and compiler-backed editor diagnostics.

Only declared imports are traversed. Package resolution remains locked/offline;
editor overlays affect analysis, never dependency locks or build artifacts.
"""
from dataclasses import dataclass, field
from pathlib import Path
from threading import RLock

from ..compiler import QUANTITY_TYPES, compile_source
from ..erc import check
from ..lexer import TokenKind
from ..library import library_factory
from ..syntax import (ComponentDecl, ConfigurationDecl, ConstraintDecl,
                      CopperScriptError, Document, ModuleInstanceDecl, RawQuantity,
                      PhysicalMacroDecl, ImportDecl)
from .frontend import Parsed, parse_recovering
from .text import Text, path_from_uri
from .packages import SnapshotResolver
from ..macro_bindings import is_macro_import


MAX_TEXT = 8 * 1024 * 1024
MAX_IMPORT_DEPTH = 64
KEYWORDS = ('component', 'module', 'net', 'supply', 'interface', 'constraint',
            'configure', 'power_state', 'import', 'use', 'port', 'mechanical', 'silkscreen', 'physical_macro')
PROPERTIES = {
    'component': ('value', 'footprint', 'role', 'decouples'),
    'supply': ('voltage', 'net', 'source', 'external'),
    'pin': ('number', 'domains', 'directions', 'drive_modes', 'voltage_min',
            'voltage_max', 'bond', 'connection', 'traits'),
    'branding': ('enabled', 'position', 'size', 'rotation', 'side', 'material', 'exposed', 'layout'),
    'text': ('text', 'position', 'size', 'thickness', 'rotation', 'side', 'material', 'exposed'),
    'logo': ('position', 'size', 'rotation', 'side', 'material', 'exposed'),
    'line': ('start', 'end', 'thickness', 'side', 'material', 'exposed'),
    'rectangle': ('start', 'end', 'thickness', 'filled', 'side', 'material', 'exposed'),
    'circle': ('center', 'radius', 'thickness', 'filled', 'side', 'material', 'exposed'),
    'arc': ('start', 'mid', 'end', 'thickness', 'side', 'material', 'exposed'),
    'polygon': ('vertices', 'thickness', 'filled', 'side', 'material', 'exposed'),
}
KINDS = {'ComponentDecl': 19, 'ModuleInstanceDecl': 2, 'NetDecl': 13,
         'SupplyDecl': 13, 'PortDecl': 8, 'PinDecl': 8, 'PadDecl': 8,
         'PeripheralDecl': 6, 'ConfigurationDecl': 6, 'InterfaceDecl': 11,
         'ConstraintDecl': 14, 'PowerStateDecl': 13, 'PhysicalMacroDecl': 2}


def scalar(value) -> str:
    if isinstance(value, RawQuantity):
        kind = QUANTITY_TYPES.get(value.unit)
        return str(kind.of(value.value, value.unit)) if kind else f'{value.value} {value.unit}'
    return str(value).lower() if isinstance(value, bool) else str(value)


@dataclass
class Unit:
    path: Path
    namespace: str
    text: Text
    parsed: Parsed
    symbols: list = field(default_factory=list)

    @property
    def document(self) -> Document | None:
        return self.parsed.document


@dataclass
class Symbol:
    name: str
    unit: Unit
    node: object
    start: int
    end: int
    selection_start: int
    selection_end: int
    kind: int

    def location(self):
        return {'uri': self.unit.path.as_uri(),
                'range': self.unit.text.range(self.selection_start, self.selection_end)}

    def outline(self):
        return {'name': self.name, 'kind': self.kind,
                'range': self.unit.text.range(self.start, self.end),
                'selectionRange': self.location()['range']}

    def description(self):
        node = self.node
        if isinstance(node, Document):
            return f'{node.kind} {node.name}'
        lines = [f'{type(node).__name__.removesuffix("Decl")}: {self.name}']
        for label in ('part', 'module', 'pin_type', 'kind', 'asset_alias', 'scope', 'path'):
            if hasattr(node, label):
                lines.append(f'{label}: {getattr(node, label)}')
        fields = getattr(node, 'attributes', getattr(node, 'parameters', {}))
        for key, value in fields.items():
            lines.append(f'{key}: {scalar(value)}')
        if hasattr(node, 'endpoints'):
            lines.append('endpoints: ' + ', '.join(node.endpoints))
        if hasattr(node, 'value'):
            lines.append(scalar(node.value))
        if isinstance(node, PhysicalMacroDecl):
            lines.extend(f'{m.kind} {m.key} = {m.target}' for m in node.mappings)
        # Preceding source comments are library documentation, not instructions.
        before = self.unit.text.source[:self.start].splitlines()
        notes = []
        for line in reversed(before):
            line = line.strip()
            if line.startswith('//'):
                notes.append(line[2:].strip())
            elif line.startswith('#'):
                notes.append(line[1:].strip())
            elif not line and not notes:
                continue
            else:
                break
        if notes:
            lines.extend(reversed(notes))
        return '\n'.join(lines)


def index_unit(unit: Unit):
    doc = unit.document
    if doc is None:
        return
    tokens = unit.parsed.tokens
    unit.symbols.append(Symbol(doc.name, unit, doc, tokens[0].location.offset,
                               len(unit.text.source), tokens[1].location.offset,
                               tokens[1].end_offset, 2))
    for node in (*doc.declarations, *(d for d in doc.imports if is_macro_import(d.path))):
        name = node.alias if isinstance(node, ImportDecl) else getattr(node, 'ref', getattr(node, 'name', getattr(node, 'kind', '')))
        if not name:
            continue
        i = next((i for i, t in enumerate(tokens) if t.location.offset == node.location.offset), None)
        if i is None:
            continue
        selection = tokens[i]
        if isinstance(node, (ConstraintDecl, ConfigurationDecl, ImportDecl)):
            selection = next((t for t in tokens[i:] if t.text == name), selection)
        start = i - 1 if i > 0 and tokens[i - 1].text in {*KEYWORDS, 'pin', 'pad', 'peripheral'} else i
        j, depth = i, 0
        while j < len(tokens) - 1:
            token = tokens[j]
            if token.kind is TokenKind.SYMBOL:
                if token.text == '{':
                    depth += 1
                elif token.text == '}':
                    depth -= 1
                    if depth <= 0:
                        break
                elif token.text == ';' and depth == 0:
                    break
            j += 1
        unit.symbols.append(Symbol(name, unit, node, tokens[start].location.offset,
                                   tokens[j].end_offset, selection.location.offset,
                                   selection.end_offset, KINDS.get(type(node).__name__, 7)))


class Analysis:
    def __init__(self, path: Path, reader, *, dependencies=None):
        self.reader = reader
        self.units = {}
        self.exports = {}
        self.import_errors = []
        self.macro_contracts = {}
        self.root = self.load(path, '')
        self.named_parts = {}
        doc = self.root.document
        if doc:
            for name in doc.libraries:
                factory = library_factory(name)
                if factory:
                    self.named_parts.update(factory())
        self.resolver = None
        if dependencies is not None:
            self.units.update({k: u for k, u in dependencies.units.items() if u is not dependencies.root})
            self.exports = dependencies.exports
            self.import_errors = dependencies.import_errors
            self.resolver = dependencies.resolver
            self.macro_contracts = dependencies.macro_contracts
            return
        if doc and doc.imports:
            try:
                self.resolver = SnapshotResolver.for_source(path, doc.location, locked=True, offline=True)
                self.imports(self.root, ())
            except (CopperScriptError, OSError, ValueError) as exc:
                self.import_errors.append(exc)

    def load(self, path, namespace):
        key = (path.resolve(), namespace)
        if key not in self.units:
            text = self.reader(key[0])
            unit = Unit(key[0], namespace, Text(text), parse_recovering(text, str(key[0])))
            index_unit(unit)
            self.units[key] = unit
        return self.units[key]

    def imports(self, unit, stack):
        if not unit.document:
            return
        for declaration in unit.document.imports:
            try:
                if is_macro_import(declaration.path):
                    from ..macro_bindings import MacroAssetReference, prepare_macro_asset
                    identity = self.resolver.qualify_macro_asset(declaration.path, declaration.location)
                    ref = MacroAssetReference(identity, str(self.resolver.source_directory), declaration.location, True, True)
                    try:
                        self.macro_contracts[(unit.path, declaration.alias)] = prepare_macro_asset(ref)
                    except (OSError, ValueError, KeyError, TypeError) as exc:
                        raise CopperScriptError('MAC005', f'macro contract unavailable: {exc}', declaration.location) from exc
                    continue
                resolved = self.resolver.resolve(declaration.path, declaration.location)
                if resolved.import_path in stack or len(stack) >= MAX_IMPORT_DEPTH:
                    raise CopperScriptError('CMP021', 'cyclic or excessively deep package import', declaration.location)
                namespace = '.'.join(filter(None, (unit.namespace, declaration.alias)))
                paths = sorted(resolved.directory.glob('*.copper'))
                for path in paths:
                    child = self.load(path, namespace)
                    if child.document:
                        key = namespace + '.' + child.document.name
                        self.exports.setdefault(key, []).append(child.symbols[0])
                        self.imports(child, (*stack, resolved.import_path))
            except (CopperScriptError, OSError, ValueError) as exc:
                self.import_errors.append(exc)

    def exported(self, unit, name):
        candidates = self.exports.get('.'.join(filter(None, (unit.namespace, name))), [])
        # Duplicate definitions never produce arbitrary navigation.
        unique = {(s.unit.path, s.selection_start): s for s in candidates}
        return next(iter(unique.values())) if len(unique) == 1 else None

    def members(self, symbol):
        node = symbol.node
        if isinstance(node, ComponentDecl):
            target = self.exported(symbol.unit, node.part)
        elif isinstance(node, ModuleInstanceDecl):
            target = self.exported(symbol.unit, node.module)
        else:
            target = symbol if isinstance(node, Document) else None
        if not target:
            return {}
        doc = target.unit.document
        types = {'part': {'PinDecl'}, 'module': {'PortDecl'}, 'device': {'PadDecl', 'PeripheralDecl'}}
        return {s.name: s for s in target.unit.symbols[1:] if type(s.node).__name__ in types.get(doc.kind, set())}

    def resolve(self, unit, name):
        if '/' in name:
            owner, tail = name.split('/', 1)
            symbol = self.resolve(unit, owner)
            target = self.exported(unit, symbol.node.module) if symbol and isinstance(symbol.node, ModuleInstanceDecl) else None
            return self.resolve(target.unit, tail) if target else None
        if name.startswith('port.') and unit.document and unit.document.kind == 'module':
            return next((s for s in unit.symbols if type(s.node).__name__ == 'PortDecl' and s.name == name[5:]), None)
        local = [s for s in unit.symbols if s.name == name]
        if local:
            # Supply and net names may intentionally coincide; prefer the net.
            local.sort(key=lambda s: type(s.node).__name__ != 'NetDecl')
            return local[0]
        exported = self.exported(unit, name)
        if exported:
            return exported
        if '.' in name:
            owner, member = name.rsplit('.', 1)
            target = self.resolve(unit, owner)
            if target:
                return self.members(target).get(member)
        return None

    def word(self, offset):
        tokens = self.root.parsed.tokens
        i = next((i for i, t in enumerate(tokens) if t.kind in {TokenKind.IDENTIFIER, TokenKind.NUMBER}
                  and t.location.offset <= offset < t.end_offset), None)
        if i is None:
            return None
        start = end = i
        while start >= 2 and tokens[start - 1].text in {'.', '/'} and tokens[start - 2].kind in {TokenKind.IDENTIFIER, TokenKind.NUMBER}:
            start -= 2
        # Resolve the prefix ending at the hovered token, rather than a later member.
        name = ''.join(t.text for t in tokens[start:end + 1])
        for candidate in self.root.symbols:
            if not isinstance(candidate.node, PhysicalMacroDecl) or not candidate.node.scope:
                continue
            if candidate.start <= offset <= candidate.end:
                preceding = [t.text for t in tokens if candidate.start <= t.location.offset < tokens[start].location.offset]
                last = max((j for j, text in enumerate(preceding) if text == ';'), default=-1)
                if '=' in preceding[last + 1:]:
                    name = candidate.node.scope + '/' + name
                break
        symbol = self.resolve(self.root, name)
        if symbol:
            return symbol, self.root.text.range(tokens[i].location.offset, tokens[i].end_offset)
        return None

    def hover(self, position):
        found = self.word(self.root.text.offset(position))
        return {'contents': {'kind': 'plaintext', 'value': found[0].description()},
                'range': found[1]} if found else None

    def definition(self, position):
        found = self.word(self.root.text.offset(position))
        if found and isinstance(found[0].node, ImportDecl):
            contract = self.macro_contracts.get((found[0].unit.path, found[0].node.alias))
            if contract:
                return {'uri': contract[0].as_uri(), 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 0}}}
        return found[0].location() if found else None

    def macro_scope_unit(self, declaration):
        unit = self.root
        if 'on' in declaration:
            scope = ''.join(declaration[declaration.index('on') + 1:])
            for instance in scope.split('/'):
                symbol = self.resolve(unit, instance)
                target = self.exported(unit, symbol.node.module) if symbol and isinstance(symbol.node, ModuleInstanceDecl) else None
                unit = target.unit if target else None
                if unit is None:
                    break
        return unit

    def completion(self, position):
        offset = self.root.text.offset(position)
        tokens = [t for t in self.root.parsed.tokens if t.kind is not TokenKind.EOF and t.location.offset < offset]
        # Do not offer identifiers in strings, comments or lexical-error gaps.
        containing = next((t for t in tokens if t.location.offset < offset < t.end_offset), None)
        if containing and containing.kind is TokenKind.STRING:
            return []
        if tokens and tokens[-1].end_offset < offset:
            gap = self.root.text.source[tokens[-1].end_offset:offset]
            if '#' in gap or '//' in gap:
                return []
        prefix = ''
        if tokens and tokens[-1].kind in {TokenKind.IDENTIFIER, TokenKind.NUMBER} and tokens[-1].end_offset >= offset:
            prefix = self.root.text.source[tokens[-1].location.offset:offset]
            tokens.pop()
        candidates = {}
        stack, header = [], []
        for token in tokens:
            if token.kind is TokenKind.SYMBOL and token.text == '{':
                stack.append(header)
                header = []
            elif token.kind is TokenKind.SYMBOL and token.text == '}':
                if stack:
                    stack.pop()
                header = []
            elif token.kind is TokenKind.SYMBOL and token.text == ';':
                header = []
            else:
                header.append(token.text)
        if tokens and tokens[-1].text == '.':
            i = len(tokens) - 2
            parts = []
            while i >= 0 and tokens[i].kind in {TokenKind.IDENTIFIER, TokenKind.NUMBER}:
                parts.insert(0, tokens[i].text)
                i -= 1
                if i >= 0 and tokens[i].text == '.':
                    i -= 1
                else:
                    break
            owner = '.'.join(parts)
            unit = self.macro_scope_unit(stack[-1]) if stack and stack[-1] and stack[-1][0] == 'physical_macro' else self.root
            if owner == 'port' and unit and unit.document and unit.document.kind == 'module':
                candidates.update({s.name: (5, s.description()) for s in unit.symbols if type(s.node).__name__ == 'PortDecl'})
            target = self.resolve(unit, owner) if unit else None
            if target:
                candidates.update({name: (5, s.description()) for name, s in self.members(target).items()})
                if isinstance(target.node, ComponentDecl) and target.node.part in self.named_parts:
                    candidates.update({name: (5, f'pin {name}; pad {pin.number}') for name, pin in self.named_parts[target.node.part].pins.items()})
            else:
                candidates.update({name[len(owner) + 1:]: (7, symbols[0].description())
                                   for name, symbols in self.exports.items()
                                   if name.startswith(owner + '.') and '.' not in name[len(owner) + 1:]})
        else:
            candidates.update({s.name: (6, s.description()) for s in self.root.symbols[1:]})
            candidates.update({name: (7, f'part {name}') for name in self.named_parts})
            candidates.update({name: (7, symbols[0].description()) for name, symbols in self.exports.items() if name.count('.') == 1})
            candidates.update({keyword: (14, 'CopperScript declaration') for keyword in KEYWORDS})
            if stack:
                kind = stack[-1][0] if stack[-1] else ''
                candidates.update({p: (10, f'{kind} property') for p in PROPERTIES.get(kind, ())})
                if kind == 'silkscreen':
                    candidates.update({p: (14, 'Silkscreen declaration') for p in
                        ('branding', 'text', 'logo', 'line', 'rectangle', 'circle', 'arc', 'polygon')})
                if kind == 'physical_macro':
                    declaration = stack[-1]
                    alias_index = declaration.index(':') + 1 if ':' in declaration else len(declaration)
                    alias = declaration[alias_index] if alias_index < len(declaration) else ''
                    contract = self.macro_contracts.get((self.root.path, alias))
                    candidates.update({p: (14, 'Physical macro mapping') for p in ('member', 'net')})
                    if contract and header and header[0] in {'member', 'net'} and '=' not in header:
                        import json
                        data = contract[2]
                        keys = ([row['reference'] for row in data['members']] if header[0] == 'member'
                                else sorted({row[2] for row in data['pad_nets']}))
                        candidates = {(key if key and (key[0].isalpha() or key[0] == '_') and all(c.isalnum() or c in '_-' for c in key) else json.dumps(key)):
                                      (5, f'Asset {header[0]} role') for key in keys}
                    elif header and '=' in header and 'on' in declaration:
                        unit = self.macro_scope_unit(declaration)
                        if unit:
                            wanted = {'ComponentDecl', 'ModuleInstanceDecl'} if header[0] == 'member' else {'NetDecl', 'ModuleInstanceDecl'}
                            candidates = {s.name: (6, s.description()) for s in unit.symbols if type(s.node).__name__ in wanted}
                            if header[0] == 'net':
                                candidates.update({f'port.{s.name}': (5, 'Existing module boundary net') for s in unit.symbols if type(s.node).__name__ == 'PortDecl'})
        return [{'label': name, 'kind': kind, 'detail': detail,
                 'textEdit': {'range': self.root.text.range(offset - len(prefix), offset), 'newText': name}}
                for name, (kind, detail) in sorted(candidates.items()) if name.startswith(prefix)]

    def error_diagnostic(self, error):
        unit = next((u for u in self.units.values() if str(u.path) == error.location.filename), self.root)
        offset = error.location.offset
        token = next((t for t in unit.parsed.tokens if t.location.offset == offset), None)
        end = token.end_offset if token else min(offset + 1, len(unit.text.source))
        diagnostic = {'range': unit.text.range(offset, end), 'severity': 1,
                      'code': error.code, 'source': 'CopperScript', 'message': error.message}
        if unit is not self.root:
            # Imported errors belong to this board's import, with the exact origin attached.
            origin = {'uri': unit.path.as_uri(), 'range': diagnostic['range']}
            declaration = next((d for d in self.root.document.imports if unit.namespace == d.alias or unit.namespace.startswith(d.alias + '.')), None)
            diagnostic['range'] = self.root.text.range(declaration.location.offset, declaration.location.offset + len(declaration.alias)) if declaration else self.root.text.range(0, 0)
            diagnostic['relatedInformation'] = [{'location': origin, 'message': error.message}]
        return diagnostic

    def subject_range(self, subject):
        """Map semantic subjects to real source tokens, without parsing messages."""
        symbol = self.resolve(self.root, subject or '')
        if symbol and symbol.unit is self.root:
            return symbol.location()['range']
        tokens = self.root.parsed.tokens
        for i, token in enumerate(tokens):
            if token.kind not in {TokenKind.IDENTIFIER, TokenKind.NUMBER}:
                continue
            name = token.text
            end = i
            while end + 2 < len(tokens) and tokens[end + 1].text in {'.', '/'} and tokens[end + 2].kind in {TokenKind.IDENTIFIER, TokenKind.NUMBER}:
                name += tokens[end + 1].text + tokens[end + 2].text
                end += 2
            if name == subject:
                return self.root.text.range(token.location.offset, tokens[end].end_offset)
        owner = (subject or '').split('/')[0].split('.')[0]
        symbol = self.resolve(self.root, owner)
        return symbol.location()['range'] if symbol and symbol.unit is self.root else self.root.text.range(0, 0)

    def diagnostics(self):
        errors = list(self.root.parsed.errors)
        for unit in self.units.values():
            if unit is not self.root:
                errors.extend(unit.parsed.errors)
        errors.extend(e for e in self.import_errors if isinstance(e, CopperScriptError))
        if errors:
            return [self.error_diagnostic(e) for e in errors]
        doc = self.root.document
        if not doc or doc.kind != 'board':
            return []  # Standalone library files receive syntax diagnostics; boards drive ERC.
        try:
            board = compile_source(self.root.text.source, str(self.root.path), locked=True,
                                   offline=True, source_reader=self.reader, package_resolver=self.resolver)
            from ..macro_bindings import prepare_macro_bindings, MacroPreparationError
            try:
                prepare_macro_bindings(board, locked=True, offline=True)
            except MacroPreparationError as exc:
                return [self.error_diagnostic(CopperScriptError('MAC005', exc.message, exc.location))]
            findings = check(board)
        except CopperScriptError as exc:
            return [self.error_diagnostic(exc)]
        result = []
        for finding in findings:
            diagnostic = {'range': self.subject_range(finding.subject),
                          'severity': 1 if finding.severity.value == 'error' else 2,
                          'code': finding.code, 'source': 'CopperScript', 'message': finding.message}
            result.append(diagnostic)
        return result


class Workspace:
    def __init__(self):
        self.documents = {}
        self.generation = 0
        self.lock = RLock()
        self.cache = {}
        self.dependencies = {}
        self.disk_generation = 0

    def update(self, uri, version, text=None, changes=None):
        path = path_from_uri(uri)
        if path.suffix.lower() != '.copper' or type(version) is not int:
            raise ValueError('a Copper document and integer version are required')
        with self.lock:
            previous = self.documents.get(path)
            if previous and version <= previous[0]:
                return False
            if changes is not None:
                if previous is None:
                    raise ValueError('document is not open')
                text = Text(previous[1]).change(changes)
            if not isinstance(text, str) or len(text.encode('utf-8')) > MAX_TEXT:
                raise ValueError('document exceeds text bounds')
            self.documents[path] = (version, text)
            self.generation += 1
            self.cache.clear()
            return True

    def invalidate(self):
        with self.lock:
            self.generation += 1
            self.cache.clear()
            self.disk_generation += 1
            self.dependencies.clear()

    def close(self, uri):
        with self.lock:
            self.documents.pop(path_from_uri(uri), None)
            self.invalidate()

    def snapshot(self):
        with self.lock:
            return self.generation, dict(self.documents)

    def analysis(self, uri, snapshot=None):
        path = path_from_uri(uri)
        generation, documents = snapshot or self.snapshot()
        with self.lock:
            cached = self.cache.get((generation, path))
            if cached:
                return cached
        texts = {p: item[1] for p, item in documents.items()}
        if path not in texts:
            with path.open(encoding='utf-8', newline='') as stream:
                texts[path] = stream.read(MAX_TEXT + 1)
            if len(texts[path].encode('utf-8')) > MAX_TEXT:
                raise ValueError('document exceeds text bounds')
        parsed = parse_recovering(texts[path], str(path))
        imports = tuple((d.alias, d.path) for d in parsed.document.imports) if parsed.document else ()
        with self.lock:
            signature = (self.disk_generation, imports, tuple(sorted((str(p), v[0]) for p, v in documents.items() if p != path)))
            cached_dependency = self.dependencies.get(path)
            base = (cached_dependency[1] if cached_dependency and cached_dependency[0] == signature
                    and not cached_dependency[1].import_errors else None)
        if base:
            for unit in base.units.values():
                if unit is not base.root:
                    texts.setdefault(unit.path, unit.text.source)

        def read(source_path):
            source_path = source_path.resolve()
            if source_path not in texts:
                with source_path.open(encoding='utf-8', newline='') as stream:
                    texts[source_path] = stream.read(MAX_TEXT + 1)
                if len(texts[source_path].encode('utf-8')) > MAX_TEXT:
                    raise ValueError('imported source exceeds text bounds')
            return texts[source_path]

        analysis = Analysis(path, read, dependencies=base)
        with self.lock:
            if generation == self.generation:
                self.cache[(generation, path)] = analysis
                self.dependencies[path] = (signature, analysis)
        return analysis
