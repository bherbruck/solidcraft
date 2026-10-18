//! Editing a sketch rolls the timeline back to it; a body deleted after the sketch is not drawn
//! then (#1167), and is back in neither the viewport nor the browser's Bodies folder.

use super::build_scene;
use crate::{Services, SolveApp};
use serde_json::json;
use solidcraft_engine::Session;

/// `Kept` and `Gone` blocks, a sketch `Edited` made after both, and `Gone` deleted after it.
fn app() -> SolveApp {
    let mut s = Session::default();
    for (name, x) in [("Kept", 0.0), ("Gone", 20.0)] {
        s.execute("sketch.create", &json!({"plane": "XY", "name": format!("{name}Sketch")})).unwrap();
        s.execute("sketch.rectangle.two_point", &json!({"p0": [x, 0], "p1": [x + 10.0, 10]})).unwrap();
        s.execute("sketch.finish", &json!({})).unwrap();
        s.execute("solid.extrude", &json!({"sketch": format!("{name}Sketch"), "distance": 5, "body_name": name})).unwrap();
    }
    s.execute("sketch.create", &json!({"plane": "XY", "offset": "30 mm", "name": "Edited"})).unwrap();
    s.execute("sketch.rectangle.two_point", &json!({"p0": [0, 0], "p1": [5, 5]})).unwrap();
    s.execute("sketch.finish", &json!({})).unwrap();
    s.execute("solid.remove", &json!({"bodies": ["Gone"]})).unwrap();
    let mut app = SolveApp::new(s, Services::default());
    app.ui.ground_shadow = false;
    app.set_origin_shown(false);
    app
}

fn triangles(app: &SolveApp) -> usize {
    let scene = build_scene(app).merged();
    scene.tris.len() + scene.pbr.len() + scene.trans.len() + scene.glass.len()
}

#[test]
fn a_body_deleted_later_is_not_drawn_while_a_sketch_is_edited() {
    let mut app = app();
    let drawn = triangles(&app);
    assert!(drawn > 0, "the kept block is drawn");
    let sketch = app.session.doc.find_feature("Edited").map(|f| f.id).unwrap();
    app.edit_sketch(sketch);
    assert_eq!(app.session.active_sketch, Some(sketch));
    assert!(app.session.model.state().body("Gone").is_some(), "the rolled-back history has it");
    assert_eq!(triangles(&app), drawn, "only the kept block is drawn");
    app.finish_sketch();
    assert_eq!(triangles(&app), drawn);
}

#[test]
fn the_bodies_folder_leaves_out_a_body_deleted_later() {
    let mut app = app();
    let sketch = app.session.doc.find_feature("Edited").map(|f| f.id).unwrap();
    let names = |app: &SolveApp| crate::browser::body_rows(app).into_iter().collect::<Vec<_>>();
    assert_eq!(names(&app), ["Kept"]);
    app.edit_sketch(sketch);
    assert_eq!(names(&app), ["Kept"], "the body is not listed with an eye that cannot show it");
}
