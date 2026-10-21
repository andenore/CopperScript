"""Generic final-native RF layout inventory, not an electromagnetic solver.

No board selectors, native edits, impedance approval or universal spacing limits.
Connected-return inventory is a polygonal lower-bound screen, not RF inductance.
"""
from math import hypot

from .rf_geometry import native_fill_polygon


def layout_inventory(native, *, signal_net, ground_net, signal_layer,
                     reference_layer, port_component, interference_nets):
    from shapely.geometry import LineString, Point, Polygon
    from shapely.ops import unary_union

    layers = native['layers']
    if (signal_layer not in layers or reference_layer not in layers
            or abs(layers.index(signal_layer)-layers.index(reference_layer)) != 1):
        raise ValueError('Select adjacent enabled signal/reference layers')
    if signal_net == ground_net or signal_net in interference_nets or ground_net in interference_nets:
        raise ValueError('Signal, return and interference nets must be distinct')
    tracks = [t for t in native['tracks'] if t['net'] == signal_net]
    if not tracks or any(t['layer'] != signal_layer for t in tracks):
        raise ValueError('Inventory requires a single-layer straight-track signal')
    lands = native.get('lands')
    if lands is None:
        raise ValueError('Actual native land polygons required, not contact centres')

    def land_shape(land, layer):
        shapes = [native_fill_polygon(p) for p in land['copper'].get(layer, [])]
        shape = unary_union(shapes)
        # Through-pad circular holes are conservatively removed. Non-circular
        # holes cannot be represented by this return screen.
        dx, dy = land.get('drill_nm', [0, 0])
        if dx or dy:
            if dx != dy:
                raise ValueError('Slotted land return geometry is unsupported')
            shape = shape.difference(Point(land['position']).buffer(dx/2, quad_segs=32))
        return shape

    def via_span(via, layer):
        a, b = sorted((layers.index(via['from_layer']), layers.index(via['to_layer'])))
        return a <= layers.index(layer) <= b

    def via_shape(via):
        if via.get('via_type') != 'through':
            raise ValueError('Non-through via return geometry is unsupported')
        center = Point(via['position'])
        return center.buffer(via['size_nm']/2, quad_segs=32).difference(
            center.buffer(via['drill_nm']/2, quad_segs=32))

    def conductor(net, layer, *, include_vias=True):
        shapes = [native_fill_polygon(f) for f in native['fills']
                  if f['net'] == net and f['layer'] == layer]
        shapes.extend(LineString([t['start'], t['end']]).buffer(t['width_nm']/2, quad_segs=32)
                      for t in native['tracks'] if t['net'] == net and t['layer'] == layer)
        shapes.extend(land_shape(p, layer) for p in lands if p['net'] == net)
        if include_vias:
            shapes.extend(via_shape(v) for v in native['vias']
                          if v['net'] == net and via_span(v, layer))
        return unary_union(shapes)

    signal = conductor(signal_net, signal_layer)
    top_ground = conductor(ground_net, signal_layer)
    # A via cannot establish its own reference-plane join merely by being
    # present on that layer. Require independently represented copper there.
    reference_ground = conductor(ground_net, reference_layer, include_vias=False)
    components = ([top_ground] if isinstance(top_ground, Polygon) else
                  [p for p in getattr(top_ground, 'geoms', ()) if isinstance(p, Polygon)])
    ground_vias = [(i, v, via_shape(v)) for i, v in enumerate(native['vias'])
                   if v['net'] == ground_net and via_span(v, signal_layer)
                   and via_span(v, reference_layer)]
    returns = []
    for p in lands:
        if p['component'] != port_component or p['net'] != ground_net or signal_layer not in p['copper']:
            continue
        pad = land_shape(p, signal_layer)
        connected = [g for g in components if g.intersection(pad).area > 0]
        candidates = []
        for index, via, annulus in ground_vias:
            # Positive area only; boundary touches and net labels are not joins.
            if (any(g.intersection(annulus).area > 0 for g in connected)
                    and reference_ground.intersection(annulus).area > 0):
                candidates.append({'native_via_index': index, 'position_nm': via['position'],
                    'pad_center_to_via_center_nm': round(hypot(
                        p['position'][0]-via['position'][0], p['position'][1]-via['position'][1])),
                    'pad_edge_to_via_annulus_nm': round(pad.distance(annulus))})
        candidates.sort(key=lambda v: (v['pad_center_to_via_center_nm'], v['native_via_index']))
        returns.append({'component': p['component'], 'pad': p['pad'],
                        'position_nm': p['position'], 'size_nm': p['size_nm'],
                        'modeled_top_ground_component_count': len(connected),
                        'modeled_spanning_return_via_count': len(candidates),
                        'nearest_modeled_return_vias': candidates[:4]})
    if not returns:
        raise ValueError('Selected port has no native ground lands on signal layer')
    nearby = []
    for net in sorted(set(interference_nets)):
        for layer in layers:
            copper = conductor(net, layer)
            if not copper.is_empty:
                nearby.append({'net': net, 'layer': layer,
                               'projected_edge_gap_nm': round(signal.distance(copper)),
                               'interpretation': 'XY projection only; not coupling or a spacing pass'})
    return {'status': 'incomplete', 'qualified_release': False,
            'signal_net': signal_net, 'ground_net': ground_net,
            'signal_layer': signal_layer, 'reference_layer': reference_layer,
            'signal_length_nm': round(sum(hypot(t['end'][0]-t['start'][0], t['end'][1]-t['start'][1]) for t in tracks)),
            'signal_widths_nm': sorted({t['width_nm'] for t in tracks}),
            'signal_via_count': sum(v['net'] == signal_net for v in native['vias']),
            'signal_lands': [{k: p[k] for k in ('component', 'pad', 'position', 'size_nm')}
                             for p in lands if p['net'] == signal_net],
            'port_ground_returns': returns,
            'interference_projection_inventory': sorted(nearby, key=lambda p: (p['projected_edge_gap_nm'], p['net'], p['layer'])),
            'round_conductor_quadrant_segments': 32,
            'limitations': ['Polygonal native land/round-conductor approximation; no native copper changes',
                'Return candidates are connected only through represented same-layer copper and spanning annuli',
                'Missing modeled joins are not proof of a real open; native DRC is an independent check',
                'Centre/edge distances do not establish return inductance, ground-web adequacy or a qualified via fence',
                'Projected noise distances include other layers; inspect shielding and actual 3D fields',
                'Pads, bends, laminate, mask, protection, matching and antenna/enclosure still require qualification']}


def main():
    import argparse
    import json
    from pathlib import Path
    import subprocess
    import sys
    from .qualification import hash_file

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pcb', type=Path)
    parser.add_argument('--kicad-python', required=True, type=Path)
    parser.add_argument('--signal-net', required=True)
    parser.add_argument('--ground-net', required=True)
    parser.add_argument('--signal-layer', required=True)
    parser.add_argument('--reference-layer', required=True)
    parser.add_argument('--port-component', required=True)
    parser.add_argument('--interference-net', action='append', default=[])
    parser.add_argument('--source', type=Path, help='Optional locked/offline source/native land and immutable-copper checkpoint')
    parser.add_argument('--hard-macro', action='append', default=[], type=Path)
    parser.add_argument('--fabrication-profile', default='generic')
    parser.add_argument('--report', required=True, type=Path)
    parser.add_argument('--native-report', required=True, type=Path)
    args = parser.parse_args()
    # Exclusive outputs and explicit host tool; sources select no executables.
    paths = [p.resolve() for p in (args.pcb, args.report, args.native_report)]
    if len(set(paths)) != 3 or any(p.exists() for p in paths[1:]):
        parser.error('Reports must be distinct, new paths outside the retained PCB')
    if args.hard_macro and args.source is None:
        parser.error('--hard-macro requires --source')
    before = hash_file(args.pcb)
    probe = Path(__file__).with_name('native_rf_probe.py')
    try:
        result = subprocess.run([str(args.kicad_python), str(probe), str(args.pcb.resolve())],
                                capture_output=True, text=True, timeout=60, check=True)
        if before != hash_file(args.pcb):
            raise ValueError('PCB changed during read-only extraction')
        native = json.loads(result.stdout)
        source_checkpoint = None
        if args.source is not None:
            from .compiler import compile_design_file
            from .footprints import FootprintResolver
            from .hard_macros import apply_hard_macro_scene
            from .native_checkpoint import read_front_native_checkpoint
            from .physicalize import PrototypePhysicalOptions, resolved_physicalize
            source = args.source.resolve()
            source_inputs = {str(p.resolve()): hash_file(p) for p in
                             [source, source.parent/'copper.mod', source.parent/'copper.lock', *args.hard_macro]}
            design = compile_design_file(source, locked=True, offline=True)
            physical = resolved_physicalize(design, FootprintResolver(
                base_directory=source.parent, locked=True, offline=True),
                PrototypePhysicalOptions(copper_layers=len(native['layers']),
                                         fabrication_profile=args.fabrication_profile))
            for macro in args.hard_macro:
                physical = apply_hard_macro_scene(physical, macro, locked=True, offline=True)
            observed = read_front_native_checkpoint(physical, args.pcb)
            if before != hash_file(args.pcb):
                raise ValueError('PCB changed during source/native checkpoint')
            if any(hash_file(Path(p)) != digest for p, digest in source_inputs.items()):
                raise ValueError('Source/lock/macro changed during source/native checkpoint')
            source_checkpoint = {'status': 'source_land_pose_and_immutable_copper_match',
                'qualified_release': False, 'source_sha256': hash_file(source),
                'manifest_sha256': hash_file(source.parent/'copper.mod'),
                'lock_sha256': hash_file(source.parent/'copper.lock'),
                'macro_scenes': {str(p.resolve()): hash_file(p) for p in args.hard_macro},
                'input_files': source_inputs,
                'components': len(observed.placements), 'tracks': len(observed.tracks),
                'vias': len(observed.vias), 'scope': observed.metadata['native_checkpoint_scope']}
        report = layout_inventory(native, signal_net=args.signal_net, ground_net=args.ground_net,
            signal_layer=args.signal_layer, reference_layer=args.reference_layer,
            port_component=args.port_component, interference_nets=args.interference_net)
        report['schema'] = 'copperscript-rf-layout-inventory/v0.1'
        report['source_checkpoint'] = source_checkpoint
        report['inputs'] = {'pcb': str(args.pcb.resolve()), 'pcb_sha256': before,
            'algorithm_sha256': hash_file(Path(__file__)), 'probe_sha256': hash_file(probe),
            'native_probe_sha256': hash_file(probe.with_name('native_qualification_probe.py')),
            'overlay_probe_sha256': hash_file(probe.parent/'editor/native_overlay.py'),
            'polygon_decoder_sha256': hash_file(probe.with_name('rf_geometry.py')),
            'kicad_version': native['kicad_version'], 'native_tool_files': native['native_tool_files'],
            'arguments': {k: ([str(p) for p in v] if k == 'hard_macro' else
                              str(v) if isinstance(v, Path) else v) for k, v in vars(args).items()}}
        if source_checkpoint is not None:
            report['inputs']['source_checkpoint_algorithm_sha256'] = hash_file(probe.with_name('native_checkpoint.py'))
        for path, data in ((args.native_report, native), (args.report, report)):
            with path.open('x', encoding='utf-8') as stream:
                json.dump(data, stream, indent=2, allow_nan=False)
                stream.write('\n')
        print(json.dumps({'status': report['status'], 'qualified_release': False,
                          'report': str(args.report), 'signal_length_nm': report['signal_length_nm'],
                          'ground_lands': len(report['port_ground_returns'])}))
        return 0  # Inventory completed; never a qualification-pass exit code.
    except (ValueError, subprocess.SubprocessError) as exc:
        print(str(exc), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
