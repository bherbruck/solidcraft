"""Original cylindrical coffee housing; editable CAD commands, no mesh assets.

Derive separate variants from the reviewed coffee-framing recipes. Python is
only an authoring tool. The emitted JSON is replayable without Python.
"""
import argparse
import copy
import json
from pathlib import Path

HERE = Path(__file__).parent


def cylinder(name, base, radius, height):
    return dict(command="solid.cylinder", params=dict(
        body_name=name, base=base, radius=radius, height=height,
        axis="Z", operation="new"))


REPLACEMENTS = {
    "Housing plinth": [cylinder("Housing plinth", [0, 8, 30], 76, 12)],
    "Housing shell": [
        cylinder("Housing shell", [0, 8, 55], 70, 210),
        dict(command="solid.cylinder", params=dict(
            base=[0, 8, 54], radius=65, height=212, axis="Z",
            operation="cut", targets=["Housing shell"],
            name="Through cylindrical housing cavity")),
    ],
    "Housing upper shoulder": [
        cylinder("Housing upper shoulder", [0, 8, 265], 70, 8),
    ],
}


def body_names(commands):
    names = []
    for item in commands:
        params = item["params"]
        if item["command"].startswith("solid.") and params.get("operation") == "new":
            names.append(params["body_name"])
        if item["command"] == "solid.combine":
            for tool in params["tools"]:
                names.remove(tool)
    return names


def variant(source, expected_names):
    result = copy.deepcopy(source)
    commands = result["commands"]
    assert body_names(commands) == expected_names
    for name, replacement in REPLACEMENTS.items():
        starts = [i for i, c in enumerate(commands)
                  if c["command"] == "sketch.create"
                  and c["params"].get("name") == name + " profile"]
        ends = [i for i, c in enumerate(commands)
                if c["command"] == "solid.extrude"
                and c["params"].get("body_name") == name]
        assert len(starts) == len(ends) == 1 and starts[0] < ends[0]
        commands[starts[0]:ends[0] + 1] = copy.deepcopy(replacement)
    cuts = [c for c in commands if c["params"].get("name") == "Open brewing bay"]
    assert len(cuts) == 1 and cuts[0]["params"]["targets"] == ["Housing shell"]
    cuts[0]["params"].update(corner=[-47, -80, 88], length=94, width=80, height=149)
    commands[0]["params"]["name"] = "Original exploded cylindrical coffee machine"
    assert body_names(commands) == expected_names
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=HERE)
    parser.add_argument("--output", type=Path, default=HERE)
    args = parser.parse_args()
    read = lambda name: json.loads((args.source / name).read_text(encoding="utf-8"))
    names = list(read("materials.json")["coffee-exploded"])
    assert len(names) == 32
    for source, destination in [("coffee-framing.json", "coffee-cylinder.json"),
                                ("coffee-framing-sweep.json", "coffee-cylinder-sweep.json")]:
        recipe = variant(read(source), names)
        (args.output / destination).write_text(
            json.dumps(recipe, indent=2) + "\n", encoding="utf-8", newline="\n")
    render = read("coffee-framing-render.json")
    for binding in render["raytrace"]["clear_coats"] + render["raytrace"]["dielectrics"]:
        assert names[binding["body_index"]] == binding["body_name"]
    # Copy bytes: camera, light orientation, coatings and closed glass stay exact.
    (args.output / "coffee-cylinder-render.json").write_bytes(
        (args.source / "coffee-framing-render.json").read_bytes())


if __name__ == "__main__":
    main()
