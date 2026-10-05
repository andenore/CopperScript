"""Operational events cannot change geometry, fingerprints or closure gates."""
import json
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest
import pcbir.detailed as detailed_module
from pcbir.cli import main, _parser
from pcbir.detailed import DetailedRouterOptions, route_detailed
from pcbir.flow import run_routing_pipeline
from pcbir.fanout import FanoutOptions
from pcbir.physical import (BoardOutline, FootprintPad, PadReference, PhysicalBoard,
    PhysicalFootprint, PhysicalNet, Placement, Point, Size, NetRoutingRule, RouteKind)
from pcbir.placement import PlacementPlannerOptions
from pcbir.progress import console_progress, critical_progress, emit
from pcbir.routing import route_global


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
    assert phases.index(('ordinary_package_exits', 'finished')) < phases.index(('ordinary_area', 'started'))
    assert phases[-1] == ('land_closure_native_drc', 'finished')
    assert next(d for p, e, d in events if p == 'package_access' and e == 'finished')['ready']
    assert ('detailed_net', 'started') in phases


def test_progress_does_not_report_area_started_when_package_gate_blocks(monkeypatch):
    import pcbir.package_access as access
    from dataclasses import replace
    real = access.route_fanout
    def pending(source, settings):
        result = real(source, settings)
        return replace(result, pending_pads=(PadReference('J1', '1'),))
    monkeypatch.setattr(access, 'route_fanout', pending)
    events = []
    result = run_routing_pipeline(board(),
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0),
        fanout_options=FanoutOptions(), package_access_options=access.PackageAccessOptions(maximum_trials=0),
        on_progress=lambda phase, event, details: events.append((phase, event, details)))
    assert result.detailed.metrics.passes == 0
    assert ('ordinary_area', 'blocked') in [(p, e) for p, e, _ in events]
    assert ('ordinary_area', 'started') not in [(p, e) for p, e, _ in events]
    assert not any(p == 'detailed_net' for p, _, _ in events)


def test_net_progress_starts_before_grid_and_preserves_exact_result(monkeypatch):
    source = board()
    guide = route_global(source)
    options = DetailedRouterOptions(maximum_passes=1)
    plain = route_detailed(source, guide, options)
    events = []
    original_grid = detailed_module._build_grid
    def checked_grid(*args, **kwargs):
        assert events[-1][0:2] == ('detailed_net', 'started')
        return original_grid(*args, **kwargs)
    monkeypatch.setattr(detailed_module, '_build_grid', checked_grid)
    observed = route_detailed(source, guide, options,
        on_progress=lambda p, e, d: events.append((p, e, d)))
    assert observed == plain and observed.to_json() == plain.to_json()
    assert [(p, e) for p, e, _ in events] == [
        ('detailed_pass', 'started'), ('detailed_net', 'started'),
        ('detailed_net', 'finished'), ('detailed_pass', 'finished')]
    start, finish = events[1][2], events[2][2]
    assert start['net'] == finish['net'] == 'SIGNAL'
    assert start['net_index'] == start['net_count'] == start['pass_index'] == 1
    assert start['maximum_search_states'] == options.maximum_search_states
    assert finish['connected'] and finish['track_count'] == observed.nets[0].track_count
    assert 'fabrication_ready' not in finish


def test_failed_and_discarded_repair_attempts_are_observed_without_extra_searches(monkeypatch):
    source = board()
    guide = route_global(source)
    options = DetailedRouterOptions(maximum_passes=3, maximum_search_states=11,
        repair_budget_multiplier=3, enable_soft_ripup=True)
    searches = []
    def failed(*args, **kwargs):
        searches.append((args[2], args[9], kwargs.get('allow_movable_conflicts')))
        return detailed_module._failed(args[2], 'search budget exhausted')
    monkeypatch.setattr(detailed_module, '_route_net', failed)
    plain = route_detailed(source, guide, options)
    plain_searches = searches[:]
    searches.clear()
    events = []
    observed = route_detailed(source, guide, options,
        on_progress=lambda p, e, d: events.append((p, e, d)))
    assert observed == plain and searches == plain_searches
    starts = [d for p, e, d in events if (p, e) == ('detailed_net', 'started')]
    finishes = [d for p, e, d in events if (p, e) == ('detailed_net', 'finished')]
    assert [d['stage'] for d in starts] == [
        'pass', 'pass', 'failed_first', 'soft_merge', 'soft_ripup', 'final_retry']
    assert len(finishes) == len(starts) and not any(d['connected'] for d in finishes)
    assert [d['maximum_search_states'] for d in starts] == [11] * 5 + [33]
    assert [d['allow_movable_conflicts'] for d in starts] == [False] * 3 + [True, True, False]
    assert next(d for p, e, d in events if (p, e) == ('detailed_repair', 'finished'))['selected'] is False


def test_interrupted_grid_build_does_not_invent_a_finished_net(monkeypatch):
    source = board()
    guide = route_global(source)
    events = []
    def interrupted(*args, **kwargs):
        raise KeyboardInterrupt
    monkeypatch.setattr(detailed_module, '_build_grid', interrupted)
    with pytest.raises(KeyboardInterrupt):
        route_detailed(source, guide, on_progress=lambda p, e, d: events.append((p, e, d)))
    assert [(p, e) for p, e, _ in events] == [
        ('detailed_pass', 'started'), ('detailed_net', 'started')]


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
    assert plain_exit == observed_exit
    assert plain_report.read_bytes() == observed_report.read_bytes()
    assert plain_pcb.read_bytes() == observed_pcb.read_bytes()
    assert plain_pcb.with_suffix('.kicad_pro').read_bytes() == observed_pcb.with_suffix('.kicad_pro').read_bytes()
    assert {path.relative_to(plain_pcb.parent): path.read_bytes()
            for path in plain_pcb.parent.rglob('*') if path.is_file()} == {
        path.relative_to(observed_pcb.parent): path.read_bytes()
        for path in observed_pcb.parent.rglob('*') if path.is_file()}
