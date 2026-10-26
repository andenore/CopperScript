"""Operational events cannot change geometry, fingerprints or closure gates."""
import json
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

from pcbir.cli import main, _parser
from pcbir.flow import run_routing_pipeline
from pcbir.fanout import FanoutOptions
from pcbir.physical import (BoardOutline, FootprintPad, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, Size, NetRoutingRule, RouteKind)
from pcbir.placement import PlacementPlannerOptions
from pcbir.progress import console_progress, critical_progress, emit


def board():
    fp = PhysicalFootprint('one', (FootprintPad('1', Point(0, 0), Size.mm(.6, .6)),), Size.mm(1, 1))
    return PhysicalBoard('progress', BoardOutline.rectangle(12, 12), {fp.name: fp},
        (Placement('J1', fp.name, Point.mm(3, 6)), Placement('J2', fp.name, Point.mm(9, 6))),
        (PhysicalNet('SIGNAL', (PadReference('J1', '1'), PadReference('J2', '1'))),))


def test_console_events_have_monotonic_elapsed_time_and_json_details(capsys):
    times = iter((10., 10.125, 11.5))
    observe = console_progress(clock=lambda: next(times))
    emit(observe, 'ordinary_area', 'started')
    emit(observe, 'ordinary_area', 'finished', failed_nets=['A'], passes=2)
    events = [json.loads(line.removeprefix('PROGRESS ')) for line in capsys.readouterr().out.splitlines()]
    assert [e['elapsed_seconds'] for e in events] == [.125, 1.5]
    assert events[1]['details'] == {'failed_nets': ['A'], 'passes': 2}


def test_optional_critical_adapter_exposes_group_results_not_signoff():
    assert critical_progress(None) is None
    events = []
    observe = critical_progress(lambda phase, event, details: events.append((phase, event, details)))
    observe('started', ('A', 'B'), None)
    observe('finished', ('A', 'B'), SimpleNamespace(connected=False, strategy='joint_pair_search', search_states=12))
    assert events[0] == ('critical_group', 'started', {'nets': ['A', 'B']})
    assert events[1][2]['connected'] is False
    assert 'fabrication_ready' not in events[1][2]


def test_pipeline_progress_leaves_every_result_and_fingerprint_identical():
    options = PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0)
    plain = run_routing_pipeline(board(), placement_options=options, fanout_options=FanoutOptions())
    events = []
    observed = run_routing_pipeline(board(), placement_options=options, fanout_options=FanoutOptions(),
        on_progress=lambda phase, event, details: events.append((phase, event, details)))
    assert observed == plain
    phases = [(phase, event) for phase, event, _ in events]
    assert phases.index(('placement_candidates', 'finished')) < phases.index(('global_graph', 'started'))
    assert phases.index(('placement_global_certification', 'finished')) < phases.index(('placement_global', 'finished'))
    assert phases.index(('ordinary_escapes', 'started')) < phases.index(('ordinary_area', 'started'))
    assert phases[-1] == ('land_closure_native_drc', 'finished')


def test_real_critical_group_progress_is_forwarded_before_area_routing():
    from dataclasses import replace
    source = replace(board(), net_routing_rules=(NetRoutingRule('SIGNAL', RouteKind.CRITICAL, max_vias=0),))
    events = []
    result = run_routing_pipeline(source,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0),
        fanout_options=FanoutOptions(),
        on_progress=lambda phase, event, details: events.append((phase, event, details)))
    groups = [(event, details) for phase, event, details in events if phase == 'critical_group']
    assert [event for event, _ in groups] == ['started', 'finished']
    assert groups[1][1]['nets'] == ['SIGNAL'] and groups[1][1]['connected']
    assert result.critical.nets[0].connected


def test_cli_progress_is_opt_in_and_does_not_change_exports(tmp_path, capsys):
    assert not _parser().parse_args(['route-board', 'example.copper']).progress
    root = Path(__file__).resolve().parents[1]
    command = ['route-board', str(root/'examples/valid_board/board.copper'), '--allow-proxy-footprints',
               '--candidates', '1', '--zone-escape-trials', '0', '--zone-local-ripup-trials', '0']
    plain_report, observed_report = tmp_path/'plain.json', tmp_path/'observed.json'
    plain_pcb, observed_pcb = tmp_path/'plain'/'board.kicad_pcb', tmp_path/'observed'/'board.kicad_pcb'
    plain_exit = main([*command, '--report', str(plain_report), '-o', str(plain_pcb)])
    assert 'PROGRESS ' not in capsys.readouterr().out
    observed_exit = main([*command, '--progress', '--report', str(observed_report), '-o', str(observed_pcb)])
    output = capsys.readouterr().out
    assert 'PROGRESS ' in output and '"phase": "export"' in output
    assert '"phase": "placement_seed"' in output and '"phase": "global_iteration"' in output
    assert plain_exit == observed_exit
    assert plain_report.read_bytes() == observed_report.read_bytes()
    assert plain_pcb.read_bytes() == observed_pcb.read_bytes()
    assert plain_pcb.with_suffix('.kicad_pro').read_bytes() == observed_pcb.with_suffix('.kicad_pro').read_bytes()
    assert {path.relative_to(plain_pcb.parent): path.read_bytes()
            for path in plain_pcb.parent.rglob('*') if path.is_file()} == {
        path.relative_to(observed_pcb.parent): path.read_bytes()
        for path in observed_pcb.parent.rglob('*') if path.is_file()}
