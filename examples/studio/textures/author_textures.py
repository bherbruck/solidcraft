"""Original deterministic studio map authoring; no imported imagery or dependencies.
Outputs are generated on demand into ignored paths; do not commit image bytes.
Copyright 2026 SolidCraft contributors. SPDX-License-Identifier: MIT OR Apache-2.0
"""
from pathlib import Path
import hashlib,json,math,struct,zlib
OUT=Path(__file__).resolve().parent
SCENES=OUT.parent
RESOURCE=Path("examples/studio/textures")
N=256;TAU=math.tau

def png(path,pixels):
    def chunk(kind,data):return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data)&0xffffffff)
    scan=b"".join(b"\0"+pixels[y*N*4:(y+1)*N*4] for y in range(N))
    path.write_bytes(b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",N,N,8,6,0,0,0))+chunk(b"IDAT",zlib.compress(scan,9))+chunk(b"IEND",b""))
def srgb(x):return round(255*(12.92*x if x<=.0031308 else 1.055*x**(1/2.4)-.055))
def wood(u,v):
    phase=TAU*(12*v+.34*math.sin(TAU*u)+.11*math.sin(TAU*(2*u+v)))
    pore=.5+.5*math.cos(phase)
    fine=.5+.5*math.sin(TAU*(37*v+2*u))
    height=.12*math.cos(phase)+.025*math.sin(TAU*(37*v+2*u))
    value=.72+.22*pore+.035*fine
    return (value,min(1.,value*.99),value*.975),height

def fabric(u,v):
    warp=math.cos(TAU*8*u);weft=math.cos(TAU*8*v)
    over=math.cos(TAU*4*u)*math.cos(TAU*4*v)
    height=.065*(warp+weft)+.035*over
    value=.86+.045*(warp+weft)+.025*over
    return (value,value,value),height

def author(name,fn,period):
    color=bytearray();normal=bytearray();linear=[];step=1/N
    for y in range(N):
        for x in range(N):
            u=(x+.5)/N;v=(y+.5)/N;rgb,_=fn(u,v);linear.extend(rgb)
            color.extend([*(srgb(c) for c in rgb),255])
            dx=(fn(u+step,v)[1]-fn(u-step,v)[1])/(2*step*period[0])
            dy=(fn(u,v+step)[1]-fn(u,v-step)[1])/(2*step*period[1])
            q=(-dx,-dy,1.);length=math.sqrt(sum(c*c for c in q))
            normal.extend([*(round(255*(.5+.5*c/length)) for c in q),255])
    png(OUT/(name+"-base-srgb.png"),bytes(color));png(OUT/(name+"-normal-opengl.png"),bytes(normal))
    return {"mean_linear_multiplier":sum(linear)/len(linear),"min_linear_multiplier":min(linear),"max_linear_multiplier":max(linear),"period_mm":period}

stats={"walnut":author("walnut",wood,(900,90)),"linen":author("linen",fabric,(24,24))}
bindings=[]
def bind(index,name,origin,u,v,material,strength):
    bindings.append(dict(body_index=index,body_name=name,frame=dict(origin=origin,u=u,v=v),base_color_srgb=(RESOURCE/(material+"-base-srgb.png")).as_posix(),normal_opengl=(RESOURCE/(material+"-normal-opengl.png")).as_posix(),normal_strength=strength))
bind(3,"Walnut tabletop",[0,0,480],[900,0,0],[0,90,0],"walnut",.65)
front=[(10,"Sofa plinth",[0,740,165]),(11,"Sofa back",[0,1270,490]),(12,"Left sofa arm",[-760,725,407.5]),(13,"Right sofa arm",[760,725,407.5])]
front += [(15+2*k,"Back cushion "+str(k+1),[-460+460*k,1140,545]) for k in range(3)]
for index,name,origin in front:bind(index,name,origin,[24,0,0],[0,0,-24],"linen",.55)
for k in range(3):bind(14+2*k,"Seat cushion "+str(k+1),[-460+460*k,1020,360],[24,0,0],[0,24,0],"linen",.55)
bind(20,"Ochre throw cushion",[-540.0861,1028.1813,560],[22.82536,-7.41641,0],[0,0,-24],"linen",.55)
bind(21,"Taupe throw cushion",[547.9164,1052.517,545],[23.07028,6.61530,0],[0,0,-24],"linen",.55)
for source,target in [("tumblers-render.json","tumblers-textures-render.json"),("tumblers-sun-render.json","tumblers-sun-textures-render.json")]:
    params=json.loads((SCENES/source).read_text(encoding="utf8",newline="\n"));params.setdefault("raytrace",{})["textures"]=bindings
    params["samples"]=2048
    for glass in params["raytrace"]["dielectrics"]: glass["shadow_transmission"]=True
    (SCENES/target).write_text(json.dumps(params,indent=2)+"\n",encoding="utf8",newline="\n")
manifest={"author":"SolidCraft contributors; original analytic patterns authored for this task", "license":"MIT OR Apache-2.0", "no_external_images":True,"dimensions":[N,N],"stats":stats,"bindings":len(bindings),"linear_pixel_bytes_across_bindings":len(bindings)*2*N*N*16,"files":{p.name:{"sha256":hashlib.sha256(p.read_bytes()).hexdigest(),"bytes":p.stat().st_size} for p in OUT.glob("*.png")}}
(OUT/"manifest.json").write_text(json.dumps(manifest,indent=2)+"\n",encoding="utf8",newline="\n")
print(json.dumps(manifest,indent=2))
