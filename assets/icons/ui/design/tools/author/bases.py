"""Canonical original bases. Read assets/icons/ui/STYLE_GUIDE.md before drawing an icon.

SVGs remain the masters; these helpers make new drawings conform to the set.
"""
from lib import Icon, Iso, INK, P1, P2, P3, A3, OW, AW, GAP, poly, f
import math

GREY = (P1, P2, P3)
CUBE_MIN = -12
CUBE_MAX = 12
CUBE_HEIGHT = 24


def cube_projection(size=54):
    """x right/down, y left/down, z up; 30° diagonals, equal physical axes."""
    return Iso().fit([(x, y, z) for x in (-12, 12) for y in (-12, 12) for z in (0, 24)], size, size)


def grey_cube(icon, size=54):
    """Standard cube: top P1, screen-left P2, screen-right P3. Return projection."""
    projection = cube_projection(size)
    projection.box(icon, -12, -12, 0, 12, 12, 24, GREY)
    return projection


def grey_slab(icon, size=52):
    """Same 30° projection and shading, with a four-unit thickness."""
    projection = Iso().fit([(x, y, z) for x in (-15, 15) for y in (-15, 15) for z in (0, 4)], size, size)
    projection.box(icon, -15, -15, 0, 15, 15, 4, GREY)
    return projection


def arrow(icon, start, tip, colour=A3, length=9, width=9, gap=0):
    """A straight action arrow. Shaft meets head; optional thin surface knockout."""
    dx, dy = tip[0] - start[0], tip[1] - start[1]
    distance = math.hypot(dx, dy)
    if distance <= length: raise ValueError('arrow shaft must be longer than its head')
    dx, dy = dx / distance, dy / distance
    base = (tip[0] - dx * length, tip[1] - dy * length)
    head = poly([tip, (base[0] - dy * width / 2, base[1] + dx * width / 2),
                 (base[0] + dy * width / 2, base[1] - dx * width / 2)])
    shaft = poly([start, (base[0] + dx, base[1] + dy)], False)
    if gap:
        icon.stroke(shaft, GAP, AW + 2 * gap)
        icon.stroke(head, GAP, 2 * gap, fill=GAP)
    icon.stroke(shaft, colour, AW); icon.fill(head, colour)


def as_svg(icon):
    return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><title>' + icon.title + '</title>' + ''.join(icon.els) + '</svg>'


def examples():
    cube = Icon('grey_cube', 'Canonical grey cube'); grey_cube(cube)
    slab = Icon('grey_slab', 'Canonical grey slab'); grey_slab(slab)
    return {'Grey cube': as_svg(cube), 'Grey slab': as_svg(slab)}
