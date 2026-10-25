"""Run discovery selects routing evidence rather than adjacent exporter metadata."""
import json
import pytest
from pcbir.workbench import load_run


@pytest.mark.parametrize('filename',('route.json','instrument-routing.json'))
def test_directory_discovers_report_with_schema_after_long_sorted_fields(tmp_path,filename):
    (tmp_path/'board.via-process.json').write_text(json.dumps({'schema':'copperscript-via-process/v1'}))
    (tmp_path/'board.editor-intent.json').write_text(json.dumps({'schema':'copperscript-editor-intent/v1'}))
    (tmp_path/'placement.state.json').write_text(json.dumps({'schema':'copperscript-state/v1'}))
    report=tmp_path/filename
    report.write_text(json.dumps({'connectivity':{'unrouted_ordinary_nets':['long-net-name']*1000},
        'schema':'copperscript-route-board/v0.1','status':'fail'},sort_keys=True))
    (tmp_path/(report.stem+'.summary.json')).write_text(json.dumps({'schema':'copperscript-route-board/v0.1'}))
    run=load_run(tmp_path)
    assert run.report_path==report and run.report['status']=='fail'


def test_export_metadata_alone_is_not_a_route_report(tmp_path):
    (tmp_path/'board.via-process.json').write_text(json.dumps({'schema':'copperscript-via-process/v1'}))
    (tmp_path/'unrelated.json').write_text('[]')
    (tmp_path/'invalid.json').write_text('{')
    assert load_run(tmp_path).report_path is None
