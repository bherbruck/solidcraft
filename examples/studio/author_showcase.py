"""Original portable studio showcase resources. No downloaded imagery.
Outputs are generated on demand into ignored paths; do not commit image bytes.
Copyright 2026 SolidCraft contributors. SPDX-License-Identifier: MIT OR Apache-2.0
"""
from pathlib import Path
import copy
import hashlib
import json
import math
import runpy

HERE = Path(__file__).resolve().parent
runpy.run_path(str(HERE / "textures" / "author_textures.py"))

def write(name, data):
    (HERE / name).write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8", newline="\n")

# Same original artistic gradient as EnvironmentSource::sky, sampled at texel
# centres and quantized to RGBE. No sun disk is baked into this environment.
width, height = 512, 256
zenith, horizon, ground = (.16,.32,.7), (.8,.9,1.), (.12,.1,.08)
encoded = bytearray(b"#?RADIANCE\n# Original SolidCraft analytic gradient, no external imagery\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 256 +X 512\n")
for row in range(height):
    z = math.cos(math.pi * (row + .5) / height)
    blend = math.sqrt(abs(z))
    endpoint = zenith if z >= 0 else ground
    rgb = [h * (1-blend) + e * blend for h,e in zip(horizon,endpoint)]
    fraction, exponent = math.frexp(max(rgb))
    scale = fraction * 256 / max(rgb)
    rgbe = [min(255, int(c * scale)) for c in rgb] + [exponent + 128]
    encoded.extend([2,2,width >> 8,width & 255])
    for value in rgbe:
        remaining = width
        while remaining:
            count = min(127, remaining)
            encoded.extend([128+count,value])
            remaining -= count
hdr = HERE / "textures" / "original-gradient.hdr"
hdr.write_bytes(encoded)
write("textures/environment-manifest.json", {
    "author":"SolidCraft contributors; original mathematical sky gradient",
    "license":"MIT OR Apache-2.0", "no_external_images":True,
    "width":width, "height":height, "color_space":"linear sRGB / D65",
    "sun_baked_in":False, "bytes":len(encoded),
    "sha256":hashlib.sha256(encoded).hexdigest(),
    "gradient":{"zenith":zenith,"horizon":horizon,"ground":ground},
    "encoding":"Radiance RGBE -Y +X; quantized texel-centre gradient"
})
sun = json.loads((HERE / "tumblers-sun-textures-render.json").read_text(encoding="utf-8"))
hdr_params = copy.deepcopy(sun)
hdr_params["environment"] = {
    "kind":"hdr", "path":"examples/studio/textures/original-gradient.hdr",
    "assume_linear_srgb":False, "rotation_rad":0, "intensity":1
}
write("tumblers-hdr-textures-render.json", hdr_params)
for name in ["tumblers-textures-render.json", "tumblers-sun-textures-render.json", "tumblers-hdr-textures-render.json"]:
    params = json.loads((HERE / name).read_text(encoding="utf-8"))
    assert len(params["raytrace"]["textures"]) == 13
    assert len(params["raytrace"]["dielectrics"]) == 2
    assert all(v["shadow_transmission"] for v in params["raytrace"]["dielectrics"])
    control = copy.deepcopy(params)
    for glass in control["raytrace"]["dielectrics"]:
        glass["shadow_transmission"] = False
    write(name.replace("-render.json", "-blocked-render.json"), control)
