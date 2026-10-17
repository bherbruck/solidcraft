//! Copy and paste of timeline features. Pasted features are independent copies: sketches get
//! their own dimension parameters, feature inputs their own names, and references inside the
//! copied set point at the copies.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solidcraft_doc::{FeatureKind, PlaneRef};

use super::CommandSpec;
use crate::params::{bad, string_list, vec3};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("timeline.copy", "Copy Features", copy).icon("copy").noundo().params("features: [ids or names] (in timeline order on paste)"),
    CommandSpec::new("timeline.paste", "Paste Features", paste)
        .icon("paste")
        .params("translate?: [x,y,z] (moves the copies' geometry); the copies go at the marker"),
];

fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.copy";
    let mut keys = string_list(p, "features");
    if let Some(Value::Array(a)) = p.get("features") {
        keys.extend(a.iter().filter_map(Value::as_u64).map(|n| n.to_string()));
    }
    if keys.is_empty() || keys.len() > 1000 {
        return Err(bad(cmd, "`features` must list 1…1000 features"));
    }
    let mut ids = Vec::new();
    for k in &keys {
        ids.push(s.doc.find_feature(k).map(|f| f.id).ok_or_else(|| bad(cmd, format!("no feature `{k}`")))?);
    }
    // Keep timeline order so sketches come before the features that use them.
    let copied: Vec<solidcraft_doc::Feature> = s.doc.features.iter().filter(|f| ids.contains(&f.id)).cloned().collect();
    let names: Vec<String> = copied.iter().map(|f| f.name.clone()).collect();
    s.clipboard = copied;
    Ok(json!({"copied": names}))
}

/// Origin planes become explicit planes so a translation can move them.
fn explicit_planes(p: &mut PlaneRef) {
    match p {
        PlaneRef::Origin { name } => {
            if let Some(plane) = solidcraft_geom::Plane::named(name) {
                *p = PlaneRef::Custom { plane };
            }
        }
        PlaneRef::Offset { base, .. } | PlaneRef::AtAngle { base, .. } | PlaneRef::Flipped { base, .. } => explicit_planes(base),
        _ => {}
    }
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.paste";
    if s.clipboard.is_empty() {
        return Err(bad(cmd, "nothing copied (timeline.copy first)"));
    }
    let translate = match p.get("translate") {
        Some(v) => Some(vec3(v).ok_or_else(|| bad(cmd, "`translate` must be [x, y, z]"))?),
        None => None,
    };
    let feats = s.clipboard.clone();
    let comp = s.active_component;
    let mut sketches: BTreeMap<u64, u64> = BTreeMap::new();
    let mut bodies: BTreeMap<String, String> = BTreeMap::new();
    let mut used: std::collections::BTreeSet<String> = s.model.state().bodies.iter().map(|body| body.name.clone()).collect();
    let mut notices = Vec::new();
    let author_path = s.active_occurrence_path.clone().unwrap_or_else(|| s.doc.first_path(comp));
    solidcraft_doc::EmbossFace::component_frame(&s.placed_document(), comp, &author_path)?;
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut made = Vec::new();
    for f in feats {
        let mut kind = f.kind.clone();
        kind.remap_refs(&sketches, &names);
        // A body made by an earlier copy is the one this copy acts on (#613).
        kind.remap_bodies(&bodies);
        if let FeatureKind::Sketch { plane, .. } = &mut kind
            && let Some(anchor) = super::sketch::face::anchor_mut(plane)
        {
            let internal = bodies.contains_key(&anchor.target.body);
            if !internal && translate.is_some_and(|translate| translate.len() > 1e-12) {
                return Err(bad(cmd, "translated paste retains an external exact face attachment; explicitly convert it to a fixed plane first"));
            }
            if !internal {
                notices.push(format!("pasted sketch retains external exact face attachment to {}", anchor.target.body));
            }
            anchor.remap_target(&sketches, &bodies)?;
            if internal {
                anchor.component = comp;
                anchor.path = author_path.clone();
            }
            anchor.author_path = author_path.clone();
            let target = anchor.target.clone();
            if let FeatureKind::Sketch { sketch, .. } = &mut kind {
                for link in &mut sketch.links {
                    if link.auto_face {
                        if let solidcraft_sketch::LinkSource::Face { body, .. } = &mut link.source
                            && let Some(mapped) = bodies.get(body)
                        {
                            *body = mapped.clone();
                        }
                        link.face_name = Some(target.name.clone());
                    }
                }
            }
        }
        let built: Vec<String> = s
            .model
            .result(f.id)
            .map(|result| result.state.bodies.iter().filter(|body| body.feature == f.id).map(|body| body.name.clone()).collect())
            .unwrap_or_default();
        let outputs = if f.body_names.is_empty() { built.clone() } else { f.body_names.clone() };
        // A later feature names a body as the model shows it, and that is not always the name stored
        // on the feature: solids that share a label are stored as `X`, `X`, `X` and shown as `X`,
        // `X (2)`, `X (3)`, and a second import of a file shows `Aa (2)` for the stored `Aa`. The
        // copies are found by the shown names; the stored ones only name the copies.
        let shown = if built.len() == outputs.len() { built } else { outputs.clone() };
        let copied_names: Vec<_> = outputs
            .iter()
            .zip(&shown)
            .map(|(original, shown)| {
                let base = format!("{original} (copy)");
                let mut name = base.clone();
                let mut suffix = 2usize;
                while !used.insert(name.clone()) {
                    name = format!("{base} {suffix}");
                    suffix += 1;
                }
                bodies.insert(shown.clone(), name.clone());
                name
            })
            .collect();
        let doc = s.doc_mut();
        if let FeatureKind::Sketch { sketch, .. } = &mut kind {
            // Each dimension of the copy gets its own parameter, with the same expression.
            for c in &mut sketch.constraints {
                if let Some(pn) = c.param.clone() {
                    let (e, u) = doc.param(&pn).map(|x| (x.expr.clone(), x.unit.clone())).unwrap_or_else(|| (pn.clone(), "mm".into()));
                    c.param = Some(doc.new_model_param(&e, &u));
                }
            }
        }
        if let Some(t) = translate {
            if let FeatureKind::Sketch { plane, .. } | FeatureKind::ConstructionPlane { plane } = &mut kind {
                explicit_planes(plane);
            }
            let mut m = solidcraft_doc::IDENTITY;
            m[3][0] = t.x;
            m[3][1] = t.y;
            m[3][2] = t.z;
            kind.transform_geometry(&m);
        }
        // A cylinder Emboss finds its face only by the persistent name recorded when it was made
        // (#1035): the copy keeps that name, pointed at the pasted features and bodies.
        let face_names: Vec<String> = if matches!(kind, FeatureKind::Emboss { face: Some(_), .. }) {
            f.face_names.iter().map(|n| solidcraft_doc::remap_topology(n, &sketches, &bodies)).collect()
        } else {
            Vec::new()
        };
        let id = doc.add_feature(kind, None)?;
        if let Some(nf) = doc.feature_mut(id) {
            nf.component = comp;
            nf.body_names = copied_names;
            nf.face_names = face_names;
        }
        let new_name = doc.feature(id).map(|x| x.name.clone()).unwrap_or_default();
        sketches.insert(f.id, id);
        names.insert(f.name.clone(), new_name.clone());
        made.push(json!({"id": id, "name": new_name, "from": f.name}));
    }
    s.refresh();
    let errors: Vec<Value> = made
        .iter()
        .filter_map(|m| m["id"].as_u64())
        .filter_map(|id| s.model.result(id).and_then(|r| r.error.clone()).map(|e| json!({"feature": id, "error": e})))
        .collect();
    Ok(json!({"pasted": made, "errors": errors, "notices":notices}))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: serde_json::Value) -> serde_json::Value {
        match s.execute(id, &p) {
            Ok(v) => v,
            Err(e) => panic!("{id} {p}: {e}"),
        }
    }

    fn volume(s: &mut Session) -> f64 {
        run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap_or(f64::NAN)
    }

    #[test]
    fn copy_paste_sketch_and_extrude() {
        let mut s = Session::default();
        run(&mut s, "parameters.change", json!({"name": "w", "expression": "10 mm"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 10]}));
        run(&mut s, "sketch.constraint.coincident", json!({"a": "p1", "b": "origin"}));
        run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": "w"}));
        run(&mut s, "sketch.dimension", json!({"entities": ["l2"], "value": 10}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "S", "distance": 5}));
        assert!((volume(&mut s) - 500.0).abs() < 1e-6);
        run(&mut s, "timeline.copy", json!({"features": ["S", "Extrude1"]}));
        let r = run(&mut s, "timeline.paste", json!({"translate": [30, 0, 0]}));
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
        assert_eq!(run(&mut s, "inspect.measure", json!({}))["body_count"], 2);
        assert!((volume(&mut s) - 1000.0).abs() < 1e-6);
        // The copy's dimensions are its own, still driven by the user parameter.
        run(&mut s, "parameters.change", json!({"name": "w", "expression": "20 mm"}));
        assert!((volume(&mut s) - 2000.0).abs() < 1e-6);
        let pasted_extrude = r["pasted"][1]["name"].as_str().unwrap_or_default().to_string();
        let d = s.doc.find_feature(&pasted_extrude).and_then(|f| f.param_names.first().cloned()).unwrap_or_default();
        run(&mut s, "parameters.change", json!({"name": d, "expression": "10"}));
        assert!((volume(&mut s) - 3000.0).abs() < 1e-6, "only the copy got thicker");
        assert!(s.execute("timeline.paste", &json!({"translate": "x"})).is_err());
        let mut t = Session::default();
        assert!(t.execute("timeline.paste", &json!({})).is_err());
    }

    #[test]
    fn paste_at_a_marker_inside_a_timeline_folder_joins_the_folder() {
        let mut s = Session::default();
        for (name, body, x) in [("First", "FirstBody", 0), ("Second", "SecondBody", 20), ("Source", "SourceBody", 40)] {
            run(
                &mut s,
                "solid.box",
                json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
            );
        }
        let group = run(&mut s, "timeline.group", json!({"name": "Pair", "features": ["First", "Second"]}))["group"].as_u64().unwrap();
        run(&mut s, "timeline.copy", json!({"features": ["Source"]}));
        run(&mut s, "timeline.roll_to", json!({"position": 1}));

        let pasted = run(&mut s, "timeline.paste", json!({}))["pasted"][0]["id"].as_u64().unwrap();
        let folder = s.doc.timeline_groups.iter().find(|folder| folder.id == group).unwrap();
        let first = s.doc.find_feature("First").unwrap().id;
        let second = s.doc.find_feature("Second").unwrap().id;
        assert_eq!(folder.features, vec![first, pasted, second]);
        assert!(solidcraft_doc::Document::from_json(&s.doc.to_json()).is_ok(), "pasting inside a folder must leave a reopenable design");
    }

    /// A STEP file of three boxes at x = 0, 30 and 60. With `shared` the three solids carry one
    /// label (`X`), as the parts of many assembly files do; otherwise each has its own.
    fn three_box_step(tag: &str, shared: bool) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("solidcraft-paste-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::default();
        for (i, label) in ["Aa", "Bb", "Cc"].iter().enumerate() {
            let corner = [30.0 * i as f64, 0.0, 0.0];
            run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": corner, "operation": "new_body", "body_name": label}));
        }
        let path = dir.join("Three.step");
        run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
        if shared {
            let mut text = std::fs::read_to_string(&path).unwrap();
            for label in ["Aa", "Bb", "Cc"] {
                assert!(text.contains(&format!("MANIFOLD_SOLID_BREP('{label}'")), "the file names its solids: {label}");
                text = text.replace(&format!("MANIFOLD_SOLID_BREP('{label}'"), "MANIFOLD_SOLID_BREP('X'");
            }
            std::fs::write(&path, text).unwrap();
        }
        path
    }

    /// An OBJ file of three 10 mm cubes at x = 0, 30 and 60, every object called `X`.
    fn three_cube_obj(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("solidcraft-paste-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let corners = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]];
        let faces =
            [[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [3, 7, 6], [3, 6, 2], [0, 4, 7], [0, 7, 3], [1, 2, 6], [1, 6, 5]];
        let mut text = String::new();
        for i in 0..3 {
            text += "o X\n";
            for c in corners {
                text += &format!("v {} {} {}\n", 30 * i + 10 * c[0], 10 * c[1], 10 * c[2]);
            }
            for f in faces {
                text += &format!("f {} {} {}\n", 8 * i + f[0] + 1, 8 * i + f[1] + 1, 8 * i + f[2] + 1);
            }
        }
        let path = dir.join("Three.obj");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn body_names(s: &Session) -> Vec<String> {
        s.model.state().bodies.iter().map(|b| b.name.clone()).collect()
    }

    /// Where every body is: the lowest corner of its bounding box, rounded, in model order.
    fn corners(s: &mut Session) -> Vec<Vec<f64>> {
        let m = run(s, "inspect.measure", json!({}));
        let bodies = m["bodies"].as_array().cloned().unwrap_or_default();
        bodies
            .iter()
            .map(|b| b["bbox"]["min"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64().map(f64::round)).collect()).unwrap_or_default())
            .collect()
    }

    /// Lifts the first of the three bodies of the import by 20 and the second by 50 (two features),
    /// copies the import with both and pastes them: the copies must be lifted, the originals once.
    fn paste_import_with_two_lifts(s: &mut Session, import: &Value, bodies: [&str; 3]) {
        run(s, "solid.move", json!({"bodies": [bodies[0]], "translate": [0, 0, 20], "name": "Lift first"}));
        run(s, "solid.move", json!({"bodies": [bodies[1]], "translate": [0, 0, 50], "name": "Lift second"}));
        run(s, "timeline.copy", json!({"features": [import, "Lift first", "Lift second"]}));
        let r = run(s, "timeline.paste", json!({}));
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    }

    const LIFTED: [[f64; 3]; 3] = [[0.0, 0.0, 20.0], [30.0, 0.0, 50.0], [60.0, 0.0, 0.0]];

    /// A control: three solids with labels of their own. The pasted features act on the pasted bodies.
    #[test]
    fn pasting_an_import_moves_the_copies_of_the_bodies_its_features_named() {
        let path = three_box_step("distinct", false);
        let mut s = Session::default();
        let import = run(&mut s, "file.insert_step", json!({"path": path.to_string_lossy()}))["feature"].clone();
        assert_eq!(body_names(&s), ["Aa", "Bb", "Cc"]);
        paste_import_with_two_lifts(&mut s, &import, ["Aa", "Bb", "Cc"]);
        assert_eq!(body_names(&s), ["Aa", "Bb", "Cc", "Aa (copy)", "Bb (copy)", "Cc (copy)"]);
        let at: Vec<Vec<f64>> = LIFTED.iter().chain(&LIFTED).map(|c| c.to_vec()).collect();
        assert_eq!(corners(&mut s), at);
    }

    /// Solids of a STEP file that share one label are built as `X`, `X (2)`, `X (3)`, and the features
    /// made on them name them so. The pasted features act on the copies of those bodies, each on its
    /// own: `X (2)` is not left on the original, and `X` does not go to the last copy.
    #[test]
    fn pasting_an_import_with_one_label_on_all_solids_moves_the_copy_of_each_body() {
        let path = three_box_step("shared", true);
        let mut s = Session::default();
        let import = run(&mut s, "file.insert_step", json!({"path": path.to_string_lossy()}))["feature"].clone();
        assert_eq!(body_names(&s), ["X", "X (2)", "X (3)"]);
        paste_import_with_two_lifts(&mut s, &import, ["X", "X (2)", "X (3)"]);
        let names = body_names(&s);
        assert_eq!(names.len(), 6, "{names:?}");
        let at: Vec<Vec<f64>> = LIFTED.iter().chain(&LIFTED).map(|c| c.to_vec()).collect();
        assert_eq!(corners(&mut s), at, "{names:?}");
        // The copies keep their names and their places when the design is saved and opened again.
        let design = path.with_file_name("Three.solidcraft");
        run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
        let mut r = Session::default();
        run(&mut r, "doc.open", json!({"path": design.to_string_lossy()}));
        assert_eq!(body_names(&r), names);
        assert_eq!(corners(&mut r), at);
    }

    /// The same for the objects of an OBJ file (3MF and STL files give the same kind of import).
    #[test]
    fn pasting_a_mesh_import_with_one_label_on_all_objects_moves_the_copy_of_each_body() {
        let path = three_cube_obj("shared-obj");
        let mut s = Session::default();
        let import = run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}))["feature"].clone();
        assert_eq!(body_names(&s), ["X", "X (2)", "X (3)"]);
        paste_import_with_two_lifts(&mut s, &import, ["X", "X (2)", "X (3)"]);
        let at: Vec<Vec<f64>> = LIFTED.iter().chain(&LIFTED).map(|c| c.to_vec()).collect();
        assert_eq!(corners(&mut s), at, "{:?}", body_names(&s));
    }

    /// No shared label is needed: a second import of the same file builds `Aa (2)`, `Bb (2)`, `Cc (2)`
    /// from the labels `Aa`, `Bb`, `Cc`, and the features made on those bodies name them so.
    #[test]
    fn pasting_a_second_import_of_a_file_moves_the_copies_of_its_bodies() {
        let path = three_box_step("twice", false);
        let mut s = Session::default();
        run(&mut s, "file.insert_step", json!({"path": path.to_string_lossy()}));
        let second = run(&mut s, "file.insert_step", json!({"path": path.to_string_lossy()}))["feature"].clone();
        assert_eq!(body_names(&s), ["Aa", "Bb", "Cc", "Aa (2)", "Bb (2)", "Cc (2)"]);
        paste_import_with_two_lifts(&mut s, &second, ["Aa (2)", "Bb (2)", "Cc (2)"]);
        // Each import is a component of its own (#1386), shown after the pasted copies: the first
        // import's bodies stay where they were, the second's and their copies are lifted.
        let plain = [[0.0, 0.0, 0.0], [30.0, 0.0, 0.0], [60.0, 0.0, 0.0]];
        let at: Vec<Vec<f64>> = LIFTED.iter().chain(&plain).chain(&LIFTED).map(|c| c.to_vec()).collect();
        assert_eq!(corners(&mut s), at, "{:?}", body_names(&s));
        let m = run(&mut s, "inspect.measure", json!({}));
        let names: Vec<&str> = m["bodies"].as_array().unwrap().iter().filter_map(|b| b["name"].as_str()).collect();
        assert_eq!(names, ["Aa (copy)", "Bb (copy)", "Cc (copy)", "Aa", "Bb", "Cc", "Aa (2)", "Bb (2)", "Cc (2)"]);
    }

    /// A cylinder Emboss finds its face by the persistent name it recorded when it was made. The
    /// pasted Emboss keeps that name, moved onto the pasted cylinder, so the copy gets its own
    /// badge, with or without a translate (#1035).
    #[test]
    fn pasted_cylinder_emboss_badges_the_pasted_cylinder() {
        for translate in [Some(json!([50, 0, 0])), None] {
            let mut s = Session::default();
            run(&mut s, "solid.cylinder", json!({"radius": "10 mm", "height": "20 mm", "body_name": "Sleeve", "name": "Tube"}));
            run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 10, 0], "x_dir": [-1, 0, 0], "y_dir": [0, 0, 1]}, "name": "Badge"}));
            run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-2, 5], "p1": [2, 10]}));
            run(&mut s, "sketch.finish", json!({}));
            run(&mut s, "solid.emboss", json!({"sketch": "Badge", "face": [0, 10, 7.5], "depth": 1, "targets": ["Sleeve"], "name": "Mark"}));
            let one = volume(&mut s);
            let plain = std::f64::consts::PI * 100.0 * 20.0;
            assert!(one > plain + 1.0, "the badge adds material: {one}");
            run(&mut s, "timeline.copy", json!({"features": ["Tube", "Badge", "Mark"]}));
            let p = translate.clone().map_or(json!({}), |t| json!({"translate": t}));
            let r = run(&mut s, "timeline.paste", p);
            assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{translate:?}: {r}");
            let mark = s.doc.find_feature(r["pasted"][2]["name"].as_str().unwrap_or_default()).cloned().unwrap();
            let tube = r["pasted"][0]["id"].as_u64().unwrap();
            assert!(mark.face_names.first().is_some_and(|n| n.starts_with(&format!("F{tube}:"))), "{:?}", mark.face_names);
            assert!((volume(&mut s) - 2.0 * one).abs() < 1e-6, "{translate:?}: both cylinders carry a badge");
            if translate.is_some() {
                let m = run(&mut s, "inspect.measure", json!({"bodies": ["Sleeve (copy)"]}));
                let body = &m["bodies"][0];
                let x = body["center_of_mass"][0].as_f64().unwrap_or(f64::NAN);
                let y = body["bbox"]["max"][1].as_f64().unwrap_or(f64::NAN);
                assert!((x - 50.0).abs() < 1e-3 && (y - 11.0).abs() < 1e-6, "the copy's badge sits on the moved cylinder: {m}");
            }
        }
    }

    #[test]
    fn pasted_fillet_rounds_the_copied_body_it_names() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 20]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "S", "distance": 10, "operation": "new", "body_name": "Block", "name": "Block"}));
        run(&mut s, "solid.fillet", json!({"body": "Block", "radius": 2, "edges": [[20, 0, 10], [20, 20, 10]], "name": "Round"}));
        let one = volume(&mut s);
        assert!(one < 8000.0 - 1.0, "the fillet removes material: {one}");
        run(&mut s, "timeline.copy", json!({"features": ["S", "Block", "Round"]}));
        let r = run(&mut s, "timeline.paste", json!({"translate": [0, 40, 0]}));
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
        let round = r["pasted"][2]["name"].as_str().unwrap_or_default().to_string();
        let def = run(&mut s, "timeline.inspect", json!({"feature": round}));
        assert_eq!(def["definition"]["body"], "Block (copy)", "{def}");
        let m = run(&mut s, "inspect.measure", json!({}));
        assert_eq!(m["body_count"], 2, "{m}");
        assert!((volume(&mut s) - 2.0 * one).abs() < 1e-6, "both blocks are rounded: {m}");
    }

    /// One body's measures (bounding box, volume).
    fn measures(s: &mut Session, body: &str) -> serde_json::Value {
        run(s, "inspect.measure", json!({"bodies": [body]}))["bodies"][0].clone()
    }

    /// Pasting with a translate moves an extrude's `to` and `from` picks with it: the pasted posts
    /// run to the pasted roof and from the pasted shelf, not to and from the originals.
    #[test]
    fn a_pasted_extrude_runs_to_and_from_the_pasted_faces() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 40, "height": 2, "corner": [0, 0, 20], "name": "Roof", "body_name": "Roof"}));
        run(&mut s, "solid.box", json!({"length": 40, "width": 40, "height": 2, "corner": [0, 0, 10], "name": "Shelf", "body_name": "Shelf"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [5, 5], "p1": [10, 10]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "S", "distance": 5, "to": [7, 7, 20], "operation": "new", "name": "Up", "body_name": "Up"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "T"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [25, 25], "p1": [30, 30]}));
        run(&mut s, "sketch.finish", json!({}));
        run(
            &mut s,
            "solid.extrude",
            json!({"sketch": "T", "distance": 5, "from": [27, 27, 12], "operation": "new", "name": "On", "body_name": "On"}),
        );
        run(&mut s, "timeline.copy", json!({"features": ["Roof", "Shelf", "S", "Up", "T", "On"]}));
        let r = run(&mut s, "timeline.paste", json!({"translate": [100, 0, 10]}));
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
        let up = measures(&mut s, "Up (copy)");
        assert!((up["bbox"]["min"][2].as_f64().unwrap_or(0.0) - 10.0).abs() < 1e-6, "{up}");
        assert!((up["bbox"]["max"][2].as_f64().unwrap_or(0.0) - 30.0).abs() < 1e-6, "it ends on the pasted roof: {up}");
        let on = measures(&mut s, "On (copy)");
        assert!((on["bbox"]["min"][2].as_f64().unwrap_or(0.0) - 22.0).abs() < 1e-6, "it starts on the pasted shelf: {on}");
        assert!((on["bbox"]["max"][2].as_f64().unwrap_or(0.0) - 27.0).abs() < 1e-6, "{on}");
    }

    /// Pasting with a translate moves a hole's `to` pick with it: the pasted through hole still goes
    /// through, rather than stopping at the original's depth.
    #[test]
    fn a_pasted_hole_drills_to_the_moved_point() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 40, "height": 10, "name": "Block", "body_name": "Block"}));
        run(&mut s, "solid.hole", json!({"position": [20, 20, 10], "direction": [0, 0, -1], "diameter": 10, "to": [30, 30, -2], "name": "Drill"}));
        run(&mut s, "timeline.copy", json!({"features": ["Block", "Drill"]}));
        let r = run(&mut s, "timeline.paste", json!({"translate": [60, 0, -6]}));
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
        let (block, copy) = (measures(&mut s, "Block"), measures(&mut s, "Block (copy)"));
        let v = |m: &serde_json::Value| m["volume_mm3"].as_f64().unwrap_or(f64::NAN);
        assert!((v(&block) - (16000.0 - std::f64::consts::PI * 25.0 * 10.0)).abs() < 1.0, "a through hole: {block}");
        assert!((v(&copy) - v(&block)).abs() < 1e-6, "the pasted hole goes through too: {copy}");
    }

    /// Pasting with a translate moves a revolve's `to` pick with it: the pasted turn ends at the same
    /// angle about its moved axis.
    #[test]
    fn a_pasted_revolve_turns_to_the_moved_point() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "S"}));
        run(&mut s, "sketch.line", json!({"points": [[10, 0], [15, 0], [15, 5], [10, 5]], "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.revolve", json!({"sketch": "S", "axis": "y", "to": [0, 12, 2], "name": "Turn", "body_name": "Arc"}));
        run(&mut s, "timeline.copy", json!({"features": ["S", "Turn"]}));
        let r = run(&mut s, "timeline.paste", json!({"translate": [50, 0, 0]}));
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
        let (arc, copy) = (measures(&mut s, "Arc"), measures(&mut s, "Arc (copy)"));
        let v = |m: &serde_json::Value| m["volume_mm3"].as_f64().unwrap_or(f64::NAN);
        // A quarter turn of a 5 × 5 section whose centre is 12.5 from the axis.
        let quarter = 25.0 * std::f64::consts::FRAC_PI_2 * 12.5;
        assert!((v(&arc) - quarter).abs() < quarter * 1e-3, "{arc}");
        assert!((v(&copy) - v(&arc)).abs() < 1e-6, "the pasted arc turns a quarter too: {copy}");
    }

    /// A pasted coil that joins a body made by an earlier copy joins that copy, not the original.
    #[test]
    fn a_pasted_coil_joins_the_pasted_body() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 30, "width": 30, "height": 5, "name": "Base", "body_name": "Plate"}));
        run(
            &mut s,
            "solid.coil",
            json!({"diameter": 16, "pitch": 6, "revolutions": 3, "section_size": 2, "base": [15, 15, 5], "operation": "join", "targets": ["Plate"], "name": "Spring"}),
        );
        run(&mut s, "timeline.copy", json!({"features": ["Base", "Spring"]}));
        let r = run(&mut s, "timeline.paste", json!({"translate": [50, 0, 0]}));
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
        let spring = r["pasted"][1]["name"].as_str().unwrap_or_default().to_string();
        let def = run(&mut s, "timeline.inspect", json!({"feature": spring}));
        assert_eq!(def["definition"]["targets"], json!(["Plate (copy)"]), "{def}");
        let st = s.world_state();
        let v = |name: &str| solidcraft_kernel::measure(&st.body(name).unwrap().body).unwrap().volume;
        // A spring on the wrong plate is hundreds of mm³ off. The two plates are measured apart
        // (one moved 50 mm), which the boolean and the measure each round in their own way:
        // a few 1e-6 mm³ on 5000.
        assert!((v("Plate") - v("Plate (copy)")).abs() < 1e-8 * v("Plate"), "each plate carries one spring: {} vs {}", v("Plate"), v("Plate (copy)"));
    }
}
