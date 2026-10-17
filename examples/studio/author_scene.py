"""Original editable CAD recipe authoring; Python is not an application dependency.

The emitted JSON is the editable authority consumed by SolidCraft commands.
All dimensions are millimetres. No meshes or imported design assets are used.
"""
import json
import math
from pathlib import Path

HERE = Path(__file__).parent
MATERIALS = {
    "coated_black": ("#101318", 0, .30),
    "matte_black": ("#171a1f", 0, .55),
    "chrome": ("#d5d9dd", 1, .12),
    "stainless": ("#aeb4bb", 1, .25),
    "zinc": ("#b0b5bc", 1, .32),
    "rubber": ("#141414", 0, .8),
    "floor": ("#e8e8e8", 0, .65),
    "smoke": ("#c2c6ca", 0, .05),
    "clear": ("#e4eff1", 0, .05),
}


class Scene:
    def __init__(self, name):
        self.commands = []
        self.materials = {}
        self.cmd("file.new", name=name)

    def cmd(self, command, **params):
        self.commands.append(dict(command=command, params=params))

    def body(self, name, material):
        self.materials[name] = material

    def box(self, name, p, size, material, **extra):
        self.cmd("solid.box", body_name=name, corner=p, length=size[0], width=size[1], height=size[2], operation="new", **extra)
        self.body(name, material)

    def cylinder(self, name, p, radius, height, material, axis="Z"):
        self.cmd("solid.cylinder", body_name=name, base=p, radius=radius, height=height, axis=axis, operation="new")
        self.body(name, material)

    def sphere(self, name, p, radius, material):
        self.cmd("solid.sphere", body_name=name, center=p, radius=radius, operation="new")
        self.body(name, material)

    def sketch(self, name, origin, x=(1, 0, 0), y=(0, 1, 0)):
        self.cmd("sketch.create", name=name + " profile", plane=dict(origin=origin, x_dir=x, y_dir=y))

    def extrude(self, name, height, material, point=None):
        self.cmd("sketch.finish")
        self.cmd("solid.extrude", distance=height, operation="new", body_name=name, **({"profiles":[{"point":point}]} if point else {}))
        self.body(name, material)

    def rounded(self, name, origin, width, depth, radius, height, material, wall=0, x=(1, 0, 0), y=(0, 1, 0)):
        self.sketch(name, origin, x, y)
        self.cmd("sketch.rectangle.rounded", p0=[-width/2, -depth/2], p1=[width/2, depth/2], radius=radius)
        if wall:
            self.cmd("sketch.rectangle.rounded", p0=[-width/2+wall, -depth/2+wall], p1=[width/2-wall, depth/2-wall], radius=max(.5, radius-wall))
        self.extrude(name, height, material, [-width/2+wall/2,0] if wall else None)

    def d_loop(self, radius, rear, prefix, segments=0):
        # Straight back, parallel sides and one exact semicircular front.
        if segments:
            points=[[-radius,0],[-radius,rear],[radius,rear],[radius,0]]
            points += [[radius*math.cos(math.pi*i/segments),-radius*math.sin(math.pi*i/segments)] for i in range(1,segments)]
            self.cmd("sketch.line",points=points,closed=True,infer=False)
            return
        self.cmd("sketch.line", points=[[-radius, 0], [-radius, rear], [radius, rear], [radius, 0]], ids=[prefix+"left", prefix+"back", prefix+"right"], infer=False)
        self.cmd("sketch.arc.three_point", start=[radius, 0], end=[-radius, 0], through=[0, -radius], id=prefix+"front")

    def d_section(self, name, origin, radius, rear, height, material, wall=0, holes=(), segments=0):
        self.sketch(name, origin)
        self.d_loop(radius, rear, "outer",segments)
        if wall:
            # Back offset makes an actual uniform-thickness D shell.
            self.d_loop(radius-wall, rear-wall, "inner",segments)
        for cx, cy, r in holes:
            self.cmd("sketch.circle.center", center=[cx, cy], radius=r)
        self.extrude(name, height, material, [0,rear-(wall/2 if wall else 1)] if wall or holes else None)

    def finish(self, stem, camera, scale=1):
        for material, (color, metallic, roughness) in MATERIALS.items():
            bodies = [name for name, mat in self.materials.items() if mat == material]
            if bodies:
                self.cmd("appearance.assign", bodies=bodies, color=color, pbr=dict(metallic=metallic, roughness=roughness, dielectric_ior=1.5))
        write(stem + ".json", dict(commands=self.commands))
        light = lambda center,u,v,power: dict(center=[q*scale for q in center], u=[q*scale for q in u], v=[q*scale for q in v], radiance=[power]*3)
        render = dict(progressive=True, samples=256, max_depth=12, seed=1948, tone_map="aces_fitted",
                      settings=dict(width=1920, height=1080, camera=camera, background=[240,240,240]),
                      lighting=dict(environment=[.5,.5,.5], area_lights=[
                          light([-140,-130,280], [110,0,0], [0,-70,0], 8),
                          light([280,-160,180], [0,-60,0], [0,0,90], 6),
                          light([0,170,250], [70,0,0], [0,0,65], 10)]))
        if stem == "coffee-exploded":
            render["raytrace"] = {
                "clear_coats":[dict(body_index=i,body_name=name,weight=1,roughness=.12,ior=1.5)
                               for i,(name,mat) in enumerate(self.materials.items()) if mat == "coated_black"],
                "dielectrics":[dict(body_index=i,body_name=name,ior=1.586 if mat=="smoke" else 1.49,
                                    absorption_per_mm=[.045,.075,.12] if mat=="smoke" else [.0004,.00015,.00005],
                                    boundary="closed_mesh",thickness_mm=0)
                               for i,(name,mat) in enumerate(self.materials.items()) if mat in {"smoke","clear"}]
            }
        write(stem + "-render.json", render)
        return self.materials


def write(name, value):
    (HERE/name).write_text(json.dumps(value, indent=2)+"\n", encoding="utf-8", newline="\n")


def camera(target, distance):
    # SolidCraft yaw zero is front (-Y); positive yaw moves left (-X).
    return dict(target=target, yaw=math.radians(32), pitch=math.radians(30), roll=0, distance=distance, fov=math.radians(34))


def stage1():
    s = Scene("Original studio light and coating study")
    s.box("Studio floor", [-1500,-1500,-4], [3000,3000,4], "floor")
    s.rounded("Coated curved shell", [-42,8,35], 85,65,20,65,"coated_black",wall=4)
    s.rounded("Floating coated lid", [-42,8,121], 85,65,20,5,"coated_black")
    s.sphere("Chrome sphere", [60,-3,60], 29,"chrome")
    s.cylinder("Chrome floating puck", [63,5,112], 24,7,"chrome")
    return s.finish("stage1", camera([0,0,70],430))


def coffee():
    s = Scene("Original exploded bench coffee machine")
    # Broad seamless studio floor. Its far edge is outside the explicit camera.
    s.box("Studio floor", [-6000,-6000,-5], [12000,12000,5], "floor")
    s.rounded("Housing plinth", [0,8,30], 140,130,24,12,"coated_black")
    s.rounded("Housing shell", [0,8,55], 130,116,23,210,"coated_black",wall=5)
    s.cmd("solid.box", corner=[-46,-60,88], length=92,width=35,height=149,operation="cut",targets=["Housing shell"],name="Open brewing bay")
    s.rounded("Housing upper shoulder", [0,8,265],130,116,23,8,"coated_black")
    s.box("Internal chassis", [-42,38,63], [84,5,189],"stainless")
    # Open head floats above the shell: hollow collar and roof separated clearly.
    s.cylinder("Open brewing head",[0,-76,307],43,95,"coated_black",axis="Y")
    s.cmd("solid.cylinder",base=[0,-77,307],radius=31,height=97,axis="Y",operation="cut",targets=["Open brewing head"],name="Horizontal open brew bore")
    s.rounded("Brewing head roof", [0,-6,352],115,103,25,7,"coated_black")
    s.cmd("solid.box",corner=[-18,-9,351],length=36,width=7,height=9,operation="cut",targets=["Brewing head roof"],name="Capsule loading slot")
    s.cylinder("Brewing collar", [0,-88,307], 35,9,"chrome",axis="Y")
    s.cmd("solid.cylinder", base=[0,-89,307],radius=28,height=11,axis="Y",operation="cut",targets=["Brewing collar"],name="Open brew throat")
    s.cylinder("Shower screen", [0,10,307], 27,2,"stainless",axis="Y")
    for x,y in [(-10,-7),(0,-7),(10,-7),(-10,7),(0,7),(10,7)]:
        s.cmd("solid.cylinder",base=[x,9,307+y],radius=1.6,height=4,axis="Y",operation="cut",targets=["Shower screen"],name="Shower aperture")
    # Smooth bent U lever: exact line / circular arc pipe, not faceted cylinders.
    s.sketch("Lever centerline", [0,-28,0], (1,0,0),(0,0,1))
    s.cmd("sketch.line",points=[[-77,306],[-77,365]],ids=["lever_left"],infer=False)
    s.cmd("sketch.arc.three_point",start=[-77,365],through=[-72.6066,375.6066],end=[-62,380],id="lever_bend_left")
    s.cmd("sketch.line",points=[[-62,380],[62,380]],ids=["lever_top"],infer=False)
    s.cmd("sketch.arc.three_point",start=[62,380],through=[72.6066,375.6066],end=[77,365],id="lever_bend_right")
    s.cmd("sketch.line",points=[[77,365],[77,306]],ids=["lever_right"],infer=False)
    s.cmd("sketch.finish")
    s.cmd("solid.pipe",path_sketch="Lever centerline profile",path=["lever_left","lever_bend_left","lever_top","lever_bend_right","lever_right"],diameter=9,operation="new",body_name="Bent chrome lever")
    s.body("Bent chrome lever","chrome")
    for side,x in [("Left",-77),("Right",77)]:
        s.sphere(side+" lever ball",[x,-28,306],8,"chrome")
    # Open D tank: wall and base joined into one closed connected dielectric.
    s.d_section("Smoky D tank", [225,35,99],43,45,171,"smoke",segments=64)
    s.sketch("Tank cavity",[225,35,103])
    s.d_loop(40,42,"cavity",segments=64)
    s.cmd("sketch.finish")
    s.cmd("solid.extrude",distance=169,operation="cut",targets=["Smoky D tank"],name="Open tank cavity with four millimetre base")
    s.d_section("Separate tank lid", [225,35,304],45,47,5,"matte_black")
    s.rounded("Tank lid grip", [225,41,309],38,12,5,5,"matte_black")
    # A ribbed bin with a continuous connected wall and bottom. Fine triangular
    # ribs are authored as an analytic line profile, then extruded as a B-rep.
    s.sketch("Clear ribbed bin",[-157,-23,58])
    front=[]
    for i in range(12):
        x=-33+i*5.5
        front.extend([[x,-30],[x+1.8,-32],[x+3.5,-30]])
    front.append([33,-30])
    outline=front+[[33,30],[-33,30]]
    s.cmd("sketch.line",points=outline,closed=True,infer=False)
    s.cmd("sketch.rectangle.two_point",p0=[-30,-27],p1=[30,27])
    s.extrude("Clear ribbed bin",72,"clear",[31,0])
    s.box("Bin base tool",[-190,-53,57],[66,60,4],"clear")
    s.cmd("solid.combine",target="Clear ribbed bin",tools=["Bin base tool"],operation="join")
    del s.materials["Bin base tool"]
    # D tray and floating perforated metal grate. Holes are profile islands,
    # making true through apertures in a single editable extrude operation.
    s.d_section("Drip tray",[0,-143,22],57,43,16,"coated_black",wall=4)
    s.d_section("Tray base tool",[0,-143,21],57,43,4,"coated_black")
    s.cmd("solid.combine",target="Drip tray",tools=["Tray base tool"],operation="join")
    del s.materials["Tray base tool"]
    holes=[(x,y,2.2) for y in [-30,-18,-6,6,18,30] for x in [-36,-24,-12,0,12,24,36] if y>=0 or x*x+y*y<47*47]
    s.d_section("Perforated stainless grate",[0,-143,65],53,39,2,"stainless",holes=holes)
    s.rounded("Rounded tray cap",[112,-143,40],38,54,15,16,"coated_black")
    # Detached service panel faces front; slots and fixing holes are real voids.
    s.sketch("Louvred service panel",[-122,143,173],(1,0,0),(0,0,1))
    s.cmd("sketch.rectangle.rounded",p0=[-47,-91],p1=[47,91],radius=8)
    for z in range(-60,61,15):
        s.cmd("sketch.rectangle.rounded",p0=[-31,z-2],p1=[31,z+2],radius=1.5)
    for x in [-38,38]:
        for z in [-81,81]:
            s.cmd("sketch.circle.center",center=[x,z],radius=2.4)
    s.extrude("Louvred service panel",3,"matte_black",[43,0])
    # Eight zinc screws in two separated assembly rails; the slotted heads are
    # actual cuts, and threaded-shaft collars add readable mechanical detail.
    for i in range(8):
        x=(-99 if i<4 else 99)
        z=87+(i%4)*55
        name=f"Zinc screw {i+1}"
        s.cylinder(name,[x,-68,z],2.1,14,"zinc",axis="Y")
        s.cmd("solid.cylinder",base=[x,-71,z],radius=4.3,height=4,axis="Y",operation="join",targets=[name],name="Screw pan head")
        s.cmd("solid.box",corner=[x-3.3,-72,z-.7],length=6.6,width=2.4,height=1.4,operation="cut",targets=[name],name="Driver slot")
    # Black S cable follows two exact tangent circular arcs near the floor.
    s.sketch("S cable centerline",[70,-75,9])
    s.cmd("sketch.arc.three_point",start=[52,94],through=[92,54],end=[132,94],id="cable_a")
    s.cmd("sketch.arc.three_point",start=[132,94],through=[172,134],end=[212,94],id="cable_b")
    s.cmd("sketch.line",points=[[212,94],[212,66]],ids=["cable_end"],infer=False)
    s.cmd("sketch.finish")
    s.cmd("solid.pipe",path_sketch="S cable centerline profile",path=["cable_a","cable_b","cable_end"],diameter=5,operation="new",body_name="Black S power cable")
    s.body("Black S power cable","rubber")
    s.rounded("Two pin plug",[282,-30,3],24,35,7,16,"rubber")
    for x in [206,218]:
        s.cylinder(f"Plug pin {x}",[x+70,-64,11],2,18,"chrome",axis="Y")
    return s.finish("coffee-exploded",camera([25,0,185],1200),scale=2.8)


if __name__ == "__main__":
    write("materials.json", {"stage1": stage1(), "coffee-exploded": coffee()})
