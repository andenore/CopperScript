"""Retained exact feeds keep placement legal with bounded incremental work."""
from dataclasses import replace
from pathlib import Path
import sys,types
from pcbir.physical import BoardOutline, DecouplingLink, PadReference, PhysicalNet, Placement, Point
from pcbir.decoupling import improve_decoupling_placement, route_decouplers
from pcbir.placement import PlacementPlannerOptions, placement_solution_is_legal
from test_routing_quality import bypass_board

def fixture():
    base=bypass_board()
    poses=[]; links=[]; nets=[]
    for i in range(4):
        u,c=f'U{i}',f'C{i}'
        poses.extend((Placement(u,'chip',Point.mm(8+15*i,8)),Placement(c,'cap',Point.mm(15+15*i,15))))
        link=DecouplingLink(PadReference(c,'1'),PadReference(u,'1'),PadReference(c,'2'),f'P{i}',f'G{i}')
        links.append(link)
        nets.extend((PhysicalNet(f'P{i}',(link.target,link.capacitor)),PhysicalNet(f'G{i}',(link.return_pad,))))
    return replace(base,outline=BoardOutline.rectangle(80,25),placements=tuple(poses),nets=tuple(nets),decoupling_links=tuple(links))

def test_multiple_capacitors_match_baseline_and_keep_every_feed_legal():
    path=Path(__file__).parents[1]/'build/decoupling-before.py'
    if not path.is_file():
        import subprocess
        source=subprocess.check_output(['git','show','976adb5c51d234a02628e15a724f34cf5c797048:pcbir/decoupling.py'],cwd=path.parents[1]).decode()
    else: source=path.read_text()
    module=types.ModuleType('pcbir._decoupling_comparison'); module.__package__='pcbir'
    sys.modules[module.__name__]=module
    try:
        exec(compile(source,str(path),'exec'),module.__dict__)
        board=fixture(); poses={p.reference:p for p in board.placements}
        options=PlacementPlannerOptions(component_clearance_nm=200000)
        result=improve_decoupling_placement(board,poses,options)
        assert result==module.improve_decoupling_placement(board,poses,options)
        assert result==improve_decoupling_placement(board,poses,options)
        assert placement_solution_is_legal(board,result[0],options)
        feeds=route_decouplers(replace(board,placements=tuple(result[0].values())))
        assert not feeds.pending
        assert len(feeds.connected_pads)==8
    finally: sys.modules.pop(module.__name__,None)
