import json
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).parents[1]
sys.path.insert(0, str(SCRIPTS))
import run  # noqa: E402

BOX = [{"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "body_name": "Cube"}}]


def report(tmp_path, commands, *args):
    script = tmp_path / "script.json"
    script.write_text(json.dumps({"commands": commands}))
    return subprocess.run(
        [sys.executable, SCRIPTS / "run.py", script, *args], capture_output=True, text=True
    )


def test_reports_bodies(tmp_path):
    result = report(tmp_path, BOX)
    assert result.returncode == 0, result.stderr
    assert "BODY Cube: volume 1000 mm3, area 600 mm2, bbox (0, 0, 0)..(10, 10, 10), com (5, 5, 5)" in result.stdout


def test_flags_underconstrained_sketch(tmp_path):
    result = report(tmp_path, [
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Loose"}},
        {"command": "sketch.circle.center", "params": {"center": [5, 5], "radius": 5}},
        {"command": "sketch.finish", "params": {}},
    ])
    assert result.returncode == 0, result.stderr
    assert 'SKETCH "Loose": dof 3, solved' in result.stdout


def test_reports_step_warnings(tmp_path):
    result = report(tmp_path, [
        {"command": "parameters.add", "params": {"name": "h", "expression": "5 mm"}},
        {"command": "solid.box", "params": {"length": 91, "width": 62, "height": 30, "center": [0, 0, 15], "body_name": "Case"}},
        {"command": "solid.fillet", "params": {"edges": [[45.5, 31, 15], [-45.5, 31, 15], [45.5, -31, 15], [-45.5, -31, 15]], "radius": 3}},
        {"command": "solid.shell", "params": {"faces": [[0, 0, 30]], "thickness": 2}},
        {"command": "solid.fillet", "params": {"edges": [[0, -31, 30]], "radius": 1}},
        {"command": "solid.cylinder", "params": {"diameter": "6 mm", "height": "h", "base": [-39, -24.5, 2], "operation": "join", "targets": ["Case"], "name": "Standoff"}},
        {"command": "solid.cylinder", "params": {"diameter": "2.5 mm", "height": "h", "base": [-39, -24.5, 2], "operation": "cut", "targets": ["Case"], "name": "Pilot"}},
    ])
    assert result.returncode == 0, result.stderr
    assert 'WARN "Pilot": Boolean retry' in result.stdout


def test_fails_on_a_bad_step(tmp_path):
    result = report(tmp_path, [{"command": "solid.box", "params": {"width": 10}}])
    assert result.returncode == 1
    assert result.stdout.startswith("FAIL") and "step 1 (solid.box)" in result.stdout


def test_applies_a_script_to_a_saved_design(tmp_path):
    design = tmp_path / "cube.json"
    built = report(tmp_path, [
        {"command": "parameters.add", "params": {"name": "size", "expression": "10 mm"}},
        {"command": "solid.box", "params": {"length": "size", "width": "size", "height": "size", "body_name": "Cube"}},
    ], "--save", design)
    assert built.returncode == 0, built.stderr

    changed = report(tmp_path, [
        {"command": "parameters.change", "params": {"name": "size", "expression": "20 mm"}},
    ], "--in", design)
    assert changed.returncode == 0, changed.stderr
    assert "BODY Cube: volume 8000 mm3" in changed.stdout


def test_execute_returns_the_final_inspection():
    inspection = run.execute(BOX)
    assert [b["name"] for b in inspection["bodies"]] == ["Cube"]


def test_execute_raises_on_failure():
    with pytest.raises(run.SolidCraftError, match="step 1"):
        run.execute([{"command": "solid.box", "params": {"width": 10}}])


@pytest.fixture
def flush_design(tmp_path):
    """A shelled case with a joined standoff and a cut pilot hole; Pilot warns tool displacement."""
    design = tmp_path / "standoff.json"
    built = report(tmp_path, [
        {"command": "parameters.add", "params": {"name": "h", "expression": "5 mm"}},
        {"command": "solid.box", "params": {"length": 91, "width": 62, "height": 30, "center": [0, 0, 15], "body_name": "Case"}},
        {"command": "solid.fillet", "params": {"edges": [[45.5, 31, 15], [-45.5, 31, 15], [45.5, -31, 15], [-45.5, -31, 15]], "radius": 3}},
        {"command": "solid.shell", "params": {"faces": [[0, 0, 30]], "thickness": 2}},
        {"command": "solid.fillet", "params": {"edges": [[0, -31, 30]], "radius": 1}},
        {"command": "solid.cylinder", "params": {"diameter": "6 mm", "height": "h", "base": [-39, -24.5, 2], "operation": "join", "targets": ["Case"], "name": "Standoff"}},
        {"command": "solid.cylinder", "params": {"diameter": "2.5 mm", "height": "h", "base": [-39, -24.5, 2], "operation": "cut", "targets": ["Case"], "name": "Pilot"}},
    ], "--save", design)
    assert built.returncode == 0, built.stdout + built.stderr
    return design


def depth(value: str) -> dict:
    return {"command": "parameters.change", "params": {"name": "h", "expression": value}}


def test_reports_warnings_after_a_parameter_change(tmp_path, flush_design):
    flush = report(tmp_path, [depth("7 mm")], "--in", flush_design)
    assert flush.returncode == 0, flush.stderr
    assert 'WARN "Pilot": Boolean retry' in flush.stdout


def test_reports_every_inspection_in_the_script(tmp_path, flush_design):
    walk = report(tmp_path, [depth("7 mm"), {"command": "document.inspect", "params": {"measure": True}},
                             depth("5 mm")], "--in", flush_design)
    lines = walk.stdout.splitlines()
    assert any(line.startswith('step 2 WARN "Pilot": Boolean retry') for line in lines)
    assert any(line.startswith("step 2 BODY Case: volume") for line in lines)
    assert any(line.startswith('WARN "Pilot": Boolean retry') for line in lines)


def test_reports_bodies_inspected_without_measurements(tmp_path):
    result = report(tmp_path, [*BOX, {"command": "document.inspect", "params": {}}])
    assert "step 2 BODY Cube\n" in result.stdout
    assert "BODY Cube: volume 1000 mm3" in result.stdout


def test_says_so_when_there_is_nothing_to_report(tmp_path):
    result = report(tmp_path, [{"command": "parameters.add", "params": {"name": "size", "expression": "10 mm"}}])
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "no bodies"
