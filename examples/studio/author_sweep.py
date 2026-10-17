"""Original CAD cyclorama variants; preserves all product bodies and render setup."""
import copy
import json
import math
from author_scene import HERE, Scene, write


def sweep_commands(scale):
    # A YZ cross-section extruded along +X. Inner and outer circular arcs share
    # a centre, so the floor and wall are tangent to a constant-thickness sweep.
    width=3000*scale
    front=-1500*scale
    start=250*scale
    radius=200*scale
    height=1500*scale
    thickness=4*scale
    q=math.sqrt(.5)
    s=Scene("Sweep construction")
    s.commands.clear()
    s.sketch("Studio floor",[-width/2,0,0],(0,1,0),(0,0,1))
    s.cmd("sketch.line",points=[[front,0],[start,0]],infer=False)
    s.cmd("sketch.arc.three_point",start=[start,0],
          through=[start+radius*q,radius-radius*q],end=[start+radius,radius],id="inner_sweep")
    s.cmd("sketch.line",points=[[start+radius,radius],[start+radius,height],
          [start+radius+thickness,height],[start+radius+thickness,radius]],infer=False)
    outer=radius+thickness
    s.cmd("sketch.arc.three_point",start=[start+outer,radius],
          through=[start+outer*q,radius-outer*q],end=[start,-thickness],id="outer_sweep")
    s.cmd("sketch.line",points=[[start,-thickness],[front,-thickness],[front,0]],infer=False)
    s.extrude("Studio floor",width,"floor")
    return s.commands


def variant(stem,scale):
    recipe=json.loads((HERE/(stem+".json")).read_text(encoding="utf-8"))
    result=copy.deepcopy(recipe)
    assert result["commands"][1]["params"]["body_name"]=="Studio floor"
    result["commands"][1:2]=sweep_commands(scale)
    write(stem+"-sweep.json",result)


if __name__=="__main__":
    variant("stage1",1)
    variant("coffee-exploded",2.8)
