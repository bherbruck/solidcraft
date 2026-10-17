//! Fixtures for the golden records in `feature_walk_tests`: one feature per variant (and per
//! fillet style, pattern, hole kind and construction definition), each expression field holding
//! its own name, plus the reference fields `with_refs` sets.

use serde_json::{Value, json};

fn plane(chain: &str) -> Value {
    json!({"type": "at_angle", "angle": format!("{chain}.angle"), "axis_origin": [0, 0, 0], "axis_dir": [1, 0, 0],
        "base": {"type": "offset", "distance": format!("{chain}.offset"), "base": {"type": "construction", "name": "CP"}}})
}

fn geo_plane(chain: &str) -> Value {
    json!({"type": "plane", "plane": plane(chain)})
}

fn along(chain: &str) -> Value {
    json!({"type": "along_path", "path": {"type": "edge", "at": [0, 0, 0]}, "t": format!("{chain}.t")})
}

pub(super) fn fixtures() -> Vec<(&'static str, Value)> {
    let o = [0, 0, 0];
    let sk = serde_json::to_value(solidcraft_sketch::Sketch::new()).unwrap();
    let extent = |n: &str| {
        json!({"distance": format!("{n}.distance"), "distance2": format!("{n}.distance2"), "start_offset": format!("{n}.start_offset"),
            "taper": format!("{n}.taper"), "to_offset": format!("{n}.to_offset")})
    };
    let fillet = |style: Value| json!({"type": "fillet", "edges": [o], "radius": "fillet.radius", "style": style});
    let pattern = |p: Value| json!({"type": "pattern", "features": ["F"], "pattern": p});
    let hole =
        |h: Value| json!({"type": "hole", "position": o, "direction": [0, 0, 1], "diameter": "hole.diameter", "depth": "hole.depth", "hole": h});
    vec![
        (
            "body_transfer",
            json!({"type": "body_transfer", "bodies": ["B"], "from": 0, "to": 1, "transform": [[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}),
        ),
        ("sketch", json!({"type": "sketch", "plane": plane("sketch"), "sketch": sk.clone()})),
        ("sketch_along_path", json!({"type": "sketch", "plane": along("sketch"), "sketch": sk})),
        ("extrude", json!({"type": "extrude", "sketch": 1, "profiles": {"type": "all"}, "extent": extent("extrude")})),
        (
            "extrude_thin",
            json!({"type": "extrude", "sketch": 1, "profiles": {"type": "all"}, "extent": extent("thin"),
                "thin": {"curves": ["c1"], "thickness": "thin.thickness", "side": "two_sides", "thickness2": "thin.thickness2"}}),
        ),
        (
            "revolve",
            json!({"type": "revolve", "sketch": 1, "profiles": {"type": "all"}, "axis": {"type": "world", "axis": "X"}, "angle": "revolve.angle", "angle2": "revolve.angle2"}),
        ),
        ("fillet_constant", fillet(json!({"type": "constant"}))),
        ("fillet_chord", fillet(json!({"type": "chord"}))),
        ("fillet_curvature", fillet(json!({"type": "curvature"}))),
        ("fillet_variable", fillet(json!({"type": "variable", "radius2": "fillet.radius2", "start": o, "mid": [[0.5, "fillet.mid"]]}))),
        ("fillet_spline", fillet(json!({"type": "spline", "start": o, "knots": [0.0, 1.0], "radii": ["fillet.r1", "fillet.r2", "fillet.r3"]}))),
        (
            "chamfer",
            json!({"type": "chamfer", "edges": [o], "distance": "chamfer.distance", "distance2": "chamfer.distance2", "angle": "chamfer.angle"}),
        ),
        ("box", json!({"type": "box", "corner": ["box.x", "box.y", "box.z"], "length": "box.length", "width": "box.width", "height": "box.height"})),
        ("cylinder", json!({"type": "cylinder", "base": ["cyl.x", "cyl.y", "cyl.z"], "radius": "cyl.radius", "height": "cyl.height"})),
        ("sphere", json!({"type": "sphere", "center": ["sph.x", "sph.y", "sph.z"], "radius": "sph.radius"})),
        ("torus", json!({"type": "torus", "center": ["tor.x", "tor.y", "tor.z"], "major": "tor.major", "minor": "tor.minor"})),
        ("combine", json!({"type": "combine", "target": "A", "tools": ["B"], "operation": "join"})),
        (
            "pattern_rectangular",
            pattern(
                json!({"type": "rectangular", "dir1": [1, 0, 0], "count1": "rect.count1", "spacing1": "rect.spacing1", "dir2": [0, 1, 0], "count2": "rect.count2", "spacing2": "rect.spacing2"}),
            ),
        ),
        ("pattern_circular", pattern(json!({"type": "circular", "origin": o, "axis": [0, 0, 1], "count": "circ.count", "angle": "circ.angle"}))),
        ("pattern_path", pattern(json!({"type": "path", "path_sketch": 1, "path": ["l1"], "count": "path.count", "spacing": "path.spacing"}))),
        ("mirror", json!({"type": "mirror", "features": ["F"], "plane": plane("mirror")})),
        ("hole_simple", hole(json!({"type": "simple"}))),
        ("hole_drilled", hole(json!({"type": "drilled", "tip_angle": "hole.tip_angle"}))),
        ("hole_counterbore", hole(json!({"type": "counterbore", "cb_diameter": "hole.cb_diameter", "cb_depth": "hole.cb_depth"}))),
        ("hole_countersink", hole(json!({"type": "countersink", "cs_diameter": "hole.cs_diameter", "cs_angle": "hole.cs_angle"}))),
        ("thread", json!({"type": "thread", "face": o, "length": "thread.length"})),
        ("loft", json!({"type": "loft", "sections": []})),
        ("sweep", json!({"type": "sweep", "sketch": 1, "profiles": {"type": "all"}, "path_sketch": 2, "path": ["l1"]})),
        (
            "sweep_options",
            json!({"type": "sweep", "sketch": 1, "profiles": {"type": "all"}, "path_sketch": 2, "path": ["l1"],
                "orientation": {"direction": [0, 1, 0]}, "twist": "sweep.twist", "scale": "sweep.scale"}),
        ),
        ("shell", json!({"type": "shell", "faces": [o], "thickness": "shell.thickness"})),
        ("draft", json!({"type": "draft", "faces": [o], "angle": "draft.angle", "neutral": plane("draft_neutral"), "pull": [0, 0, 1]})),
        ("construction_plane", json!({"type": "construction_plane", "plane": plane("cplane")})),
        ("split", json!({"type": "split", "body": "B", "plane": plane("split")})),
        ("split_line", json!({"type": "split", "body": "B", "plane": plane("split_line")})),
        (
            "construction_axis",
            json!({"type": "construction_axis", "def": {"type": "two_planes", "a": geo_plane("axis_a"), "b": geo_plane("axis_b")}}),
        ),
        (
            "construction_point_planes",
            json!({"type": "construction_point", "def": {"type": "three_planes", "a": geo_plane("pt_a"), "b": geo_plane("pt_b"), "c": geo_plane("pt_c")}}),
        ),
        (
            "construction_point_along",
            json!({"type": "construction_point", "def": {"type": "along_path", "path": {"type": "edge", "at": o}, "t": "pt.t"}}),
        ),
        ("split_face", json!({"type": "split_face", "faces": [o], "plane": plane("split_face")})),
        ("scale", json!({"type": "scale", "bodies": ["B"], "factor": "scale.factor", "factors": ["scale.x", "scale.y", "scale.z"]})),
        ("offset_face", json!({"type": "offset_face", "faces": [o], "distance": "offset_face.distance"})),
        ("delete_face", json!({"type": "delete_face", "faces": [o]})),
        ("bounding_solid", json!({"type": "bounding_solid", "bodies": ["B"], "margin": "bounding.margin"})),
        ("pipe", json!({"type": "pipe", "path_sketch": 1, "path": ["l1"], "diameter": "pipe.diameter", "wall": "pipe.wall"})),
        ("emboss", json!({"type": "emboss", "sketch": 1, "profiles": {"type": "all"}, "depth": "emboss.depth"})),
        ("rib", json!({"type": "rib", "sketch": 1, "curves": ["l1"], "thickness": "rib.thickness", "depth": "rib.depth"})),
        ("replace_face", json!({"type": "replace_face", "faces": [o], "target": plane("replace_face")})),
        ("align", json!({"type": "align", "bodies": ["B"], "from": o, "to": o})),
        (
            "coil",
            json!({"type": "coil", "base": ["coil.x", "coil.y", "coil.z"], "diameter": "coil.diameter", "pitch": "coil.pitch", "turns": "coil.turns", "section_size": "coil.section_size", "start_angle": "coil.start_angle"}),
        ),
        ("sheet_base", json!({"type": "sheet_base", "sketch": 1, "profiles": {"type": "all"}})),
        ("sheet_contour", json!({"type": "sheet_contour", "sketch": 1, "curves": ["l1"], "distance": "contour.distance"})),
        (
            "sheet_flange",
            json!({"type": "sheet_flange", "edges": [{"point": o, "angle": "flange.e1_angle", "height": "flange.e1_height"}, {"point": o, "angle": null, "height": "flange.e2_height"}], "height": "flange.height", "angle": "flange.angle", "radius": "flange.radius", "corner_relief_shape": "round", "corner_relief": "flange.relief"}),
        ),
        ("sheet_hem", json!({"type": "sheet_hem", "edges": [o], "length": "hem.length", "gap": "hem.gap"})),
        (
            "sheet_perforate",
            json!({"type": "sheet_perforate", "diameter": "perforate.diameter", "pitch": "perforate.pitch", "margin": "perforate.margin"}),
        ),
        (
            "sheet_fold",
            json!({"type": "sheet_fold", "a": o, "b": [1, 0, 0], "angle": "fold.angle", "radius": "fold.radius", "position": "centerline"}),
        ),
        ("sheet_unfold", json!({"type": "sheet_unfold"})),
        ("sheet_convert", json!({"type": "sheet_convert", "body": "B", "face": o})),
        (
            "boss",
            json!({"type": "boss", "position": o, "diameter": "boss.diameter", "height": "boss.height", "hole_diameter": "boss.hole_diameter", "hole_depth": "boss.hole_depth",
            "draft": "boss.draft", "fillet": "boss.fillet", "ribs": "boss.ribs", "rib_thickness": "boss.rib_thickness", "rib_length": "boss.rib_length", "rib_offset": "boss.rib_offset"}),
        ),
        ("lip", json!({"type": "lip", "face": o, "width": "lip.width", "height": "lip.height", "gap": "lip.gap"})),
        (
            "snap_fit",
            json!({"type": "snap_fit", "position": o, "hook": [1, 0, 0], "length": "snap.length", "thickness": "snap.thickness", "width": "snap.width", "catch_depth": "snap.catch_depth", "catch_length": "snap.catch_length"}),
        ),
        (
            "rest",
            json!({"type": "rest", "position": o, "width": "rest.width", "length": "rest.length", "height": "rest.height", "draft": "rest.draft", "thickness": "rest.thickness"}),
        ),
        ("boundary_fill", json!({"type": "boundary_fill", "tools": ["B"], "cells": [o]})),
        ("remove", json!({"type": "remove", "bodies": ["B"]})),
        ("patch", json!({"type": "patch"})),
        ("stitch", json!({"type": "stitch", "bodies": ["B"], "tolerance": "stitch.tolerance"})),
        ("thicken", json!({"type": "thicken", "bodies": ["B"], "thickness": "thicken.thickness", "operation": "new_body"})),
        ("surface_extrude", json!({"type": "surface_extrude", "sketch": 1, "extent": extent("surface_extrude")})),
        (
            "surface_revolve",
            json!({"type": "surface_revolve", "sketch": 1, "axis": {"type": "world", "axis": "X"}, "angle": "srev.angle", "angle2": "srev.angle2"}),
        ),
        ("surface_sweep", json!({"type": "surface_sweep", "sketch": 1, "path_sketch": 2, "path": ["l1"]})),
        (
            "surface_sweep_options",
            json!({"type": "surface_sweep", "sketch": 1, "path_sketch": 2, "path": ["l1"],
                "orientation": "parallel", "twist": "surface_sweep.twist", "scale": "surface_sweep.scale"}),
        ),
        ("surface_loft", json!({"type": "surface_loft", "sections": []})),
        ("surface_blend", json!({"type": "surface_blend", "from": {"body": "B", "edges": [o]}, "to": {"body": "C", "edges": [o]}, "align": [o, o]})),
        ("surface_offset", json!({"type": "surface_offset", "faces": [o], "distance": "soff.distance", "tolerance": "soff.tolerance"})),
        // The Class-A surface family (surface.blend/boundary/match/fair).
        ("surface_blend", json!({"type": "surface_blend", "from": {"body": "B", "edges": [o]}, "to": {"body": "C", "edges": [o]}, "align": [o, o]})),
        ("surface_boundary", json!({"type": "surface_boundary", "sketch": 1, "curves": ["l1", "l2"]})),
        (
            "surface_network",
            json!({"type": "surface_network", "u": [{"sketch": 1, "curves": ["l1"]}, {"body": "B", "edges": [o], "continuity": "g1"}], "v": [{"sketch": 2, "curves": ["l1"]}, {"point": o}], "operation": "join"}),
        ),
        ("surface_match", json!({"type": "surface_match", "body": "B", "faces": [o], "target": "C", "edges": [o]})),
        ("surface_fair", json!({"type": "surface_fair", "body": "B", "faces": [o], "tolerance": "sfair.tolerance"})),
        ("unstitch", json!({"type": "unstitch", "body": "B"})),
        ("reverse_normal", json!({"type": "reverse_normal", "body": "B"})),
        ("surface_untrim", json!({"type": "surface_untrim", "face": {"body": "B", "point": o}, "holes": [o], "outer": true})),
        ("surface_extend", json!({"type": "surface_extend", "body": "B", "edges": [o], "distance": "extend.distance"})),
        (
            "surface_extend_to_plane",
            json!({"type": "surface_extend", "body": "B", "edges": [o], "distance": "extend.distance", "mode": "curvature", "up_to": {"plane": plane("extend")}}),
        ),
        (
            "surface_extend_to_face",
            json!({"type": "surface_extend", "body": "B", "edges": [o], "distance": "extend.distance", "up_to": {"face": {"body": "B", "point": o}}, "neighbours": false}),
        ),
        (
            "surface_ruled",
            json!({"type": "surface_ruled", "body": "B", "edges": [o], "distance": "ruled.distance", "mode": "angle", "angle": "ruled.angle",
                "direction": [0, 0, 1], "face": [1, 0, 0]}),
        ),
        ("surface_trim", json!({"type": "surface_trim", "body": "B", "plane": plane("trim"), "keep": o})),
        ("move", json!({"type": "move", "bodies": ["B"], "translate": ["move.x", "move.y", "move.z"], "angle": "move.angle"})),
        ("import", json!({"type": "import", "file": "a.step", "step": ""})),
        ("mesh_convert", json!({"type": "mesh_convert", "body": "B", "tolerance": "mesh_convert.tolerance"})),
        ("mesh_import", json!({"type": "mesh_import", "file": "a.stl", "meshes": []})),
    ]
}

/// The reference fields a fixture leaves out (they don't change its expressions), set here so
/// every sketch, feature and body reference is exercised: sketches 1–8, feature `F`,
/// construction plane `CP`, body `B`.
pub(super) fn with_refs(name: &str, mut v: Value) -> Value {
    let o = [0, 0, 0];
    let tool = json!({"body": "B", "point": o});
    let extra = match name {
        "extrude" | "extrude_thin" | "revolve" | "sweep" | "sweep_options" | "pipe" | "coil" | "thicken" => json!({"targets": ["B"]}),
        "fillet_constant" | "fillet_chord" | "fillet_curvature" | "fillet_variable" | "fillet_spline" | "chamfer" | "shell" | "draft"
        | "offset_face" | "delete_face" | "replace_face" | "sheet_flange" | "sheet_hem" | "sheet_perforate" | "sheet_unfold" | "boss" | "lip"
        | "snap_fit" | "rest" | "surface_offset" => {
            json!({"body": "B"})
        }
        "combine" => json!({"target": "B", "tools": ["B"]}),
        "pattern_rectangular" | "pattern_circular" | "pattern_path" | "mirror" => json!({"bodies": ["B"]}),
        "hole_simple" | "hole_drilled" | "hole_counterbore" | "hole_countersink" => json!({"points": {"sketch": 3, "ids": ["p"]}}),
        "loft" => json!({"sections": [{"sketch": 4, "profiles": {"type": "all"}}, {"sketch": 5, "profiles": {"type": "all"}}], "targets": ["B"]}),
        "split" | "surface_trim" => json!({"tool": tool}),
        "split_line" => json!({"sketch": 6, "line": "l1"}),
        "surface_loft" => json!({"sections": [{"sketch": 4}, {"sketch": 5}]}),
        "split_face" => json!({"body": "B", "tool": tool, "sketch": 6}),
        "emboss" => json!({"face": {"body": "B", "at": o}, "targets": ["B"]}),
        "sheet_fold" => json!({"sketch": 7, "body": "B"}),
        "patch" => json!({"sketch": 8, "body": "B"}),
        _ => json!({}),
    };
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    v
}

/// The positions and directions a fixture leaves out (they don't change its expressions or
/// references), set here so every point, direction and axis the geometry walks read is present:
/// `to` / `from` picks, line axes, normals and origins. `hole_simple` places its hole at
/// `position` (no sketch points), the other holes at sketch points.
pub(super) fn with_points(name: &str, mut v: Value) -> Value {
    let p = |i: f64| json!([i, i + 0.5, i + 0.25]);
    let line = json!({"type": "line", "origin": p(9.0), "dir": [0.0, 0.6, 0.8]});
    let extra = match name {
        "extrude" | "surface_extrude" => {
            if let Some(e) = v.get_mut("extent").and_then(Value::as_object_mut) {
                e.insert("to".into(), p(1.0));
                e.insert("from".into(), p(2.0));
            }
            json!({})
        }
        "revolve" => json!({"to": p(3.0), "axis": line}),
        "surface_revolve" => json!({"axis": line}),
        "hole_simple" => json!({"points": null, "to": p(4.0)}),
        "hole_drilled" | "hole_counterbore" | "hole_countersink" => json!({"to": p(4.0)}),
        "emboss" => json!({"face": {"body": "B", "at": p(5.0)}}),
        "sheet_convert" => json!({"bends": [p(6.0)]}),
        "sheet_fold" => json!({"fixed": p(7.0)}),
        "rest" => json!({"along": [0.6, 0.8, 0.0]}),
        "align" => json!({"from_normal": [0.0, 0.0, 1.0], "to_normal": [1.0, 0.0, 0.0]}),
        "scale" => json!({"origin": p(8.0)}),
        "move" => json!({"rotate_axis": [0.0, 1.0, 0.0]}),
        "unstitch" | "reverse_normal" => json!({"faces": [p(10.0)]}),
        _ => json!({}),
    };
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    v
}

/// The paths of the leaves that differ between two JSON values.
pub(super) fn changed(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                changed(v, y.get(k).unwrap_or(&Value::Null), &format!("{path}.{k}"), out);
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, v) in x.iter().enumerate() {
                changed(v, y.get(i).unwrap_or(&Value::Null), &format!("{path}[{i}]"), out);
            }
        }
        _ if a != b => out.push(path.trim_start_matches('.').to_string()),
        _ => {}
    }
}
