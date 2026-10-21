"""Source-owned physical feature inventory and bounded token patches."""
from __future__ import annotations

import copy
from functools import lru_cache

from ..lexer import tokenize
from ..parser import parse
from ..syntax import ConstraintDecl, MechanicalDecl, SilkscreenDecl, SilkscreenItemDecl
from ..mechanical_stackup import StackupDecl
from .source import (insert_member, SourceEditError, SourceSnapshot, SourcePatch, SourceSpan, TextEdit,
                     declaration_index, declaration_spans, _erase_tokens)

CONSTRAINTS = frozenset({'max_distance', 'min_distance', 'placement_region',
    'allowed_orientations', 'align', 'placement_group', 'keepout', 'routing',
    'copper_zone', 'via_in_pad', 'length_match', 'hole_clearance'})
ARTWORK = frozenset({'branding', 'text', 'line', 'rectangle', 'circle', 'arc', 'polygon', 'logo'})


def _inventory(snapshot):
    document = parse(snapshot.text, snapshot.filename)
    tokens = tokenize(snapshot.text, snapshot.filename)
    root = declaration_index(snapshot)
    entries = []
    for index, declaration in enumerate(document.declarations):
        if isinstance(declaration, ConstraintDecl) and declaration.kind in CONSTRAINTS:
            entries.append(('constraint', declaration.kind, f'constraint:{index}', declaration, root))
        elif isinstance(declaration, (MechanicalDecl, SilkscreenDecl)):
            category = 'artwork' if isinstance(declaration, SilkscreenDecl) else 'stackup'
            container = next(n for n in root.children if n.header == ('silkscreen' if category == 'artwork' else 'mechanical',))
            for item in declaration.items:
                if isinstance(item, SilkscreenItemDecl):
                    entries.append((category, item.kind, item.name, item, container))
                elif isinstance(item, StackupDecl):
                    entries.append((category, 'stackup', 'stackup', item, container))
    return document, tokens, root, entries


def source_features(snapshot):
    # The scene asks for this on every move; the source changes far less often.
    return copy.deepcopy(_source_features(snapshot.raw, snapshot.filename))


@lru_cache(maxsize=8)
def _source_features(raw, filename):
    snapshot = SourceSnapshot(raw, filename)
    _, tokens, _, entries = _inventory(snapshot)
    result = []
    for category, kind, name, item, container in entries:
        if category == 'stackup':
            node = next(n for n in container.children if n.header == ('stackup',))
            parameters = {'declaration': snapshot.text[node.span.start:node.span.end]}
        else:
            spans = declaration_spans(tokens, item)
            parameters = {k: snapshot.text[v[0].start:v[-1].end] for k, v in spans.properties}
        result.append(dict(category=category, kind=kind, name=name, parameters=parameters,
                           targets=list(item.targets) if category == 'constraint' else [],
                           editable=True, line=item.location.line, owner=snapshot.filename))
    return result


def feature_patch(snapshot, *, category, kind, name='', parameters=None, targets=None, remove=False):
    """Return a patch plus the precise root constraint identity exemption.

    Existing targets cannot be retargeted. Scratch parsing rejects declaration
    injection; the compiler then validates the actual candidate before review.
    """
    parameters = {} if parameters is None else parameters
    targets = [] if targets is None else targets
    if category == 'artwork' and kind == 'branding' and name == '':
        name = 'branding'
    if (not isinstance(parameters, dict) or len(parameters) > 96
        or not all(isinstance(k, str) and isinstance(v, str) and len(v) <= 131072 for k, v in parameters.items())
        or not isinstance(targets, list) or not all(isinstance(t, str) for t in targets)
        or type(remove) is not bool or not isinstance(name, str)):
        raise SourceEditError('feature requires bounded literal parameters and target names')
    document, tokens, root, entries = _inventory(snapshot)
    if document.kind != 'board':
        raise SourceEditError('board feature edits require a board source')
    matches = [e for e in entries if e[:3] == (category, kind, name)]
    if len(matches) > 1:
        raise SourceEditError('ambiguous feature ownership')
    item = matches[0][3] if matches else None
    if category == 'constraint' and kind in CONSTRAINTS:
        if item is not None and tuple(targets) != item.targets:
            raise SourceEditError('existing constraint targets must remain unchanged; remove and create explicitly')
        literal = f'constraint {kind}({", ".join(targets)}) {{ ' + ' '.join(f'{k} = {v};' for k,v in parameters.items()) + ' }'
        wrapper = literal
        scope = (kind, tuple(targets))
    elif category == 'artwork' and kind in ARTWORK:
        literal = f'{kind} {"" if kind == "branding" else name} {{ ' + ' '.join(f'{k} = {v};' for k,v in parameters.items()) + ' }'
        wrapper = 'silkscreen { ' + literal + ' }'
        scope = None
    elif category == 'stackup' and kind == name == 'stackup':
        if set(parameters) != {'declaration'} and not remove:
            raise SourceEditError('stackup requires its complete declaration')
        literal = parameters.get('declaration', 'stackup {}')
        wrapper = 'mechanical { ' + literal + ' }'
        scope = None
    else:
        raise SourceEditError('unsupported physical feature')
    scratch = parse('board Candidate { ' + wrapper + ' }')
    if len(scratch.declarations) != 1:
        raise SourceEditError('feature must contain exactly one declaration')
    candidate = scratch.declarations[0]
    if category != 'constraint':
        if not isinstance(candidate, (MechanicalDecl, SilkscreenDecl)) or len(candidate.items) != 1:
            raise SourceEditError('feature must contain exactly one item')
        candidate = candidate.items[0]
    if category == 'stackup':
        valid = isinstance(candidate, StackupDecl)
    else:
        valid = (isinstance(candidate, ConstraintDecl if category == 'constraint' else SilkscreenItemDecl)
                 and candidate.kind == kind and set(candidate.parameters) == set(parameters)
                 and (candidate.targets == tuple(targets) if category == 'constraint' else candidate.name == name))
    if not valid:
        raise SourceEditError('feature literal changes its declared identity')
    edits = []
    if item is not None:
        if category == 'stackup':
            span = next(n.span for n in matches[0][4].children if n.header == ('stackup',))
            edits = _erase_tokens(tokens, span)
            if not remove:
                edits[0] = TextEdit(edits[0].span, literal)
        else:
            spans = declaration_spans(tokens, item)
            if remove:
                edits = _erase_tokens(tokens, spans.declaration)
            else:
                remaining = dict(parameters)
                for key, values in spans.properties:
                    if key in remaining:
                        value = remaining.pop(key)
                        edits.extend(TextEdit(s, value if i == 0 else '') for i,s in enumerate(values))
                    else:
                        edits.extend(TextEdit(s, '') for k, assignment in spans.assignments if k == key for s in assignment)
                if remaining:
                    edits.append(TextEdit(SourceSpan(spans.closing_brace, spans.closing_brace),
                        ' ' + ' '.join(f'{k} = {v};' for k,v in remaining.items()) + ' '))
    elif remove or (category == 'constraint' and name):
        raise SourceEditError('feature is not owned by this source')
    else:
        parent = root
        if category != 'constraint':
            block = 'silkscreen' if category == 'artwork' else 'mechanical'
            parents = [n for n in root.children if n.header == (block,)]
            if len(parents) > 1:
                raise SourceEditError('ambiguous feature container')
            if parents:
                parent = parents[0]
            else:
                literal = wrapper
        edits.append(insert_member(snapshot, parent.span.end - 1, literal))
    patch = SourcePatch(snapshot.revision, tuple(edits))
    parse(SourceSnapshot(patch.apply(snapshot)).text, snapshot.filename)
    return patch, scope


def feature_catalog():
    defaults = {
        'max_distance': {'distance': '3 mm'}, 'min_distance': {'distance': '3 mm'},
        'placement_region': {'name': '"region"', 'x': '0 mm', 'y': '0 mm', 'width': '10 mm', 'height': '10 mm'},
        'allowed_orientations': {'values': '"0,90,180,270"'}, 'align': {'axis': 'y', 'tolerance': '0.5 mm'},
        'placement_group': {'name': '"group"'}, 'keepout': {'name': '"keepout"', 'x': '0 mm', 'y': '0 mm', 'width': '5 mm', 'height': '5 mm'},
        'routing': {'width': '0.2 mm', 'clearance': '0.2 mm', 'allowed_layers': '"F.Cu,B.Cu"'},
        'copper_zone': {'layers': '"F.Cu"'}, 'via_in_pad': {'process': '"filled-capped"'},
        'length_match': {'max_skew': '1 mm'}, 'hole_clearance': {'clearance': '0.2 mm', 'reason': '"Replace with source-backed reason"'},
    }
    result = [dict(category='constraint', kind=k, name='', targets=[], parameters=v) for k,v in defaults.items()]
    art = {'branding': {'enabled': 'true'}, 'text': {'text': '"Label"', 'position': '(10 mm, 10 mm)', 'size': '1 mm'},
           'line': {'start': '(5 mm, 5 mm)', 'end': '(10 mm, 5 mm)'},
           'rectangle': {'start': '(5 mm, 5 mm)', 'end': '(10 mm, 10 mm)'},
           'circle': {'center': '(10 mm, 10 mm)', 'radius': '1 mm'},
           'arc': {'start': '(5 mm, 5 mm)', 'mid': '(6 mm, 6 mm)', 'end': '(7 mm, 5 mm)'},
           'polygon': {'vertices': '[(5 mm, 5 mm), (10 mm, 5 mm), (5 mm, 10 mm)]'},
           'logo': {'position': '(10 mm, 10 mm)', 'size': '3 mm'}}
    result += [dict(category='artwork', kind=k, name='branding' if k == 'branding' else 'NEW_ART', parameters=v) for k,v in art.items()]
    result.append(dict(category='stackup', kind='stackup', name='stackup', parameters={'declaration':
        'stackup { copper F.Cu { thickness = 35 um; } dielectric core { thickness = 1.53 mm; er = 4.2; } copper B.Cu { thickness = 35 um; } }'}))
    return result


def feature_batch(snapshot, features):
    """Prepare an atomic batch; source identities refer to the original revision.

    Sequential syntax-only patches allow new sibling features to share a newly
    created container. Constraint ordinals are adjusted only for earlier root
    declaration removals. Complete design validation happens after the batch.
    """
    if not isinstance(features, list) or not 1 <= len(features) <= 256:
        raise SourceEditError('feature batch requires 1–256 edits')
    current, seen, removed, scopes = snapshot, set(), [], []
    owned = {(f['category'], f['kind'], f['name']) for f in source_features(snapshot)}
    for feature in features:
        required = {'category', 'kind', 'name', 'parameters'}
        if (not isinstance(feature, dict) or not required <= set(feature)
                or set(feature) - required - {'remove', 'targets'}):
            raise SourceEditError('unexpected or missing feature batch fields')
        entry = dict(feature)
        # Type checking before constructing an identity avoids unhashable input.
        if not all(isinstance(entry[k], str) for k in ('category','kind','name')):
            raise SourceEditError('feature identity requires strings')
        targets = entry.get('targets', [])
        if not isinstance(targets, list) or not all(isinstance(t,str) for t in targets):
            raise SourceEditError('feature targets require a list of names')
        identity = (entry['category'], entry['kind'], entry['name'],
                    tuple(targets) if entry['category'] == 'constraint' and not entry['name'] else ())
        if identity in seen:
            raise SourceEditError('batch cannot edit a feature twice')
        seen.add(identity)
        original_index = None
        if entry['category'] == 'constraint' and entry['name']:
            if (entry['category'], entry['kind'], entry['name']) not in owned:
                raise SourceEditError('constraint identity is not owned by the original revision')
            try:
                prefix, ordinal = entry['name'].split(':')
                original_index = int(ordinal)
                if prefix != 'constraint' or original_index < 0:
                    raise ValueError()
            except ValueError:
                raise SourceEditError('invalid source constraint identity') from None
            entry['name'] = f'constraint:{original_index - sum(i < original_index for i in removed)}'
        patch, scope = feature_patch(current, **entry)
        current = SourceSnapshot(patch.apply(current), snapshot.filename)
        if scope is not None and scope not in scopes:
            scopes.append(scope)
        if original_index is not None and entry.get('remove'):
            removed.append(original_index)
    return current.raw, (None if not scopes else scopes[0] if len(scopes) == 1 else tuple(scopes))


def physical_feature_geometry(board):
    """Expose the actual physical IR, with no fill or qualification inference."""
    from ..breakout import BreakoutRegions
    from ..placement import reserved_corridors
    point = lambda p: [p.x_nm, p.y_nm]
    geometry = [dict(category='constraint', kind='copper_zone', name=z.id, net=z.net,
        targets=[z.net], layers=[layer.value for layer in z.layers],
        vertices=[point(p) for p in z.outline.outer.vertices],
        holes=[[point(p) for p in hole.vertices] for hole in z.outline.holes],
        priority=z.priority, clearance_nm=z.clearance_nm, reserve_routing=z.reserve_routing,
        pad_connection=z.pad_connection.value, verified_fill=False) for z in board.zones]
    from math import hypot
    from ..placement import _target_point
    from ..physical import RelativePlacementKind, AlignmentAxis
    poses = {p.reference: p for p in board.placements}
    for index, rule in enumerate(board.relative_rules):
        anchors = [_target_point(board, poses, target) for target in rule.targets]
        if any(p is None for p in anchors):
            continue
        common = dict(category='constraint', kind=rule.kind.value,
            name=f'relative:{index}', rule_index=index,
            references=[t.reference for t in rule.targets],
            resolved_targets=[dict(reference=t.reference, pad=t.pad) for t in rule.targets])
        if rule.kind is RelativePlacementKind.ALIGN:
            values = [p.x_nm if rule.axis is AlignmentAxis.X else p.y_nm for p in anchors]
            measured = max(values) - min(values)
            geometry.append(dict(**common, points=[point(p) for p in anchors], axis=rule.axis.value,
                measured_nm=measured, limit_nm=rule.tolerance_nm, satisfied=measured <= rule.tolerance_nm))
        else:
            for pair, (left, right) in enumerate(zip(anchors, anchors[1:])):
                measured = round(hypot(left.x_nm-right.x_nm, left.y_nm-right.y_nm))
                geometry.append(dict(**common, points=[point(left),point(right)], pair_index=pair,
                    measured_nm=measured, limit_nm=rule.distance_nm,
                    satisfied=(measured <= rule.distance_nm if rule.kind is RelativePlacementKind.MAX_DISTANCE
                               else measured >= rule.distance_nm)))
    reservations = reserved_corridors(board)
    for corridor in reservations.corridors:
        geometry.append(dict(category='constraint', kind='routing_corridor', name=corridor.name,
            nets=list(corridor.nets), targets=list(corridor.nets),
            vertices=[point(p) for p in corridor.keepout.outline.vertices],
            side=corridor.side.value if corridor.side else 'both',
            references=list(corridor.terminal_references), margin_nm=corridor.margin_nm))
    regions = BreakoutRegions(board)
    if regions:
        for placement in board.placements:
            for index, pad in enumerate(board.footprints[placement.footprint].pads):
                identity = f'pad:{placement.reference}.{pad.number}:{index}'
                try:
                    land = regions.land(identity)
                except KeyError:
                    continue
                # Exact rounded convex region: spine swept by this radius.
                geometry.append(dict(category='constraint', kind='breakout', name=identity,
                    net=land.net, targets=[land.net], references=[placement.reference],
                    points=[point(p) for p in land.shape.spine],
                    radius_nm=land.shape.radius_nm + land.breakout_length_nm,
                    layers=[layer.value for layer in board.stackup.copper_layers]))
    layers = [dict(kind=layer.kind.value, name=layer.id, thickness_nm=layer.thickness_nm,
        material=layer.material, er=str(layer.relative_permittivity) if layer.relative_permittivity is not None else None,
        loss_tangent=str(layer.loss_tangent) if layer.loss_tangent is not None else None,
        dielectric_type=layer.dielectric_type,
        copper_layer=layer.copper_layer.value if layer.copper_layer else None)
        for layer in board.stackup.physical_layers]
    return dict(physical_geometry=geometry, stackup_layers=layers,
        stackup_thickness_nm=board.stackup.thickness_nm,
        routing_overlay_diagnostics=[dict(nets=list(item.nets), reason=item.reason) for item in reservations.skipped])


def dimension_source_names(board, electrical, features):
    """Map authored targets through the compiler's pin resolver, never spelling guesses."""
    from ..elaborate import elaborate
    from ..physicalize import _physical_pin_number
    flat = elaborate(electrical)
    components = {c.ref: c for c in flat.components}
    identities = {}
    for feature in features:
        if feature['category'] != 'constraint' or feature['kind'] not in {'max_distance','min_distance','align'}:
            continue
        resolved = []
        for target in feature['targets']:
            reference, separator, pin = target.partition('.')
            component = components.get(reference)
            if component is None:
                break
            pad = _physical_pin_number(component, flat.library[component.part], flat.devices, pin) if separator else None
            if separator and pad is None:
                break
            resolved.append((reference,pad))
        else:
            key = (feature['kind'], tuple(resolved))
            identities.setdefault(key, []).append(feature)
    result = {}
    from collections import Counter
    rule_keys = [(r.kind.value, tuple((t.reference,t.pad) for t in r.targets)) for r in board.relative_rules]
    counts = Counter(rule_keys)
    for index, rule in enumerate(board.relative_rules):
        matches = identities.get(rule_keys[index], [])
        # Ambiguous parallel declarations remain inspectable in the feature list.
        if len(matches) == 1 and counts[rule_keys[index]] == 1:
            result[index] = matches[0]
    return result
