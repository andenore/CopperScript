"""Ordinary escape proposals cannot invalidate a complete bypass cohort."""
from dataclasses import replace
import pytest
from pcbir.fanout import route_fanout
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.bypass_links import verified_bypass_links as _verified_bypass_links
from pcbir.physical import FootprintPad, PadReference, Point, Size
from test_decoupling_crosslayer_paths import fixture

@pytest.mark.parametrize("renamed",[False,True])
def test_ordinary_escape_cannot_spend_prior_crosslayer_supply_via_budget(renamed):
    source,link=fixture(renamed=renamed)
    chip=replace(source.footprints["chip"],pads=(*source.footprints["chip"].pads,
        FootprintPad("X",Point.mm(1,-.8),Size.mm(.4,.4))))
    extra=PadReference(link.target.component,"X")
    source=replace(source,footprints={**source.footprints,"chip":chip},
        nets=tuple(replace(n,pads=(*n.pads,extra)) if n.name==link.net else n for n in source.nets),
        net_routing_rules=tuple(replace(r,max_vias=1) if r.net==link.net else r for r in source.net_routing_rules))
    result=route_fanout(source)
    assert extra in result.pending_pads and not result.decoupling_pending
    assert sum(v.net==link.net for v in result.created_vias)==1
    assert _verified_bypass_links(result.board,{link.net})==frozenset({link})
    assert verified_bypass_reservations(source,result)[2] is None
    assert result.created_vias==result.decoupling_vias
    assert result.created_tracks==result.decoupling_tracks
