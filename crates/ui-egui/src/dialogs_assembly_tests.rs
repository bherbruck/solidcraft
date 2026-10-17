use serde_json::json;
use solidcraft_engine::Session;

use super::*;
use crate::Services;
use crate::dialogs::{apply_commands, face_sel, preview_commands};

/// Components A (grounded) and B: a 20 x 20 x 10 box each, B 30 mm along X.
fn two_boxes() -> SolveApp {
    let mut s = Session::default();
    for (name, x) in [("A", 0), ("B", 30)] {
        s.execute("component.activate", &json!({"component": "root"})).unwrap();
        s.execute("component.create", &json!({ "name": name })).unwrap();
        s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0]})).unwrap();
    }
    s.execute("component.activate", &json!({"component": "root"})).unwrap();
    s.execute("occurrence.ground", &json!({"occurrence": "A:1", "grounded": true})).unwrap();
    SolveApp::new(s, Services::default())
}

fn occ(app: &SolveApp, name: &str) -> u64 {
    let c = app.session.doc.find_component(name).unwrap();
    app.session.doc.occurrence_of(c).unwrap().id
}

/// Two face picks make a joint between the components that own them; offsets across the face
/// turn component 1's origin into a frame; limits follow the type.
#[test]
fn joint_from_two_snaps() {
    let mut app = two_boxes();
    app.start("joint.create");
    let mut d = app.dialog.clone().unwrap();
    let (fa, fb) = (face_sel(&app.session, Vec3::new(10.0, 10.0, 10.0)).unwrap(), face_sel(&app.session, Vec3::new(40.0, 10.0, 0.0)).unwrap());
    assert!(apply_commands(&app, &d).is_err(), "no snaps yet");
    d.pick(&app.session, fa);
    assert_eq!(d.active, 1, "component 2 is next");
    d.pick(&app.session, fb);
    if let Kind::Assembly(Asm::Joint(f)) = &mut d.kind {
        f.kind = 1;
        f.fit_limits();
        f.limits[0] = (true, "-45 deg".into(), "45 deg".into());
    }
    let c = apply_commands(&app, &d).unwrap();
    let (id, p) = &c[0];
    assert_eq!(id, "joint.create");
    assert_eq!(p["type"], "revolute");
    assert_eq!(p["a"]["occurrence"], occ(&app, "A"));
    assert_eq!(p["b"]["occurrence"], occ(&app, "B"));
    assert!(p["a"]["face"].is_array() && p["b"]["face"].is_array(), "{p}");
    assert_eq!(p["limits"], json!([["-45 deg", "45 deg"]]));
    // The bottom face of B snaps at its centre, z down.
    let sb = snap_of(&app.session, &d.inputs[1].items[0]).unwrap();
    assert!(sb.at.dist(Vec3::new(40.0, 10.0, 0.0)) < 1e-6 && sb.z.dist(-Vec3::Z) < 1e-9, "{sb:?}");
    // Offset X: an explicit frame 5 mm along the face.
    if let Kind::Assembly(Asm::Joint(f)) = &mut d.kind {
        f.offset[0] = "5 mm".into();
    }
    let p = apply_commands(&app, &d).unwrap()[0].1.clone();
    let o = &p["a"]["frame"]["origin"];
    let at = Vec3::new(o[0].as_f64().unwrap(), o[1].as_f64().unwrap(), o[2].as_f64().unwrap());
    assert!((at.dist(Vec3::new(10.0, 10.0, 10.0)) - 5.0).abs() < 1e-9, "{p}");
    // OK makes it.
    for (id, p) in apply_commands(&app, &d).unwrap() {
        app.run(&id, p).unwrap();
    }
    assert_eq!(app.session.doc.assembly.joints.len(), 1);
}

/// The preview moves B where the joint puts it and plays the motion while animating.
#[test]
fn joint_and_drive_preview_on_the_placed_model() {
    let mut app = two_boxes();
    app.start("joint.create");
    let mut d = app.dialog.clone().unwrap();
    d.pick(&app.session, face_sel(&app.session, Vec3::new(10.0, 10.0, 10.0)).unwrap());
    d.pick(&app.session, face_sel(&app.session, Vec3::new(40.0, 10.0, 0.0)).unwrap());
    let colors = crate::preview::Colors {
        body: [1, 1, 1, 255],
        added: [2, 2, 2, 255],
        new_body: [2, 2, 2, 255],
        removed: [3, 3, 3, 100],
        edge: [0, 0, 0, 255],
    };
    let cmds = preview_commands(&app, &d).unwrap();
    let b = world_preview(&app.session, &cmds, colors).unwrap().unwrap();
    assert_eq!(b.replaced, vec!["Body2".to_string()], "B moves");
    assert!(app.session.doc.assembly.joints.is_empty(), "the preview changes nothing");
    if let Kind::Assembly(Asm::Joint(f)) = &mut d.kind {
        f.kind = 1;
        f.animate = true;
        f.phase = 0.25;
    }
    let cmds = preview_commands(&app, &d).unwrap();
    assert_eq!(cmds[0].1["values"], json!(["90 deg"]), "a quarter turn into the animation");
    assert!(apply_commands(&app, &d).unwrap()[0].1.get("values").is_none(), "OK makes the joint at rest");
    // Drive: the dialog's value turns the joint, the preview shows it.
    for (id, p) in apply_commands(&app, &d).unwrap() {
        app.run(&id, p).unwrap();
    }
    app.start("joint.drive");
    let mut d = app.dialog.clone().unwrap();
    if let Kind::Assembly(Asm::Drive { values, .. }) = &mut d.kind {
        assert_eq!(values, &vec!["0 deg".to_string()]);
        values[0] = "30 deg".into();
    }
    let cmds = apply_commands(&app, &d).unwrap();
    assert_eq!(cmds[0].0, "joint.drive");
    let b = world_preview(&app.session, &cmds, colors).unwrap().unwrap();
    assert_eq!(b.replaced, vec!["Body2".to_string()]);
    // Features preview as before while nothing is placed away from its frame, and on the
    // placed model once something is (B moved by its joint).
    assert!(world_preview(&two_boxes().session, &[("solid.extrude".into(), json!({}))], colors).is_none());
    assert!(world_preview(&app.session, &[("solid.extrude".into(), json!({}))], colors).is_some());
}

/// Edit Joint changes type, alignment and limits of the joint in place.
#[test]
fn edit_joint_runs_edit_and_limits() {
    let mut app = two_boxes();
    let (a, b) = (occ(&app, "A"), occ(&app, "B"));
    app.run("joint.create", json!({"type": "slider", "a": {"occurrence": a, "face": [10, 10, 10]}, "b": {"occurrence": b, "face": [40, 10, 0]}}))
        .unwrap();
    let id = app.session.doc.assembly.joints[0].id;
    let mut d = edit_joint(&app, id).unwrap();
    assert!(d.inputs.is_empty());
    if let Kind::Assembly(Asm::Joint(f)) = &mut d.kind {
        assert_eq!(TYPES[f.kind], "slider");
        f.limits[0] = (true, "0 mm".into(), "15 mm".into());
        f.offset[2] = "2 mm".into();
    }
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["joint.edit", "joint.limits"]);
    for (id, p) in c {
        app.run(&id, p).unwrap();
    }
    let j = &app.session.doc.assembly.joints[0];
    assert_eq!(j.limits[0], Some((0.0, 15.0)));
    assert!((j.offset - 2.0).abs() < 1e-12);
}

/// As-built joints and rigid groups take components from body picks; New Component makes one
/// from bodies when bodies are picked.
#[test]
fn component_picks_become_occurrences() {
    let mut app = two_boxes();
    app.session.selection = vec![Sel::Body { name: "Body1".into() }, Sel::Body { name: "Body2".into() }];
    app.start("joint.as_built");
    let d = app.dialog.clone().unwrap();
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["a"], occ(&app, "A"));
    assert_eq!(c[0].1["b"], occ(&app, "B"));
    app.session.selection = vec![Sel::Body { name: "Body1".into() }, Sel::Body { name: "Body2".into() }];
    app.start("joint.rigid_group");
    let c = apply_commands(&app, app.dialog.as_ref().unwrap()).unwrap();
    assert_eq!(c[0].1["occurrences"], json!([occ(&app, "A"), occ(&app, "B")]));
    app.dialog = None;
    app.start("component.create");
    let c = apply_commands(&app, app.dialog.as_ref().unwrap()).unwrap();
    assert_eq!(c[0].0, "component.create");
}

/// Round edges snap to their centre; straight ones are not circles.
#[test]
fn circles_from_edge_polylines() {
    let pts: Vec<Vec3> =
        (0..=24).map(|i| f64::from(i) / 24.0 * std::f64::consts::TAU).map(|t| Vec3::new(5.0 + 3.0 * t.cos(), 2.0 + 3.0 * t.sin(), 7.0)).collect();
    let (c, n) = circle_of(&pts).unwrap();
    assert!(c.dist(Vec3::new(5.0, 2.0, 7.0)) < 1e-9 && n.cross(Vec3::Z).len() < 1e-9);
    let line: Vec<Vec3> = (0..10).map(|i| Vec3::new(f64::from(i), 0.0, 0.0)).collect();
    assert!(circle_of(&line).is_none());
}

/// The centre of the first text shape reading `text` in a frame's output.
fn text_at(out: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
    out.shapes.iter().find_map(|c| match &c.shape {
        egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos + t.galley.rect.center().to_vec2()),
        _ => None,
    })
}

/// OK pressed while the animated preview is computing and released once it is not (the busy
/// spinner coming and going between the two frames) still applies the joint.
#[test]
fn ok_click_survives_a_busy_preview() {
    let mut app = two_boxes();
    app.start("joint.create");
    let mut d = app.dialog.take().unwrap();
    d.pick(&app.session, face_sel(&app.session, Vec3::new(10.0, 10.0, 10.0)).unwrap());
    d.pick(&app.session, face_sel(&app.session, Vec3::new(40.0, 10.0, 0.0)).unwrap());
    if let Kind::Assembly(Asm::Joint(f)) = &mut d.kind {
        f.kind = 1;
        f.animate = true;
    }
    app.dialog = Some(d);
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
    let frame = |app: &mut SolveApp, busy: bool, events: Vec<egui::Event>| {
        app.preview.busy = busy;
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| crate::dialogs::show(app, ui.ctx()));
        // Nothing is painted: drop the texture changes (an unapplied delta panics on drop).
        out.textures_delta.clear();
        out
    };
    frame(&mut app, false, vec![]);
    let out = frame(&mut app, false, vec![]);
    let ok = text_at(&out, "OK").expect("an OK button");
    let button = |pressed| egui::Event::PointerButton { pos: ok, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&mut app, true, vec![egui::Event::PointerMoved(ok)]);
    frame(&mut app, true, vec![button(true)]);
    frame(&mut app, false, vec![button(false)]);
    assert_eq!(app.session.doc.assembly.joints.len(), 1, "the click applied the joint");
    assert!(app.dialog.is_none());
}

/// #1239: two parts as bodies of one component (a multi-body STEP import). Picks on both used to
/// make no joint with nothing said, so no option seemed to do anything. The dialog now says why
/// under the picks and offers Create Components from Bodies, after which the same picks join.
#[test]
fn picks_on_one_component_say_why_and_offer_components() {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 10})).unwrap();
    s.execute("solid.box", &json!({"length": 20, "width": 20, "height": 10, "corner": [30, 0, 0]})).unwrap();
    let mut app = SolveApp::new(s, Services::default());
    let bodies = vec!["Body1".to_string(), "Body2".to_string()];
    // As-built: two bodies of one component are one component.
    app.session.selection = vec![Sel::Body { name: "Body1".into() }, Sel::Body { name: "Body2".into() }];
    app.start("joint.as_built");
    let d = app.dialog.take().unwrap();
    let Kind::Assembly(k) = &d.kind else { panic!("{:?}", d.kind) };
    assert_eq!(joint_problem(&app, k, &d.inputs).map(|p| p.1), Some(bodies.clone()));
    // Two faces of one body: pick another part (nothing to part).
    app.session.selection.clear();
    app.start("joint.create");
    let mut d = app.dialog.take().unwrap();
    d.pick(&app.session, face_sel(&app.session, Vec3::new(10.0, 10.0, 10.0)).unwrap());
    d.pick(&app.session, face_sel(&app.session, Vec3::new(10.0, 10.0, 0.0)).unwrap());
    let Kind::Assembly(k) = &d.kind else { panic!("{:?}", d.kind) };
    assert_eq!(joint_problem(&app, k, &d.inputs).map(|p| p.1), Some(vec![]));
    // A face of each body.
    app.start("joint.create");
    let mut d = app.dialog.take().unwrap();
    d.pick(&app.session, face_sel(&app.session, Vec3::new(10.0, 10.0, 10.0)).unwrap());
    d.pick(&app.session, face_sel(&app.session, Vec3::new(40.0, 10.0, 0.0)).unwrap());
    assert!(apply_commands(&app, &d).is_err(), "one component: no joint");
    app.dialog = Some(d);
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
    let frame = |app: &mut SolveApp, events: Vec<egui::Event>| {
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| crate::dialogs::show(app, ui.ctx()));
        // Nothing is painted: drop the texture changes (an unapplied delta panics on drop).
        out.textures_delta.clear();
        out
    };
    // The auto-sized dialog takes a few frames to settle its width.
    for _ in 0..3 {
        frame(&mut app, vec![]);
    }
    let out = frame(&mut app, vec![]);
    let why = crate::i18n::format("joint.one_component", &[("bodies", "Body1, Body2")]);
    assert!(text_at(&out, &why).is_some(), "the dialog says why");
    let at = text_at(&out, crate::i18n::tr("cmd.component_from_bodies")).expect("and offers the way out");
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&mut app, vec![egui::Event::PointerMoved(at)]);
    frame(&mut app, vec![button(true)]);
    frame(&mut app, vec![button(false)]);
    assert!(bodies.iter().all(|b| app.session.doc.find_component(b).is_some()), "each body has a component");
    let d = app.dialog.as_ref().expect("the dialog stays open");
    let c = apply_commands(&app, d).unwrap();
    assert_eq!((c[0].1["a"]["occurrence"].clone(), c[0].1["b"]["occurrence"].clone()), (json!(occ(&app, "Body1")), json!(occ(&app, "Body2"))));
    let out = frame(&mut app, vec![]);
    assert!(text_at(&out, &why).is_none(), "nothing left to say");
}

/// Two top faces mate face to face as in Fusion: B ends up upside down on A, not inside it;
/// Flip turns it back over (into A).
#[test]
fn face_snaps_mate_face_to_face() {
    let mut app = two_boxes();
    app.start("joint.create");
    let mut d = app.dialog.clone().unwrap();
    d.pick(&app.session, face_sel(&app.session, Vec3::new(10.0, 10.0, 10.0)).unwrap());
    d.pick(&app.session, face_sel(&app.session, Vec3::new(40.0, 10.0, 10.0)).unwrap());
    let c = apply_commands(&app, &d).unwrap();
    assert_eq!(c[0].1["flip"], false, "mating is the engine's default; no flip");
    for (id, p) in c {
        app.run(&id, p).unwrap();
    }
    let st = app.session.world_state();
    let b = st.body("Body2").unwrap().mesh().bounds();
    assert!((b.min.z - 10.0).abs() < 1e-6 && (b.max.z - 20.0).abs() < 1e-6, "{b:?}");
    // Edit Joint shows the mate unflipped; Flip puts B back into A's volume.
    let id = app.session.doc.assembly.joints[0].id;
    let mut e = edit_joint(&app, id).unwrap();
    if let Kind::Assembly(Asm::Joint(f)) = &mut e.kind {
        assert!(!f.flip);
        f.flip = true;
    }
    for (id, p) in apply_commands(&app, &e).unwrap() {
        app.run(&id, p).unwrap();
    }
    let b = app.session.world_state().body("Body2").unwrap().mesh().bounds();
    assert!((b.max.z - 10.0).abs() < 1e-6, "{b:?}");
}

/// Drive Joints takes the keyboard: typing a value and Enter drives the joint.
#[test]
fn drive_value_takes_the_keyboard() {
    let mut app = two_boxes();
    let (a, b) = (occ(&app, "A"), occ(&app, "B"));
    app.run("joint.create", json!({"type": "revolute", "a": {"occurrence": a, "face": [10, 10, 10]}, "b": {"occurrence": b, "face": [40, 10, 0]}}))
        .unwrap();
    app.start("joint.drive");
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
    let frame = |app: &mut SolveApp, events: Vec<egui::Event>| {
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        // Nothing renders here: drop the frame's texture changes deliberately.
        ctx.run_ui(input, |ui| crate::dialogs::show(app, ui.ctx())).textures_delta.clear();
    };
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    frame(&mut app, vec![egui::Event::Text("90".into())]);
    let key = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
    frame(&mut app, vec![key(true), key(false)]);
    frame(&mut app, vec![]);
    let j = &app.session.doc.assembly.joints[0];
    assert!((j.values[0] - 90f64.to_radians()).abs() < 1e-9, "{:?}", j.values);
    assert!(app.dialog.is_none());
}

/// A real slider drag updates the requested pose while the pointer is still down.
/// Previewing and cancelling must leave the saved joint and occurrence positions alone.
#[test]
fn drive_slider_previews_intermediate_positions_before_release() {
    let mut h = crate::scenario::Harness::new();
    h.app = two_boxes();
    let (a, b) = (occ(&h.app, "A"), occ(&h.app, "B"));
    h.app
        .run(
            "joint.create",
            json!({"type": "revolute", "a": {"occurrence": a, "face": [10, 10, 10]},
        "b": {"occurrence": b, "face": [40, 10, 0]}, "limits": [[-90, 90]]}),
        )
        .unwrap();
    let before = h.app.session.doc.clone();
    h.app.start("joint.drive");
    h.frames(4);
    let bounds = h
        .output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find_map(|(_, n)| (n.role() == egui::accesskit::Role::Slider).then(|| n.bounds()).flatten())
        .expect("drive slider");
    let at = |fraction: f64| egui::pos2((bounds.x0 + fraction * (bounds.x1 - bounds.x0)) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    let start = at(0.25);
    h.app.synthetic.extend([
        egui::Event::PointerMoved(start),
        egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
    ]);
    h.frames(3);
    let first = preview_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    h.app.synthetic.push(egui::Event::PointerMoved(at(0.75)));
    h.frames(3);
    let second = preview_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
    assert_ne!(first, second, "the held slider requests a new pose before release");
    assert!(h.ctx.input(|i| i.pointer.primary_down()));
    // The preview runs on a worker thread; on a machine busy with other test binaries it can
    // take several seconds, so wait as long as the other preview tests do (#1549).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.app.preview.busy {
        assert!(std::time::Instant::now() < deadline, "held-slider preview did not finish");
        std::thread::sleep(std::time::Duration::from_millis(1));
        h.frame();
    }
    assert!(h.app.preview.active, "the viewport shows the pose before release");
    assert!(h.app.preview.error.is_none());
    assert_eq!(h.app.preview.replaced, ["Body2"]);
    assert_eq!(h.app.session.doc, before);
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: at(0.75),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.frames(3);
    h.step(&json!({"key": "Escape"})).unwrap();
    assert!(h.app.dialog.is_none());
    assert!(!h.app.preview.active);
    assert_eq!(h.app.session.doc, before, "cancel leaves the joint at its original angle");
}

/// Named joint origins are drawn where their component is: a drive of the occurrence, or a free
/// move not captured yet, carries them along.
#[test]
fn joint_origins_follow_their_component() {
    let mut app = two_boxes();
    app.session.execute("joint.origin", &json!({"name": "Top of B", "face": [40, 10, 10]})).unwrap();
    let at = |app: &SolveApp| {
        let v = origins_world(&app.session);
        let (_, m) = v.iter().find(|(n, _)| n == "Top of B").unwrap();
        solidcraft_engine::doc::apply_point(m, Vec3::ZERO)
    };
    assert!(at(&app).dist(Vec3::new(40.0, 10.0, 10.0)) < 1e-6, "{:?}", at(&app));
    let b = occ(&app, "B");
    app.session.execute("occurrence.move", &json!({"occurrence": b, "translate": [0, 0, 15]})).unwrap();
    assert!(at(&app).dist(Vec3::new(40.0, 10.0, 25.0)) < 1e-6, "{:?}", at(&app));
    // A drag not captured yet.
    let mut m = app.session.doc.occurrence_of(app.session.doc.find_component("B").unwrap()).unwrap().transform;
    m[3][0] += 5.0;
    app.session.pending_moves.insert(b, m);
    assert!(at(&app).dist(Vec3::new(45.0, 10.0, 25.0)) < 1e-6, "{:?}", at(&app));
}

/// Edit Feature on a moved component: the dialog shows the feature's picks where the
/// component is (its stored points are in the component's frame).
#[test]
fn edit_feature_on_a_moved_component_refills_in_the_world() {
    let mut app = two_boxes();
    let b = occ(&app, "B");
    app.session.execute("occurrence.move", &json!({"occurrence": b, "translate": [0, 0, 15]})).unwrap();
    let r = app.session.execute("solid.fillet", &json!({"edges": [[40, 0, 25]], "radius": 2})).unwrap();
    let id = r["feature"].as_u64().unwrap();
    let d = crate::dialogs::for_feature(&app, id, None).unwrap();
    let items: Vec<Sel> = d.inputs.iter().flat_map(|i| i.items.clone()).collect();
    let Some(Sel::Edge { body, point, .. }) = items.first() else { panic!("{items:?}") };
    assert_eq!(body, "Body2");
    assert!(point.dist(Vec3::new(40.0, 0.0, 25.0)) < 1e-6, "{point:?}");
}

/// A crank-rocker four-bar (crank 20, coupler 50, rocker 40, ground pivots 45 apart), built as
/// it stands with the crank at 90°: joints Crank, Crank pin, Rocker pin, Rocker.
fn four_bar() -> SolveApp {
    let mut s = Session::default();
    let (p0, o4) = ([0.0, 20.0], [45.0, 0.0]);
    // The rocker pin: where the coupler (50 from the crank pin) meets the rocker (40 from O4).
    let (dx, dy) = (o4[0] - p0[0], o4[1] - p0[1]);
    let l = f64::hypot(dx, dy);
    let a = (2500.0 - 1600.0 + l * l) / (2.0 * l);
    let h = (2500.0 - a * a).sqrt();
    let q0 = [p0[0] + dx * a / l - dy * h / l, p0[1] + dy * a / l + dx * h / l];
    for (name, at) in [("Ground", [0.0, 0.0]), ("Crank", p0), ("Coupler", q0), ("Rocker", o4)] {
        s.execute("component.activate", &json!({"component": "root"})).unwrap();
        s.execute("component.create", &json!({ "name": name })).unwrap();
        s.execute("solid.box", &json!({"length": 2, "width": 2, "height": 2, "corner": [at[0], at[1], -5]})).unwrap();
    }
    s.execute("component.activate", &json!({"component": "root"})).unwrap();
    s.execute("occurrence.ground", &json!({"occurrence": "Ground:1", "grounded": true})).unwrap();
    for (name, a, b, p) in [
        ("Crank", "Ground", "Crank", [0.0, 0.0]),
        ("Crank pin", "Crank", "Coupler", p0),
        ("Rocker pin", "Coupler", "Rocker", q0),
        ("Rocker", "Ground", "Rocker", o4),
    ] {
        let at = |o: &str| json!({"occurrence": o, "point": [p[0], p[1], 0], "z": [0, 0, 1]});
        s.execute("joint.create", &json!({"type": "revolute", "a": at(a), "b": at(b), "flip": true, "name": name})).unwrap();
    }
    SolveApp::new(s, Services::default())
}

/// Drive Joints says where a drive stopped short and why: the preview names the loop that can't
/// close further, OK says it on the status line, and a drive within reach says nothing.
#[test]
fn drive_says_where_a_loop_stopped_it() {
    let mut app = four_bar();
    let colors = crate::preview::Colors {
        body: [1, 1, 1, 255],
        added: [2, 2, 2, 255],
        new_body: [2, 2, 2, 255],
        removed: [3, 3, 3, 100],
        edge: [0, 0, 0, 255],
    };
    let rocker = app.session.doc.assembly.joints.iter().position(|j| j.name == "Rocker").unwrap();
    let mut d = drive_joint(&app, app.session.doc.assembly.joints[rocker].id).unwrap();
    let ask = |d: &mut Dialog, v: &str| {
        if let Kind::Assembly(Asm::Drive { values, .. }) = &mut d.kind {
            values[0] = v.into();
        }
    };
    // Within its swing: no stop.
    ask(&mut d, "20 deg");
    let b = world_preview(&app.session, &apply_commands(&app, &d).unwrap(), colors).unwrap().unwrap();
    assert!(b.stopped.is_none());
    // Past its toggle position (about +50.4° from where it stands).
    ask(&mut d, "100 deg");
    let b = world_preview(&app.session, &apply_commands(&app, &d).unwrap(), colors).unwrap().unwrap();
    let why = b.stopped.as_ref().and_then(drive_stop).unwrap();
    assert!(why.starts_with("Stopped at 50.") && why.contains("can't close any further"), "{why}");
    // OK drives it there and says so.
    crate::dialogs::run_dialog(&mut app, &d).unwrap();
    let (status, _, error) = app.status.clone().unwrap();
    assert!(status == why && !error, "{status}");
}

/// A contact set stops a drive too: the message names the parts that would pass through each other.
#[test]
fn drive_says_which_parts_a_contact_set_stopped() {
    let mut app = two_boxes();
    let at = |o: &str| json!({"occurrence": o, "point": [30, 10, 5], "z": [1, 0, 0]});
    app.run("joint.create", json!({"type": "slider", "a": at("A"), "b": at("B"), "flip": true, "name": "Rail"})).unwrap();
    app.run("contact.create", json!({"occurrences": ["A:1", "B:1"]})).unwrap();
    let r = app.run("joint.drive", json!({"joint": "Rail", "value": -20})).unwrap();
    let why = drive_stop(&r).unwrap();
    assert!(why.starts_with("Stopped at -1") && why.contains("A:1") && why.contains("B:1") && why.contains("would pass through"), "{why}");
}

/// A drive whose joint is joined to nothing grounded moves nothing: the preview row and the
/// status line name the part that floats.
#[test]
fn drive_says_when_its_joint_is_joined_to_nothing_grounded() {
    let mut app = two_boxes();
    for (name, x) in [("C", 100), ("D", 130)] {
        app.run("component.activate", json!({"component": "root"})).unwrap();
        app.run("component.create", json!({ "name": name })).unwrap();
        app.run("solid.box", json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0]})).unwrap();
    }
    let at = |o: &str, x: f64| json!({"occurrence": o, "point": [x, 10, 5], "z": [1, 0, 0]});
    app.run("joint.create", json!({"type": "slider", "a": at("A", 30.0), "b": at("B", 30.0), "flip": true, "name": "Rail"})).unwrap();
    app.run("joint.create", json!({"type": "revolute", "a": at("C", 130.0), "b": at("D", 130.0), "flip": true, "name": "Turn"})).unwrap();
    let turn = app.session.doc.assembly.joints.iter().find(|j| j.name == "Turn").unwrap().id;
    let mut d = drive_joint(&app, turn).unwrap();
    if let Kind::Assembly(Asm::Drive { values, .. }) = &mut d.kind {
        values[0] = "45 deg".into();
    }
    let colors = crate::preview::Colors {
        body: [1, 1, 1, 255],
        added: [2, 2, 2, 255],
        new_body: [2, 2, 2, 255],
        removed: [3, 3, 3, 100],
        edge: [0, 0, 0, 255],
    };
    let b = world_preview(&app.session, &apply_commands(&app, &d).unwrap(), colors).unwrap().unwrap();
    let why = b.stopped.as_ref().and_then(drive_stop).unwrap();
    assert!(why.starts_with("Moves nothing") && why.contains("C:1"), "{why}");
    crate::dialogs::run_dialog(&mut app, &d).unwrap();
    assert_eq!(app.status.clone().map(|(status, _, error)| (status, error)), Some((why, false)));
}

/// Drive Joints opens on the joint picked in the browser; with none picked, on the first joint
/// that moves.
#[test]
fn drive_opens_on_the_picked_joint() {
    let mut app = four_bar();
    let drive_on = |app: &mut SolveApp| {
        app.dialog = None;
        app.start("joint.drive");
        match &app.dialog.as_ref().unwrap().kind {
            Kind::Assembly(Asm::Drive { joint, .. }) => app.session.doc.assembly.joints[*joint].name.clone(),
            k => panic!("{k:?}"),
        }
    };
    assert_eq!(drive_on(&mut app), "Crank");
    let rocker = app.session.doc.assembly.joints.iter().find(|j| j.name == "Rocker").unwrap().id;
    pick_joint(&mut app, rocker, false);
    assert_eq!(drive_on(&mut app), "Rocker");
}

/// A rigid joint has nothing to drive: Drive Joints won't start on it.
#[test]
fn drive_preset_refuses_a_rigid_joint() {
    let mut app = two_boxes();
    let at = |o: &str| json!({"occurrence": o, "point": [25, 10, 5], "z": [1, 0, 0]});
    app.run("joint.create", json!({"type": "rigid", "a": at("A"), "b": at("B"), "name": "Glue"})).unwrap();
    let mut k = Asm::Drive { joint: 0, values: Vec::new(), focus: true };
    let e = preset(&app.session, &mut k, &json!({"joint": "Glue"})).unwrap_err();
    assert!(e.contains("rigid"), "{e}");
}

/// Motion Trace on the four-bar: a pick on the coupler and the first joint (the crank) driven a
/// turn give the trace's parameters, and OK keeps the path as a 3D sketch spline. A pick on the
/// coupler where an exploded view shows it traces from where the coupler is placed.
#[test]
fn motion_trace_keeps_the_picked_point_path() {
    let mut app = four_bar();
    let coupler = occ(&app, "Coupler");
    let top = |app: &SolveApp| {
        let b = occurrence_bounds(&app.session, &[coupler]);
        Vec3::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0, b.max.z)
    };
    let placed = top(&app);
    app.start("motion.trace");
    let mut d = app.dialog.clone().unwrap();
    assert!(apply_commands(&app, &d).is_err(), "no point yet");
    d.pick(&app.session, face_sel(&app.session, placed).unwrap());
    let c = apply_commands(&app, &d).unwrap();
    let (id, p) = &c[0];
    assert_eq!(id, "motion.trace.keep");
    let crank = app.session.doc.assembly.joints.iter().find(|j| j.name == "Crank").unwrap().id;
    assert_eq!((&p["occurrence"], &p["joint"], &p["from"], &p["to"]), (&json!([coupler]), &json!(crank), &json!("0 deg"), &json!("360 deg")), "{p}");
    let at = |p: &Value| Vec3::new(p["point"][0].as_f64().unwrap(), p["point"][1].as_f64().unwrap(), p["point"][2].as_f64().unwrap());
    assert!(at(p).dist(placed) < 1e-6, "{p}");
    let preview = app.session.scratch().execute("motion.trace", p).unwrap();
    assert!(preview["samples"] == 100 && preview["stopped_at"].is_null(), "{preview}");
    crate::dialogs::run_dialog(&mut app, &d).unwrap();
    let sketch = app.session.doc.find_feature("Motion Trace1").unwrap().id;
    let wire = app.session.doc.sketch(sketch).unwrap().wires[0].clone();
    assert_eq!(wire.fit.len(), 100);
    assert_eq!(wire.fit.first(), wire.fit.last(), "a full turn closes the spline");
    // Exploded: the coupler shows away from its placement; the pick maps back to it.
    app.run("explode.create", json!({"name": "Apart", "scale": 1})).unwrap();
    app.run("explode.show", json!({"view": "Apart"})).unwrap();
    let shown = top(&app);
    assert!(shown.dist(placed) > 1.0, "the exploded view moves the coupler");
    app.start("motion.trace");
    let mut d = app.dialog.clone().unwrap();
    d.pick(&app.session, face_sel(&app.session, shown).unwrap());
    let p = apply_commands(&app, &d).unwrap()[0].1.clone();
    assert!(at(&p).dist(placed) < 1e-6, "{p}");
    // The preview is drawn moved with the coupler: it starts at the point picked where it shows.
    let start = app.session.scratch().execute("motion.trace", &p).unwrap()["points"][0].clone();
    let (_, _, seen, put) = trace::placement(&app.session, &d.inputs).unwrap();
    let drawn = {
        use solidcraft_engine::doc::{apply_point, mat_inverse};
        apply_point(&seen, apply_point(&mat_inverse(&put).unwrap(), at(&json!({"point": start}))))
    };
    assert!(drawn.dist(shown) < 1e-6, "{drawn:?} wants {shown:?}");
}
