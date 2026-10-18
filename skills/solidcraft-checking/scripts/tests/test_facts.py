"""SolidCraft behaviour that SKILL.md states, and the facts behind the corrections in
solidcraft-modeling. A failure here means the skill text is out of date."""

import json
import subprocess
import sys
from math import cos, dist, pi, radians, sin
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))
from run import SolidCraftError, execute, run_commands  # noqa: E402


def cmd(command: str, /, **params) -> dict:
    return {"command": command, "params": params}


def bbox(body: dict) -> tuple:
    return (*body["bbox"]["min"], *body["bbox"]["max"])


def sketch_dof(commands: list) -> int:
    return execute(commands)["sketches"][-1]["dof"]


def warnings(commands: list) -> list:
    return [f["warning"] for f in execute(commands)["timeline"] if f.get("warning")]


def volumes(inspection: dict) -> dict:
    return {b["name"]: b["volume_mm3"] for b in inspection["bodies"]}


BLOCK = [cmd("solid.box", length=40, width=40, height=12, body_name="B", name="Block")]


def tapered_cut(start: str, distance: str, taper: str = "10 deg") -> list:
    return [
        cmd("sketch.create", plane="XY", offset=start, name="Cut sketch"),
        cmd("sketch.circle.center", center=[20, 20], diameter=20),
        cmd("sketch.finish"),
        cmd("solid.extrude", sketch="Cut sketch", distance=distance, taper=taper, operation="cut", targets=["B"],
            name="Cut"),
    ]


def holes(count: str) -> list:
    return [
        cmd("parameters.add", name="n", expression=count, unit=""),
        *BLOCK,
        cmd("solid.cylinder", diameter="4 mm", height="20 mm", base=["5 mm", "5 mm", "-1 mm"], operation="cut",
            targets=["B"], name="Hole"),
        cmd("solid.pattern.rectangular", features=["Hole"], dir1=[1, 0, 0], count1="n", spacing1="10 mm",
            name="Row"),
    ]


# Traps

def test_a_dimension_driven_to_zero_can_break_the_next_change():
    with pytest.raises(SolidCraftError, match="would conflict"):
        execute([
            cmd("parameters.add", name="y", expression="5 mm"),
            cmd("sketch.create", plane="XY", name="Pin sketch"),
            cmd("sketch.circle.center", center=[-10, -5], radius=2),
            cmd("sketch.dimension", entities=["origin", "c1.center"], type="horizontal", value="10 mm"),
            cmd("sketch.dimension", entities=["origin", "c1.center"], type="vertical", value="y"),
            cmd("sketch.dimension", entities=["c1"], type="radius", value="2 mm"),
            cmd("sketch.finish"),
            cmd("solid.extrude", sketch="Pin sketch", distance="1 mm"),
            cmd("parameters.change", name="y", expression="0 mm"),
            cmd("parameters.change", name="y", expression="5 mm"),
        ])


def test_a_slot_held_by_other_constraints_can_cross_to_the_other_side_at_zero():
    body = execute([
        cmd("parameters.add", name="n", expression="3", unit=""),
        cmd("sketch.create", plane="XY", name="Slot sketch"),
        cmd("sketch.slot.overall", p0=[-6, -15], p1=[-6, 15], width=2),
        cmd("sketch.constraint.symmetry", a="a1.center", b="a2.center", line="x"),
        cmd("sketch.dimension", entities=["a1"], type="radius", value="1 mm"),
        cmd("sketch.dimension", entities=["a1.center", "a2.center"], type="vertical", value="28 mm"),
        cmd("sketch.dimension", entities=["origin", "a1.center"], type="horizontal", value="(n - 1) * 3 mm"),
        cmd("sketch.finish"),
        cmd("solid.extrude", sketch="Slot sketch", distance="1 mm", body_name="Slot"),
        cmd("parameters.change", name="n", expression="1"),
        cmd("parameters.change", name="n", expression="3"),
    ])["bodies"][0]
    # Started at x = -6; after n 3 -> 1 -> 3 it sits at +6, with no error
    assert (body["bbox"]["min"][0] + body["bbox"]["max"][0]) / 2 == pytest.approx(6, abs=1e-3)


def centred_row(position: list) -> list:
    """A row of n holes centred on x = 0, its first hole placed by `position`, walked n 3 -> 1 -> 3."""
    return [
        cmd("parameters.add", name="n", expression="3", unit=""),
        cmd("solid.box", length=100, width=40, height=4, center=[0, 0, 2], body_name="Plate"),
        cmd("sketch.create", plane="XY", offset="4 mm", name="Hole sketch"),
        cmd("sketch.circle.center", center=[-10, 0], radius=1),
        cmd("sketch.dimension", entities=["c1"], type="diameter", value="2 mm"),
        *position,
        cmd("sketch.constraint.horizontal_vertical", points=["origin", "c1.center"], mode="horizontal"),
        cmd("sketch.finish"),
        cmd("solid.extrude", sketch="Hole sketch", distance="6 mm", direction="negative", operation="cut",
            targets=["Plate"], name="Hole"),
        cmd("solid.pattern.rectangular", features=["Hole"], dir1=[1, 0, 0], count1="n", spacing1="10 mm"),
        cmd("parameters.change", name="n", expression="1"),
        cmd("parameters.change", name="n", expression="3"),
    ]


def test_a_position_measured_from_an_anchor_point_survives_reaching_zero():
    with pytest.raises(SolidCraftError, match="would conflict"):
        execute(centred_row([
            cmd("sketch.dimension", entities=["origin", "c1.center"], type="horizontal", value="(n - 1) / 2 * 10 mm"),
        ]))
    body = execute(centred_row([
        cmd("sketch.point", point=[-100, 0]),
        cmd("sketch.dimension", entities=["origin", "p1"], type="horizontal", value="100 mm"),
        cmd("sketch.constraint.horizontal_vertical", points=["origin", "p1"], mode="horizontal"),
        cmd("sketch.dimension", entities=["p1", "c1.center"], type="horizontal", value="100 mm - (n - 1) / 2 * 10 mm"),
    ]))["bodies"][0]
    assert body["volume_mm3"] == pytest.approx(100 * 40 * 4 - 3 * pi * 4, rel=2e-4)


def test_pattern_counts_run_from_1_to_1000_and_a_range_rule_does_not_stop_0():
    with pytest.raises(SolidCraftError, match="pattern count must be 1…1000"):
        execute([
            *holes("2"),
            cmd("config.configure", params=["n"]),
            cmd("config.rule", rule={"id": "n-range", "name": "n", "condition": {"type": "always"},
                                     "effect": {"type": "parameter_range", "parameter": "n", "minimum": "1",
                                                "maximum": "9"}}),
            cmd("parameters.change", name="n", expression="0"),
        ])


def test_a_fractional_count_rounds_to_the_nearest_whole_number():
    def copies(n):
        return len(execute([
            cmd("parameters.add", name="n", expression=n, unit=""),
            cmd("solid.cylinder", diameter="2 mm", height="2 mm", operation="new", body_name="Pin", name="Pin"),
            cmd("solid.pattern.rectangular", bodies=["Pin"], dir1=[1, 0, 0], count1="n", spacing1="10 mm"),
        ])["bodies"])

    assert (copies("6.4"), copies("6.5")) == (6, 7)


HEXAGON = [
    cmd("sketch.create", plane="XY"),
    cmd("sketch.polygon.circumscribed", center=[0, 0], radius=10, sides=6, angle=0),
    cmd("sketch.dimension", entities=["c1"], type="diameter", value="20 mm"),
    cmd("sketch.constraint.horizontal_vertical", line="l3", mode="vertical"),
]


def test_a_circumscribed_polygon_needs_the_polygon_constraint():
    assert sketch_dof(HEXAGON) == 1
    assert sketch_dof([*HEXAGON, cmd("sketch.constraint.polygon", lines=[f"l{i}" for i in range(1, 7)])]) == 0


def test_round_features_measure_about_1e_4_off():
    body = execute([cmd("solid.cylinder", diameter="20 mm", height="10 mm")])["bodies"][0]
    error = abs(body["volume_mm3"] / (pi * 100 * 10) - 1)
    assert 1e-7 < error < 1.5e-4


def test_shallow_arcs_measure_much_worse_than_round_features():
    radius, span = 1000, radians(20)
    half = radius * sin(span / 2)
    rise = radius - radius * cos(span / 2)
    body = execute([
        cmd("sketch.create", plane="XY", name="Segment sketch"),
        cmd("sketch.arc.three_point", start=[-half, 0], through=[0, rise], end=[half, 0]),
        cmd("sketch.line", points=[[-half, 0], [half, 0]]),
        cmd("sketch.finish"),
        cmd("solid.extrude", sketch="Segment sketch", distance="10 mm"),
    ])["bodies"][0]
    exact = radius ** 2 / 2 * (span - sin(span)) * 10
    assert 2e-4 < abs(body["volume_mm3"] / exact - 1) < 5e-3


def test_centre_of_mass_and_bounding_box_measure_within_5e_5_of_the_part_size():
    radius = 100
    body = execute([
        cmd("sketch.create", plane="XY", name="Half disc sketch"),
        cmd("sketch.arc.three_point", start=[-radius, 0], through=[0, radius], end=[radius, 0]),
        cmd("sketch.line", points=[[-radius, 0], [radius, 0]]),
        cmd("sketch.finish"),
        cmd("solid.extrude", sketch="Half disc sketch", distance="10 mm"),
    ])["bodies"][0]
    exact_box = (-radius, 0, 0, radius, radius, 10)
    size = dist(exact_box[:3], exact_box[3:])
    assert max(abs(a - b) for a, b in zip(bbox(body), exact_box)) < 5e-5 * size
    assert abs(body["center_of_mass"][1] - 4 * radius / (3 * pi)) < 5e-5 * size


def test_expressions_have_no_conditionals_and_sign_of_zero_is_one():
    def evaluate(expression):
        done = subprocess.run(
            ["solidcraft-cli", "exec", "expr.evaluate", json.dumps({"expression": expression, "kind": "unitless"})],
            capture_output=True, text=True,
        )
        return json.loads(done.stdout)

    assert not evaluate("if(1 > 0, 1, 2)")["ok"]
    assert evaluate("sign(0)")["value"] == 1
    assert evaluate("sign(-3)")["value"] == -1
    assert evaluate("log(100)")["value"] == 2
    with pytest.raises(SolidCraftError, match="got an angle"):
        execute([cmd("parameters.add", name="a", expression="atan(1)", unit="")])


def test_a_point_created_on_the_origin_snaps_to_it():
    assert sketch_dof([cmd("sketch.create", plane="XY"), cmd("sketch.circle.center", center=[0, 0], radius=5)]) == 1


def test_symmetric_rectangle_sides_conflict_and_corners_do_not():
    rectangle = [cmd("sketch.create", plane="XY"), cmd("sketch.rectangle.two_point", p0=[-10, 2], p1=[10, 8])]
    with pytest.raises(SolidCraftError, match="conflicts"):
        execute([*rectangle, cmd("sketch.constraint.symmetry", a="l4", b="l2", line="y")])
    execute([*rectangle, cmd("sketch.constraint.symmetry", a="p1", b="p2", line="y")])


def test_a_repeated_body_name_is_renamed_and_targets_reach_the_first_body():
    inspection = execute([
        cmd("solid.box", length=10, width=10, height=10, body_name="M"),
        cmd("solid.box", length=10, width=10, height=10, center=[30, 0, 5], body_name="M"),
        cmd("solid.cylinder", diameter="4 mm", height="20 mm", base=["30 mm", "0 mm", "-5 mm"], operation="cut",
            targets=["M"]),
    ])
    assert volumes(inspection) == pytest.approx({"M": 1000, "Body1": 1000})


# Conventions

def test_positive_taper_widens_in_either_direction():
    for direction, z in (("positive", 10), ("negative", -10)):
        body = execute([
            cmd("sketch.create", plane="XY", name="Square"),
            cmd("sketch.rectangle.center", center=[5, 5], corner=[10, 10]),
            cmd("sketch.finish"),
            cmd("solid.extrude", sketch="Square", distance="10 mm", direction=direction, taper="10 deg"),
        ])["bodies"][0]
        assert body["bbox"]["max"][0] > 10.5, direction
        assert body["bbox"]["max"][2] == pytest.approx(max(z, 0), abs=1e-6)


def test_a_circumscribed_hexagon_at_angle_0_has_vertices_up_and_down():
    sketch = run_commands([
        cmd("sketch.create", plane="XY"),
        cmd("sketch.polygon.circumscribed", center=[0, 0], radius=10, sides=6, angle=0),
        cmd("sketch.inspect"),
    ])[-2]
    vertices = [p["at"] for p in sketch["points"] if p["id"].startswith("p")]
    assert sorted(round(y, 3) for x, y in vertices if abs(x) < 1e-6) == [-11.547, 11.547]


def test_a_pattern_of_a_cylinder_cut_repeats_the_cut():
    body = execute([
        cmd("parameters.add", name="d", expression="4 mm"),
        *BLOCK,
        cmd("solid.cylinder", diameter="d", height="d * 5", base=["5 mm", "d + 1 mm", "-1 mm"], operation="cut",
            targets=["B"], name="Hole"),
        cmd("solid.pattern.rectangular", features=["Hole"], dir1=[1, 0, 0], count1=3, spacing1="10 mm"),
    ])["bodies"][0]
    assert body["volume_mm3"] == pytest.approx(40 * 40 * 12 - 3 * pi * 4 * 12, rel=2e-4)


def test_a_later_join_fills_an_earlier_cut():
    body = execute([
        *BLOCK,
        cmd("solid.cylinder", diameter="6 mm", height="20 mm", base=["20 mm", "20 mm", "-4 mm"], operation="cut",
            targets=["B"]),
        cmd("solid.cylinder", diameter="10 mm", height="6 mm", base=["20 mm", "20 mm", "3 mm"], operation="join",
            targets=["B"]),
    ])["bodies"][0]
    assert body["volume_mm3"] == pytest.approx(40 * 40 * 12 - pi * 9 * 12 + pi * 9 * 6, rel=2e-4)


def test_a_failed_feature_names_itself_and_saves_nothing(tmp_path):
    design = tmp_path / "design.json"
    run_commands(holes("2"), save=design)
    with pytest.raises(SolidCraftError, match="Row: pattern count"):
        run_commands([cmd("parameters.change", name="n", expression="0")], design=design, save=tmp_path / "out.json")
    assert not (tmp_path / "out.json").exists()


def test_a_command_script_works_as_a_design(tmp_path):
    script = tmp_path / "build.json"
    script.write_text(json.dumps({"commands": BLOCK}))
    assert execute([], design=script)["bodies"][0]["volume_mm3"] == pytest.approx(40 * 40 * 12)
    shot = tmp_path / "shot.png"
    subprocess.run(["solidcraft-cli", "snapshot", script, "--out", shot, "--width", "64", "--height", "64"],
                   capture_output=True, check=True)
    assert shot.stat().st_size > 0


# Corrections to solidcraft-modeling

def slot_along_y(last: dict) -> list:
    return [
        cmd("sketch.create", plane="XY"),
        cmd("sketch.draw.dimensioned", command="sketch.slot.overall", params={"p0": [10, 4], "p1": [10, 34], "width": 2},
            dimensions={"length": "30 mm", "width": "2 mm"}),
        cmd("sketch.dimension", entities=["origin", "a1.center"], type="horizontal", value="10 mm"),
        cmd("sketch.dimension", entities=["origin", "a1.center"], type="vertical", value="5 mm"),
        last,
    ]


def test_a_slot_along_y_takes_a_horizontal_dimension_on_its_second_arc():
    with pytest.raises(SolidCraftError, match="over-constrain"):
        execute(slot_along_y(cmd("sketch.dimension", entities=["origin", "a2.center"], type="vertical", value="33 mm")))
    assert sketch_dof(slot_along_y(
        cmd("sketch.dimension", entities=["origin", "a2.center"], type="horizontal", value="10 mm"))) == 0


OPEN_BOX = [
    cmd("solid.box", length=40, width=30, height=20, center=[0, 0, 10], body_name="B"),
    cmd("solid.fillet", edges=[[20, 15, 10], [-20, 15, 10], [20, -15, 10], [-20, -15, 10]], radius=3),
]
RIM = cmd("solid.fillet", edges=[[0, -15, 20]], radius=1)
SHELL = cmd("solid.shell", faces=[[0, 0, 20]], thickness=2)
FILLET_CENTROID = (10 - 3 * pi) / (12 - 3 * pi)  # distance of a fillet's cross-section centroid from its edge, per unit radius


def test_an_open_rim_is_rounded_after_the_shell():
    with pytest.raises(SolidCraftError, match="offset"):
        execute([*OPEN_BOX, RIM, SHELL])
    outer = (40 * 30 - (4 - pi) * 9) * 20
    cavity = (36 * 26 - (4 - pi) * 1) * 18
    rim = (1 - pi / 4) * (2 * (40 - 6) + 2 * (30 - 6) + 2 * pi * (3 - FILLET_CENTROID))
    body = execute([*OPEN_BOX, SHELL, RIM])["bodies"][0]
    assert body["volume_mm3"] == pytest.approx(outer - cavity - rim, rel=2e-4)


def test_the_centre_of_a_centred_rectangle_is_p5():
    inspection = run_commands([
        cmd("sketch.create", plane="XY"),
        cmd("sketch.rectangle.center", center=[3, 4], corner=[13, 9]),
        cmd("sketch.inspect"),
    ])[-2]
    assert {p["id"]: p["at"] for p in inspection["points"]}["p5"] == [3, 4]


SHELLED_CASE = [
    cmd("parameters.add", name="h", expression="5 mm"),
    cmd("solid.box", length=91, width=62, height=30, center=[0, 0, 15], body_name="Case"),
    cmd("solid.fillet", edges=[[45.5, 31, 15], [-45.5, 31, 15], [45.5, -31, 15], [-45.5, -31, 15]], radius=3),
    cmd("solid.shell", faces=[[0, 0, 30]], thickness=2),
    cmd("solid.fillet", edges=[[0, -31, 30]], radius=1),
]
STANDOFF_GRID = dict(dir1=[1, 0, 0], count1=2, spacing1="58 mm", dir2=[0, 1, 0], count2=2, spacing2="49 mm")
RING_GROWTH = 4 * pi * (9 - 1.5625)  # mm3 per mm of standoff height: four rings, 6 mm outside, 2.5 mm hole
TWO_CYLINDERS = [
    cmd("solid.cylinder", diameter="6 mm", height="h", base=["-39 mm", "-24.5 mm", "2 mm"], operation="join",
        targets=["Case"], name="Standoff"),
    cmd("solid.cylinder", diameter="2.5 mm", height="h", base=["-39 mm", "-24.5 mm", "2 mm"], operation="cut",
        targets=["Case"], name="Pilot"),
    cmd("solid.pattern.rectangular", features=["Standoff", "Pilot"], **STANDOFF_GRID),
]
RING = [
    cmd("sketch.create", plane="XY", offset="2 mm", name="Ring sketch"),
    cmd("sketch.circle.center", center=[-39, -24.5], radius=3),
    cmd("sketch.circle.center", center=[-39, -24.5], radius=1.25),
    cmd("sketch.dimension", entities=["c1"], type="diameter", value="6 mm"),
    cmd("sketch.dimension", entities=["c2"], type="diameter", value="2.5 mm"),
    cmd("sketch.dimension", entities=["origin", "c1.center"], type="horizontal", value="39 mm"),
    cmd("sketch.dimension", entities=["origin", "c1.center"], type="vertical", value="24.5 mm"),
    cmd("sketch.finish"),
    cmd("solid.extrude", sketch="Ring sketch", distance="h", profiles=[{"point": [-36.9, -24.5]}],
        operation="join", targets=["Case"], name="Ring"),
    cmd("solid.pattern.rectangular", features=["Ring"], **STANDOFF_GRID),
]


def standoffs_at(standoffs: list, height: float) -> tuple[float, dict]:
    """Volume gap at `height` against the height-5 volume plus the rings' growth, and each feature's warning."""
    low = execute([*SHELLED_CASE, *standoffs])["bodies"][0]["volume_mm3"]
    high = execute([*SHELLED_CASE, *standoffs, cmd("parameters.change", name="h", expression=f"{height} mm")])
    gap = high["bodies"][0]["volume_mm3"] - low - (height - 5) * RING_GROWTH
    return gap, {f["name"]: f["warning"] for f in high["timeline"] if f.get("warning")}


def test_a_boolean_retry_reports_tool_displacement_without_excess_material():
    gap, warned = standoffs_at(TWO_CYLINDERS, 7)
    assert abs(gap) < 0.5
    assert "Standoff" not in warned
    assert "tool displacement" in warned["Pilot"]


def test_sinking_the_join_removes_the_warning():
    gap, warned = standoffs_at([
        cmd("solid.cylinder", diameter="6 mm", height="h + 1 mm", base=["-39 mm", "-24.5 mm", "1 mm"], operation="join",
            targets=["Case"], name="Standoff"),
        cmd("solid.cylinder", diameter="2.5 mm", height="h + 1 mm", base=["-39 mm", "-24.5 mm", "2 mm"], operation="cut",
            targets=["Case"], name="Pilot"),
        cmd("solid.pattern.rectangular", features=["Standoff", "Pilot"], **STANDOFF_GRID),
    ], 7)
    assert abs(gap) < 0.5
    assert warned == {}


@pytest.mark.parametrize("height", [3, 7, 12, 20])
def test_a_ring_profile_builds_the_same_standoffs_exactly(height):
    gap, warned = standoffs_at(RING, height)
    assert abs(gap) < 0.5
    assert warned == {}


def test_a_sunk_join_on_a_shelled_floor_does_not_warn():
    warned = warnings([
        *SHELLED_CASE,
        cmd("solid.cylinder", diameter="6 mm", height="h + 1 mm", base=["-39 mm", "-24.5 mm", "1 mm"], operation="join",
            targets=["Case"], name="Standoff"),
    ])
    assert warned == []


