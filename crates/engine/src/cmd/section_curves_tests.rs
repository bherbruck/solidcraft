use super::*;
use std::f64::consts::PI;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap()
}
fn xy(v: &Value) -> Vec2 {
    Vec2::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}
/// A 40 × 20 × 10 plate with a radius 5 hole through its middle.
fn plate() -> Session {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 20, "height": 10, "body_name": "Plate"}));
    run(&mut s, "solid.cylinder", json!({"radius": 5, "height": 10, "base": [20, 10, 0], "operation": "cut", "targets": ["Plate"]}));
    s
}

#[test]
fn a_plate_with_a_hole_cuts_to_a_rectangle_and_a_circle() {
    let mut s = plate();
    let undo = s.undo.len();
    let r = run(&mut s, CMD, json!({"plane": "XY", "at": 5}));
    assert_eq!(s.undo.len(), undo, "nothing to undo");
    assert_eq!(r["frame"]["origin"], json!([0.0, 0.0, 5.0]));
    assert_eq!(r["frame"]["x"], json!([1.0, 0.0, 0.0]));
    let loops = r["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 2, "{r}");
    let (outer, hole) = if loops[0]["area"].as_f64().unwrap() > 0.0 { (&loops[0], &loops[1]) } else { (&loops[1], &loops[0]) };
    for l in loops {
        assert_eq!(l["body"], "Plate");
        assert_eq!(l["closed"], true);
    }
    // Outer: four exact lines around 40 × 20, counter-clockwise.
    let segs = outer["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 4, "{outer}");
    let mut corners: Vec<Vec2> = Vec::new();
    for (i, g) in segs.iter().enumerate() {
        assert_eq!(g["type"], "line");
        let next = &segs[(i + 1) % 4];
        assert!(xy(&g["end"]).dist(xy(&next["start"])) < 1e-9, "joined end to end");
        corners.push(xy(&g["start"]));
    }
    for c in [Vec2::new(0., 0.), Vec2::new(40., 0.), Vec2::new(40., 20.), Vec2::new(0., 20.)] {
        assert!(corners.iter().any(|p| p.dist(c) < 1e-9), "corner {c:?} in {corners:?}");
    }
    assert!((outer["area"].as_f64().unwrap() - 800.0).abs() < 1e-9, "{}", outer["area"]);
    // Hole: one whole circle, clockwise.
    let segs = hole["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 1, "{hole}");
    let c = &segs[0];
    assert_eq!(c["type"], "arc");
    assert!(xy(&c["center"]).dist(Vec2::new(20., 10.)) < 1e-9, "{c}");
    assert!((c["radius"].as_f64().unwrap() - 5.0).abs() < 1e-9, "{c}");
    assert!((c["sweep_deg"].as_f64().unwrap() + 360.0).abs() < 1e-9, "{c}");
    assert!((hole["area"].as_f64().unwrap() + 25.0 * PI).abs() < 1e-9, "{}", hole["area"]);
}

#[test]
fn a_cylinder_cut_at_30_degrees_is_an_ellipse() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 40, "body_name": "Rod"}));
    // Tilted 30° about X: semi-axes 10 (along the frame's x, world −X) and 10 / cos 30°.
    let (sin, cos) = (30f64.to_radians().sin(), 30f64.to_radians().cos());
    let r = run(&mut s, CMD, json!({"plane": {"origin": [0, 0, 20], "normal": [0, sin, cos]}}));
    let loops = r["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 1, "{r}");
    let (a, b) = (10.0, 10.0 / cos);
    let on = |p: Vec2| ((p.x / a).powi(2) + (p.y / b).powi(2)).sqrt() - 1.0;
    let mut beziers = 0;
    for g in loops[0]["segments"].as_array().unwrap() {
        assert_eq!(g["type"], "spline", "{g}");
        // Fit points on the ellipse; the curve between them within a micrometre.
        for p in g["points"].as_array().unwrap() {
            assert!(on(xy(p)).abs() < 1e-9, "{p}");
        }
        for piece in g["beziers"].as_array().unwrap() {
            let [p0, p1, p2, p3] = [0, 1, 2, 3].map(|i| xy(&piece[i]));
            for t in [0.25, 0.5, 0.75] {
                let u = 1.0 - t;
                let q = p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t);
                assert!(on(q).abs() * a < 1e-3, "{q:?} is {} mm off", on(q) * a);
            }
            beziers += 1;
        }
    }
    assert!(beziers >= 8, "{r}");
    let area = loops[0]["area"].as_f64().unwrap();
    assert!((area - PI * a * b).abs() < 1e-4 * PI * a * b, "{area} vs {}", PI * a * b);
}

#[test]
fn section_curves_write_svg_and_dxf_one_group_per_body() {
    let dir = std::env::temp_dir().join(format!("solidcraft-section-curves-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = plate();
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 10, "corner": [50, 0, 0], "body_name": "Block"}));
    let svg = dir.join("cut.svg");
    let r = run(&mut s, CMD, json!({"plane": "XY", "at": 5, "path": svg.to_string_lossy()}));
    assert_eq!(r["path"], json!(svg.to_string_lossy()));
    let text = std::fs::read_to_string(&svg).unwrap();
    assert!(text.contains(r#"<g id="Plate""#) && text.contains(r#"<g id="Block""#), "{text}");
    // The hole as two half arcs of radius 5, the frame's y negated.
    assert_eq!(text.matches(" A 5 5 0 0 0 ").count(), 2, "{text}");
    assert!(text.contains("M 0 -20") || text.contains("L 0 -20"), "{text}");
    let dxf = dir.join("cut.dxf");
    run(&mut s, CMD, json!({"plane": "XY", "at": 5, "path": dxf.to_string_lossy()}));
    let text = std::fs::read_to_string(&dxf).unwrap().replace("\r\n", "\n");
    assert_eq!(text.matches("\nLINE\n").count(), 8, "{text}");
    assert_eq!(text.matches("\nCIRCLE\n").count(), 1, "{text}");
    assert!(text.contains("\nPlate\n") && text.contains("\nBlock\n"));
    // Read back into a sketch: the plate less its hole, and the block.
    let mut back = Session::default();
    let id = run(&mut back, "sketch.insert_dxf", json!({"text": text}))["sketch"].as_u64().unwrap();
    run(&mut back, "sketch.finish", json!({}));
    let areas: Vec<f64> = back.model.state().sketch(id).map(|ss| ss.profiles.iter().map(|p| p.area).collect()).unwrap_or_default();
    for want in [800.0 - 25.0 * PI, 25.0] {
        assert!(areas.iter().any(|a| (a - want).abs() < 1e-6), "{want} in {areas:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn section_curves_of_a_spline_cut_write_a_dxf_spline() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 40, "body_name": "Rod"}));
    let dir = std::env::temp_dir().join(format!("solidcraft-section-spline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dxf = dir.join("cut.dxf");
    run(&mut s, CMD, json!({"plane": {"origin": [0, 0, 20], "normal": [0, 1, 1]}, "path": dxf.to_string_lossy()}));
    let text = std::fs::read_to_string(&dxf).unwrap().replace("\r\n", "\n");
    assert!(text.contains("\nSPLINE\n"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn section_curves_select_bodies_and_reject_bad_input() {
    let mut s = plate();
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 10, "corner": [50, 0, 0], "body_name": "Block"}));
    let r = run(&mut s, CMD, json!({"plane": "XY", "at": 5, "bodies": ["Block"]}));
    assert_eq!(r["loops"].as_array().map(Vec::len), Some(1));
    assert_eq!(r["loops"][0]["body"], "Block");
    // A plane that misses everything.
    assert_eq!(run(&mut s, CMD, json!({"plane": "XY", "at": 50}))["loops"], json!([]));
    // Flipped: the frame's y and the normal turn over, the loops stay counter-clockwise outside.
    let r = run(&mut s, CMD, json!({"plane": "XY", "at": 5, "flip": true, "bodies": ["Block"]}));
    assert_eq!(r["frame"]["normal"][2], -1.0);
    assert!((r["loops"][0]["area"].as_f64().unwrap() - 25.0).abs() < 1e-9);
    for p in [
        json!({}),
        json!({"plane": "XY", "bodies": ["Nope"]}),
        json!({"plane": "XY", "bodies": []}),
        json!({"plane": "XY", "tolerance": 0}),
        json!({"plane": "XY", "tolerance": "x"}),
        json!({"plane": "XY", "path": "cut.png"}),
        json!({"plane": "XY", "path": ""}),
        json!({"plane": {"origin": [0, 0, 0], "normal": [0, 0, 0]}}),
        json!({"plane": "XY", "offset": "1e400"}),
    ] {
        assert!(s.execute(CMD, &p).is_err(), "{p}");
    }
}
