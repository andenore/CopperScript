"""Actual straight differential-pair inventory, not impedance or delay signoff.

Only exactly parallel, same-layer segments within an explicit search gap count
as candidates. Bends, pads, vias and distant parallel copper are not silently
treated as a uniform coupled line. Lengths are copper inventory, not paths.
"""
from collections import defaultdict
from math import hypot

from .qualification import check
from .rf_geometry import reference_coverage


def _length(track):
    return hypot(track["end"][0] - track["start"][0],
                 track["end"][1] - track["start"][1])


def _union_length(intervals):
    total, end = 0.0, float("-inf")
    for low, high in sorted(intervals):
        total += max(0.0, high - max(low, end))
        end = max(end, high)
    return total


def _parallel(a, b, maximum_gap_nm):
    if a["layer"] != b["layer"]:
        return None
    dx, dy = (a["end"][i] - a["start"][i] for i in (0, 1))
    ex, ey = (b["end"][i] - b["start"][i] for i in (0, 1))
    if dx * ey != dy * ex:
        return None
    length = _length(a)
    def project(point):
        return ((point[0] - a["start"][0]) * dx +
                (point[1] - a["start"][1]) * dy) / length
    p, q = project(b["start"]), project(b["end"])
    low, high = max(0.0, min(p, q)), min(length, max(p, q))
    if high <= low:
        return None
    distance = abs(dx * (b["start"][1] - a["start"][1]) -
                   dy * (b["start"][0] - a["start"][0])) / length
    gap = distance - (a["width_nm"] + b["width_nm"]) / 2
    if gap > maximum_gap_nm:
        return None
    # Convert the overlap interval to each member's own start/end coordinates.
    other = sorted([abs(low - p), abs(high - p)])
    return low, high, other, distance, gap


def differential_pair_inventory(native, context, *, identifier, positive_net,
                                negative_net, maximum_search_gap_nm):
    """Report geometry and adjacent filled copper, without assuming its RF role."""
    if (not isinstance(positive_net, str) or not positive_net or
            not isinstance(negative_net, str) or not negative_net or positive_net == negative_net):
        raise ValueError("pair needs two distinct nonempty net names")
    if type(maximum_search_gap_nm) is not int or maximum_search_gap_nm <= 0:
        raise ValueError("maximum_search_gap_nm must be an explicit positive integer")
    if native is None:
        return check(identifier, "incomplete", "Native pair geometry unavailable")
    layers = native["layers"]
    if len(set(layers)) != len(layers):
        raise ValueError("duplicate native layers")
    members = {net: [t for t in native["tracks"] if t["net"] == net]
               for net in (positive_net, negative_net)}
    if any(not tracks for tracks in members.values()):
        return check(identifier, "incomplete", "Both routed pair members are required")
    for tracks in members.values():
        for t in tracks:
            if (t["layer"] not in layers or type(t["width_nm"]) is not int or t["width_nm"] <= 0 or
                    any(len(t[key]) != 2 or any(type(n) is not int for n in t[key])
                        for key in ("start", "end")) or _length(t) == 0):
                raise ValueError("invalid native straight pair segment")
    sections, intervals = [], defaultdict(list)
    for i, a in enumerate(members[positive_net]):
        for j, b in enumerate(members[negative_net]):
            candidate = _parallel(a, b, maximum_search_gap_nm)
            if candidate is None:
                continue
            low, high, other, distance, gap = candidate
            intervals[positive_net, i].append((low, high))
            intervals[negative_net, j].append(other)
            sections.append(dict(positive_segment=i, negative_segment=j, layer=a["layer"],
                                 positive_width_nm=a["width_nm"], negative_width_nm=b["width_nm"],
                                 centre_distance_nm=distance, edge_gap_nm=gap,
                                 parallel_overlap_length_nm=high - low))
    inventory = {}
    for net, tracks in members.items():
        by_layer = {}
        for layer in sorted({t["layer"] for t in tracks}, key=layers.index):
            selected = [(i, t) for i, t in enumerate(tracks) if t["layer"] == layer]
            total = sum(_length(t) for _, t in selected)
            covered = sum(_union_length(intervals[net, i]) for i, _ in selected)
            by_layer[layer] = dict(track_count=len(selected), widths_nm=sorted({t["width_nm"] for _, t in selected}),
                                   total_track_length_nm=total,
                                   parallel_candidate_length_nm=covered,
                                   length_without_parallel_candidate_nm=max(0.0, total - covered))
        vias = [v for v in native.get("vias", []) if v["net"] == net]
        inventory[net] = dict(layers=by_layer, segments=tracks, via_count=len(vias), vias=vias,
                             total_track_length_nm=sum(_length(t) for t in tracks))
    adjacent = []
    physical = context.get("physical_layers", [])
    if physical and context.get("copper_layers") != layers:
        raise ValueError("pair stackup order differs from native board")
    for signal in sorted({t["layer"] for tracks in members.values() for t in tracks}, key=layers.index):
        index = layers.index(signal)
        for neighbour in (index - 1, index + 1):
            if not 0 <= neighbour < len(layers):
                continue
            reference = layers[neighbour]
            entry = dict(signal_layer=signal, adjacent_layer=reference, surface_separation_nm=None, filled_nets=[])
            if physical:
                a, b = sorted(next(i for i, p in enumerate(physical) if p.get("name") == name)
                              for name in (signal, reference))
                entry["surface_separation_nm"] = sum(p["thickness_nm"] for p in physical[a+1:b])
            for net in sorted({f["net"] for f in native.get("fills", []) if f["layer"] == reference}):
                coverage = {}
                for member, tracks in members.items():
                    selected = [t for t in tracks if t["layer"] == signal]
                    if selected:
                        coverage[member] = reference_coverage({**native, "tracks": selected}, identifier=identifier,
                            net=member, reference_layer=reference, reference_net=net, margin_nm=0)
                entry["filled_nets"].append(dict(net=net, member_coverage=coverage))
            adjacent.append(entry)
    return check(identifier, "fail" if any(s["edge_gap_nm"] <= 0 for s in sections) else "incomplete",
                 "Actual pair geometry inventory only; no impedance, delay, topology or return-path approval",
                 evidence_grade="screening", maximum_search_gap_nm=maximum_search_gap_nm,
                 members=inventory, parallel_sections=sections, adjacent_copper=adjacent,
                 limitations=["Exactly parallel straight segment candidates only; endpoint/bend fields excluded",
                              "Multiple nearby candidates are not a unique electrical pair; interval union prevents double counting",
                              "Total track length is not a pin-to-pin path or propagation-delay model",
                              "Filled copper coverage does not prove electrical reference connectivity or transition return paths"])
