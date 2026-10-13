"""Declarative Copper macro authoring lowered to the existing data contract.

No electrical elaboration, generator execution or consumer geometry inference.
"""
from decimal import Decimal
from hashlib import sha256
import json
from pathlib import Path

from .clusters import footprint_geometry_digest
from .footprints import FootprintResolver
from .hard_macros import validate_macro_asset_data
from .packages import PackageResolver
from .parser import parse
from .physical import CopperLayer
from .quantities import Length, decimal_text
from .syntax import (CopperScriptError, MacroItemDecl, MacroPadDecl,
                     MacroPadReference, MacroPropertyDecl, RawQuantity)


ROOT_PROPERTIES = ('source', 'anchor', 'allowed_rotations', 'required_layers',
                   'internal_clearance', 'unresolved')
ITEM_PROPERTIES = {
    'member': ('footprint', 'at', 'rotation', 'edge_clearance'),
    'track': ('net', 'layer', 'width', 'points'),
    'via': ('net', 'at', 'size', 'drill', 'from_layer', 'to_layer', 'technology', 'finish'),
    'port': ('net', 'point', 'layer', 'pads'),
    'protected_region': ('layers', 'vertices', 'block_tracks', 'block_vias', 'block_zones'),
    'keepout': ('layers', 'vertices', 'block_tracks', 'block_vias', 'block_zones'),
    'zone': ('net', 'layers', 'vertices', 'priority', 'clearance', 'minimum_width', 'pad_connection'),
    'polygon': ('net', 'layer', 'vertices'),
    'plane_return': ('net', 'layers', 'pads'),
    'contact': ('pad', 'at'),
    'width_contract': ('track', 'minimum_width', 'purpose', 'evidence'),
    'edge_opening': ('keepout', 'edge_indices'),
}


def canonical_macro_bytes(data):
    return (json.dumps(data, sort_keys=True, separators=(',', ':'), ensure_ascii=False) + '\n').encode('utf-8')


def lower_macro(document, resolver, *, resolve_footprints=True):
    """Return canonical asset data and every file fingerprint used to author it.

    Editor structural checks can omit footprint loading; physical preparation
    always resolves exact independent footprint assets.
    """
    def fail(message, node=document):
        raise CopperScriptError('MAC010', message, node.location)

    def text(value, node):
        if not isinstance(value, str) or not value:
            fail('expected a nonempty name or string', node)
        return value

    def sequence(value, node):
        if not isinstance(value, tuple):
            fail('expected a list or point', node)
        return value

    def integer(value, node):
        if type(value) is not int:
            fail('expected an integer', node)
        return value

    def length(value, node, *, positive=False):
        if not isinstance(value, RawQuantity) or value.unit not in Length.UNITS:
            fail('expected a length quantity (mm, um or m)', node)
        nm = Length.of(value.value, value.unit).in_unit('m') * Decimal(1_000_000_000)
        if nm != nm.to_integral_value():
            fail('length must be an exact integer number of nanometres', node)
        if positive and nm <= 0:
            fail('length must be positive', node)
        return int(nm)

    def angle(value, node):
        if not isinstance(value, RawQuantity) or value.unit != 'deg':
            fail('expected an angle quantity in deg', node)
        return Decimal(value.value)

    def point(value, node):
        values = sequence(value, node)
        if len(values) != 2:
            fail('a point requires exactly two lengths', node)
        return [length(v, node) for v in values]

    def layer(value, node):
        name = text(value, node)
        try:
            CopperLayer(name)
        except ValueError:
            fail(f'unsupported copper layer {name!r}', node)
        return name

    def layers(value, node):
        result = [layer(v, node) for v in sequence(value, node)]
        if not result or len(set(result)) != len(result):
            fail('layers must be nonempty and unique', node)
        return result

    def evidence(value, node):
        if isinstance(value, dict):
            return {k: evidence(v, node) for k, v in value.items()}
        if isinstance(value, tuple):
            return [evidence(v, node) for v in value]
        if isinstance(value, (str, bool, int, float)):
            return value
        fail('source evidence accepts strings, numbers, booleans, lists and records', node)

    if document.kind != 'macro':
        fail('macro source must declare a macro')
    properties, groups, ids, nets, pad_rows = {}, {}, {}, set(), []
    for node in document.declarations:
        if isinstance(node, MacroPropertyDecl):
            if node.name not in ROOT_PROPERTIES or node.name in properties:
                fail(f'unknown or duplicate macro property {node.name!r}', node)
            properties[node.name] = node
        elif isinstance(node, MacroPadDecl):
            pad_rows.append(node)
        elif isinstance(node, MacroItemDecl):
            if node.kind == 'net':
                if node.name in nets:
                    fail(f'duplicate net role {node.name!r}', node)
                nets.add(node.name)
                continue
            if node.kind not in ITEM_PROPERTIES or node.kind == 'contact':
                fail(f'unsupported macro declaration {node.kind!r}', node)
            if node.name in ids.setdefault(node.kind, set()):
                fail(f'duplicate {node.kind} name {node.name!r}', node)
            ids[node.kind].add(node.name)
            groups.setdefault(node.kind, []).append(node)
        else:
            fail('unsupported macro declaration', node)

    def root(name, default=None, required=False):
        node = properties.get(name)
        if node is None:
            if required:
                fail(f'missing macro property {name!r}')
            return default
        return node.value

    def attrs(node, defaults=None):
        defaults = defaults or {}
        expected = set(ITEM_PROPERTIES[node.kind])
        unknown = set(node.parameters) - expected
        missing = expected - set(node.parameters) - set(defaults)
        if unknown or missing:
            fail(f'{node.kind} {node.name!r}: unknown properties {sorted(unknown)}, missing {sorted(missing)}', node)
        if node.children and node.kind != 'plane_return':
            fail('contacts are allowed only inside plane_return', node)
        return {**defaults, **node.parameters}

    def net(value, node):
        name = text(value, node)
        if name not in nets:
            fail(f'undeclared net role {name!r}', node)
        return name

    members = ids.get('member', set())
    if not members:
        fail('a macro requires members')
    anchor = text(root('anchor', required=True), document)
    if anchor not in members:
        fail(f'unknown anchor member {anchor!r}')
    rotations = [angle(v, document) for v in sequence(root('allowed_rotations', (RawQuantity('0', 'deg'),)), document)]
    if not rotations or len(set(rotations)) != len(rotations) or any(v < 0 or v >= 360 for v in rotations):
        fail('allowed rotations must be unique angles in [0 deg, 360 deg)')
    data = dict(schema='copperlib-physical-hard-macro/v0.5',
                production_publishable=False, source=evidence(root('source', required=True), document),
                anchor=anchor, members=[], pad_nets=[], isolated_pads=[], tracks=[], vias=[], ports=[],
                protected_regions=[], keepouts=[], zones=[], polygons=[], plane_returns=[],
                width_contracts=[], edge_openings=[],
                required_layers=layers(root('required_layers', required=True), document),
                allowed_rotations=[int(v) if v == int(v) else decimal_text(v) for v in rotations],
                internal_clearance_nm=length(root('internal_clearance', RawQuantity('0', 'mm')), document),
                unresolved=[text(v, document) for v in sequence(root('unresolved', ()), document)])
    if not data['source'] or not isinstance(data['source'], (str, dict)):
        fail('source must be nonempty evidence text or a record')
    if data['internal_clearance_nm'] < 0:
        fail('internal clearance must be nonnegative')
    aliases = {}
    for imported in document.imports:
        if imported.alias in aliases or not imported.path.endswith('.kicad_mod'):
            fail('macro imports require unique aliases for exact .kicad_mod footprints', imported)
        aliases[imported.alias] = resolver.qualify_footprint(imported.path, imported.location)
    footprints = FootprintResolver(Path(resolver.source_directory), locked=resolver.locked, offline=resolver.offline)
    inputs, member_footprints = {}, {}
    for node in groups.get('member', ()):
        a = attrs(node, {'rotation': RawQuantity('0', 'deg'), 'edge_clearance': RawQuantity('0.25', 'mm')})
        ref = text(a['footprint'], node)
        ref = aliases.get(ref, resolver.qualify_footprint(ref, node.location))
        digest = 'pending'
        if resolve_footprints:
            try:
                fp = footprints.resolve(ref).footprint
            except ValueError as exc:
                fail(str(exc), node)
            digest = footprint_geometry_digest(fp)
            member_footprints[node.name] = fp
            path = Path(fp.metadata['source_path']).resolve()
            raw_digest = sha256(path.read_bytes()).hexdigest()
            if raw_digest != fp.metadata['source_sha256']:
                fail('footprint changed during macro preparation', node)
            inputs[str(path)] = raw_digest
        data['members'].append(dict(reference=node.name, footprint=ref, footprint_digest=digest,
            center_nm=point(a['at'], node), rotation_degrees=decimal_text(angle(a['rotation'], node)),
            edge_clearance_nm=length(a['edge_clearance'], node, positive=True)))

    assigned, isolated = {}, set()
    def pad(value, node):
        if not isinstance(value, MacroPadReference) or value.member not in members or not value.pad:
            fail('expected an existing member.pad reference', node)
        if resolve_footprints and not any(p.number == value.pad for p in member_footprints[value.member].pads):
            fail(f'footprint for {value.member!r} has no pad {value.pad!r}', node)
        return (value.member, value.pad)
    for node in pad_rows:
        ref = pad(node.reference, node)
        if ref in assigned or ref in isolated:
            fail('duplicate or conflicting pad assignment', node)
        if node.net is None:
            isolated.add(ref)
            data['isolated_pads'].append(list(ref))
        else:
            role = net(node.net, node)
            assigned[ref] = role
            data['pad_nets'].append([*ref, role])
    if set(assigned.values()) != nets:
        fail('each declared net role requires an explicit pad assignment')

    def connected_pad(value, node, role):
        ref = pad(value, node)
        if assigned.get(ref) != role:
            fail(f'pad {ref} does not belong to net role {role!r}', node)
        return list(ref)
    def endpoint(value, node, role):
        if isinstance(value, MacroPadReference):
            ref = connected_pad(value, node, role)
            if resolve_footprints and sum(p.number == value.pad for p in member_footprints[value.member].pads) != 1:
                fail('pad endpoints require a unique physical land', node)
            return {'pad': ref}
        return point(value, node)
    def vertices(value, node):
        from .mechanical import validated_ring
        from .physical import Point
        result = [point(v, node) for v in sequence(value, node)]
        try:
            validated_ring(tuple(Point(*v) for v in result))
        except ValueError as exc:
            fail(str(exc), node)
        return result
    used_layers = set()
    for node in groups.get('track', ()):
        a = attrs(node)
        role = net(a['net'], node)
        points = [endpoint(v, node, role) for v in sequence(a['points'], node)]
        if len(points) < 2:
            fail('a track requires at least two points', node)
        copper_layer = layer(a['layer'], node)
        used_layers.add(copper_layer)
        data['tracks'].append(dict(net=role, layer=copper_layer, width_nm=length(a['width'], node, positive=True), points=points))
    for node in groups.get('via', ()):
        a = attrs(node, {'technology': 'through', 'finish': 'standard'})
        start, end = layer(a['from_layer'], node), layer(a['to_layer'], node)
        size, drill = length(a['size'], node, positive=True), length(a['drill'], node, positive=True)
        finish = text(a['finish'], node)
        if drill >= size or start == end or finish not in {'standard', 'filled-capped'}:
            fail('unsupported via span/process or invalid drill/size', node)
        used_layers.update((start, end))
        technology = None if a['technology'] == 'none' else text(a['technology'], node)
        data['vias'].append(dict(net=net(a['net'], node), position_nm=point(a['at'], node), size_nm=size,
            drill_nm=drill, from_layer=start, to_layer=end, technology=technology, finish=finish))
    for node in groups.get('port', ()):
        a = attrs(node)
        role = net(a['net'], node)
        port_pads = [connected_pad(v, node, role) for v in sequence(a['pads'], node)]
        if not port_pads or len({tuple(v) for v in port_pads}) != len(port_pads):
            fail('port pads must be nonempty and unique', node)
        copper_layer = layer(a['layer'], node)
        used_layers.add(copper_layer)
        data['ports'].append(dict(name=node.name, net=role, point=endpoint(a['point'], node, role), layer=copper_layer, pads=port_pads))
    for kind, key in [('protected_region', 'protected_regions'), ('keepout', 'keepouts')]:
        for node in groups.get(kind, ()):
            defaults = {'block_tracks': kind == 'protected_region', 'block_vias': kind == 'protected_region', 'block_zones': True}
            a = attrs(node, defaults)
            if any(type(a[k]) is not bool for k in defaults):
                fail('region blocking flags require booleans', node)
            ls = layers(a['layers'], node)
            used_layers.update(ls)
            data[key].append(dict(id=node.name, layers=ls, vertices=vertices(a['vertices'], node), **{k: a[k] for k in defaults}))
    for node in groups.get('polygon', ()):
        a = attrs(node)
        copper_layer = layer(a['layer'], node)
        used_layers.add(copper_layer)
        data['polygons'].append(dict(id=node.name, net=net(a['net'], node), layer=copper_layer, vertices=vertices(a['vertices'], node)))
    for node in groups.get('zone', ()):
        a = attrs(node)
        ls = layers(a['layers'], node)
        used_layers.update(ls)
        clearance = length(a['clearance'], node)
        connection = text(a['pad_connection'], node)
        if clearance < 0 or connection not in {'solid', 'thermal'} or integer(a['priority'], node) < 0:
            fail('unsupported zone connection, clearance or priority', node)
        data['zones'].append(dict(id=node.name, net=net(a['net'], node), layers=ls, vertices=vertices(a['vertices'], node),
            priority=a['priority'], clearance_nm=clearance, minimum_width_nm=length(a['minimum_width'], node, positive=True), pad_connection=connection))
    for node in groups.get('plane_return', ()):
        a = attrs(node)
        role, contacts, contact_names = net(a['net'], node), [], set()
        ls = layers(a['layers'], node)
        used_layers.update(ls)
        return_pads = [connected_pad(v, node, role) for v in sequence(a['pads'], node)]
        if not return_pads or len({tuple(v) for v in return_pads}) != len(return_pads):
            fail('plane return pads must be nonempty and unique', node)
        for child in node.children:
            if child.name in contact_names:
                fail('duplicate dedicated contact name', child)
            contact_names.add(child.name)
            c = attrs(child)
            contact_pad = connected_pad(c['pad'], child, role)
            if contact_pad not in return_pads:
                fail('dedicated contact pad must belong to its plane return', child)
            contacts.append(dict(pad=contact_pad, via_position_nm=point(c['at'], child)))
        data['plane_returns'].append(dict(net=role, layers=ls, pads=return_pads, dedicated_contacts=contacts))
    track_names = {n.name: i for i, n in enumerate(groups.get('track', ()))}
    contracted = set()
    for node in groups.get('width_contract', ()):
        a = attrs(node)
        index = track_names.get(text(a['track'], node))
        if index is None or index in contracted:
            fail('width contract requires an existing, uniquely contracted track', node)
        contracted.add(index)
        minimum = length(a['minimum_width'], node, positive=True)
        purpose = text(a['purpose'], node)
        if minimum > data['tracks'][index]['width_nm'] or purpose not in {'pin_entry', 'control_supply', 'output_sense', 'enable', 'main_power'}:
            fail('invalid width contract minimum or purpose', node)
        data['width_contracts'].append(dict(track_index=index, minimum_width_nm=minimum,
            purpose=purpose, evidence=text(a['evidence'], node)))
    keepouts = {r['id']: r for r in data['keepouts']}
    opened = set()
    for node in groups.get('edge_opening', ()):
        a = attrs(node)
        keepout = text(a['keepout'], node)
        region = keepouts.get(keepout)
        indices = [integer(v, node) for v in sequence(a['edge_indices'], node)]
        if region is None or not indices or len(set(indices)) != len(indices) or any(i < 0 or i >= len(region['vertices']) for i in indices):
            fail('edge opening requires an existing keepout and unique valid edge indices', node)
        if keepout in opened:
            fail('duplicate edge opening for keepout', node)
        opened.add(keepout)
        data['edge_openings'].append(dict(keepout=keepout, edge_indices=indices))
    if used_layers - set(data['required_layers']):
        fail(f'geometry uses undeclared required layers: {sorted(used_layers - set(data["required_layers"]))}')
    validate_macro_asset_data(data)
    return data, inputs


def compile_macro_file(path, *, locked=False, offline=False):
    unresolved = Path(path).absolute()
    if any(p.is_symlink() for p in (unresolved, *unresolved.parents)):
        raise ValueError('macro source inputs may not be symlinks')
    path = Path(path).resolve()
    raw = path.read_bytes()
    document = parse(raw.decode('utf-8'), str(path))
    resolver = PackageResolver.for_source(path, document.location, locked=locked, offline=offline)
    data, inputs = lower_macro(document, resolver)
    inputs[str(path)] = sha256(raw).hexdigest()
    verify_macro_inputs(inputs)
    return data, inputs


def verify_macro_inputs(inputs):
    for name, digest in inputs.items():
        path = Path(name)
        if any(p.is_symlink() for p in (path, *path.parents)):
            raise ValueError(f'macro inputs may not be symlinks: {name}')
        if sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f'macro input changed after preparation: {name}')


def macro_definition_source(data, name, *, footprint_references=None):
    """Translate a reviewed asset to declarative source without running generators.

    Footprint replacements must name exact reviewed assets. Hashes are generated
    by normal preparation, never copied into member declarations.
    """
    import re
    validate_macro_asset_data(data)
    if not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*', name):
        raise ValueError('macro export requires an identifier')
    def identifier(value):
        return value if re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*', value) and value not in {'true', 'false'} else json.dumps(value, ensure_ascii=False)
    def value(v, indent=8):
        if isinstance(v, dict):
            return '{\n' + '\n'.join(' ' * indent + f'{identifier(k)} = {value(x, indent + 4)};' for k, x in v.items()) + '\n' + ' ' * (indent - 4) + '}'
        if isinstance(v, (list, tuple)):
            return '[' + ', '.join(value(x) for x in v) + ']'
        return json.dumps(v, ensure_ascii=False)
    def length(nm):
        if type(nm) is not int:
            raise ValueError('macro lengths must be integer nanometres')
        return str(Length.of(Decimal(nm) / 1_000_000, 'mm'))
    def point(p):
        return '(' + ', '.join(length(x) for x in p) + ')'
    def pad(p):
        return identifier(p[0]) + '.' + json.dumps(p[1])
    def endpoint(p):
        return pad(p['pad']) if isinstance(p, dict) else point(p)
    def points(ps):
        return '[' + ', '.join(endpoint(p) for p in ps) + ']'
    def pads(ps):
        return '[' + ', '.join(pad(p) for p in ps) + ']'
    lines = [f'macro {name} {{', f'    source = {value(data["source"])};',
             f'    unresolved = {value(data["unresolved"])};',
             f'    anchor = {identifier(data["anchor"])};',
             '    allowed_rotations = [' + ', '.join(f'{v} deg' for v in data['allowed_rotations']) + '];',
             f'    required_layers = {value(data["required_layers"])};',
             f'    internal_clearance = {length(data["internal_clearance_nm"])};', '']
    def block(kind, label, fields, children=()):
        lines.append(f'    {kind} {identifier(label)} {{')
        lines.extend(f'        {key} = {val};' for key, val in fields.items())
        lines.extend(children)
        lines.extend(['    }', ''])
    for role in sorted({row[2] for row in data['pad_nets']}):
        lines.append(f'    net {identifier(role)};')
    lines.append('')
    for m in data['members']:
        ref = (footprint_references or {}).get(m['footprint'], m['footprint'])
        block('member', m['reference'], dict(footprint=value(ref), at=point(m['center_nm']),
              rotation=f'{m["rotation_degrees"]} deg', edge_clearance=length(m['edge_clearance_nm'])))
    for member, number, role in data['pad_nets']:
        lines.append(f'    pad {pad((member, number))} = {identifier(role)};')
    for p in data['isolated_pads']:
        lines.append(f'    isolated {pad(p)};')
    lines.append('')
    for i, t in enumerate(data['tracks']):
        block('track', f'track_{i}', dict(net=identifier(t['net']), layer=value(t['layer']),
              width=length(t['width_nm']), points=points(t['points'])))
    for i, v in enumerate(data['vias']):
        block('via', f'via_{i}', dict(net=identifier(v['net']), at=point(v['position_nm']),
              size=length(v['size_nm']), drill=length(v['drill_nm']), from_layer=value(v['from_layer']),
              to_layer=value(v['to_layer']), technology='none' if v['technology'] is None else value(v['technology']),
              finish=value(v.get('finish', 'standard'))))
    for p in data['ports']:
        block('port', p['name'], dict(net=identifier(p['net']), layer=value(p['layer']),
              point=endpoint(p['point']), pads=pads(p['pads'])))
    for field, kind in [('protected_regions', 'protected_region'), ('keepouts', 'keepout')]:
        for r in data[field]:
            block(kind, r['id'], dict(layers=value(r['layers']), vertices=points(r['vertices']),
                **{k: value(r[k]) for k in ('block_tracks', 'block_vias', 'block_zones')}))
    for z in data.get('zones', ()):
        block('zone', z['id'], dict(net=identifier(z['net']), layers=value(z['layers']), vertices=points(z['vertices']),
              priority=value(z['priority']), clearance=length(z['clearance_nm']),
              minimum_width=length(z['minimum_width_nm']), pad_connection=value(z['pad_connection'])))
    for p in data.get('polygons', ()):
        block('polygon', p['id'], dict(net=identifier(p['net']), layer=value(p['layer']), vertices=points(p['vertices'])))
    for i, r in enumerate(data.get('plane_returns', ())):
        children = []
        for j, c in enumerate(r['dedicated_contacts']):
            children.extend([f'        contact contact_{j} {{', f'            pad = {pad(c["pad"])};',
                             f'            at = {point(c["via_position_nm"])};', '        }'])
        block('plane_return', f'return_{i}', dict(net=identifier(r['net']), layers=value(r['layers']), pads=pads(r['pads'])), children)
    for i, c in enumerate(data.get('width_contracts', ())):
        block('width_contract', f'width_{i}', dict(track=f'track_{c["track_index"]}', minimum_width=length(c['minimum_width_nm']),
              purpose=value(c['purpose']), evidence=value(c['evidence'])))
    for i, e in enumerate(data.get('edge_openings', ())):
        block('edge_opening', f'opening_{i}', dict(keepout=value(e['keepout']), edge_indices=value(e['edge_indices'])))
    return '\n'.join([*lines, '}', ''])
