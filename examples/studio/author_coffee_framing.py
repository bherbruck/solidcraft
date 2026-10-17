"""Original framing variant; derives only new files from the reviewed baseline.

Keep the baseline 32 bodies and names: append rigid translations, never copies.
The camera is lower and closer; optical settings and corrected lights are copied.
Run from any working directory with Python's standard library.
"""
import copy
import json
import math
from pathlib import Path

HERE = Path(__file__).parent
SHIFTS = {'Open brewing head': [0, 0, 28],
 'Brewing collar': [0, 0, 28],
 'Shower screen': [0, 0, 28],
 'Brewing head roof': [0, 0, 40],
 'Bent chrome lever': [0, 0, 40],
 'Left lever ball': [0, 0, 40],
 'Right lever ball': [0, 0, 40],
 'Smoky D tank': [100, 8, 0],
 'Separate tank lid': [100, 8, 22],
 'Tank lid grip': [100, 8, 22],
 'Clear ribbed bin': [-70, -15, 0],
 'Louvred service panel': [-110, 15, 0],
 'Drip tray': [0, -34, 0],
 'Perforated stainless grate': [0, -34, 0],
 'Rounded tray cap': [38, -34, 0],
 'Black S power cable': [50, 8, 0],
 'Two pin plug': [50, 8, 0],
 'Plug pin 206': [50, 8, 0],
 'Plug pin 218': [50, 8, 0],
 'Zinc screw 1': [-28, 0, 0],
 'Zinc screw 2': [-28, 0, 0],
 'Zinc screw 3': [-28, 0, 0],
 'Zinc screw 4': [-28, 0, 0],
 'Zinc screw 5': [28, 0, 0],
 'Zinc screw 6': [28, 0, 0],
 'Zinc screw 7': [28, 0, 0],
 'Zinc screw 8': [28, 0, 0]}
CAMERA = dict(target=[-25, -10, 170], yaw=math.radians(30), pitch=math.radians(18),
              roll=0, distance=950, fov=math.radians(34))


def read(name):
    return json.loads((HERE / name).read_text(encoding="utf-8"))


def write(name, value):
    (HERE / name).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8", newline="\n")


def main():
    names = list(read("materials.json")["coffee-exploded"])
    assert len(names) == 32 and set(SHIFTS).issubset(names)
    for source, destination in [("coffee-exploded.json", "coffee-framing.json"),
                                ("coffee-exploded-sweep.json", "coffee-framing-sweep.json")]:
        recipe = copy.deepcopy(read(source))
        for name, translation in SHIFTS.items():
            recipe["commands"].append(dict(command="solid.move", params=dict(
                bodies=[name], translate=translation, copy=False,
                name="Framing: " + name)))
        write(destination, recipe)
    render = copy.deepcopy(read("coffee-exploded-render.json"))
    render["settings"]["camera"] = CAMERA
    # Preserve the physical closed glass, all seven coats and the corrected fill.
    # No smoothing or denoising is introduced by the framing variant.
    write("coffee-framing-render.json", render)


if __name__ == "__main__":
    main()
