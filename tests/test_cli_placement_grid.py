"""CLI sampling controls retain source gates and the original search limits."""
from dataclasses import replace
import json
import pytest
from pcbir import cli
from pcbir.placement import placement_solution_is_legal

COMMANDS=("plan-layout","route-global","route-board")


@pytest.mark.parametrize('command', COMMANDS)
def test_explicit_placement_work_budgets_preserve_other_options(command):
    parser = cli._parser()
    default = cli._planner_options(parser.parse_args([command, 'board.copper']))
    bounded = cli._planner_options(parser.parse_args([command, 'board.copper',
        '--placement-seeds', '1', '--placement-refinement-passes', '0', '--escape-spacing-passes', '0']))
    assert replace(bounded, seed_count=None, refinement_passes=2, escape_spacing_passes=2) == default


@pytest.mark.parametrize('command', COMMANDS)
@pytest.mark.parametrize('flag,value', [('--placement-seeds','0'), ('--placement-seeds','-1'),
    ('--placement-seeds','1.5'), ('--placement-refinement-passes','-1'), ('--escape-spacing-passes','-1')])
def test_invalid_placement_work_budgets_fail_before_source_loading(command, flag, value):
    with pytest.raises(SystemExit):
        cli._parser().parse_args([command, 'missing.copper', flag, value])


@pytest.mark.parametrize('profile,expected',[('quick',4),('balanced',16),('thorough',64)])
def test_routing_profiles_bound_decoupler_pose_work_and_respect_override(profile,expected):
    argv=['route-board','board.copper','--routing-profile',profile]
    args=cli._parser().parse_args(argv);cli._apply_routing_profile(args,argv)
    assert cli._planner_options(args).decoupling_placement_trials==expected
    argv+=['--decoupling-placement-trials','0']
    args=cli._parser().parse_args(argv);cli._apply_routing_profile(args,argv)
    assert cli._planner_options(args).decoupling_placement_trials==0

@pytest.mark.parametrize("command",COMMANDS)
def test_grid_controls_only_the_existing_sampling_parameter(command):
    parser=cli._parser()
    original=cli._planner_options(parser.parse_args([command,"board.copper"]))
    explicit=cli._planner_options(parser.parse_args([command,"board.copper","--placement-grid-mm","0.250001"]))
    assert original.grid_step_nm==1000000
    assert explicit.grid_step_nm==250001
    assert replace(explicit,grid_step_nm=original.grid_step_nm)==original
    assert explicit.legalization_candidates==96 and explicit.exact_repair_candidates==28
    assert explicit.exact_repair_limit==6 and not explicit.fixed_references

@pytest.mark.parametrize("command",COMMANDS)
@pytest.mark.parametrize("value",("0","-0.25","NaN","Infinity","0.0000001"))
def test_invalid_or_subnanometre_zero_sampling_is_rejected_before_loading(command,value):
    with pytest.raises(SystemExit):
        cli._parser().parse_args([command,"missing.copper","--placement-grid-mm",value])

def test_cli_uses_the_grid_in_the_real_planner_and_retains_readiness_gates(monkeypatch,tmp_path):
    observed=[];original=cli.plan_placement
    def run(board,options):
        result=original(board,options)
        assert placement_solution_is_legal(board,{p.reference:p for p in result.board.placements},options)
        observed.append(options.grid_step_nm)
        return result
    monkeypatch.setattr(cli,"plan_placement",run)
    report=tmp_path/"placement.json";pcb=tmp_path/"placed.kicad_pcb"
    assert cli.main(["plan-layout","examples/valid_board/board.copper","--allow-proxy-footprints",
                     "--placement-grid-mm","0.25","--candidates","1","-o",str(pcb),"--report",str(report)])==0
    assert observed==[250000] and pcb.is_file()
    document=json.loads(report.read_text(encoding="utf-8"))
    assert document["gates"][-1]["status"]=="blocked"
