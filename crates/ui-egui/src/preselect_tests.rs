//! Pre-selection fills every input of a command, each item going to the input that takes its
//! type, whether the command starts from a shortcut, the toolbar or a menu.

use serde_json::json;
use solidcraft_engine::doc::expr::Kind as ValueKind;
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::{Sel, Session};

use crate::dialogs::{Dialog, Kind, apply_commands};
use crate::{Services, SolveApp};

#[test]
fn feature_mirror_and_patterns_reopen_as_feature_replays_even_for_body_creators() {
    for (cmd, params) in [
        ("solid.mirror", json!({"features":["Base"],"plane":"YZ"})),
        (
            "solid.pattern.rectangular",
            json!({"features":["Base"],"dir1":[1,0,0],"count1":4,"spacing1":20,
            "dir2":[0.6,0.8,0],"count2":2,"spacing2":20,"alignment":"center","alignment2":"center","reversed":true,"reversed2":true}),
        ),
        ("solid.pattern.circular", json!({"features":["Base"],"axis":"Z","count":6,"angle":90,"alignment":"center","reversed":true})),
    ] {
        let mut app = app_with(json!([
            {"command":"solid.box","params":{"corner":[40,0,0],"length":2,"width":2,"height":2,"name":"Base"}}
        ]));
        app.session.execute(cmd, &params).unwrap();
        let feature = app.session.doc.features.last().unwrap().clone();
        // Exercise the save/reopen boundary as well as reopening the feature editor.
        app.session = Session::new(solidcraft_engine::doc::Document::from_json(&app.session.doc.to_json()).unwrap());
        let d = crate::dialogs::for_feature(&app, feature.id, None).unwrap();
        assert_eq!(d.inputs[0].accept, crate::selection::FEATURES, "{cmd}");
        assert_eq!(d.inputs[0].items, vec![Sel::Feature { id: app.session.doc.features[0].id }]);
        let (edit, p) = apply_commands(&app, &d).unwrap().remove(0);
        assert_eq!(edit, "timeline.redefine");
        assert_eq!(p["params"]["features"], json!(["Base"]));
        assert!(p["params"].get("bodies").is_none());
        app.session.execute(&edit, &p).unwrap();
        assert_eq!(app.session.doc.feature(feature.id).unwrap().kind, feature.kind, "{cmd}: accepting the reopened editor preserves its definition");
    }
}

#[test]
fn feature_pattern_controls_preview_without_committing_and_cancel_cleanly() {
    for command in ["solid.pattern.rectangular", "solid.pattern.circular"] {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"corner":[40,0,0],"length":2,"width":2,"height":2,"name":"Base"})).unwrap();
        let source = h.app.session.doc.features[0].id;
        h.app.session.selection = vec![Sel::Feature { id: source }];
        h.step(&json!({"start":command})).unwrap();
        let d = h.app.dialog.as_mut().unwrap();
        d.inputs[1].items = vec![Sel::Axis { name: if command.ends_with("rectangular") { "X".into() } else { "Z".into() } }];
        if command.ends_with("rectangular") {
            d.inputs[2].items = vec![Sel::Axis { name: "Y".into() }];
        }
        let saved = h.app.session.doc.to_json();
        let undo = h.app.session.undo.len();
        h.frames(3);
        for handle in ["dialog:Alignment:Center", "dialog:Alignment:Flip direction"] {
            h.step(&json!({"click":{"handle":handle}})).unwrap();
        }
        if command.ends_with("rectangular") {
            for handle in ["dialog:Alignment 2:Center", "dialog:Alignment 2:Flip direction"] {
                h.step(&json!({"click":{"handle":handle}})).unwrap();
            }
        }
        let d = h.app.dialog.as_ref().unwrap();
        let (_, p) = apply_commands(&h.app, d).unwrap().remove(0);
        assert_eq!(p["alignment"], json!("center"));
        assert_eq!(p["reversed"], json!(true));
        if command.ends_with("rectangular") {
            assert_eq!(p["alignment2"], json!("center"));
            assert_eq!(p["reversed2"], json!(true));
        }
        assert_eq!(h.app.session.doc.to_json(), saved);
        assert_eq!(h.app.session.undo.len(), undo);
        wait_feature_preview(&mut h);
        h.step(&json!({"key":"Escape"})).unwrap();
        h.frames(3);
        assert!(h.app.dialog.is_none());
        assert!(!h.app.preview.active);
        assert_eq!(h.app.session.doc.to_json(), saved);
    }
}

fn wait_feature_preview(h: &mut crate::scenario::Harness) {
    for _ in 0..300 {
        h.frames(2);
        if h.app.preview.active && !h.app.preview.busy {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.app.preview.active && !h.app.preview.busy, "preview failed: {:?}", h.app.preview.error);
    assert!(h.app.preview.error.is_none());
}

#[test]
fn saved_feature_pattern_vectors_are_visible_clearable_and_replaceable() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("solid.box", json!({"length":2,"width":2,"height":2,"name":"Base"})).unwrap();
    h.app
        .run(
            "solid.pattern.rectangular",
            json!({"features":["Base"],"dir1":[1,1,0],"count1":2,"spacing1":30,
        "dir2":[0.6,0.8,0],"count2":2,"spacing2":50}),
        )
        .unwrap();
    let id = h.app.session.doc.features.last().unwrap().id;
    h.step(&json!({"call":"ui.editFeature","params":{"feature":id}})).unwrap();
    h.step(&json!({"click":{"handle":"dialog:clear:Direction 2"}})).unwrap();
    let d = h.app.dialog.as_ref().unwrap();
    let (_, p) = apply_commands(&h.app, d).unwrap().remove(0);
    assert!(p["params"].get("dir2").is_none(), "clearing the saved second vector makes a single row: {d:?} -> {p}");
    assert_eq!(p["params"]["dir1"], json!([1.0, 1.0, 0.0]));
    h.step(&json!({"click":{"handle":"dialog:clear:Direction"}})).unwrap();
    assert!(apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).is_err());
    let d = h.app.dialog.as_mut().unwrap();
    d.pick(&h.app.session, Sel::Axis { name: "X".into() });
    let (_, p) = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap().remove(0);
    assert_eq!(p["params"]["dir1"], json!([1.0, 0.0, 0.0]));
}

#[test]
fn feature_mirror_picks_clear_preview_and_replay_the_source_cut() {
    let mut h = crate::scenario::Harness::new();
    for (cmd, p) in [
        ("solid.box", json!({"length":100,"width":50,"height":5,"corner":[0,0,0]})),
        ("solid.hole", json!({"position":[30,25,5],"diameter":4,"name":"Hole"})),
        ("construct.plane.offset", json!({"base":"YZ","offset":50,"name":"Mid"})),
    ] {
        h.app.run(cmd, p).unwrap();
    }
    let source = Sel::Feature { id: h.app.session.doc.find_feature("Hole").unwrap().id };
    h.app.session.selection = vec![source.clone()];
    h.step(&json!({"start":"solid.mirror"})).unwrap();
    let saved = h.app.session.doc.to_json();
    let undo = h.app.session.undo.len();
    let d = h.app.dialog.as_mut().unwrap();
    d.active = 0;
    d.pick(&h.app.session, source.clone());
    assert!(d.inputs[0].items.is_empty());
    d.pick(&h.app.session, source);
    d.active = 1;
    d.pick(&h.app.session, Sel::Plane { name: "Mid".into() });
    wait_feature_preview(&mut h);
    assert_eq!(h.app.session.doc.to_json(), saved);
    assert_eq!(h.app.session.undo.len(), undo);
    h.step(&json!({"click":{"handle":"dialog:clear:Mirror Plane"}})).unwrap();
    assert!(h.app.dialog.as_ref().unwrap().inputs[1].items.is_empty());
    assert!(apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).is_err());
    let d = h.app.dialog.as_mut().unwrap();
    d.pick(&h.app.session, Sel::Plane { name: "Mid".into() });
    wait_feature_preview(&mut h);
    h.step(&json!({"click":{"handle":"dialog:OK"}})).unwrap();
    h.step(&json!({"until":{"dialog":null,"features":4}})).unwrap();
    let state = h.app.session.world_state();
    assert_eq!(state.bodies.len(), 1, "a mirrored hole cuts the plate instead of copying it");
    assert!(!state.bodies[0].body.contains(Vec3::new(70.0, 25.0, 2.5)));
    let mirror = h.app.session.doc.features.last().unwrap().id;
    let reopened = crate::dialogs::for_feature(&h.app, mirror, None).unwrap();
    assert_eq!(reopened.inputs[0].accept, crate::selection::FEATURES);
    assert_eq!(reopened.inputs[1].items, vec![Sel::Plane { name: "Mid".into() }]);
}

#[test]
fn a_face_extrude_keeps_the_picked_body_when_faces_coincide() {
    let mut app = app_with(json!([
        {"command":"solid.box","params":{"length":40,"width":30,"height":20,"body_name":"Other"}},
        {"command":"solid.box","params":{"length":40,"width":30,"height":20,"body_name":"Wall"}}
    ]));
    let before = app.session.model.state();
    let pick = Sel::Face { body: "Wall".into(), index: 2, point: Vec3::new(40.0, 15.0, 10.0) };
    let d = start(&mut app, vec![pick], "solid.extrude");
    let (command, params) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(params["face_body"], json!("Wall"));
    app.session.execute(&command, &params).unwrap();
    let after = app.session.model.state();
    assert_eq!(after.bodies.len(), 2);
    assert!(after.body("Other").unwrap().body.same(&before.body("Other").unwrap().body));
    assert!(after.body("Wall").unwrap().body.contains(Vec3::new(49.0, 15.0, 10.0)));
}

#[test]
fn path_plane_click_retains_source_and_previews_then_edits_in_place() {
    let mut h = crate::scenario::Harness::new();
    let source = h.app.run("sketch.create", json!({"plane":"XY","name":"Bridge"})).unwrap()["sketch"].as_u64().unwrap();
    h.app.run("sketch.options", json!({"sketch_3d":true})).unwrap();
    let wire = h.app.run("sketch.line3d", json!({"points":[[10,20,10],[30,35,40]]})).unwrap()["curves"][0].as_str().unwrap().to_string();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("sketch.create", json!({"plane":"XY","name":"Other"})).unwrap();
    h.app.run("sketch.options", json!({"sketch_3d":true})).unwrap();
    let duplicate = h.app.run("sketch.line3d", json!({"points":[[-30,0,0],[-20,0,0]]})).unwrap()["curves"][0].as_str().unwrap().to_string();
    assert_eq!(wire, duplicate, "exercise duplicate curve ids in different sketches");
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.ui.show_origin = false;
    h.settle();
    h.call("ui.view", json!({"view":"home"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.app.start("construct.plane.along_path");
    h.frames(2);
    h.step(&json!({"click":{"world":[20,27.5,25]}})).unwrap();
    let d = h.app.dialog.as_ref().unwrap();
    let commands = apply_commands(&h.app, d).unwrap();
    assert_eq!(commands[0].1["path"]["sketch"], json!(source));
    let history = h.app.session.undo.len();
    let count = h.app.session.doc.features.len();
    let preview = h.app.session.preview_visible_world(&commands).unwrap();
    let built = crate::preview::compute(&h.app.session, &commands, crate::preview::Colors::from_tokens(&crate::theme::Tokens::dark())).unwrap();
    assert!(!built.scene.xray.is_empty(), "construction preview must contain visible geometry");
    let plane = preview.after.construct.last().unwrap();
    let solidcraft_engine::doc::construct::ConstructGeom::Plane(pl) = plane.geom else { panic!("plane") };
    assert!(pl.origin.dist(Vec3::new(20.0, 27.5, 25.0)) < 1e-6);
    assert_eq!(h.app.session.doc.features.len(), count);
    assert_eq!(h.app.session.undo.len(), history);
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    let feature = h.app.session.doc.features.last().unwrap().clone();
    assert_eq!(h.app.session.undo.len(), history + 1);
    h.app.edit_feature(feature.id);
    h.frames(2);
    let d = h.app.dialog.as_mut().unwrap();
    assert!(matches!(d.kind,Kind::PathPlane{sketch:Some(s),..} if s==source));
    if let Kind::PathPlane { t, .. } = &mut d.kind {
        *t = "0.25".into();
    }
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    assert_eq!(h.app.session.doc.features.len(), count + 1);
    let p = solidcraft_engine::view::construction_planes(&h.app.session).into_iter().find(|p| p.0 == feature.id).unwrap().2;
    assert!(p.origin.dist(Vec3::new(15.0, 23.75, 17.5)) < 1e-6, "{p:?}");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.session.doc.feature(feature.id).unwrap().kind, feature.kind);
}

#[test]
fn cylinder_emboss_empty_target_face_applies_automatic_wrap_and_deboss() {
    for deboss in [false, true] {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"})).unwrap();
        let initial = h.app.session.model.state().body("Sleeve").unwrap().body.tessellate_with(1e-4, true).unwrap().measure().volume;
        let sketch =
            h.app.run("sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"})).unwrap()["sketch"]
                .as_u64()
                .unwrap();
        h.app.run("sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
        h.app.session.selection = vec![Sel::Profile { sketch, index: 0 }];
        h.app.start("solid.emboss");
        h.frames(2);
        if deboss {
            h.settle();
            h.step(&json!({"click":{"handle":"dialog:Type"}})).unwrap();
            h.step(&json!({"click":{"handle":"dialog:Type:Deboss"}})).unwrap();
        }
        let dialog = h.app.dialog.as_ref().unwrap();
        assert!(dialog.inputs[1].items.is_empty());
        let commands = apply_commands(&h.app, dialog).unwrap();
        assert!(commands[0].1.get("face").is_none());
        let preview = h.app.session.preview_world(&commands).unwrap();
        let expected = if deboss { -19.0 } else { 21.0 };
        let preview_delta = preview.after.body("Sleeve").unwrap().body.tessellate_with(1e-4, true).unwrap().measure().volume - initial;
        assert!(
            (preview_delta - expected).abs() < 0.05,
            "deboss={deboss}, preview delta={preview_delta}, expected={expected}, commands={commands:?}"
        );
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
        let feature = h.app.session.doc.features.last().unwrap().clone();
        let solidcraft_engine::doc::FeatureKind::Emboss { face: Some(face), deboss: actual, .. } = &feature.kind else {
            panic!("automatic Apply must save its cylinder anchor: {:?}", feature.kind)
        };
        assert_eq!(*actual, deboss);
        assert_eq!(face.body, "Sleeve");
        assert!(
            (h.app.session.model.state().body("Sleeve").unwrap().body.tessellate_with(1e-4, true).unwrap().measure().volume - initial - expected)
                .abs()
                < 0.05
        );
        h.app.edit_feature(feature.id);
        h.frames(2);
        assert!(matches!(h.app.dialog.as_ref().unwrap().inputs[1].items.first(), Some(Sel::Face { body, .. }) if body == "Sleeve"));
        h.step(&json!({"key":"Escape"})).unwrap();
        assert_eq!(h.app.session.doc.feature(feature.id).unwrap().kind, feature.kind);
    }
}

#[test]
fn cylinder_emboss_target_face_and_edit_keep_the_anchored_operation() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"})).unwrap();
    let sketch = h.app.run("sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"})).unwrap()["sketch"]
        .as_u64()
        .unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.session.selection = vec![Sel::Profile { sketch, index: 0 }];
    h.app.start("solid.emboss");
    h.frames(2);
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[0].items.len(), 1);
    let planar = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    assert!(planar[0].1.get("face").is_none());
    h.call("ui.dialogInput", json!({"index":1}));
    h.call("ui.view", json!({"view":"back"}));
    h.frames(2);
    h.step(&json!({"click":{"world":[0,10,7.5]}})).unwrap();
    let dialog = h.app.dialog.as_ref().unwrap();
    assert!(matches!(dialog.inputs[1].items.first(), Some(Sel::Face { body, .. }) if body == "Sleeve"));
    let commands = apply_commands(&h.app, dialog).unwrap();
    assert_eq!(commands[0].1["targets"], json!(["Sleeve"]));
    assert!(commands[0].1.get("face").is_some());
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    let feature = h.app.session.doc.features.last().unwrap().id;
    let before = serde_json::to_value(&h.app.session.doc.feature(feature).unwrap().kind).unwrap();
    h.app.edit_feature(feature);
    h.frames(2);
    let edited = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    assert_eq!(edited[0].0, "timeline.redefine");
    assert_eq!(edited[0].1["params"]["targets"], json!(["Sleeve"]));
    assert!(edited[0].1["params"].get("face").is_some());
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    assert_eq!(serde_json::to_value(&h.app.session.doc.feature(feature).unwrap().kind).unwrap(), before);
}

#[test]
fn cylinder_emboss_edit_retains_second_source_and_author_paths_and_all_profiles() {
    let mut h = crate::scenario::Harness::new();
    let source = h.app.run("component.create", json!({"name":"Source"})).unwrap()["component"].as_u64().unwrap();
    h.app.run("solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"})).unwrap();
    h.app.run("occurrence.move", json!({"occurrence":"Source:1","axis":[0,0,1],"angle":"90 deg","translate":[50,0,0]})).unwrap();
    let source_second = h.app.run("occurrence.copy", json!({"component":source,"translate":[80,0,0]})).unwrap()["occurrence"].as_u64().unwrap();
    h.app.run("component.activate", json!({"component":"root"})).unwrap();
    let author = h.app.run("component.create", json!({"name":"Author"})).unwrap()["component"].as_u64().unwrap();
    h.app.run("occurrence.move", json!({"occurrence":"Author:1","axis":[1,0,0],"angle":"90 deg","translate":[-100,40,30]})).unwrap();
    let author_second = h.app.run("occurrence.copy", json!({"component":author,"translate":[0,100,0]})).unwrap()["occurrence"].as_u64().unwrap();
    h.app.run("component.activate", json!({"component":author,"occurrence":author_second})).unwrap();
    h.app.run("sketch.create", json!({"plane":{"origin":[120,0,0],"x_dir":[0,-1,0],"y_dir":[0,0,1]},"name":"Badge"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let target = h.app.session.occurrence_bodies(&[source_second], false).unwrap().into_iter().next().unwrap();
    let feature = h.app.run("solid.emboss", json!({"sketch":"Badge","profiles":"all","face":[120,0,7.5],"depth":"1mm","targets":[target]})).unwrap()
        ["feature"]
        .as_u64()
        .unwrap();
    let before = h.app.session.doc.feature(feature).unwrap().clone();
    let solidcraft_engine::doc::FeatureKind::Emboss { face: Some(face), .. } = &before.kind else { panic!("selected face") };
    assert_eq!(face.path, vec![source_second]);
    assert_eq!(face.author_path, vec![author_second]);
    h.app.run("component.activate", json!({"component":"root"})).unwrap();
    h.app.edit_feature(feature);
    h.frames(2);
    let dialog = h.app.dialog.as_ref().unwrap();
    let Sel::Face { body, point, .. } = &dialog.inputs[1].items[0] else { panic!("refilled face") };
    assert_eq!(body, &target);
    assert!(point.dist(Vec3::new(120.0, 0.0, 7.5)) < 1e-8);
    let commands = apply_commands(&h.app, dialog).unwrap();
    assert_eq!(commands[0].1["params"]["profiles"], json!("all"));
    let Sel::Profile { sketch, index } = dialog.inputs[0].items[0] else { panic!("refilled profile") };
    let plane = crate::dialogs::emboss_profile_plane(&h.app, sketch).unwrap().unwrap();
    assert!(plane.origin.dist(Vec3::new(120.0, 0.0, 0.0)) < 1e-8, "{plane:?}");
    let mut guide = crate::gpu::GpuScene::default();
    crate::viewport::profile_fill(&mut guide, &h.app, sketch, index, egui::Color32::BLUE, egui::Color32::BLUE);
    let (vertices, remainder) = guide.trans.as_chunks::<{ crate::gpu::TRI_SIZE }>();
    assert!(remainder.is_empty() && !vertices.is_empty());
    let lift = h.app.cam.back() * (h.app.cam.half_height() * 0.002);
    for vertex in vertices {
        let coordinate = |offset| f32::from_le_bytes(vertex[offset..offset + 4].try_into().unwrap()) as f64;
        let point = Vec3::new(coordinate(0), coordinate(4), coordinate(8)) - lift;
        assert!((point.x - 120.0).abs() < 1e-4 && point.y.abs() < 2.01 && (4.99..=10.01).contains(&point.z), "{point:?}");
    }
    let preview = h.app.session.preview_world(&commands).unwrap();
    let volume = preview.after.body(&target).unwrap().mesh().measure().volume;
    assert!(volume > preview.before.body(&target).unwrap().mesh().measure().volume + 10.0);
    let colors = crate::preview::Colors::from_tokens(&crate::theme::Tokens::get());
    let built = crate::preview::compute(&h.app.session, &commands, colors).unwrap();
    assert!(built.replaced.contains(&target) && !built.scene.tris.is_empty(), "{:?}", built.replaced);
    let source_first = h.app.session.doc.occurrence_of(source).unwrap().id;
    h.app.run("browser.visibility", json!({"paths":[[source_first]],"visible":false})).unwrap();
    let hidden = crate::preview::compute(&h.app.session, &commands, colors).unwrap();
    assert_eq!(hidden.replaced, vec![target.clone()]);
    let visible = h.app.session.preview_visible_world(&commands).unwrap();
    assert!(visible.before.body("Sleeve").is_none() && visible.after.body("Sleeve").is_none());
    assert!(visible.after.body(&target).is_some() && !hidden.scene.tris.is_empty());
    let mut shared = h.app.session.scratch();
    shared.execute("component.activate", &json!({"component":source,"occurrence":source_first})).unwrap();
    let new_body = shared
        .preview_visible_world(&[("solid.box".into(), json!({"body_name":"NewPreview","length":2,"width":2,"height":2,"operation":"new"}))])
        .unwrap();
    let added = new_body.after.bodies.iter().filter(|body| new_body.before.body(&body.name).is_none()).collect::<Vec<_>>();
    assert_eq!(added.len(), 1, "{:?}", added.iter().map(|body| &body.name).collect::<Vec<_>>());
    assert!(added[0].name.contains("Source:2"), "{}", added[0].name);
    let doc = h.app.session.doc.to_json();
    let history = (h.app.session.undo.len(), h.app.session.redo.len());
    let selection = h.app.session.selection.clone();
    let Kind::Emboss { depth, .. } = &mut h.app.dialog.as_mut().unwrap().kind else { panic!("Emboss editor") };
    *depth = "2 mm".into();
    let changed = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    let changed_preview = h.app.session.preview_world(&changed).unwrap();
    assert!(changed_preview.after.body(&target).unwrap().mesh().measure().volume > volume + 10.0);
    let mut invalid = changed.clone();
    invalid[0].1["params"]["depth"] = json!("unknown_depth");
    assert!(h.app.session.preview_world(&invalid).is_err());
    assert_eq!(h.app.session.doc.to_json(), doc);
    assert_eq!((h.app.session.undo.len(), h.app.session.redo.len()), history);
    assert_eq!(h.app.session.selection, selection);
    h.step(&json!({"key":"Escape"})).unwrap();
    assert!(h.app.dialog.is_none());
    assert_eq!(h.app.session.doc.feature(feature).unwrap().kind, before.kind);
    h.app.edit_feature(feature);
    h.frames(2);
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    let after = h.app.session.doc.feature(feature).unwrap();
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.face_names, before.face_names);
    assert_eq!(h.app.session.active_component, 0);
}

fn app_with(script: serde_json::Value) -> SolveApp {
    let mut s = Session::default();
    s.run_script(&script).unwrap();
    SolveApp::new(s, Services::default())
}

#[test]
fn cylinder_emboss_profile_guide_uses_target_or_unique_foreign_sketch_frame() {
    for target_owned in [true, false] {
        let mut h = crate::scenario::Harness::new();
        let source = if target_owned { h.app.run("component.create", json!({"name":"Source"})).unwrap()["component"].as_u64().unwrap() } else { 0 };
        h.app.run("solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"})).unwrap();
        let (profile_owner, expected, target, face) = if target_owned {
            h.app.run("occurrence.move", json!({"occurrence":"Source:1","axis":[0,0,1],"angle":"90 deg","translate":[50,0,0]})).unwrap();
            let second = h.app.run("occurrence.copy", json!({"component":source,"translate":[80,0,0]})).unwrap()["occurrence"].as_u64().unwrap();
            let target = h.app.session.occurrence_bodies(&[second], false).unwrap().remove(0);
            (source, Vec3::new(120.0, 0.0, 0.0), target, json!([120, 0, 7.5]))
        } else {
            let profile = h.app.run("component.create", json!({"name":"Profile"})).unwrap()["component"].as_u64().unwrap();
            h.app.run("occurrence.move", json!({"occurrence":"Profile:1","axis":[0,0,1],"angle":"90 deg","translate":[50,20,0]})).unwrap();
            (profile, Vec3::new(0.0, 10.0, 0.0), "Sleeve".to_string(), json!([0, 10, 7.5]))
        };
        let plane = if target_owned {
            json!({"origin":[40,0,0],"x_dir":[0,-1,0],"y_dir":[0,0,1]})
        } else {
            json!({"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]})
        };
        let sketch = h.app.run("sketch.create", json!({"plane":plane,"name":"Badge"})).unwrap()["sketch"].as_u64().unwrap();
        h.app.run("sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
        h.app.run("component.activate", json!({"component":"root"})).unwrap();
        let feature =
            h.app.run("solid.emboss", json!({"sketch":sketch,"profiles":"all","face":face,"depth":"1mm","targets":[target]})).unwrap()["feature"]
                .as_u64()
                .unwrap();
        h.app.edit_feature(feature);
        h.frames(2);
        assert!(crate::dialogs::emboss_profile_plane(&h.app, sketch).unwrap().unwrap().origin.dist(expected) < 1e-8);
        if !target_owned {
            h.app.run("occurrence.copy", json!({"component":profile_owner,"translate":[100,0,0]})).unwrap();
            assert!(crate::dialogs::emboss_profile_plane(&h.app, sketch).unwrap_err().contains("ambiguous"));
            let mut guide = crate::gpu::GpuScene::default();
            crate::viewport::profile_fill(&mut guide, &h.app, sketch, 0, egui::Color32::BLUE, egui::Color32::BLUE);
            assert!(guide.trans.is_empty() && guide.lines.is_empty());
        }
    }
}

fn boxed() -> SolveApp {
    app_with(json!([{"command": "solid.box", "params": {"length": 40, "width": 30, "height": 20}}]))
}

/// The face of Body1 whose centre is at `c`.
fn face(app: &SolveApp, c: Vec3) -> Sel {
    let st = app.session.model.state();
    let b = st.body("Body1").unwrap();
    let f = b.body.faces(0.01).unwrap().into_iter().find(|f| f.centroid.dist(c) < 1e-6).unwrap();
    Sel::Face { body: "Body1".into(), index: f.index, point: c }
}

fn edge(app: &SolveApp, p: Vec3) -> Sel {
    let st = app.session.model.state();
    let m = st.body("Body1").unwrap().mesh();
    let i = m.edges.iter().position(|e| e.windows(2).any(|w| p.dist_to_segment(w[0], w[1]) < 1e-9)).unwrap();
    Sel::Edge { body: "Body1".into(), index: i, point: p }
}

fn start(app: &mut SolveApp, sels: Vec<Sel>, id: &str) -> Dialog {
    app.session.selection = sels;
    app.start(id);
    app.dialog.clone().unwrap()
}

#[test]
fn profiles_then_extrude_takes_them_all() {
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.circle.center", "params": {"center": [0, 0], "radius": 5}},
        {"command": "sketch.circle.center", "params": {"center": [20, 0], "radius": 5}},
        {"command": "sketch.finish", "params": {}},
    ]));
    let sk = app.session.doc.features.first().unwrap().id;
    let d = start(&mut app, vec![Sel::Profile { sketch: sk, index: 0 }, Sel::Profile { sketch: sk, index: 1 }], "solid.extrude");
    assert_eq!(d.inputs[0].items.len(), 2);
    assert!(d.focus, "the value box takes the keyboard");
    assert!(apply_commands(&app, &d).is_ok());
}

/// #684: with nothing selected, Extrude and Revolve take the last sketch's single profile only
/// while no feature uses it yet. A cube's sketch lies on its bottom face: taking that profile
/// again made the dialog default to Cut, so OK cut into the part nobody had picked.
#[test]
fn used_profile_of_the_last_sketch_is_not_pre_picked() {
    let mut h = crate::scenario::Harness::new();
    let base = h.app.run("sketch.create", json!({"plane":"XY"})).unwrap()["sketch"].as_u64().unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[20,20]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("solid.extrude", json!({"distance":20})).unwrap();
    h.app.run("select.clear", json!({})).unwrap();
    let features = h.app.session.doc.features.len();
    h.call("ui.start", json!({"command":"solid.extrude"}));
    h.frames(2);
    let ui = h.call("ui.selection", json!({}))["result"].clone();
    assert_eq!(ui["selection"], json!([]));
    assert_eq!(ui["dialog"], json!([{"label":"Profiles","items":[]}]), "the extruded sketch's profile is taken again");
    let d = h.app.dialog.as_ref().unwrap();
    assert!(matches!(d.kind, Kind::Extrude { operation: 1, auto_op: true, .. }), "Join, not Cut: {:?}", d.kind);
    assert_eq!(d.active, 0, "picks go to Profiles");
    // OK with nothing picked builds nothing: the cube stays whole.
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_some(), "OK waits for a pick");
    assert_eq!(h.app.session.doc.features.len(), features);
    h.step(&json!({"expect":{"bodies":1,"volume":8000}})).unwrap();
    h.step(&json!({"key":"Escape"})).unwrap();
    assert!(h.app.dialog.is_none());
    // Revolve takes the same default.
    h.app.start("solid.revolve");
    h.frames(2);
    assert!(h.app.dialog.as_ref().unwrap().inputs[0].items.is_empty(), "Revolve takes the used profile");
    h.step(&json!({"key":"Escape"})).unwrap();
    // A sketch being edited still offers its profile, used or not.
    h.app.edit_sketch(base);
    h.frames(2);
    h.app.start("solid.extrude");
    h.frames(2);
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[0].items, vec![Sel::Profile { sketch: base, index: 0 }]);
    h.step(&json!({"key":"Escape"})).unwrap();
    h.app.finish_sketch();
    // A new sketch on the top face, not used yet, is taken right after Finish Sketch.
    let top = h.app.run("sketch.create", json!({"plane":{"origin":[0,0,20],"x_dir":[1,0,0],"y_dir":[0,1,0]}})).unwrap()["sketch"].as_u64().unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[15,15]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("select.clear", json!({})).unwrap();
    h.app.start("solid.extrude");
    h.frames(2);
    let d = h.app.dialog.as_ref().unwrap();
    assert_eq!(d.inputs[0].items, vec![Sel::Profile { sketch: top, index: 0 }]);
    assert!(matches!(d.kind, Kind::Extrude { operation: 1, .. }), "a boss on the top face joins: {:?}", d.kind);
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog);
    h.step(&json!({"expect":{"bodies":1,"volume":9000}})).unwrap();
}

/// #684: what stops the pre-pick is a feature building from the sketch's profile (not just an
/// extrude), anywhere in the timeline; a feature that only follows its curves does not.
#[test]
fn profile_use_not_curve_use_stops_the_pre_pick() {
    // A surface patch fills the sketch's region.
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Region"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [20, 10]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "surface.patch", "params": {"sketch": "Region"}},
    ]));
    assert_eq!(app.session.doc.features.len(), 2);
    assert!(start(&mut app, vec![], "solid.extrude").inputs[0].items.is_empty(), "the patched region is taken again");
    // Rolled back to just after the sketch: the extrude after the marker still builds from it.
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Base"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [20, 20]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 20}},
        {"command": "timeline.roll_to", "params": {"position": 1}},
    ]));
    let base = app.session.doc.features.first().unwrap().id;
    assert_eq!(app.session.world_state().sketch(base).map(|s| s.profiles.len()), Some(1));
    assert!(start(&mut app, vec![], "solid.extrude").inputs[0].items.is_empty(), "the rolled-back extrude's profile is taken");
    // A pipe follows the circle; the disc inside it is used by nothing.
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Path"}},
        {"command": "sketch.circle.center", "params": {"center": [0, 0], "radius": 20, "id": "c"}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.pipe", "params": {"path_sketch": "Path", "path": ["c"], "diameter": 4}},
    ]));
    let path = app.session.doc.features.first().unwrap().id;
    assert_eq!(app.session.doc.features.len(), 2);
    assert_eq!(start(&mut app, vec![], "solid.extrude").inputs[0].items, vec![Sel::Profile { sketch: path, index: 0 }]);
}

#[test]
fn edges_then_fillet_takes_them_with_chains() {
    let mut app = boxed();
    let (a, b) = (edge(&app, Vec3::new(0.0, 0.0, 10.0)), edge(&app, Vec3::new(20.0, 0.0, 20.0)));
    let d = start(&mut app, vec![a, b], "solid.fillet");
    assert_eq!(d.inputs[0].items.len(), 2, "box edges are not tangent to others");
    let cmds = apply_commands(&app, &d).unwrap();
    assert_eq!(cmds[0].1["edges"].as_array().unwrap().len(), 2);
}

#[test]
fn face_then_extrude_press_pulls_it() {
    let mut app = boxed();
    let top = face(&app, Vec3::new(20.0, 15.0, 20.0));
    let d = start(&mut app, vec![top], "solid.extrude");
    let cmds = apply_commands(&app, &d).unwrap();
    assert!(cmds[0].1.get("face").is_some());
}

#[test]
fn faces_then_shell_and_face_then_hole() {
    let mut app = boxed();
    let top = face(&app, Vec3::new(20.0, 15.0, 20.0));
    let front = face(&app, Vec3::new(20.0, 0.0, 10.0));
    let d = start(&mut app, vec![top.clone(), front], "solid.shell");
    assert_eq!(d.inputs[0].items.len(), 2);
    let d = start(&mut app, vec![top], "solid.hole");
    assert_eq!(d.inputs[0].items.len(), 1);
    let cmds = apply_commands(&app, &d).unwrap();
    assert!(cmds[0].1.get("position").is_some());
}

#[test]
fn line_and_profile_then_revolve_routes_each() {
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XZ"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [10, 0], "p1": [20, 10]}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [0, 30]], "ids": ["axis"]}},
    ]));
    let sk = app.session.active_sketch.unwrap();
    let d = start(&mut app, vec![Sel::SketchCurve { id: "axis".into() }, Sel::Profile { sketch: sk, index: 0 }], "solid.revolve");
    assert_eq!(d.inputs[0].items, vec![Sel::Profile { sketch: sk, index: 0 }]);
    assert_eq!(d.inputs[1].items, vec![Sel::SketchCurve { id: "axis".into() }]);
}

#[test]
fn face_then_move_takes_the_body_and_odd_items_are_ignored() {
    let mut app = boxed();
    let top = face(&app, Vec3::new(20.0, 15.0, 20.0));
    let d = start(&mut app, vec![top, Sel::Axis { name: "X".into() }], "solid.move");
    assert_eq!(d.inputs[0].items, vec![Sel::Body { name: "Body1".into() }]);
    assert!(matches!(d.kind, Kind::Move { .. }));
}

#[test]
fn hole_at_sketch_points() {
    let mut app = app_with(json!([
        {"command": "solid.box", "params": {"length": 40, "width": 30, "height": 20}},
        {"command": "sketch.create", "params": {"plane": {"face": [20, 15, 20]}}},
        {"command": "sketch.point", "params": {"point": [10, 10], "id": "h1"}},
        {"command": "sketch.point", "params": {"point": [30, 20], "id": "h2"}},
        {"command": "sketch.finish", "params": {}},
    ]));
    let sk = app.session.doc.features.iter().find(|f| matches!(f.kind, solidcraft_engine::doc::FeatureKind::Sketch { .. })).unwrap().id;
    app.start("solid.hole");
    let mut d = app.dialog.clone().unwrap();
    if let Kind::Hole { opts, .. } = &mut d.kind {
        opts.multiple = true;
    }
    d.inputs = vec![crate::selection::SelInput::new("Points", crate::selection::POINTS, true)];
    d.inputs[0].items = vec![Sel::SketchPoint { id: format!("{sk}:h1") }, Sel::SketchPoint { id: format!("{sk}:h2") }];
    let cmds = apply_commands(&app, &d).unwrap();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].1["points"], json!(["h1", "h2"]));
    let r = app.session.execute(&cmds[0].0, &cmds[0].1);
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn a_hole_is_patterned_as_a_feature() {
    let mut app = app_with(json!([
        {"command": "solid.box", "params": {"length": 60, "width": 40, "height": 10}},
        {"command": "solid.hole", "params": {"position": [10, 10, 10], "diameter": 5}}
    ]));
    let hole = app.session.doc.features.last().unwrap().id;
    // Picked in the timeline before the command: the objects become features.
    let mut d = start(&mut app, vec![Sel::Feature { id: hole }], "solid.pattern.rectangular");
    assert_eq!(d.inputs[0].accept, crate::selection::FEATURES);
    assert_eq!(d.inputs[0].items, vec![Sel::Feature { id: hole }]);
    d.inputs[1].items = vec![Sel::Axis { name: "X".into() }];
    if let Kind::PatternRect { count, spacing, .. } = &mut d.kind {
        *count = "4".into();
        *spacing = "12 mm".into();
    }
    let cmds = apply_commands(&app, &d).unwrap();
    assert_eq!(cmds[0].1["features"], json!(["Hole1"]));
    let before = app.session.model.state().bodies[0].mesh().measure().volume;
    for (id, p) in cmds {
        app.session.execute(&id, &p).unwrap();
    }
    let after = app.session.model.state().bodies[0].mesh().measure().volume;
    assert!(after < before - 500.0, "three more holes: {before} -> {after}");
    // On the canvas, a hole's wall stands for the hole; the top face for the box.
    let st = app.session.model.state();
    let m = st.bodies[0].mesh();
    let face_at = |p: Vec3| {
        let t = m.triangles.iter().position(|t| super::dialogs::point_tri_dist(p, m.tri(t).unwrap()) < 0.05).unwrap();
        let [a, b, c] = m.tri(&m.triangles[t]).unwrap();
        crate::viewport::Hit::Face { body: "Body1".into(), index: m.tri_face[t] as usize, point: (a + b + c) * (1.0 / 3.0) }
    };
    let mut d = start(&mut app, vec![], "solid.pattern.circular");
    d.inputs[0].accept = crate::selection::FEATURES;
    let box_id = app.session.doc.features[0].id;
    assert_eq!(d.candidate(&app.session, &face_at(Vec3::new(12.5, 10.0, 5.0))), Some(Sel::Feature { id: hole }));
    assert_eq!(d.candidate(&app.session, &face_at(Vec3::new(30.0, 30.0, 10.0))), Some(Sel::Feature { id: box_id }));
    app.dialog = None;
    // Edit Feature shows the hole as a feature again.
    let pat = app.session.doc.features.last().unwrap().id;
    let e = crate::dialogs::for_feature(&app, pat, None).unwrap();
    assert_eq!(e.inputs[0].accept, crate::selection::FEATURES);
    assert_eq!(e.inputs[0].items, vec![Sel::Feature { id: hole }]);
}

#[test]
fn a_hole_is_mirrored_as_a_feature() {
    let mut app = app_with(json!([
        {"command": "solid.box", "params": {"length": 60, "width": 40, "height": 10}},
        {"command": "solid.hole", "params": {"position": [10, 10, 10], "diameter": 5}}
    ]));
    let hole = app.session.doc.features.last().unwrap().id;
    let mut d = start(&mut app, vec![Sel::Feature { id: hole }], "solid.mirror");
    assert_eq!(d.inputs[0].accept, crate::selection::FEATURES);
    d.inputs[1].items = vec![Sel::Plane { name: "YZ".into() }];
    let cmds = apply_commands(&app, &d).unwrap();
    assert_eq!(cmds[0].1["features"], json!(["Hole1"]));
}

/// The canvas names the G2 size correctly and edits the same value when the feature is reopened.
#[test]
fn curvature_canvas_value_uses_equivalent_radius_when_created_and_reopened() {
    for style in ["curvature", "g2"] {
        let mut app = boxed();
        let e = edge(&app, Vec3::new(20.0, 0.0, 20.0));
        let mut d = start(&mut app, vec![e], "solid.fillet");
        d.extra.insert("type".into(), json!(style));
        let (label, kind, value) = d.primary().unwrap();
        assert_eq!((label, kind), ("Equivalent Radius", ValueKind::Length));
        *value = "3 mm".into();
        let (command, params) = apply_commands(&app, &d).unwrap().remove(0);
        assert_eq!(params["radius"], json!("3 mm"), "the canvas edits the actual command value");
        app.session.execute(&command, &params).unwrap();
        let feature = app.session.doc.features.last().unwrap().id;
        let mut reopened = crate::dialogs::for_feature(&app, feature, None).unwrap();
        let (label, kind, value) = reopened.primary().unwrap();
        assert_eq!((label, kind), ("Equivalent Radius", ValueKind::Length));
        assert_eq!(value.as_str(), "3 mm");
    }
    for (command, label) in [("solid.fillet", "Radius"), ("solid.chamfer", "Distance")] {
        let mut app = boxed();
        let e = edge(&app, Vec3::new(20.0, 0.0, 20.0));
        let mut d = start(&mut app, vec![e], command);
        assert_eq!(d.primary().unwrap().0, label);
    }
}

/// The radius arrow points into the body: dragging toward the material grows the round.
#[test]
fn fillet_and_chamfer_arrows_point_into_the_body() {
    for id in ["solid.fillet", "solid.chamfer"] {
        let mut app = boxed();
        let e = edge(&app, Vec3::new(20.0, 0.0, 20.0));
        let d = start(&mut app, vec![e], id);
        let (_, dir) = crate::canvas::anchor(&app, &d).unwrap();
        let dir = dir.unwrap();
        assert!(dir.y > 0.5 && dir.z < -0.5, "{id}: {dir:?}");
    }
}

#[test]
fn chamfer_types_send_their_values() {
    let mut app = boxed();
    let e = edge(&app, Vec3::new(20.0, 0.0, 20.0));
    let mut d = start(&mut app, vec![e], "solid.chamfer");
    let mut with = |t: usize| {
        if let Kind::Fillet { ctype, distance2, angle, flip, .. } = &mut d.kind {
            *ctype = t;
            *distance2 = "3 mm".into();
            *angle = "30 deg".into();
            *flip = true;
        }
        apply_commands(&app, &d).unwrap().remove(0).1
    };
    let p = with(0);
    assert!(p.get("distance2").is_none() && p.get("angle").is_none() && p.get("flip").is_none());
    let p = with(1);
    assert_eq!((p["distance2"].clone(), p["flip"].clone()), (json!("3 mm"), json!(true)));
    let p = with(2);
    assert_eq!(p["angle"], json!("30 deg"));
    assert!(p.get("distance2").is_none());
    // Applied and edited again, the chamfer keeps its type, angle and flip.
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    app.session.execute(&id, &p).unwrap();
    let ch = app.session.doc.features.last().unwrap().id;
    let e = crate::dialogs::for_feature(&app, ch, None).unwrap();
    assert!(matches!(&e.kind, Kind::Fillet { chamfer: true, ctype: 2, angle, flip: true, .. } if angle == "30 deg"), "{:?}", e.kind);
}

/// Radius Fit: the fillet reduces a radius too large for its edges, the preview shows the radius
/// it used, and editing the feature keeps the choice (#293).
#[test]
fn fillet_radius_fit_reduces_and_shows_the_applied_radius() {
    let mut app = app_with(serde_json::json!([
        {"command": "solid.box", "params": {"length": 14, "width": 6, "height": 10, "corner": [14, 0, 0]}}
    ]));
    let edges: Vec<Sel> = [Vec3::new(14.0, 0.0, 5.0), Vec3::new(28.0, 0.0, 5.0), Vec3::new(14.0, 6.0, 5.0), Vec3::new(28.0, 6.0, 5.0)]
        .into_iter()
        .map(|p| edge(&app, p))
        .collect();
    let mut d = start(&mut app, edges, "solid.fillet");
    if let Kind::Fillet { radius, .. } = &mut d.kind {
        *radius = "3.5 mm".into();
    }
    // Exact (the default) sends no fit and the preview refuses the radius.
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert!(p.get("fit").is_none(), "{p}");
    assert!(app.session.preview(&[(id.clone(), p)]).is_err());
    // Reduce to Fit: the preview builds it, at the radius that fits.
    d.extra.insert("fit".into(), json!("reduce"));
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(p["fit"], json!("reduce"));
    let job = crate::preview::compute(&app.session, &[(id.clone(), p.clone())], crate::preview::Colors::from_tokens(&crate::theme::Tokens::dark()))
        .unwrap();
    assert_eq!(job.applied, Some((3.0, 3.0)));
    // Applied and edited again, the fillet keeps its fit.
    app.session.execute(&id, &p).unwrap();
    let fillet = app.session.doc.features.last().unwrap().id;
    let e = crate::dialogs::for_feature(&app, fillet, None).unwrap();
    assert_eq!(e.extra.get("fit"), Some(&json!("reduce")));
    // Its edit preview shows the radius it uses too.
    let cmds = apply_commands(&app, &e).unwrap();
    let job = crate::preview::compute(&app.session, &cmds, crate::preview::Colors::from_tokens(&crate::theme::Tokens::dark())).unwrap();
    assert_eq!(job.applied, Some((3.0, 3.0)));
}

/// Apply a dialog's commands to the session; the new feature's id.
fn apply(app: &mut SolveApp, d: &Dialog) -> u64 {
    for (id, p) in apply_commands(app, d).unwrap() {
        app.session.execute(&id, &p).unwrap();
    }
    app.session.doc.features.last().unwrap().id
}

#[test]
fn shell_direction_and_tangent_chain_round_trip() {
    let mut app = boxed();
    let top = face(&app, Vec3::new(20.0, 15.0, 20.0));
    let mut d = start(&mut app, vec![top], "solid.shell");
    // Tangent Chain is on, as in Fusion; Direction Outside.
    assert_eq!(d.extra.get("tangent_chain"), Some(&json!(true)));
    d.extra.insert("direction".into(), json!("outside"));
    let p = apply_commands(&app, &d).unwrap().remove(0).1;
    assert_eq!((p["direction"].clone(), p["tangent_chain"].clone()), (json!("outside"), json!(true)));
    let id = apply(&mut app, &d);
    let e = crate::dialogs::for_feature(&app, id, None).unwrap();
    assert_eq!(e.extra.get("direction"), Some(&json!("outside")));
    assert_eq!(e.extra.get("tangent_chain"), Some(&json!(true)));
}

#[test]
fn variable_fillet_round_trip() {
    let mut app = boxed();
    let e0 = edge(&app, Vec3::new(40.0, 15.0, 20.0));
    let mut d = start(&mut app, vec![e0], "solid.fillet");
    d.inputs[0].items.truncate(1);
    d.extra.insert("type".into(), json!("variable"));
    d.extra.insert("radius2".into(), json!("4 mm"));
    let p = apply_commands(&app, &d).unwrap().remove(0).1;
    assert_eq!((p["type"].clone(), p["radius2"].clone()), (json!("variable"), json!("4 mm")));
    assert!(p["start"].is_array(), "{p}");
    let id = apply(&mut app, &d);
    let e = crate::dialogs::for_feature(&app, id, None).unwrap();
    assert_eq!(e.extra.get("type"), Some(&json!("variable")));
    assert_eq!(e.extra.get("radius2"), Some(&json!("4 mm")));
}

#[test]
fn variable_fillet_mid_radii_round_trip() {
    let mut app = boxed();
    let e0 = edge(&app, Vec3::new(40.0, 15.0, 20.0));
    let mut d = start(&mut app, vec![e0], "solid.fillet");
    d.inputs[0].items.truncate(1);
    d.extra.insert("type".into(), json!("variable"));
    d.extra.insert("radius2".into(), json!("2 mm"));
    d.extra.insert("mid".into(), json!([[0.5, "4 mm"]]));
    d.extra.insert("smooth".into(), json!(true));
    let p = apply_commands(&app, &d).unwrap().remove(0).1;
    assert_eq!((p["mid"].clone(), p["smooth"].clone()), (json!([[0.5, "4 mm"]]), json!(true)), "{p}");
    let id = apply(&mut app, &d);
    let e = crate::dialogs::for_feature(&app, id, None).unwrap();
    assert_eq!(e.extra.get("mid"), Some(&json!([[0.5, "4 mm"]])));
    assert_eq!(e.extra.get("smooth"), Some(&json!(true)));
    // All middle radii removed: a plain two-radius fillet again.
    let mut d = e;
    d.extra.insert("mid".into(), json!([]));
    let p = apply_commands(&app, &d).unwrap().remove(0).1;
    // (an edit: the feature's new parameters)
    assert_eq!(p["params"]["mid"], json!([]), "{p}");
}

/// Use an actual mesh point, as a viewport pick does; analytic cylinder points need not lie
/// on the display mesh's facets.
fn thread_face(app: &SolveApp, body: &str) -> Sel {
    let state = app.session.world_state();
    let body_state = state.body(body).unwrap();
    let index = solidcraft_engine::doc::naming::face_spots(body_state)
        .into_iter()
        .enumerate()
        .find_map(|(index, spot)| {
            spot.filter(|spot| matches!(spot.surface, Some(solidcraft_engine::kernel::FaceSurface::Cylinder { .. }))).map(|_| index)
        })
        .unwrap();
    let mesh = body_state.mesh();
    let point = mesh
        .triangles
        .iter()
        .zip(&mesh.tri_face)
        .find_map(|(triangle, face)| {
            if *face as usize != index {
                return None;
            }
            mesh.tri(triangle).map(|[a, b, c]| (a + b + c) / 3.0)
        })
        .unwrap();
    Sel::Face { body: body.into(), index, point }
}

#[test]
fn thread_size_suggestion_matches_the_feature_for_shafts_and_holes() {
    for (diameter, internal, expected) in [(8.0, false, "M8"), (20.0, false, "M20"), (5.0, true, "M6")] {
        let mut app = app_with(json!([]));
        if internal {
            app.run("solid.box", json!({"corner":[-10,-10,0],"length":20,"width":20,"height":10})).unwrap();
        }
        app.run("solid.cylinder", json!({"radius":diameter / 2.0,"height":10,"body_name":"Cylinder"})).unwrap();
        if internal {
            app.run("solid.combine", json!({"target":"Body1","tools":["Cylinder"],"operation":"cut"})).unwrap();
        }
        let pick = thread_face(&app, if internal { "Body1" } else { "Cylinder" });
        let saved = app.session.doc.to_json();
        let d = start(&mut app, vec![pick], "solid.thread");
        assert!(matches!(&d.kind, Kind::Thread { designation, .. } if designation == expected), "{:?}", d.kind);
        assert_eq!(app.session.doc.to_json(), saved, "suggesting a size is read-only");
        let (command, params) = apply_commands(&app, &d).unwrap().remove(0);
        assert!(params.get("designation").is_none(), "an automatic size stays automatic");
        let result = app.session.execute(&command, &params).unwrap();
        assert_eq!(result["thread"]["designation"], expected);
        assert_eq!(result["thread"]["internal"], internal);
    }
}

#[test]
fn thread_size_suggestion_follows_picks_without_overwriting_manual_sizes() {
    let mut app = app_with(json!([
        {"command":"solid.cylinder","params":{"radius":4,"height":10}},
        {"command":"solid.cylinder","params":{"base":[30,0,0],"radius":10,"height":10}},
        {"command":"solid.cylinder","params":{"base":[200,0,0],"radius":50,"height":10}}
    ]));
    let small = thread_face(&app, "Body1");
    let large = thread_face(&app, "Body2");
    let unsupported = thread_face(&app, "Body3");
    let planar = crate::dialogs::face_sel(&app.session, Vec3::new(0.0, 0.0, 10.0)).unwrap();
    let mut d = start(&mut app, vec![], "solid.thread");
    for (pick, expected) in [
        (small.clone(), "M8"),
        (large.clone(), "M20"),
        (large.clone(), "M20"),
        (small.clone(), "M8"),
        (unsupported, ""),
        (small.clone(), "M8"),
        (planar, ""),
    ] {
        d.pick(&app.session, pick);
        assert!(matches!(&d.kind, Kind::Thread { designation, .. } if designation == expected), "{:?}", d.kind);
    }
    d.take_box(&app.session, vec![], false);
    assert!(matches!(&d.kind, Kind::Thread { designation, .. } if designation.is_empty()));
    d.take_box(&app.session, vec![small], false);
    assert!(matches!(&d.kind, Kind::Thread { designation, .. } if designation == "M8"));
    if let Kind::Thread { designation, .. } = &mut d.kind {
        *designation = "M8x1".into();
    }
    d.pick(&app.session, large);
    assert!(matches!(&d.kind, Kind::Thread { designation, .. } if designation == "M8x1"));
    assert_eq!(apply_commands(&app, &d).unwrap()[0].1["designation"], "M8x1");
}

#[test]
fn thread_size_suggestion_reopens_without_changing_saved_thread_parameters() {
    for designation in [None, Some("M8"), Some("M8x1")] {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.cylinder", json!({"radius":4,"height":10})).unwrap();
        let Sel::Face { point, .. } = thread_face(&h.app, "Body1") else { panic!("a face") };
        let mut params = json!({"face":point});
        if let Some(size) = designation {
            params["designation"] = json!(size);
        }
        let id = h.app.run("solid.thread", params).unwrap()["feature"].as_u64().unwrap();
        let before = h.app.session.doc.feature(id).unwrap().kind.clone();
        h.app.edit_feature(id);
        h.frames(2);
        let d = h.app.dialog.as_ref().unwrap();
        assert!(matches!(&d.kind, Kind::Thread { designation: size, .. } if size == designation.unwrap_or("M8")), "{d:?}");
        let (command, params) = apply_commands(&h.app, d).unwrap().remove(0);
        h.app.session.execute(&command, &params).unwrap();
        assert_eq!(h.app.session.doc.feature(id).unwrap().kind, before);
    }
}

#[test]
fn thread_size_suggestion_can_be_cleared_and_edited_in_the_dialog() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("solid.cylinder", json!({"radius":4,"height":10})).unwrap();
    let pick = thread_face(&h.app, "Body1");
    h.app.session.selection = vec![pick.clone()];
    h.step(&json!({"start":"solid.thread"})).unwrap();
    h.step(&json!({"click":{"handle":"dialog:clear:Cylindrical face"}})).unwrap();
    assert!(matches!(&h.app.dialog.as_ref().unwrap().kind, Kind::Thread { designation, .. } if designation.is_empty()));
    h.app.dialog.as_mut().unwrap().pick(&h.app.session, pick.clone());
    h.frames(2);
    h.step(&json!({"click":{"handle":"dialog:thread_size"}})).unwrap();
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    // Even deliberately typing the same size turns it into an explicit override.
    h.step(&json!({"text":"M8"})).unwrap();
    let d = h.app.dialog.as_ref().unwrap();
    assert_eq!(apply_commands(&h.app, d).unwrap()[0].1["designation"], "M8");
    h.step(&json!({"click":{"handle":"dialog:clear:Cylindrical face"}})).unwrap();
    h.app.dialog.as_mut().unwrap().pick(&h.app.session, pick);
    h.frames(2);
    assert_eq!(apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap()[0].1["designation"], "M8");
}

#[test]
fn thread_choices_apply_a_fine_pitch_and_partial_modelled_thread() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("solid.cylinder", json!({"radius":4,"height":10})).unwrap();
    h.app.session.selection = vec![thread_face(&h.app, "Body1")];
    h.step(&json!({"start":"solid.thread"})).unwrap();
    let saved = h.app.session.doc.to_json();
    for handle in ["dialog:Pitch", "dialog:Pitch:1 mm", "dialog:Full Length", "dialog:Length"] {
        h.step(&json!({"click":{"handle":handle}})).unwrap();
    }
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"4 mm"})).unwrap();
    h.step(&json!({"click":{"handle":"dialog:Modeled"}})).unwrap();
    let (command, params) = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap().remove(0);
    assert_eq!(params["designation"], "M8x1");
    assert_eq!(params["length"], "4 mm");
    assert_eq!(params["modeled"], true);
    assert_eq!(h.app.session.doc.to_json(), saved, "changing options only previews");
    let before = h.app.session.world_state().body("Body1").unwrap().mesh().measure().volume;
    let result = h.app.session.execute(&command, &params).unwrap();
    let state = h.app.session.world_state();
    let after = state.body("Body1").unwrap().mesh().measure().volume;
    assert!(before - after > 1.0, "the selected fine pitch must cut the shaft");
    let id = result["feature"].as_u64().unwrap();
    let kind = &h.app.session.doc.feature(id).unwrap().kind;
    assert!(matches!(kind, solidcraft_engine::doc::FeatureKind::Thread { designation: Some(d), length: Some(l), modeled: true, .. }
        if d == "M8x1" && l == "4 mm"));
}

#[test]
fn thread_choices_reset_to_the_face_and_can_freeze_the_same_size() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("solid.cylinder", json!({"radius":4,"height":10})).unwrap();
    h.app.session.selection = vec![thread_face(&h.app, "Body1")];
    h.step(&json!({"start":"solid.thread"})).unwrap();
    for handle in ["dialog:Size", "dialog:Size:6 mm", "dialog:Pitch", "dialog:Pitch:0.75 mm"] {
        h.step(&json!({"click":{"handle":handle}})).unwrap();
    }
    let d = h.app.dialog.as_ref().unwrap();
    assert_eq!(apply_commands(&h.app, d).unwrap()[0].1["designation"], "M6x0.75");
    h.step(&json!({"click":{"handle":"dialog:Automatic size"}})).unwrap();
    h.frames(2);
    let d = h.app.dialog.as_ref().unwrap();
    assert!(matches!(&d.kind, Kind::Thread { designation, .. } if designation == "M8"));
    assert!(apply_commands(&h.app, d).unwrap()[0].1.get("designation").is_none());
    h.step(&json!({"click":{"handle":"dialog:Automatic size"}})).unwrap();
    assert_eq!(apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap()[0].1["designation"], "M8");
}

#[test]
fn thread_choices_preserve_custom_saved_pairs_and_offer_large_fine_pitches() {
    for (diameter, designation) in [(7.3, "M7.3x0.6"), (56.0, "M56")] {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.cylinder", json!({"radius":diameter / 2.0,"height":10})).unwrap();
        let Sel::Face { point, .. } = thread_face(&h.app, "Body1") else { panic!("a face") };
        let id = h.app.run("solid.thread", json!({"face":point,"designation":designation})).unwrap()["feature"].as_u64().unwrap();
        h.app.edit_feature(id);
        h.frames(2);
        assert!(h.widget(&format!("{diameter} mm")).is_some());
        assert!(h.widget(if diameter == 56.0 { "5.5 mm" } else { "0.6 mm" }).is_some());
        let (_, params) = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap().remove(0);
        assert_eq!(params["params"]["designation"], designation);
        if diameter == 56.0 {
            for handle in ["dialog:Pitch", "dialog:Pitch:2 mm"] {
                h.step(&json!({"click":{"handle":handle}})).unwrap();
            }
            let (command, params) = apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap().remove(0);
            assert_eq!(params["params"]["designation"], "M56x2");
            h.app.session.execute(&command, &params).unwrap();
            let mut reopened = solidcraft_engine::doc::Document::from_json(&h.app.session.doc.to_json()).unwrap();
            // This test submits the edit command directly; finish the editor's rollback
            // before checking the complete saved design.
            reopened.marker = None;
            let session = Session::new(reopened);
            assert_eq!(session.world_state().threads[0].designation, "M56x2");
        }
    }
}

#[test]
fn thread_choices_can_start_without_a_face_and_recover_from_invalid_text() {
    let mut h = crate::scenario::Harness::new();
    h.step(&json!({"start":"solid.thread"})).unwrap();
    for handle in ["dialog:Size", "dialog:Size:8 mm", "dialog:thread_size"] {
        h.step(&json!({"click":{"handle":handle}})).unwrap();
    }
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"Munknown"})).unwrap();
    h.frames(2);
    assert!(matches!(&h.app.dialog.as_ref().unwrap().kind, Kind::Thread { designation, .. } if designation == "Munknown"));
    for handle in ["dialog:Size", "dialog:Size:8 mm"] {
        h.step(&json!({"click":{"handle":handle}})).unwrap();
    }
    assert!(matches!(&h.app.dialog.as_ref().unwrap().kind, Kind::Thread { designation, .. } if designation == "M8x1.25"));
    assert!(apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).is_err(), "a face is still required");
}

#[test]
fn newer_options_reach_their_commands() {
    let mut app = boxed();
    // Thread: Modeled.
    let side = face(&app, Vec3::new(20.0, 0.0, 10.0));
    let mut d = start(&mut app, vec![side.clone()], "solid.thread");
    d.extra.insert("modeled".into(), json!(true));
    assert_eq!(apply_commands(&app, &d).unwrap()[0].1["modeled"], json!(true));
    // Combine into a new component.
    app.dialog = None;
    let mut d = start(&mut app, vec![], "solid.combine");
    d.extra.insert("new_component".into(), json!(true));
    d.inputs[0].items = vec![Sel::Body { name: "Body1".into() }];
    d.inputs[1].items = vec![Sel::Body { name: "Body2".into() }];
    assert_eq!(apply_commands(&app, &d).unwrap()[0].1["new_component"], json!(true));
    // Hole: To Object sends the place to drill to instead of a depth.
    app.dialog = None;
    let top = face(&app, Vec3::new(20.0, 15.0, 20.0));
    let mut d = start(&mut app, vec![top], "solid.hole");
    d.inputs.push(crate::selection::SelInput::new("To Object", crate::selection::FACES | crate::selection::VERTICES, false));
    d.inputs[1].items = vec![face(&app, Vec3::new(20.0, 15.0, 0.0))];
    let p = apply_commands(&app, &d).unwrap().remove(0).1;
    assert!(p["to"].is_array() && p.get("depth").is_none(), "{p}");
}

#[test]
fn boundary_fill_finds_the_cells_and_fills_the_ticked_ones() {
    let mut app = app_with(json!([
        {"command": "solid.box", "params": {"length": 20, "width": 20, "height": 20}},
        {"command": "solid.box", "params": {"length": 20, "width": 20, "height": 20, "corner": [10, 10, 0], "operation": "new"}}
    ]));
    let mut d = start(&mut app, vec![], "solid.boundary_fill");
    d.inputs[0].items = vec![Sel::Body { name: "Body1".into() }, Sel::Body { name: "Body2".into() }];
    app.dialog = Some(d);
    // The dialog finds the cells on its next frame.
    let ctx = egui::Context::default();
    ctx.run_ui(Default::default(), |ui| crate::dialogs::show(&mut app, ui.ctx())).textures_delta.clear();
    let mut d = app.dialog.take().unwrap();
    let Kind::BoundaryFill { cells, .. } = &mut d.kind else { panic!() };
    // Only A, only B, and both.
    assert_eq!(cells.len(), 3, "{cells:?}");
    assert!(cells.iter().any(|c| c.1 == "Inside Body1, Body2"));
    // Fill just the overlap.
    for c in cells.iter_mut() {
        c.2 = c.1 == "Inside Body1, Body2";
    }
    let id = apply(&mut app, &d);
    assert!(app.session.model.result(id).is_some_and(|r| r.error.is_none()));
    // A new body filling the 10 x 10 x 20 overlap.
    let vols: Vec<f64> = app.session.model.state().bodies.iter().map(|b| b.mesh().measure().volume).collect();
    assert!(vols.len() == 3 && vols.iter().any(|v| (v - 2000.0).abs() < 1.0), "{vols:?}");
}

#[test]
fn align_snaps_to_centres_and_edge_middles() {
    let mut app = boxed();
    let mut d = start(&mut app, vec![], "solid.align");
    d.inputs[0].items = vec![Sel::Body { name: "Body1".into() }];
    d.inputs[1].items = vec![face(&app, Vec3::new(20.0, 15.0, 0.0))];
    d.inputs[2].items = vec![edge(&app, Vec3::new(20.0, 0.0, 20.0))];
    d.extra.insert("from_snap".into(), json!("center"));
    let p = apply_commands(&app, &d).unwrap().remove(0).1;
    assert_eq!(p["from"]["snap"], json!("face_center"));
    assert_eq!(p["to"]["snap"], json!("edge_mid"));
}

#[test]
fn surface_dialogs_make_their_commands() {
    // A planar patch of a sketch's rectangle, then thickened; Edit Feature brings both back.
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [30, 20]}},
        {"command": "sketch.finish", "params": {}}
    ]));
    let sk = app.session.doc.features.last().unwrap().id;
    let mut d = start(&mut app, vec![], "surface.patch");
    d.inputs[0].items = vec![Sel::Profile { sketch: sk, index: 0 }];
    let patch = {
        for (id, p) in apply_commands(&app, &d).unwrap() {
            app.session.execute(&id, &p).unwrap();
        }
        app.session.doc.features.last().unwrap().id
    };
    let surf = app.session.model.state().bodies.last().unwrap().name.clone();
    assert!(matches!(crate::dialogs::for_feature(&app, patch, None).unwrap().kind, Kind::Surface(_)));
    app.dialog = None;
    let mut d = start(&mut app, vec![], "surface.thicken");
    d.inputs[0].items = vec![Sel::Body { name: surf }];
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(id, "surface.thicken");
    app.session.execute(&id, &p).unwrap();
    let v = app.session.model.state().bodies.iter().map(|b| b.mesh().measure().volume).fold(0.0, f64::max);
    assert!((v - 30.0 * 20.0 * 2.0).abs() < 1.0, "{v}");
}

#[test]
fn surface_extrude_dialog_takes_profiles_and_curves() {
    // A rectangle and a free line in one sketch: the dialog sends both (the profile's walls
    // and the line's wall) as one surface extrude, and Edit Feature brings the picks back.
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [30, 20]}},
        {"command": "sketch.line", "params": {"points": [[40, 0], [50, 0]], "ids": ["free"]}},
        {"command": "sketch.finish", "params": {}}
    ]));
    let sk = app.session.doc.features.last().unwrap().id;
    let mut d = start(&mut app, vec![], "surface.extrude");
    d.inputs[0].items = vec![Sel::Profile { sketch: sk, index: 0 }, Sel::SketchCurve { id: "free".into() }];
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(id, "surface.extrude");
    assert_eq!(p["curves"], json!(["free"]), "{p}");
    assert_eq!(p["profiles"], json!([0]), "{p}");
    app.session.execute(&id, &p).unwrap();
    let st = app.session.model.state();
    let sheet = st.bodies.last().unwrap();
    assert!(sheet.body.is_surface() && sheet.body.face_count() == 5);
    let area = sheet.mesh().measure().area;
    assert!((area - (100.0 + 10.0) * 10.0).abs() < 1e-6, "{area}");
    let fid = app.session.doc.features.last().unwrap().id;
    let back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert!(matches!(back.kind, Kind::Surface(_)));
    assert_eq!(back.inputs[0].items.len(), 2);
}

#[test]
fn unstitch_and_reverse_normal_dialogs_take_a_body_or_faces() {
    // A whole box picked: Unstitch sends the body alone; two of its faces: their points.
    let mut app = app_with(json!([{"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "body_name": "Box"}}]));
    let mut d = start(&mut app, vec![], "surface.unstitch");
    d.inputs[0].items = vec![Sel::Body { name: "Box".into() }];
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!((id.as_str(), &p["body"], &p["faces"]), ("surface.unstitch", &json!("Box"), &json!([])), "{p}");
    let top = Vec3::new(5.0, 5.0, 10.0);
    d.inputs[0].items = vec![Sel::Face { body: "Box".into(), index: 0, point: top }];
    let (_, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(p["faces"], json!([[5.0, 5.0, 10.0]]), "{p}");
    app.session.execute("surface.unstitch", &p).unwrap();
    assert_eq!(app.session.model.state().bodies.len(), 2);
    let fid = app.session.doc.features.last().unwrap().id;
    assert!(matches!(crate::dialogs::for_feature(&app, fid, None).unwrap().kind, Kind::Surface(_)));
    app.dialog = None;
    let sheet = app.session.model.state().bodies.last().unwrap().name.clone();
    let mut d = start(&mut app, vec![], "surface.reverse_normal");
    d.inputs[0].items = vec![Sel::Body { name: sheet.clone() }];
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!((id.as_str(), &p["body"]), ("surface.reverse_normal", &json!(sheet)), "{p}");
    app.session.execute(&id, &p).unwrap();
}

#[test]
fn surface_revolve_dialog_takes_curves_and_an_axis() {
    // A line beside the sketch's y axis, turned a quarter round: a quarter cylinder. Edit
    // Feature brings back the curve and the axis.
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.line", "params": {"points": [[3, 0], [3, 4]], "ids": ["side"]}},
        {"command": "sketch.finish", "params": {}}
    ]));
    let mut d = start(&mut app, vec![], "surface.revolve");
    d.inputs[0].items = vec![Sel::SketchCurve { id: "side".into() }];
    assert!(apply_commands(&app, &d).is_err(), "no axis yet");
    d.inputs[1].items = vec![Sel::Axis { name: "y".into() }];
    if let Kind::Surface(crate::dialogs_surface::Surf::Revolve { angle, .. }) = &mut d.kind {
        *angle = "90 deg".into();
    }
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!((id.as_str(), &p["axis"], &p["curves"]), ("surface.revolve", &json!("y"), &json!(["side"])), "{p}");
    app.session.execute(&id, &p).unwrap();
    let area = app.session.model.state().bodies.last().unwrap().mesh().measure().area;
    let want = std::f64::consts::FRAC_PI_2 * 3.0 * 4.0;
    assert!((area - want).abs() < want * 2e-3, "{area} vs {want}");
    let fid = app.session.doc.features.last().unwrap().id;
    let back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert_eq!(back.inputs[0].items, vec![Sel::SketchCurve { id: "side".into() }]);
    assert_eq!(back.inputs[1].items, vec![Sel::Axis { name: "y".into() }]);
}

#[test]
fn surface_sweep_dialog_takes_curves_and_a_path() {
    // A line along y carried 5 up a line in the XZ plane: a 10 × 5 sheet. Edit Feature brings
    // back the curve and the path.
    let mut app = app_with(json!([
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [0, 10]], "ids": ["l"]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "sketch.create", "params": {"plane": "XZ"}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [0, 5]], "ids": ["up"]}},
        {"command": "sketch.finish", "params": {}}
    ]));
    let mut d = start(&mut app, vec![], "surface.sweep");
    d.inputs[0].items = vec![Sel::SketchCurve { id: "l".into() }];
    assert!(apply_commands(&app, &d).is_err(), "no path yet");
    d.inputs[1].items = vec![Sel::SketchCurve { id: "up".into() }];
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!((id.as_str(), &p["path"], &p["curves"]), ("surface.sweep", &json!(["up"]), &json!(["l"])), "{p}");
    assert_ne!(p["path_sketch"], p["sketch"], "{p}");
    app.session.execute(&id, &p).unwrap();
    let area = app.session.model.state().bodies.last().unwrap().mesh().measure().area;
    assert!((area - 50.0).abs() < 1e-6, "{area}");
    let fid = app.session.doc.features.last().unwrap().id;
    let back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert_eq!(back.inputs[0].items, vec![Sel::SketchCurve { id: "l".into() }]);
    assert_eq!(back.inputs[1].items, vec![Sel::SketchCurve { id: "up".into() }]);
}

#[test]
fn surface_loft_dialog_makes_a_section_per_sketch() {
    // A line in each of two sketches 5 apart: two sections, a 10 × 5 strip; Edit Feature brings
    // the curves back.
    let mut app = app_with(json!([
        {"command": "construct.plane.offset", "params": {"base": "XY", "offset": 5, "name": "Mid"}},
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [10, 0]], "ids": ["a"]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "sketch.create", "params": {"plane": "Mid"}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [10, 0]], "ids": ["b"]}},
        {"command": "sketch.finish", "params": {}}
    ]));
    let mut d = start(&mut app, vec![], "surface.loft");
    d.inputs[0].items = vec![Sel::SketchCurve { id: "a".into() }];
    assert!(apply_commands(&app, &d).is_err(), "one section");
    d.inputs[0].items.push(Sel::SketchCurve { id: "b".into() });
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(id, "surface.loft");
    assert_eq!(p["sections"].as_array().map(Vec::len), Some(2), "{p}");
    assert_eq!((&p["sections"][0]["curves"], &p["sections"][1]["curves"]), (&json!(["a"]), &json!(["b"])), "{p}");
    app.session.execute(&id, &p).unwrap();
    let area = app.session.model.state().bodies.last().unwrap().mesh().measure().area;
    assert!((area - 50.0).abs() < 1e-6, "{area}");
    let fid = app.session.doc.features.last().unwrap().id;
    let back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert_eq!(back.inputs[0].items, vec![Sel::SketchCurve { id: "a".into() }, Sel::SketchCurve { id: "b".into() }]);
}

#[test]
fn ruled_surface_dialog_takes_edges_a_face_and_a_direction() {
    // A box top's front and right edges, ruled across the top 2 mm with round corners: two
    // 10 × 2 strips and a quarter disc between them. Edit Feature brings it back; along a
    // direction it needs one, and keeps the one it was made with.
    let mut app = app_with(json!([{"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "body_name": "Box"}}]));
    let mut d = start(&mut app, vec![], "surface.ruled");
    assert!(apply_commands(&app, &d).is_err(), "no edges yet");
    let edge = |i: usize, p: Vec3| Sel::Edge { body: "Box".into(), index: i, point: p };
    d.inputs[0].items = vec![edge(0, Vec3::new(5.0, 0.0, 10.0)), edge(1, Vec3::new(10.0, 5.0, 10.0))];
    d.inputs[1].items = vec![Sel::Face { body: "Box".into(), index: 0, point: Vec3::new(5.0, 5.0, 10.0) }];
    if let Kind::Surface(crate::dialogs_surface::Surf::Ruled { distance, mode, round, .. }) = &mut d.kind {
        *distance = "2 mm".into();
        *mode = 1;
        *round = true;
    }
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(
        (id.as_str(), &p["body"], &p["mode"], &p["corners"], &p["face"]),
        ("surface.ruled", &json!("Box"), &json!("tangent"), &json!("round"), &json!([5.0, 5.0, 10.0])),
        "{p}"
    );
    app.session.execute(&id, &p).unwrap();
    let m = app.session.model.state().bodies.last().unwrap().mesh().measure();
    let want = 40.0 + std::f64::consts::PI;
    assert!((m.area - want).abs() < want * 2e-3, "{} vs {want}", m.area); // (measured on meshes)
    let fid = app.session.doc.features.last().unwrap().id;
    let mut back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert_eq!((back.inputs[0].items.len(), back.inputs[1].items.len()), (2, 1));
    assert!(matches!(back.kind, Kind::Surface(crate::dialogs_surface::Surf::Ruled { mode: 1, round: true, .. })));
    if let Kind::Surface(crate::dialogs_surface::Surf::Ruled { mode, .. }) = &mut back.kind {
        *mode = 3;
    }
    assert!(apply_commands(&app, &back).is_err(), "a direction is needed");
    back.inputs[2].items = vec![Sel::Axis { name: "Z".into() }];
    // Edit Feature runs the command's parameters as an edit of the feature.
    let (_, p) = apply_commands(&app, &back).unwrap().remove(0);
    assert_eq!((&p["params"]["mode"], &p["params"]["direction"]), (&json!("direction"), &json!([0.0, 0.0, 1.0])), "{p}");
}

#[test]
fn untrim_dialog_takes_a_face_its_holes_and_the_outer_boundary() {
    // A 40 × 20 × 2 plate with three holes: its top untrimmed (every hole) is the whole 800 mm²
    // rectangle; one hole picked by an edge; Edit Feature brings the picks and options back.
    let mut app = app_with(json!([
        {"command": "solid.box", "params": {"length": 40, "width": 20, "height": 2, "body_name": "Plate"}},
        {"command": "solid.cylinder", "params": {"radius": 2, "height": 4, "base": [10, 10, -1], "operation": "cut", "targets": ["Plate"]}},
        {"command": "solid.cylinder", "params": {"radius": 2, "height": 4, "base": [20, 10, -1], "operation": "cut", "targets": ["Plate"]}},
        {"command": "solid.cylinder", "params": {"radius": 2, "height": 4, "base": [30, 10, -1], "operation": "cut", "targets": ["Plate"]}}
    ]));
    let mut d = start(&mut app, vec![], "surface.untrim");
    assert!(apply_commands(&app, &d).is_err(), "no face yet");
    d.inputs[0].items = vec![Sel::Face { body: "Plate".into(), index: 0, point: Vec3::new(5.0, 5.0, 2.0) }];
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!((id.as_str(), &p["face"], &p["holes"]), ("surface.untrim", &json!({"body": "Plate", "point": [5.0, 5.0, 2.0]}), &json!("all")), "{p}");
    app.session.execute(&id, &p).unwrap();
    let m = app.session.model.state().bodies.last().unwrap().mesh().measure();
    assert!((m.area - 800.0).abs() < 1e-6, "{}", m.area);
    app.session.execute("edit.undo", &json!({})).unwrap();
    // Picked holes: none picked is refused; an edge of the middle hole sends its point.
    if let Kind::Surface(crate::dialogs_surface::Surf::Untrim { holes, outer, .. }) = &mut d.kind {
        *holes = 1;
        *outer = true;
    }
    assert!(apply_commands(&app, &d).is_err(), "no holes picked");
    d.inputs[1].items = vec![Sel::Edge { body: "Plate".into(), index: 0, point: Vec3::new(22.0, 10.0, 2.0) }];
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!((&p["holes"], &p["outer"]), (&json!([[22.0, 10.0, 2.0]]), &json!(true)), "{p}");
    app.session.execute(&id, &p).unwrap();
    let fid = app.session.doc.features.last().unwrap().id;
    let back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert!(matches!(back.kind, Kind::Surface(crate::dialogs_surface::Surf::Untrim { holes: 1, outer: true, in_place: false })));
    assert_eq!((back.inputs[0].items.len(), back.inputs[1].items.len()), (1, 1));
}

#[test]
fn offset_surface_dialog_takes_faces_a_distance_and_the_chain() {
    // A box top picked, moved up 5: a 10 × 10 square at z = 15; Edit Feature brings it back.
    let mut app = app_with(json!([{"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "body_name": "Box"}}]));
    let mut d = start(&mut app, vec![], "surface.offset");
    assert!(apply_commands(&app, &d).is_err(), "no faces yet");
    d.inputs[0].items = vec![Sel::Face { body: "Box".into(), index: 0, point: Vec3::new(5.0, 5.0, 10.0) }];
    if let Kind::Surface(crate::dialogs_surface::Surf::Offset { distance, chain }) = &mut d.kind {
        *distance = "5 mm".into();
        *chain = true;
    }
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!(
        (id.as_str(), &p["body"], &p["faces"], &p["chain"]),
        ("surface.offset", &json!("Box"), &json!([[5.0, 5.0, 10.0]]), &json!(true)),
        "{p}"
    );
    app.session.execute(&id, &p).unwrap();
    let m = app.session.model.state().bodies.last().unwrap().mesh().measure();
    assert!((m.area - 100.0).abs() < 1e-6, "{}", m.area);
    let fid = app.session.doc.features.last().unwrap().id;
    let back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert_eq!(back.inputs[0].items.len(), 1);
}

#[test]
fn patch_dialog_fills_a_loop_of_edges_that_is_not_planar() {
    // A box with its top cut away by a sideways cylinder: its rim is a closed loop of four
    // curved edges, not in a plane. Picking them in the Patch dialog fills it with one face.
    let mut app = app_with(json!([
        {"command": "solid.box", "params": {"length": 28, "width": 28, "height": 20, "corner": [-14, -14, 0], "body_name": "Saddle"}},
        {"command": "solid.cylinder", "params": {"radius": 40, "height": 120, "base": [0, -60, 50], "axis": "Y", "operation": "cut", "targets": ["Saddle"]}}
    ]));
    let z = |x: f64| 50.0 - (1600.0 - x * x).sqrt();
    let rim = [Vec3::new(14.0, 0.0, z(14.0)), Vec3::new(0.0, 14.0, z(0.0)), Vec3::new(-14.0, 0.0, z(-14.0)), Vec3::new(0.0, -14.0, z(0.0))];
    let mut d = start(&mut app, vec![], "surface.patch");
    d.inputs[0].items = rim.iter().enumerate().map(|(i, p)| Sel::Edge { body: "Saddle".into(), index: i, point: *p }).collect();
    let cmds = apply_commands(&app, &d).unwrap();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].0, "surface.patch");
    app.session.execute(&cmds[0].0, &cmds[0].1).unwrap();
    let st = app.session.model.state();
    let fill = st.bodies.iter().find(|b| b.name != "Saddle").unwrap();
    assert!(fill.body.is_surface() && fill.body.face_count() == 1);
    // One smooth face over the rim: its area is close to the cylinder's inside the rim.
    let area = fill.mesh().measure().area;
    let want = 28.0 * 40.0 * 2.0 * (14.0f64 / 40.0).asin();
    assert!((area - want).abs() < want * 1e-2, "{area} vs {want}");
}

#[test]
fn split_body_by_a_curved_face_sends_it_as_a_tool() {
    let mut app = app_with(json!([
        {"command": "solid.box", "params": {"length": 40, "width": 30, "height": 20}},
        {"command": "solid.cylinder", "params": {"diameter": 20, "height": 40, "base": [20, 15, -10], "operation": "new"}}
    ]));
    let st = app.session.model.state();
    let cyl = st.bodies.iter().find(|b| b.name != "Body1").unwrap();
    let m = cyl.mesh();
    // A point on the round side.
    let p = Vec3::new(30.0, 15.0, 5.0);
    let t = m.triangles.iter().position(|t| crate::dialogs::point_tri_dist(p, m.tri(t).unwrap()) < 0.3).unwrap();
    let side = Sel::Face { body: cyl.name.clone(), index: m.tri_face[t] as usize, point: p };
    drop(st);
    let mut d = start(&mut app, vec![], "solid.split_body");
    d.inputs[0].items = vec![Sel::Body { name: "Body1".into() }];
    d.inputs[1].items = vec![side];
    let p = apply_commands(&app, &d).unwrap().remove(0).1;
    assert!(p["tool"]["point"].is_array() && p.get("plane").is_none(), "{p}");
}

/// Runs a primitive's dialog with `sels` pre-selected and returns the new body's centroid.
fn primitive_on(app: &mut SolveApp, sels: Vec<Sel>, id: &str, edit: impl FnOnce(&mut Dialog)) -> (serde_json::Value, Vec3) {
    let mut d = start(app, sels, id);
    edit(&mut d);
    let (cmd, p) = apply_commands(app, &d).unwrap().remove(0);
    let name = app.session.execute(&cmd, &p).unwrap()["created_bodies"][0].as_str().unwrap().to_string();
    let st = app.session.model.state();
    (p, st.body(&name).unwrap().mesh().measure().centroid)
}

/// The face of Body1 whose centre is at `c`, picked at `p`.
fn picked(app: &SolveApp, c: Vec3, p: Vec3) -> Sel {
    let Sel::Face { body, index, .. } = face(app, c) else { panic!("a face") };
    Sel::Face { body, index, point: p }
}

fn near(a: Vec3, b: Vec3) -> bool {
    a.dist(b) < 0.05
}

#[test]
fn primitives_go_on_the_picked_face() {
    // #580: a sphere (or any primitive) made with a face selected went to the origin.
    let mut app = boxed();
    let top = picked(&app, Vec3::new(20.0, 15.0, 20.0), Vec3::new(8.0, 10.0, 20.0));
    let (p, c) = primitive_on(&mut app, vec![top.clone()], "solid.sphere", |_| {});
    assert_eq!(p["center"], json!([8.0, 10.0, 20.0]), "{p}");
    assert!(near(c, Vec3::new(8.0, 10.0, 20.0)), "{c:?}");

    let (p, c) = primitive_on(&mut app, vec![top.clone()], "solid.cylinder", |_| {});
    assert_eq!(p["axis"], json!([0.0, 0.0, 1.0]), "standing on the top face: {p}");
    assert!(near(c, Vec3::new(8.0, 10.0, 30.0)), "{c:?}");

    // A box stands on the face, centred on the picked point across it. Sizes that are plain
    // lengths ("20 mm") give a plain corner.
    let (p, c) = primitive_on(&mut app, vec![top.clone()], "solid.box", |_| {});
    assert_eq!(p["corner"], json!([-2.0, 0.0, 20.0]), "{p}");
    assert!(near(c, Vec3::new(8.0, 10.0, 30.0)), "{c:?}");

    let (p, c) = primitive_on(&mut app, vec![top], "solid.torus", |_| {});
    assert_eq!(p["center"], json!([8.0, 10.0, 20.0]), "{p}");
    assert!(near(c, Vec3::new(8.0, 10.0, 20.0)), "{c:?}");
}

#[test]
fn a_box_stands_out_of_a_face_that_faces_down_an_axis() {
    let mut app = boxed();
    // The x = 0 side faces -X: the box goes the other way, away from the body.
    let side = picked(&app, Vec3::new(0.0, 15.0, 10.0), Vec3::new(0.0, 12.0, 8.0));
    let (_, c) = primitive_on(&mut app, vec![side.clone()], "solid.box", |_| {});
    assert!(near(c, Vec3::new(-10.0, 12.0, 8.0)), "{c:?}");
    // Sizes that are expressions stay expressions in the corner.
    app.session.execute("parameters.add", &json!({"name": "s", "expression": "6 mm"})).unwrap();
    let (p, c) = primitive_on(&mut app, vec![side.clone()], "solid.box", |d| {
        if let Kind::Primitive { fields, .. } = &mut d.kind {
            fields.iter_mut().for_each(|f| f.1 = "s".into());
        }
    });
    assert_eq!(p["corner"][0], json!("0 mm - (s)"), "{p}");
    assert!(near(c, Vec3::new(-3.0, 12.0, 8.0)), "{c:?}");
    // The cylinder's axis points out of the face.
    let (p, c) = primitive_on(&mut app, vec![side], "solid.cylinder", |_| {});
    assert_eq!(p["axis"], json!([-1.0, 0.0, 0.0]), "{p}");
    assert!(near(c, Vec3::new(-10.0, 12.0, 8.0)), "{c:?}");
}

#[test]
fn a_primitive_without_a_pick_or_with_its_position_cleared_stays_at_the_origin() {
    let mut app = boxed();
    let (p, c) = primitive_on(&mut app, vec![], "solid.sphere", |_| {});
    assert!(p.get("center").is_none(), "{p}");
    assert!(near(c, Vec3::ZERO), "{c:?}");
    let top = face(&app, Vec3::new(20.0, 15.0, 20.0));
    let (p, _) = primitive_on(&mut app, vec![top], "solid.sphere", |d| {
        if let Kind::Primitive { at, .. } = &mut d.kind {
            *at = None;
        }
    });
    assert!(p.get("center").is_none(), "{p}");
    // A vertex places it too.
    let v = Sel::Vertex { body: "Body1".into(), point: Vec3::new(40.0, 30.0, 20.0) };
    let (p, _) = primitive_on(&mut app, vec![v], "solid.sphere", |_| {});
    assert_eq!(p["center"], json!([40.0, 30.0, 20.0]), "{p}");
}

#[test]
fn patch_dialog_takes_continuity_and_points_and_brings_them_back() {
    // A tube with its top face taken off: the rim's edges, G1, through a point above it.
    let mut app = app_with(json!([
        {"command": "solid.cylinder", "params": {"radius": 10, "height": 30, "body_name": "Tube"}},
        {"command": "surface.unstitch", "params": {"body": "Tube", "faces": [[0, 0, 30]]}}
    ]));
    let rim: Vec<Sel> = {
        let st = app.session.model.state();
        let m = st.body("Tube").unwrap().mesh();
        m.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.iter().all(|p| (p.z - 30.0).abs() < 1e-6))
            .map(|(i, e)| Sel::Edge { body: "Tube".into(), index: i, point: e[e.len() / 2] })
            .collect()
    };
    assert!(!rim.is_empty());
    let mut d = start(&mut app, vec![], "surface.patch");
    d.inputs[0].items = rim;
    d.inputs[1].items = vec![Sel::Vertex { body: "Tube".into(), point: Vec3::new(0.0, 0.0, 36.0) }];
    if let Kind::Surface(crate::dialogs_surface::Surf::Patch { continuity }) = &mut d.kind {
        *continuity = 1;
    }
    let (id, p) = apply_commands(&app, &d).unwrap().remove(0);
    assert_eq!((id.as_str(), &p["continuity"], &p["points"]), ("surface.patch", &json!("g1"), &json!([[0.0, 0.0, 36.0]])), "{p}");
    app.session.execute(&id, &p).unwrap();
    let top = app.session.model.state().bodies.last().unwrap().mesh().bounds().max.z;
    assert!(top > 35.9, "the cap reaches the point: {top}");
    // Edit Feature: G1 again, and the point (on no face) kept as it was.
    let fid = app.session.doc.features.last().unwrap().id;
    let back = crate::dialogs::for_feature(&app, fid, None).unwrap();
    assert!(matches!(back.kind, Kind::Surface(crate::dialogs_surface::Surf::Patch { continuity: 1 })));
    let (_, p) = apply_commands(&app, &back).unwrap().remove(0);
    assert_eq!((&p["params"]["continuity"], &p["params"]["points"]), (&json!("g1"), &json!([[0.0, 0.0, 36.0]])), "{p}");
}

#[test]
fn patch_dialog_rows_are_translated() {
    for (lang, continuity, through) in [("en", "Continuity", "Pass through"), ("de", "Stetigkeit", "Durch Punkte"), ("ja", "連続性", "通過点")]
    {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.language = lang.into();
        h.app.home.open = false;
        h.call("ui.start", json!({"command": "surface.patch"}));
        h.frames(3);
        assert!(h.widget(continuity).is_some(), "{lang}: the Continuity row must show {continuity:?}");
        assert!(h.widget(through).is_some(), "{lang}: the points input must show {through:?}");
    }
}

/// Midplane opens a two-plane dialog. Starting it with nothing selected used to run the
/// command and report that `a` is required.
#[test]
fn midplane_asks_for_two_planes_then_builds_the_one_between_them() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("construct.plane.offset", json!({"base":"XY","offset":"20 mm","name":"Shelf"})).unwrap();
    h.app.start("construct.plane.midplane");
    h.frames(2);
    let d = h.app.dialog.as_ref().unwrap();
    assert!(matches!(d.kind, Kind::Midplane));
    assert_eq!(d.inputs.len(), 2);
    let status = h.app.status.as_ref().map(|s| s.0.clone()).unwrap_or_default();
    assert!(!status.contains("is required"), "{status}");
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::Plane { name: "XY".into() }];
    h.app.dialog.as_mut().unwrap().inputs[1].items = vec![Sel::Plane { name: "Shelf".into() }];
    h.step(&json!({"key":"Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().and_then(|d| d.error.clone()));
    let (_, _, pl) =
        solidcraft_engine::view::construction_planes(&h.app.session).into_iter().find(|(_, name, _)| name != "Shelf").expect("the midplane");
    assert!(pl.normal().cross(Vec3::Z).len() < 1e-6, "{pl:?}");
    assert!((pl.origin.z - 10.0).abs() < 1e-6, "{pl:?}");
}
