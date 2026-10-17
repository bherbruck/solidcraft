"""Original CAD room and thick tumbler authoring; no third-party scene assets."""
import math
import copy
from author_scene import Scene, MATERIALS, write

MATERIALS.update({
    "walnut": ("#4c2818",0,.42),
    "linen": ("#ada69b",0,.88),
    "pillow_ochre": ("#a66c32",0,.9),
    "pillow_taupe": ("#71685e",0,.9),
    "leaf": ("#315a2b",0,.6),
    "stem": ("#425c2b",0,.75),
    "soil": ("#23190f",0,1),
    "ceramic": ("#b4b3a5",0,.38),
    "wall": ("#ded9d1",0,.85),
    "blind": ("#bdac91",0,.7),
    "glass": ("#ffffff",0,.045),
})


def ellipsoid(s, name, center, radii, material, yaw=0):
    s.sphere(name,[0,0,0],1,material)
    s.cmd("solid.scale",bodies=[name],factors=radii)
    if yaw:
        s.cmd("solid.move",bodies=[name],axis=[0,0,1],angle=f"{yaw} deg")
    s.cmd("solid.move",bodies=[name],translate=center)


def cushion(s,name,center,size,radius,material,yaw=0):
    half=[v/2 for v in size]
    s.box(name,[-v for v in half],size,material)
    edges=[]
    for axis in range(3):
        others=[i for i in range(3) if i!=axis]
        for a in [-1,1]:
            for b in [-1,1]:
                p=[0,0,0]
                p[others[0]]=a*half[others[0]]
                p[others[1]]=b*half[others[1]]
                edges.append(p)
    s.cmd("solid.fillet",body=name,edges=edges,radius=radius)
    if yaw:
        s.cmd("solid.move",bodies=[name],axis=[0,0,1],angle=f"{yaw} deg")
    s.cmd("solid.move",bodies=[name],translate=center)


def tumbler(s,name,position,r0,r1,height,wall,bottom):
    s.sketch(name,[0,0,0],(1,0,0),(0,0,1))
    s.cmd("sketch.line",points=[[0,0],[r0,0],[r1,height],[r1-wall,height],[r0-wall,bottom],[0,bottom]],closed=True,infer=False)
    s.cmd("sketch.finish")
    s.cmd("solid.revolve",axis="y",angle="360 deg",operation="new",body_name=name)
    s.body(name,"glass")
    s.cmd("solid.move",bodies=[name],translate=position)


def room():
    s=Scene("Original two tumblers in a warm sitting room")
    s.box("Room floor",[-2400,-1800,-30],[4800,4300,30],"floor")
    s.box("Rear plaster wall",[-2400,2000,0],[4800,30,2300],"wall")
    s.box("Left plaster wall",[-2400,-1800,0],[30,3800,2300],"wall")
    s.rounded("Walnut tabletop",[0,0,445],1200,700,55,35,"walnut")
    for i,(x,y) in enumerate([(-470,-230),(470,-230),(-470,230),(470,230)]):
        s.rounded(f"Table leg {i+1}",[x,y,0],55,55,12,445,"walnut")
    # Cups sit on the table; disjoint bounds allow independent dielectric media.
    tumbler(s,"Tall thick tumbler",[-102,-88,480],36,42,112,4.5,9)
    tumbler(s,"Short thick tumbler",[88,35,480],37,43,94,5,10)
    # Original tailored sofa with distinct soft seats, arms, back and cushions.
    s.rounded("Sofa plinth",[0,1075,95],1680,670,65,140,"pillow_taupe")
    s.rounded("Sofa back",[0,1350,235],1680,160,65,510,"linen")
    for side,x in [("Left",-760),("Right",760)]:
        s.rounded(side+" sofa arm",[x,1050,235],160,650,60,345,"linen")
    for i,x in enumerate([-460,0,460]):
        cushion(s,f"Seat cushion {i+1}",[x,1020,285],[455,540,150],55,"linen")
        cushion(s,f"Back cushion {i+1}",[x,1230,545],[450,180,405],65,"linen")
    cushion(s,"Ochre throw cushion",[-520,1090,560],[315,130,330],55,"pillow_ochre",yaw=-18)
    cushion(s,"Taupe throw cushion",[530,1115,545],[290,130,310],55,"pillow_taupe",yaw=16)
    # Plant on a separate side stand, with independently modelled stems/leaves.
    s.rounded("Plant stand",[620,575,0],200,200,30,430,"walnut")
    s.cylinder("Ceramic plant pot",[620,575,430],82,146,"ceramic")
    s.cmd("solid.cylinder",base=[620,575,442],radius=73,height=136,operation="cut",targets=["Ceramic plant pot"],name="Open ceramic pot")
    s.cylinder("Pot soil",[620,575,550],71,8,"soil")
    for i in range(12):
        phi=math.radians(i*137.5)
        reach=100+(i%3)*24
        end=[620+math.cos(phi)*reach,575+math.sin(phi)*reach,730+(i%4)*62]
        base=[620,575,556]
        vec=[b-a for a,b in zip(base,end)]
        length=math.sqrt(sum(v*v for v in vec))
        s.cylinder(f"Plant stem {i+1}",base,2.5,length,"stem",axis=vec)
        ellipsoid(s,f"Plant leaf {i+1}",end,[62,23,4],"leaf",yaw=i*137.5)
    # A detached slatted window screen within the room receives a small bright
    # light behind it. The gaps are geometry, not a projected shadow texture.
    for i in range(14):
        s.box(f"Window blind slat {i+1}",[-1030,1440,660+i*45],[850,52,12],"blind")
    for x in [-1048,-180]:
        s.box(f"Window jamb {x}",[x,1434,620],[18,70,710],"wall")
    for z in [620,1320]:
        s.box(f"Window rail {z}",[-1030,1434,z],[850,70,18],"wall")
    for x in [-860,-350]:
        s.cylinder(f"Blind cord {x}",[x,1425,635],1.5,690,"linen")
    cam=dict(target=[-8,-8,537],yaw=math.radians(-10),pitch=math.radians(15),roll=0,distance=670,fov=math.radians(40))
    materials=s.finish("tumblers",cam)
    # Render request is deliberately separate from scene geometry and textures.
    render={
        "progressive":True,"samples":512,"max_depth":16,"seed":1948,"tone_map":"aces_fitted",
        "settings":{"width":1920,"height":1080,"camera":cam,"background":[218,222,226]},
        "lens":{"aperture_radius_mm":5.5,"focus_distance_mm":670},
        "lighting":{"environment":[.22,.25,.30],"area_lights":[
            {"center":[-650,1750,1470],"u":[95,0,0],"v":[0,0,95],"radiance":[38,30,21]},
            {"center":[-550,-300,1350],"u":[450,0,0],"v":[0,-300,0],"radiance":[3.5,3.5,3.5]},
            {"center":[850,300,1100],"u":[0,-240,0],"v":[0,0,300],"radiance":[2.5,2.7,3.0]}
        ]},
        "raytrace":{"dielectrics":[dict(body_index=i,body_name=name,ior=1.52,absorption_per_mm=[.0008,.00025,.00012],boundary="closed_mesh",thickness_mm=0)
                                     for i,(name,mat) in enumerate(materials.items()) if mat=="glass"]}
    }
    write("tumblers-render.json",render)
    # Sun variation retains the original baseline. A real rear-wall opening is
    # necessary: an infinite sun cannot illuminate through the intact wall.
    sun_scene=copy.deepcopy(s.commands)
    sun_scene.append({"command":"solid.box","params":{
        "corner":[-1450,1990,760],"length":1850,"width":50,"height":1080,
        "operation":"cut","targets":["Rear plaster wall"],"name":"Sunlight window opening"}})
    write("tumblers-sun.json",{"commands":sun_scene})
    sun_render=copy.deepcopy(render)
    sun_render["environment"]={"kind":"sky","gradient":{
        "zenith":[.16,.32,.7],"horizon":[.8,.9,1],"ground":[.12,.1,.08]},
        "intensity":1,"rotation_rad":0}
    sun_render["lighting"]={"environment":[0,0,0],"area_lights":[],
        "sun":{"direction":[-.4,1,.4],"angular_radius_rad":.00465,
               "radiance":[18000,16500,14000]}}
    write("tumblers-sun-render.json",sun_render)
    write("tumblers-materials.json",{
        "bodies":materials,
        "texture_coordinates":{
            "Walnut tabletop":{"projection":"world_xy","grain_direction":[1,0,0],"period_mm":[900,90],"note":"Original procedural walnut grain; do not copy photographs. Coordinate is mm, rotate grain along table long X axis."},
            "linen":{"projection":"triplanar_world","period_mm":[3,3,3],"note":"Fine low-contrast original woven noise, use broad albedo and subtle normal perturbation. No material runtime API assumed."},
            "glass":{"projection":"none","note":"Smooth thick dielectric; geometry provides rims and bases. No normal-map waviness requested for these tumblers."}
        }})


if __name__=="__main__":
    room()
