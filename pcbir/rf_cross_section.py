"""Sample actual coplanar fill gaps without claiming impedance qualification.

This is a geometry/model applicability screen, not a field solver. Explicit
launch/bend exclusions are reported, never silently treated as verified copper.
Ground pads/tracks not represented by filled polygons, electrical ground
continuity, materials, mask and pad discontinuities require separate review.
"""
from math import ceil, hypot

from .qualification import check
from .rf_geometry import native_fill_polygon


def sample_coplanar_cross_sections(native, *, identifier, net, ground_net,
        signal_layer, expected_width_nm, expected_gap_nm, gap_tolerance_nm,
        sample_pitch_nm, probe_length_nm, exclusions=()):
    for name, value in (('width', expected_width_nm), ('gap', expected_gap_nm),
                        ('pitch', sample_pitch_nm), ('probe', probe_length_nm)):
        if type(value) is not int or value <= 0:
            raise ValueError(f'{name} must be a positive integer in nm')
    if type(gap_tolerance_nm) is not int or gap_tolerance_nm < 0:
        raise ValueError('gap tolerance must be a nonnegative integer in nm')
    if probe_length_nm <= expected_width_nm // 2 + expected_gap_nm + gap_tolerance_nm:
        raise ValueError('probe must extend beyond the expected ground boundary')
    for item in exclusions:
        if (not item.get('reason') or len(item['center_nm']) != 2
                or any(type(v) is not int for v in item['center_nm'])
                or type(item['radius_nm']) is not int or item['radius_nm'] <= 0):
            raise ValueError('exclusions require integer geometry and an audit reason')
    try:
        from shapely.geometry import LineString, Point
        from shapely.ops import unary_union
    except ImportError:
        return check(identifier, 'incomplete', 'Vector geometry unavailable')
    tracks = [t for t in native['tracks'] if t['net'] == net]
    fills = [f for f in native['fills'] if f['net'] == ground_net and f['layer'] == signal_layer]
    if not tracks or not fills:
        return check(identifier, 'incomplete', 'Actual signal tracks and filled coplanar ground required')
    try:
        copper = unary_union([native_fill_polygon(f) for f in fills])
    except ValueError:
        return check(identifier, 'incomplete', 'Unsupported filled polygon; no healing')
    samples, skipped, unsupported = [], 0, []
    for index, track in enumerate(tracks):
        if track['layer'] != signal_layer or track['width_nm'] != expected_width_nm:
            unsupported.append(index)
            continue
        x, y = track['start']
        dx, dy = track['end'][0] - x, track['end'][1] - y
        length = hypot(dx, dy)
        if not length:
            raise ValueError('zero-length signal track')
        nx, ny = -dy / length, dx / length
        count = max(1, ceil(length / sample_pitch_nm))
        for sample in range(count):
            fraction = (sample + 0.5) / count
            px, py = x + fraction * dx, y + fraction * dy
            if any(hypot(px-e['center_nm'][0], py-e['center_nm'][1]) <= e['radius_nm'] for e in exclusions):
                skipped += 1
                continue
            center = Point(px, py)
            gaps = []
            for sign in (-1, 1):
                ray = LineString([(px, py), (px+sign*nx*probe_length_nm, py+sign*ny*probe_length_nm)])
                intersection = ray.intersection(copper)
                gaps.append(None if intersection.is_empty else
                            round(center.distance(intersection)-expected_width_nm/2))
            samples.append({'track_index':index, 'position_nm':[round(px),round(py)],
                            'gaps_nm':gaps})
    missing = [i for i, s in enumerate(samples) if any(g is None for g in s['gaps_nm'])]
    outside = [i for i, s in enumerate(samples) if any(g is not None and
               abs(g-expected_gap_nm) > gap_tolerance_nm for g in s['gaps_nm'])]
    vias = sum(v['net'] == net for v in native.get('vias', []))
    matches = bool(samples) and not (unsupported or missing or outside or vias)
    return check(identifier, 'incomplete',
        'Sampled geometry only; no continuous gap, launch, electrical-return or impedance approval',
        geometry_matches_model_at_samples=matches, unsupported_track_indices=unsupported,
        missing_ground_sample_indices=missing, out_of_bounds_sample_indices=outside,
        signal_via_count=vias, expected_width_nm=expected_width_nm, expected_gap_nm=expected_gap_nm,
        gap_tolerance_nm=gap_tolerance_nm, sample_pitch_nm=sample_pitch_nm,
        probe_length_nm=probe_length_nm, samples=samples, excluded_sample_count=skipped,
        exclusions=list(exclusions))
