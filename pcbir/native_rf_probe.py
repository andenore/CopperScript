"""Read-only native RF land extraction; explicit KiCad Python, never refill/save."""
import json
import sys

from native_qualification_probe import extract


def extract_rf(filename):
    import pcbnew
    result = extract(filename)
    board = pcbnew.LoadBoard(filename)
    lands = []
    for footprint in board.GetFootprints():
        for pad in footprint.Pads():
            if not pad.GetNumber() or pad.GetAttribute() == pcbnew.PAD_ATTRIB_NPTH:
                continue
            copper = {}
            for name in result['layers']:
                layer = board.GetLayerID(name)
                if not pad.IsOnLayer(layer):
                    continue
                polygons = pad.GetEffectivePolygon(layer)
                rings = []
                for i in range(polygons.OutlineCount()):
                    def ring(chain):
                        if chain.ArcCount():
                            raise ValueError('Unsegmented pad arcs are unsupported')
                        return [[int(chain.CPoint(j).x), int(chain.CPoint(j).y)]
                                for j in range(chain.PointCount())]
                    rings.append({'outer': ring(polygons.COutline(i)),
                                  'holes': [ring(polygons.CHole(i, j))
                                            for j in range(polygons.HoleCount(i))]})
                copper[name] = rings
            lands.append({'component': footprint.GetReference(), 'pad': pad.GetNumber(),
                          'net': pad.GetNetname() or 'N/C',
                          'position': [int(pad.GetPosition().x), int(pad.GetPosition().y)],
                          'size_nm': [int(pad.GetSize().x), int(pad.GetSize().y)],
                          'drill_nm': [int(pad.GetDrillSize().x), int(pad.GetDrillSize().y)],
                          'copper': copper})
    result['lands'] = lands
    return result


if __name__ == '__main__':
    try:
        if len(sys.argv) != 2:
            raise ValueError('One explicit native PCB path required')
        print(json.dumps(extract_rf(sys.argv[1]), allow_nan=False))
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(2)
