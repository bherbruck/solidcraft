#!/usr/bin/env python3
"""Closed-form expected values for the surface oracle cases.

Run from the worktree root:  python3 tools/oracle-analytic/cases_surfaces.py [case ...]
Writes tests/oracle/<case>/expected.json with "source": "analytic".

Every value is derived from first principles (areas and centroids of plane regions bounded by
lines and circular arcs, a slab of constant thickness over such a region). Nothing here is
measured from a CAD kernel. Stdlib only.
"""
import json
import math
import os
import sys

PI = math.pi
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "tests", "oracle")


def c83_boundary_loop_thicken():
    """A boundary surface through four sketch curves on XY (the 10 x 10 square's bottom, right and
    left sides and, for its top, a half circle of radius 5 bulging out to y = 15) is the flat
    region they bound; thickened 2 mm symmetrically it is a slab from z = -1 to z = 1."""
    side, r, t = 10.0, 5.0, 2.0
    sq_a, sq_c = side * side, (5.0, 5.0)
    half_a, half_c = PI * r * r / 2, (5.0, side + 4 * r / (3 * PI))
    area = sq_a + half_a
    cx = (sq_a * sq_c[0] + half_a * half_c[0]) / area
    cy = (sq_a * sq_c[1] + half_a * half_c[1]) / area
    perimeter = 3 * side + PI * r
    return {
        "source": "analytic",
        "method": "region = 10 x 10 square + half disc r 5 on its top side: A = 100 + 12.5 pi, centroid by moments "
        "(half disc's at 4r / (3 pi) above its diameter); slab of thickness 2 over it (symmetric about z = 0): "
        "V = 2A, area = 2A + 2 (30 + 5 pi)",
        "body_count": 1,
        "volume_mm3": t * area,
        "area_mm2": 2 * area + t * perimeter,
        "center_of_mass": [cx, cy, 0.0],
        "bbox": {"min": [0.0, 0.0, -t / 2], "max": [side, side + r, t / 2]},
        "faces": 6,
    }


CASES = {
    "83-boundary-loop-thicken": c83_boundary_loop_thicken,
}


def main(argv):
    names = argv[1:] or list(CASES)
    for n in names:
        d = CASES[n]()
        path = os.path.join(ROOT, n, "expected.json")
        with open(path, "w") as f:
            json.dump(d, f, indent=2)
            f.write("\n")
        print(f"{n}: V={d['volume_mm3']!r} A={d['area_mm2']!r} com={d['center_of_mass']}")


if __name__ == "__main__":
    main(sys.argv)
