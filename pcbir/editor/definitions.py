"""Source-authoritative local authoring for macro and mechanical-profile documents.

Definition coordinates are local. A viewport is never a manufacturing outline;
consumer-dependent qualification remains explicitly pending.
"""
from dataclasses import dataclass, replace
from difflib import unified_diff
from hashlib import sha256
from pathlib import Path
import json

from ..parser import parse
from ..lexer import tokenize, TokenKind
from ..packages import PackageResolver, find_manifest
from ..syntax import MacroItemDecl, MacroPropertyDecl, MacroPadDecl, MechanicalItemDecl, MechanicalDecl
from .source import SourceSnapshot, SourcePatch, SourceSpan, TextEdit, SourceEditError
from .transactions import SourceWorkspace


def _node_tokens(snapshot, node):
    tokens = tokenize(snapshot.text, snapshot.filename)
    start = next(i for i, t in enumerate(tokens) if t.location.offset == node.location.offset)
    assignment = start + 1 < len(tokens) and tokens[start + 1].text == '='
    depth = 0
    for i in range(start, len(tokens)):
        t = tokens[i]
        if t.kind is TokenKind.SYMBOL:
            if t.text in ('{', '[', '('): depth += 1
            elif t.text in ('}', ']', ')'):
                depth -= 1
                if depth == 0 and t.text == '}' and not assignment: return tokens[start:i + 1]
            elif t.text == ';' and depth == 0: return tokens[start:i + 1]
    raise SourceEditError('incomplete definition declaration')


def _properties(snapshot, node):
    """Direct assignments only, preserving exact literal spelling and nested records."""
    tokens = _node_tokens(snapshot, node)
    start = next(i for i, t in enumerate(tokens) if t.text == '{' and t.kind is TokenKind.SYMBOL) + 1
    result = {}
    i = start
    while i < len(tokens) - 1:
        begin = i
        depth = 0
        assignment = i + 1 < len(tokens) and tokens[i + 1].text == '='
        while i < len(tokens) - 1:
            t = tokens[i]
            if t.kind is TokenKind.SYMBOL:
                if t.text in ('{', '[', '('): depth += 1
                elif t.text in ('}', ']', ')'):
                    depth -= 1
                    if depth == 0 and t.text == '}' and not assignment:
                        i += 1
                        break
                elif t.text == ';' and depth == 0:
                    if assignment:
                        result[tokens[begin].text] = tokens[begin:i + 1]
                    i += 1
                    break
            i += 1
    return result


def definition_features(snapshot):
    document = parse(snapshot.text, snapshot.filename)
    category = 'macro' if document.kind == 'macro' else 'profile'
    features = []
    root = {}
    for node in document.declarations:
        if isinstance(node, MacroPadDecl):
            features.append(dict(category=category,kind='pad' if node.net else 'isolated',name=node.reference.member+'.'+node.reference.pad,
                shape='',parent='',parameters=dict(member=json.dumps(node.reference.member),pad=json.dumps(node.reference.pad),**({'net':json.dumps(node.net)} if node.net else {}))))
        elif isinstance(node, MacroPropertyDecl):
            ts = _node_tokens(snapshot, node)
            root[node.name] = snapshot.text[ts[2].location.offset:ts[-1].location.offset].strip()
        elif isinstance(node, (MacroItemDecl, MechanicalItemDecl)) and getattr(node, 'parameters', None) is not None:
            if isinstance(node, MacroItemDecl) and node.kind == 'net':
                features.append(dict(category=category,kind='net',name=node.name,shape='',parent='',parameters={}))
                continue
            def add(item, parent=''):
                props = _properties(snapshot, item)
                features.append(dict(category=category, kind=item.kind, name=item.name,
                    shape=getattr(item, 'shape', ''), parent=parent,
                    parameters={k: snapshot.text[v[2].location.offset:v[-1].location.offset].strip() for k, v in props.items()}))
            add(node)
            for child in getattr(node, 'children', ()):
                add(child, node.name)
    if root: features.insert(0, dict(category=category, kind='properties', name='', shape='', parent='', parameters=root))
    return features


def definition_patch(snapshot, *, category, kind, name, parameters, shape='', parent='', remove=False):
    document = parse(snapshot.text, snapshot.filename)
    if category != ('macro' if document.kind == 'macro' else 'profile'):
        raise SourceEditError('feature category does not match document')
    if not isinstance(parameters, dict) or len(parameters) > 40 or not all(isinstance(k, str) and isinstance(v, str) and len(v) <= 65536 for k, v in parameters.items()):
        raise SourceEditError('feature requires bounded source literal parameters')
    if not all(isinstance(v, str) for v in (kind, name, shape, parent)) or type(remove) is not bool:
        raise SourceEditError('invalid feature identity')
    edits = []
    def erase(ts):
        edits.extend(TextEdit(SourceSpan(t.location.offset, t.end_offset), '') for t in ts)
    if kind in {'pad','isolated','net'} and document.kind == 'macro':
        if parent or shape: raise SourceEditError('pad/net declarations have no parent or shape')
        if kind == 'net':
            if parameters: raise SourceEditError('net declaration has no properties')
            literal=f'net {json.dumps(name)};'
            matches=[d for d in document.declarations if isinstance(d,MacroItemDecl) and d.kind=='net' and d.name==name]
        else:
            if set(parameters) != ({'member','pad','net'} if kind=='pad' else {'member','pad'}): raise SourceEditError('pad assignment requires member, pad and net; isolated requires member and pad')
            literal=f"{kind} {parameters['member']}.{parameters['pad']}" + (f" = {parameters['net']}" if kind=='pad' else '') + ';'
            matches=[d for d in document.declarations if isinstance(d,MacroPadDecl) and d.reference.member+'.'+d.reference.pad==name]
        scratch=parse(f'macro Candidate {{ {literal} }}')
        if len(scratch.declarations)!=1: raise SourceEditError('one declaration required')
        if len(matches)>1: raise SourceEditError('ambiguous declaration')
        if matches:
            ts=_node_tokens(snapshot,matches[0]); erase(ts)
            if not remove: edits[0]=TextEdit(edits[0].span,literal)
        elif remove: raise SourceEditError('declaration is not owned by this definition')
        else:
            end=tokenize(snapshot.text,snapshot.filename)[-2].location.offset
            edits.append(TextEdit(SourceSpan(end,end),snapshot.newline+'    '+literal+snapshot.newline))
    elif kind == 'properties' and document.kind == 'macro':
        from ..macro_definitions import ROOT_PROPERTIES
        if name or parent or remove or set(parameters) - set(ROOT_PROPERTIES):
            raise SourceEditError('invalid macro root properties')
        scratch = parse('macro Candidate { ' + ' '.join(f'{k} = {v};' for k,v in parameters.items()) + ' }')
        if len(scratch.declarations) != len(parameters) or any(not isinstance(d,MacroPropertyDecl) or d.name not in parameters for d in scratch.declarations):
            raise SourceEditError('root properties must contain only literal assignments')
        remaining = dict(parameters)
        for node in document.declarations:
            if not isinstance(node, MacroPropertyDecl): continue
            ts = _node_tokens(snapshot, node)
            if node.name in remaining:
                value = remaining.pop(node.name)
                edits.extend(TextEdit(SourceSpan(t.location.offset, t.end_offset), value if i == 0 else '') for i, t in enumerate(ts[2:-1]))
            else: erase(ts)
        if remaining:
            end = tokenize(snapshot.text, snapshot.filename)[-2].location.offset
            edits.append(TextEdit(SourceSpan(end, end), snapshot.newline + ''.join(f'    {k} = {v};{snapshot.newline}' for k, v in remaining.items())))
    else:
        declarations = document.declarations
        owner = None
        if parent:
            matches = [d for d in declarations if isinstance(d, MacroItemDecl) and d.kind == 'plane_return' and d.name == parent]
            if len(matches) != 1 or kind != 'contact': raise SourceEditError('contact requires one existing plane_return parent')
            owner = matches[0]
            declarations = owner.children
        matches = [d for d in declarations if isinstance(d, (MacroItemDecl, MechanicalItemDecl)) and d.kind == kind and d.name == name]
        if len(matches) > 1: raise SourceEditError('ambiguous definition ownership')
        header = ' '.join(v for v in (kind, (json.dumps(name) if document.kind=='macro' else name) if name else '', shape) if v)
        literal = header + ' { ' + ' '.join(f'{k} = {v};' for k, v in parameters.items()) + ' }'
        # A scratch parse rejects injected declarations and unexpected grammar.
        scratch = parse(f'{document.kind} Candidate {{ {literal} }}') if not parent else parse(f'macro Candidate {{ plane_return P {{ {literal} }} }}')
        candidate = scratch.declarations[0].children[0] if parent else scratch.declarations[0]
        if len(scratch.declarations) != 1 or (parent and len(scratch.declarations[0].children) != 1) or not isinstance(candidate, (MacroItemDecl, MechanicalItemDecl)) or candidate.kind != kind or candidate.name != name or set(candidate.parameters) != set(parameters) or getattr(candidate,'children',()):
            raise SourceEditError('feature must contain exactly one literal declaration')
        if matches:
            node = matches[0]
            ts = _node_tokens(snapshot, node)
            if remove:
                erase(ts)
            elif shape != getattr(node, 'shape', ''):
                raise SourceEditError('remove and recreate a feature to change its shape')
            else:
                props = _properties(snapshot, node)
                remaining = dict(parameters)
                for key, values in props.items():
                    if key in remaining:
                        value = remaining.pop(key)
                        edits.extend(TextEdit(SourceSpan(t.location.offset, t.end_offset), value if i == 0 else '') for i, t in enumerate(values[2:-1]))
                    else: erase(values)
                if remaining:
                    offset = ts[-1].location.offset
                    edits.append(TextEdit(SourceSpan(offset, offset), ' ' + ' '.join(f'{k} = {v};' for k, v in remaining.items()) + ' '))
        elif remove: raise SourceEditError('feature is not owned by this definition')
        else:
            offset = _node_tokens(snapshot, owner)[-1].location.offset if owner else tokenize(snapshot.text, snapshot.filename)[-2].location.offset
            edits.append(TextEdit(SourceSpan(offset, offset), snapshot.newline + '    ' + literal + snapshot.newline))
    patch = SourcePatch(snapshot.revision, tuple(edits))
    parse(SourceSnapshot(patch.apply(snapshot), snapshot.filename).text, snapshot.filename)
    return patch


def _viewport(points):
    from ..physical import BoardOutline, Point
    xs, ys = zip(*(points or [(0, 0)]))
    margin = 5_000_000
    return BoardOutline(tuple(Point(x, y) for x, y in [(min(xs)-margin,min(ys)-margin),(max(xs)+margin,min(ys)-margin),(max(xs)+margin,max(ys)+margin),(min(xs)-margin,max(ys)+margin)]))


def _macro_scene(document, resolver, revision):
    from ..macro_definitions import lower_macro
    from ..footprints import FootprintResolver
    from ..physical import PhysicalBoard, Placement, Point, PhysicalNet, PadReference
    from ..placement import transformed_local_point
    from .scene import board_scene
    data, inputs = lower_macro(document, resolver)
    footprints = {}
    poses = []
    fr = FootprintResolver(Path(resolver.source_directory), locked=True, offline=True)
    for row in data['members']:
        footprints[row['footprint']] = fr.resolve(row['footprint']).footprint
        poses.append(Placement(row['reference'], row['footprint'], Point(*row['center_nm']), row['rotation_degrees']))
    nets = tuple(PhysicalNet(net, tuple(PadReference(r, p) for r, p, n in data['pad_nets'] if n == net)) for net in sorted({n for _, _, n in data['pad_nets']}))
    def endpoint(value):
        if isinstance(value, list): return value
        ref, pad = value['pad']
        pose = next(p for p in poses if p.reference == ref)
        land = next(p for p in footprints[pose.footprint].pads if p.number == pad)
        point = transformed_local_point(pose, land.position)
        return [point.x_nm, point.y_nm]
    tracks = [dict(net=t['net'], layer=t['layer'], width_nm=t['width_nm'], start=endpoint(a), end=endpoint(b)) for t in data['tracks'] for a, b in zip(t['points'], t['points'][1:])]
    points = [row['center_nm'] for row in data['members']] + [p for t in tracks for p in (t['start'],t['end'])]
    board = PhysicalBoard(document.name, _viewport(points), footprints, tuple(poses), nets)
    _validate_ports(board, data, tracks, endpoint)
    scene = board_scene(board, source_revision=revision)
    scene['outline'] = dict(vertices=[], path=None, circle=None, cutouts=[])
    scene['macro_definition'] = data
    geometry = []
    for kind,key in (('track','tracks'),('via','vias'),('port','ports'),('polygon','polygons'),('zone','zones'),('keepout','keepouts'),('protected_region','protected_regions')):
        names=[n.name for n in document.declarations if isinstance(n,MacroItemDecl) and n.kind==kind]
        for name,row in zip(names,data[key]):
            item=dict(category='macro',kind=kind,name=name,net=row.get('net'),layers=row.get('layers',[row['layer']] if 'layer' in row else []))
            if 'vertices' in row:item['vertices']=row['vertices']
            if 'points' in row:item.update(points=[endpoint(p) for p in row['points']],width_nm=row['width_nm'])
            if 'point' in row:item['position']=endpoint(row['point'])
            if 'position_nm' in row:item.update(position=row['position_nm'],size_nm=row['size_nm'])
            geometry.append(item)
    scene['physical_geometry']=geometry
    scene['routed_overlay'] = dict(stale=False, connectivity_credit=False, notice='Authored macro copper; consumer validation and native DRC pending',
        native=dict(layers=data['required_layers'], fills=[], tracks=tracks, vias=[dict(v, position=v['position_nm']) for v in data['vias']]), evidence=dict(remaining=[]))
    scene['copper_keepouts'] = [dict(name=r['id'], layers=r['layers'], vertices=r['vertices'], holes=[]) for r in (*data['keepouts'], *data['protected_regions'])]
    scene['warnings']['qualification_pending'] = 'Local port copper continuity checked; macro binding, host stackup, plane-return proof and native DRC require consumer validation.'
    return scene, inputs


def _validate_ports(board,data,tracks,endpoint):
    """Same explicit copper graph and contact predicates used by macro materialization.

    No zone fill or hypothetical host plane contributes connectivity here.
    """
    if not data['ports']:
        return
    from ..physical import TrackSegment, Via, CopperLayer, Point, PadReference, Stackup, CopperPolygon, PolygonRing
    from ..drc import explicit_copper_connectivity
    from ..geometry import RoundedConvexShape, shapes_clear
    layers=tuple(CopperLayer(x) for x in data['required_layers'])
    # Keep standard physical ordering even when author lists required layers differently.
    order=[CopperLayer.FRONT]+[CopperLayer(f'In{i}.Cu') for i in range(1,31) if f'In{i}.Cu' in data['required_layers']]+[CopperLayer.BACK]
    span=tuple(l for l in order if l in layers or l in (CopperLayer.FRONT,CopperLayer.BACK))
    t=tuple(TrackSegment(net=r['net'],layer=CopperLayer(r['layer']),start=Point(*r['start']),end=Point(*r['end']),width_nm=r['width_nm']) for r in tracks)
    v=tuple(Via(r['net'],Point(*r['position_nm']),r['size_nm'],r['drill_nm'],CopperLayer(r['from_layer']),CopperLayer(r['to_layer']),r['technology'],r['finish']) for r in data['vias'])
    polygons=tuple(CopperPolygon(r['id'],r['net'],CopperLayer(r['layer']),PolygonRing(tuple(Point(*p) for p in r['vertices']))) for r in data['polygons'])
    owner=replace(board,tracks=t,vias=v,stackup=Stackup(copper_layers=span))
    graph=explicit_copper_connectivity(owner,polygons=polygons)
    for port in data['ports']:
        position=Point(*endpoint(port['point'])); layer=CopperLayer(port['layer'])
        roots={graph.roots[f'track:{i}'] for i,track in enumerate(t) if track.net==port['net'] and track.layer is layer and not shapes_clear(RoundedConvexShape((position,)),RoundedConvexShape((track.start,track.end),track.width_nm//2),1)}
        for i,via in enumerate(v):
            a,b=sorted((span.index(via.from_layer),span.index(via.to_layer)))
            if via.net==port['net'] and via.position==position and layer in span[a:b+1]:roots.add(graph.roots[f'via:{i}'])
        if not roots:raise SourceEditError('macro port has no owner copper contact')
        if len(roots)!=1 or any(not graph.pad_nodes.get(PadReference(*p)) or any(graph.roots[n] not in roots for n in graph.pad_nodes[PadReference(*p)]) for p in port['pads']):
            raise SourceEditError('macro port is not connected to its declared private pads')


def _profile_scene(document, resolver, revision):
    from ..compiler import _load_imports
    from ..mechanical_profiles import expand_mechanical_items
    from ..design import lower_mechanical
    from ..physical import PhysicalBoard
    from .scene import board_scene
    inputs = {}
    def read_import(path):
        raw = Path(path).read_bytes()
        inputs[str(Path(path).resolve())] = sha256(raw).hexdigest()
        return raw.decode('utf-8-sig')
    imported = _load_imports(document, resolver, (), source_reader=read_import)
    items, sources, instances = expand_mechanical_items(document.declarations, imported.profiles)
    # Connector roles have no electrical components in a definition. Bind them
    # to their own names only for typed mechanical validation, never instantiate.
    items = tuple(replace(i, parameters={**i.parameters, 'component': i.name}) if i.kind in {'connector','attach','overhang','component_height','assembly_access'} and 'component' not in i.parameters else i for i in items)
    has_outline = any(i.kind == 'outline' for i in items)
    if not has_outline:
        viewport = parse('board_profile Viewport { outline rectangle { width = 2000000 mm; height = 2000000 mm; origin = (-1000000 mm, -1000000 mm); } }').declarations[0]
        items = (viewport, *items)
    wrapped = replace(document, kind='board', declarations=(MechanicalDecl(document.location, items),))
    mechanical = lower_mechanical(wrapped)
    board = PhysicalBoard(document.name, mechanical.outline, {}, (), (), mechanical_holes=mechanical.holes,
        mechanical_slots=mechanical.slots, keepouts=mechanical.keepouts, copper_keepouts=mechanical.copper_keepouts,
        datums=mechanical.datums, boundary_edges=mechanical.boundary_edges, mechanical_references=mechanical.references,
        assembly_envelopes=mechanical.assembly_envelopes)
    scene = board_scene(board, source_revision=revision)
    from ..mechanical_profiles import mechanical_provenance
    scene['mechanical_provenance'] = mechanical_provenance(replace(mechanical,sources=sources,profiles=instances))
    inherited=[]
    for item,source in zip(items if has_outline else items[1:], sources):
        if not source.profile:continue
        owner=Path(source.location.filename)
        snapshot=SourceSnapshot(owner.read_bytes(),str(owner))
        original=next((f for f in definition_features(snapshot) if f['kind']==item.kind and f['name']==item.name.split('/')[-1]),None)
        if original:
            inherited.append(dict(original,name=item.name,editable=False,definition_path=str(owner.resolve()),profile=source.profile))
    scene['inherited_features']=inherited
    scene['connector_roles'] = [dict(name=c.role, position=[c.position.x_nm,c.position.y_nm], rotation=str(c.rotation), side=c.side.value, anchor_pad=c.anchor_pad) for c in mechanical.connectors]
    scene['physical_geometry'] = [dict(category='profile',kind='connector',**c) for c in scene['connector_roles']]
    if not has_outline:
        scene['outline'] = dict(vertices=[], path=None, circle=None, cutouts=[])
        points = [h['position'] for h in scene['holes']] + [c['position'] for c in scene['connector_roles']]
        vertices = _viewport(points).vertices
        scene['bounds'] = [vertices[0].x_nm, vertices[0].y_nm, vertices[2].x_nm, vertices[2].y_nm]
    scene['warnings']['qualification_pending'] = 'Profile roles and fragment boundaries require consumer validation.'
    for reference in mechanical.references:
        path = getattr(reference, 'asset_path', None)
        if path and Path(path).is_file(): inputs[str(Path(path).resolve())] = sha256(Path(path).read_bytes()).hexdigest()
    return scene, inputs


def feature_catalog(scene):
    if scene['document_kind'] == 'board_profile':
        polygon='[(0 mm, 0 mm), (4 mm, 0 mm), (4 mm, 4 mm), (0 mm, 4 mm)]'
        return [dict(category='profile', kind=k, name='' if k in {'outline','rules'} else 'new_feature', shape=s, parameters=p) for k,s,p in (
            ('hole','',{'position':'(0 mm, 0 mm)','diameter':'2 mm'}),
            ('connector','',{'position':'(0 mm, 0 mm)','anchor_pad':'"1"','rotation':'0','side':'front'}),
            ('outline','rectangle',{'width':'50 mm','height':'30 mm'}),
            ('outline','circle',{'diameter':'50 mm','center':'(25 mm, 25 mm)'}),
            ('outline','rounded_rectangle',{'width':'50 mm','height':'30 mm','corner_radius':'3 mm'}),
            ('outline','polygon',{'vertices':polygon}),
            ('outline','path',{'maximum_chord_error':'0.01 mm'}),
            ('boundary','line',{'start':'(0 mm, 0 mm)','end':'(4 mm, 0 mm)'}),
            ('boundary','arc',{'start':'(0 mm, 0 mm)','mid':'(2 mm, 2 mm)','end':'(4 mm, 0 mm)'}),
            ('slot','',{'start':'(2 mm, 2 mm)','end':'(4 mm, 2 mm)','width':'1 mm'}),
            ('cutout','polygon',{'vertices':polygon}),
            ('keepout','rectangle',{'width':'4 mm','height':'4 mm','origin':'(0 mm, 0 mm)','side':'front'}),
            ('keepout','polygon',{'vertices':polygon,'side':'front'}),
            ('copper_keepout','rectangle',{'width':'4 mm','height':'4 mm','origin':'(0 mm, 0 mm)','layers':'"F.Cu"'}),
            ('copper_keepout','polygon',{'vertices':polygon,'layers':'"F.Cu"'}),
            ('rules','',{'minimum_clearance':'0.2 mm','minimum_track_width':'0.2 mm'}),
            ('datum','',{'position':'(0 mm, 0 mm)'}),
            ('edge','',{'start':'(0 mm, 0 mm)','end':'(4 mm, 0 mm)'}),
            ('attach','',{'position':'(0 mm, 0 mm)','anchor':'origin','rotation':'0','side':'front'}),
            ('overhang','',{'edge':'TOP','start':'1 mm','end':'3 mm','distance':'2 mm','reason':'"Source evidence required"'}),
            ('component_height','',{'height':'3 mm'}),
            ('enclosure','rectangle',{'origin':'(0 mm, 0 mm)','width':'4 mm','height':'4 mm','side':'front','maximum_height':'4 mm'}),
            ('enclosure','polygon',{'vertices':polygon,'side':'front','maximum_height':'4 mm'}),
            ('assembly_access','rectangle',{'origin':'(0 mm, 0 mm)','width':'4 mm','height':'4 mm','side':'component','purpose':'"Insertion clearance"'}),
            ('assembly_access','polygon',{'vertices':polygon,'side':'component','purpose':'"Insertion clearance"'}),
            ('reference','dxf',{'file':'"assets/case.dxf"','sha256':'"REPLACE_WITH_FILE_SHA256"','units':'mm','frame':'cartesian','position':'(0 mm, 0 mm)','rotation':'0','mirror_x':'false','side':'both','purpose':'"Enclosure reference only"'}))]
    features = scene['source_features']
    member = next(f for f in features if f['kind'] == 'member')
    data = scene['macro_definition']
    ref,pad,role = data['pad_nets'][0] if data['pad_nets'] else (member['name'],'1','NET_ROLE')
    net = json.dumps(role)
    endpoint = json.dumps(ref) + '.' + json.dumps(pad)
    layer = json.dumps(data['required_layers'][0])
    vertices = '[(0 mm, 0 mm), (2 mm, 0 mm), (2 mm, 2 mm), (0 mm, 2 mm)]'
    templates = {
        'net':{},
        'pad':dict(member=json.dumps(ref),pad=json.dumps(pad),net=net),
        'isolated':dict(member=json.dumps(ref),pad='"PAD_NUMBER"'),
        'member':dict(member['parameters']),
        'track':dict(net=net,layer=layer,width='0.2 mm',points=f'[{endpoint}, (2 mm, 0 mm)]'),
        'via':dict(net=net,at='(0 mm, 0 mm)',size='0.6 mm',drill='0.3 mm',from_layer='"F.Cu"',to_layer='"B.Cu"'),
        'port':dict(net=net,layer=layer,point=endpoint,pads=f'[{endpoint}]'),
        'polygon':dict(net=net,layer=layer,vertices=vertices),
        'zone':dict(net=net,layers=f'[{layer}]',vertices=vertices,priority='0',clearance='0.2 mm',minimum_width='0.2 mm',pad_connection='solid'),
        'keepout':dict(layers=f'[{layer}]',vertices=vertices),
        'protected_region':dict(layers=f'[{layer}]',vertices=vertices),
        'plane_return':dict(net=net,layers=f'[{layer}]',pads=f'[{endpoint}]'),
        'contact':dict(pad=endpoint,at='(0 mm, 0 mm)'),
        'width_contract':dict(track=next((json.dumps(f['name']) for f in features if f['kind']=='track'),'"track_name"'),minimum_width='0.1 mm',purpose='pin_entry',evidence='"Unresolved: supply source evidence"'),
        'edge_opening':dict(keepout=next((json.dumps(f['name']) for f in features if f['kind']=='keepout'),'"keepout_name"'),edge_indices='[0]')}
    return [dict(category='macro',kind=k,name='new_feature',parameters=p,parent=next((f['name'] for f in features if f['kind']=='plane_return'),'') if k=='contact' else '') for k,p in templates.items()]


@dataclass(frozen=True)
class DefinitionReview:
    before: SourceSnapshot
    after: SourceSnapshot
    board: dict
    diff: str
    @property
    def id(self): return sha256((self.before.revision + self.after.revision).encode()).hexdigest()


class DefinitionWorkspace:
    def __init__(self, session):
        self.session = session
        self.source = session.source
        self.source_reader = session.reader
        self.inputs = None
    def check_inputs(self):
        if self.inputs is not None:
            for path, digest in self.inputs.items():
                if not Path(path).is_file() or sha256(Path(path).read_bytes()).hexdigest() != digest:
                    raise SourceEditError('definition dependency changed; reload source')
    def validate(self, raw, seeds=None, options=None):
        self.check_inputs()
        scene, inputs = self.session.build(raw)
        if self.inputs is None: self.inputs = inputs
        elif inputs != self.inputs: raise SourceEditError('definition dependency inventory changed; reload source')
        self.check_inputs()
        return None, scene
    def save(self, review):
        # Share the hardened same-directory CAS writer without board assumptions.
        proxy = object.__new__(SourceWorkspace)
        proxy.source = self.source
        proxy.source_reader = self.source_reader
        proxy.check_inputs = self.check_inputs
        from types import SimpleNamespace
        proxy.save(SimpleNamespace(before=review.before, after=review.after, board=SimpleNamespace(mechanical_references=())))


class DefinitionSession:
    def __init__(self, source, reader=None, **settings):
        self.source = Path(source).absolute()
        if any(p.is_symlink() for p in (self.source,*self.source.parents)): raise SourceEditError('definition editing does not follow source symlinks')
        self.reader = reader
        from .dependencies import dependency_context
        self.dependency_source = dependency_context(self.source, settings.get('project'))
        self.readonly = any(p.casefold()=='.copper-cache' for p in self.source.parts)
        self.options = None
        self.revision = 0
        self.source_pending = None
        self.undo, self.redo = [], []
        self.workspace = DefinitionWorkspace(self)
        self.snapshot = SourceSnapshot(self.read_source(), str(self.source))
        _, self.accepted = self.workspace.validate(self.snapshot.raw)
    def read_source(self): return self.reader() if self.reader else self.source.read_bytes()
    def build(self, raw):
        snapshot = SourceSnapshot(raw, str(self.source))
        document = parse(snapshot.text, snapshot.filename)
        resolver = PackageResolver.for_source(self.dependency_source, document.location, locked=True, offline=True)
        if self.dependency_source != self.source:
            # Register the owner using the consumer's verified module inventory.
            # Relative references stay confined to that source module.
            for module in resolver.manifest.requirements:
                package = resolver.resolve(module, document.location)
                if self.source.is_relative_to(package.directory):
                    break
        if document.kind == 'macro': scene, inputs = _macro_scene(document, resolver, snapshot.revision)
        elif document.kind == 'board_profile': scene, inputs = _profile_scene(document, resolver, snapshot.revision)
        else: raise SourceEditError('definition editor requires macro or board_profile')
        manifest = find_manifest(self.dependency_source)
        if manifest:
            for path in (manifest, manifest.parent / 'copper.lock'):
                if path.is_file(): inputs[str(path.resolve())] = sha256(path.read_bytes()).hexdigest()
        scene.update(document_kind=document.kind, source_features=definition_features(snapshot), feature_catalog=[],
            source_writable=True, notice='Definition authoring in local coordinates. Consumer integration and qualification remain pending.')
        scene['capabilities'].update(auto_place=False, pose_preview=False, session_locks=False, source_save=not self.readonly, mechanical_edit=False, definition_edit=not self.readonly, physical_feature_edit=not self.readonly)
        scene['source_writable'] = not self.readonly
        for feature in scene['source_features']: feature['editable'] = not self.readonly
        scene['source_features'].extend(scene.pop('inherited_features',[]))
        scene['feature_catalog'] = feature_catalog(scene)
        return scene, inputs
    def _check(self, revision):
        if type(revision) is not int or revision != self.revision: raise SourceEditError('stale editor revision')
        if self.read_source() != self.snapshot.raw: raise SourceEditError('source changed externally; reload source')
        self.workspace.check_inputs()
    def scene(self):
        import copy
        result = copy.deepcopy(self.accepted)
        result.update(revision=self.revision, can_undo=False, can_redo=False, can_source_undo=bool(self.undo), can_source_redo=bool(self.redo))
        result['source_stale'] = self.read_source() != self.snapshot.raw
        result['source_review'] = dict(id=self.source_pending.id,diff=self.source_pending.diff) if self.source_pending else None
        result['source_preview'] = copy.deepcopy(self.source_pending.board) if self.source_pending else None
        return result
    def close(self): pass
    def operation(self, request):
        if not isinstance(request, dict): raise SourceEditError('operation must be an object')
        action = request.get('action')
        if self.readonly and action != 'reload_source': raise SourceEditError('cached dependency definitions are read-only; open the owning library source or create an adaptation')
        if action == 'reload_source':
            if set(request) != {'action','revision'}: raise SourceEditError('unexpected reload fields')
            if type(request['revision']) is not int or request['revision'] != self.revision: raise SourceEditError('stale editor revision')
            previous_inputs=self.workspace.inputs
            self.workspace.inputs = None
            raw = self.read_source()
            try:
                _, scene = self.workspace.validate(raw)
            except Exception:
                self.workspace.inputs=previous_inputs
                raise
            self.snapshot, self.accepted = SourceSnapshot(raw,str(self.source)), scene
            self.source_pending = None
            self.undo.clear(); self.redo.clear(); self.revision += 1
            return dict(scene=self.scene(), accepted=True)
        self._check(request.get('revision'))
        if action in {'review_feature','review_features'}:
            if action=='review_features':
                if set(request)!={'action','revision','features'}:raise SourceEditError('unexpected batch fields')
                features=request['features']
                if not isinstance(features,list) or not 1<=len(features)<=64:raise SourceEditError('definition batch requires 1–64 feature edits')
            else:
                features=[{k:v for k,v in request.items() if k not in {'action','revision'}}]
            current=self.snapshot
            for feature in features:
                if not isinstance(feature,dict) or set(feature)-{'category','kind','name','parameters','shape','parent','remove','editable'}:raise SourceEditError('unexpected feature fields')
                if not {'category','kind','name','parameters'}<=set(feature):raise SourceEditError('feature requires category, kind, name and parameters')
                raw=definition_patch(current,**{k:v for k,v in feature.items() if k!='editable'}).apply(current)
                current=SourceSnapshot(raw,str(self.source))
            raw=current.raw
            if raw == self.snapshot.raw: raise SourceEditError('edit has no source changes')
            _, scene = self.workspace.validate(raw)
            after = SourceSnapshot(raw,str(self.source))
            self.source_pending = DefinitionReview(self.snapshot,after,scene,''.join(unified_diff(self.snapshot.text.splitlines(True),after.text.splitlines(True),fromfile='current.copper',tofile='proposed.copper')))
        elif action == 'discard_source':
            if set(request) != {'action','revision'}: raise SourceEditError('unexpected discard fields')
            self.source_pending = None
        elif action == 'save_source':
            if set(request) != {'action','revision','review_id'}: raise SourceEditError('unexpected save fields')
            review = self.source_pending
            if review is None or request['review_id'] != review.id: raise SourceEditError('review changed')
            _, scene = self.workspace.validate(review.after.raw)
            if scene != review.board: raise SourceEditError('definition inputs changed after review')
            self.workspace.save(review)
            self.undo.append(self.snapshot.raw); self.undo = self.undo[-64:]; self.redo.clear()
            self.snapshot, self.accepted, self.source_pending = review.after, scene, None
        elif action in {'undo_source','redo_source'}:
            if set(request) != {'action','revision'}: raise SourceEditError('unexpected history fields')
            take, put = (self.undo,self.redo) if action == 'undo_source' else (self.redo,self.undo)
            if not take: raise SourceEditError('no source history available')
            raw = take[-1]; _, scene = self.workspace.validate(raw)
            after = SourceSnapshot(raw,str(self.source))
            self.workspace.save(DefinitionReview(self.snapshot,after,scene,''))
            take.pop(); put.append(self.snapshot.raw)
            self.snapshot,self.accepted,self.source_pending = after,scene,None
        else: raise SourceEditError('unsupported definition operation')
        self.revision += 1
        return dict(scene=self.scene(),accepted=True)


def board_macro_features(board):
    """Read-only immutable instance contracts; definitions remain their owners."""
    from dataclasses import asdict
    provenance = {r['name']: r for r in json.loads(board.metadata.get('source_physical_macros','[]'))}
    return [dict(category='macro_instance', kind='contracts', name=m.cluster, editable=False,
        parameters={k: str(v) for k,v in asdict(m).items()}, definition_path=provenance.get(m.cluster,{}).get('source_path')) for m in board.hard_macros]
