"""Read-only KiCad geometry probe, run by KiCad's own Python (stdlib + pcbnew).

Never loads CopperScript packages into the native interpreter. Never fills,
modifies or saves a board. Zone outlines are deliberately not exported.
"""
import json
import sys


def extract(filename):
    import pcbnew
    board = pcbnew.LoadBoard(filename)
    if board is None:
        raise ValueError("KiCad could not load the board")
    layers = list(board.GetEnabledLayers().CuStack())
    layer = board.GetLayerName
    point = lambda p: [int(p.x), int(p.y)]
    poses = [{"reference": f.GetReference(), "position": point(f.GetPosition()),
              "rotation": str(f.GetOrientationDegrees()),
              "side": "back" if f.IsFlipped() else "front", "library_id": f.GetFPIDAsString()}
             for f in board.GetFootprints()]
    tracks, vias = [], []
    for t in board.GetTracks():
        if isinstance(t, pcbnew.PCB_VIA):
            vias.append({"net": t.GetNetname(), "position": point(t.GetPosition()),
                         "size_nm": t.GetWidth(), "drill_nm": t.GetDrillValue(),
                         "from_layer": layer(t.TopLayer()), "to_layer": layer(t.BottomLayer())})
        elif isinstance(t, pcbnew.PCB_ARC):
            raise ValueError("arc tracks are not supported by the routed overlay yet")
        elif isinstance(t, pcbnew.PCB_TRACK):
            tracks.append({"net": t.GetNetname(), "start": point(t.GetStart()), "end": point(t.GetEnd()),
                           "width_nm": t.GetWidth(), "layer": layer(t.GetLayer())})
        else:
            raise ValueError("unsupported native copper object")
    fills = []

    def ring(chain):
        if chain.ArcCount():
            raise ValueError("native fill contains unsegmented arcs")
        return [point(chain.CPoint(i)) for i in range(chain.PointCount())]

    for z in board.Zones():
        if z.GetIsRuleArea() or not z.IsFilled():
            continue
        for l in layers:
            if not z.HasFilledPolysForLayer(l):
                continue
            polygons = z.GetFilledPolysList(l)
            for i in range(polygons.OutlineCount()):
                fills.append({"net": z.GetNetname(), "layer": layer(l),
                              "outer": ring(polygons.COutline(i)),
                              "holes": [ring(polygons.CHole(i, h)) for h in range(polygons.HoleCount(i))]})
    return {"kicad_version": pcbnew.Version(), "layers": [layer(l) for l in layers],
            "poses": poses, "tracks": tracks, "vias": vias, "fills": fills}


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise ValueError("one explicit board filename is required")
        print(json.dumps(extract(sys.argv[1]), allow_nan=False))
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(2)
