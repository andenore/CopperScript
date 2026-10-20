import json
import subprocess
import sys

import pytest

from pcbir import compile_source, check
from pcbir.agent_guide import TOPICS, SYNTAX_EXAMPLE, get_guide, render_guide
from pcbir.cli import _parser, main


def test_guide_works_without_a_project_and_does_not_create_files(tmp_path):
    result = subprocess.run([sys.executable, "-m", "copperscript", "agent-guide"],
                            cwd=tmp_path, text=True, capture_output=True, check=False)
    assert result.returncode == 0
    assert "CopperScript agent guide v1 / start" in result.stdout
    assert "copper library search" in result.stdout
    assert "copper agent-guide TOPIC" in result.stdout
    assert len(result.stdout) < 3000
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize("topic", TOPICS)
def test_topic_output_is_compact_and_command_templates_match_current_cli(topic, capsys):
    assert main(["agent-guide", topic, "--json"]) == 0
    output = capsys.readouterr().out
    guide = json.loads(output)
    assert guide["schema"] == "copperscript.agent-guide/v1"
    assert guide["guide_version"] == 1 and guide["topic"] == topic
    assert guide["content_digest"].startswith("sha256:")
    assert len(output) < 6500
    parser = _parser()
    for command in guide["commands"]:
        # Validate actual command names/options without executing build templates.
        try:
            parser.parse_args(command[1:])
        except SystemExit as exc:
            assert exc.code == 0 and "--help" in command


def test_syntax_example_compiles_and_passes_erc_without_dependencies():
    board = compile_source(SYNTAX_EXAMPLE)
    assert check(board) == []
    assert {c.ref for c in board.components} == {"R1", "R2"}
    assert board.dependencies == ()


def test_unknown_topic_is_a_clear_cli_error(capsys):
    with pytest.raises(SystemExit) as error:
        main(["agent-guide", "unknown"])
    assert error.value.code == 2
    assert "invalid choice" in capsys.readouterr().err
    with pytest.raises(ValueError, match="unknown guide topic"):
        get_guide("unknown")


def test_topic_navigation_is_available_in_text_and_json():
    guide = get_guide()
    assert set(guide["topics"]) == {"start", "syntax", "libraries", "verification", "iteration", "recovery"}
    for topic in guide["topics"]:
        assert topic in render_guide(guide)
    assert get_guide("libraries")["content_digest"] != guide["content_digest"]
