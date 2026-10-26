

from pcbir import CopperLayer, CopperZone, Point, PolygonRing, PolygonWithHoles, PadReference


def zone(name, net, x1, x2):
    return CopperZone(name, net, (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(x1, 1), Point.mm(x2, 1), Point.mm(x2, 9), Point.mm(x1, 9)))))


def test_passive_feed_lowering_keeps_upstream_obligations_and_rejects_cycle():
    from test_passive_dc_power import compile_source, SOURCE
    from pcbir import prototype_physicalize
    source_with_footprints = SOURCE.replace('component F1: Feed;', 'component F1: Feed { footprint = "feed"; }').replace(
        'component F2: Feed;', 'component F2: Feed { footprint = "feed"; }').replace(
        'component U: Load;', 'component U: Load { footprint = "load"; }')
    source_with_footprints = source_with_footprints.replace(': Feed {', ': p.Feed {').replace(': Load {', ': p.Load {')
    physical = prototype_physicalize(compile_source(source_with_footprints))
    feeds = {f.net: f for f in physical.power_feeds}
    assert feeds['FILTERED'].terminal == PadReference('F2', '2')
    assert feeds['FILTERED'].upstream_net == 'MID'
    assert feeds['MID'].upstream_net == 'SOURCE'
    assert all(not d.sources for d in physical.power_domains)
    source = source_with_footprints.replace('supply SOURCE { voltage = 1.8 V; external = true; }', '')
    assert not prototype_physicalize(compile_source(source)).power_feeds
