//! Booleans that failed, panicked or hung, replayed from the scripts in
//! `examples/regressions/` (each script's description says what went wrong).

use serde_json::{Value, json};

use crate::Session;

/// Runs in PR CI as well as the heavier fixed-seed suite. Two pattern copies are
/// disconnected, and one cylinder overlaps the block by half its volume.
#[test]
fn partial_face_contact_pattern_has_analytic_material() {
    // Repeated construction exposes re-imprinting a cylinder through both of its
    // side faces, which can create a redundant arc along the first imprint.
    for _ in 0..8 {
        check_partial_face_contact_pattern();
    }
}

fn check_partial_face_contact_pattern() {
    let s = replay(include_str!("../../../examples/regressions/partial_face_contact_pattern.json"));
    let (mut volume, mut area, mut shells) = (0.0, 0.0, 0);
    for b in &s.model.state().bodies {
        assert!(b.body.validity().is_empty(), "{:?}", b.body.validity());
        let m = b.body.tessellate(1e-5).unwrap().measure();
        volume += m.volume;
        area += m.area;
        shells += b.body.shell_count();
    }
    let expected_volume = 25.0 * 47.5 * 37.5 + 3.5 * std::f64::consts::PI * 25.0 * 2.5;
    let expected_area = 2.0 * (25.0 * 47.5 + 25.0 * 37.5 + 47.5 * 37.5) + 237.5 * std::f64::consts::PI - 25.0;
    assert!((volume - expected_volume).abs() < 0.02, "{volume} expected {expected_volume}");
    assert!((area - expected_area).abs() < 0.02, "{area} expected {expected_area}");
    assert_eq!(shells, 3);
    println!("#674 volume {volume:.6} / {expected_volume:.6} mm³; area {area:.6} / {expected_area:.6} mm²; {shells} shells");
}

fn replay(script: &str) -> Session {
    let mut s = Session::default();
    let script: Value = serde_json::from_str(script).unwrap();
    s.run_script(&script).unwrap();
    s
}

fn volume(s: &Session, body: &str) -> f64 {
    s.model.state().body(body).map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).unwrap_or(0.0)
}

/// #2059: tabs exactly as wide as their supporting ribs share the ribs' side
/// planes. The base's remote conical chamfer must not prevent their join.
#[test]
fn eighteen_tabs_join_the_ribs_of_a_chamfered_base() {
    let s = replay(include_str!("../../../examples/regressions/tabs_on_chamfered_ribbed_base.json"));
    let check = |s: &Session| {
        assert!(s.model.results.iter().all(|r| r.error.is_none()), "{:?}", s.model.results);
        assert_eq!(s.model.state().bodies.len(), 1);
        let state = s.model.state();
        let body = &state.body("Base").unwrap().body;
        assert_eq!(body.shell_count(), 1);
        assert!(body.validity().is_empty(), "{:?}", body.validity());
        let mesh = body.tessellate(1e-4).unwrap();
        // R40 x 4 disc less its 1 x 1 chamfer, six 32 x 2 ribs
        // standing 4 above it, and eighteen 1 x 2 tabs another 8 higher.
        let expected = std::f64::consts::PI * (6400.0 - 40.0 + 1.0 / 3.0) + 1536.0 + 288.0;
        let got = mesh.measure().volume;
        assert!((got - expected).abs() < 0.1, "{got} vs {expected}");
        println!("#2059 volume {got:.6} / {expected:.6} mm³; one valid shell, eighteen tabs");
        for k in 0..6 {
            let angle = k as f64 * std::f64::consts::TAU / 6.0;
            let p = |x: f64, y: f64, z| solidcraft_geom::Vec3::new(x * angle.cos() - y * angle.sin(), x * angle.sin() + y * angle.cos(), z);
            for x in [15.5, 25.5, 35.5] {
                assert!(mesh.contains(p(x, 0.0, 12.0)), "tab {k} at {x}");
                assert!(!mesh.contains(p(x, 1.2, 12.0)), "tab {k} grew sideways at {x}");
            }
            assert!(mesh.contains(p(20.0, 0.0, 6.0)), "rib {k}");
            assert!(mesh.contains(p(39.5, 0.0, 2.0)), "rim {k}");
            assert!(!mesh.contains(p(39.75, 0.0, 3.75)), "chamfer {k}");
        }
        assert!((mesh.bounds().max.z - 16.0).abs() < 1e-8);
    };
    check(&s);
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    check(&reopened);
}

#[test]
fn buried_join_end_ignores_bodies_in_other_components() {
    for own_body in [false, true] {
        let mut s = Session::default();
        let run = |s: &mut Session, command: &str, params: Value| s.execute(command, &params).unwrap();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 8, "body_name": "Other"}));
        run(&mut s, "component.create", json!({"name": "Child"}));
        if own_body {
            run(&mut s, "solid.box", json!({"corner": [50, 0, 0], "length": 10, "width": 10, "height": 8, "body_name": "Child"}));
        }
        run(&mut s, "sketch.create", json!({"plane": "XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [2, 2], "p1": [4, 4]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance": 16, "operation": "join", "body_name": "Tab"}));
        let got = volume(&s, if own_body { "Child" } else { "Tab" });
        let expected = 64.0 + if own_body { 800.0 } else { 0.0 };
        assert!((got - expected).abs() < 1e-6, "{got}");
        assert!((volume(&s, "Other") - 800.0).abs() < 1e-6);
    }
}

/// Slab less a 557.68 × 100 × 120 pocket and the loft below its floor (556.68 × 99 down 4 mm to
/// 80 × 80; prismatoid: h/6 · (A_top + A_bottom + 4 A_mid)).
fn slab_pocket_loft() -> (f64, f64) {
    let slab = 900.0 * 480.0 * 160.0;
    let loft = 4.0 / 6.0 * (556.68 * 99.0 + 80.0 * 80.0 + 4.0 * (556.68 + 80.0) / 2.0 * (99.0 + 80.0) / 2.0);
    (slab - 557.68 * 100.0 * 120.0, loft)
}

#[test]
fn loft_cut_beside_a_round_hole() {
    let s = replay(include_str!("../../../examples/regressions/loft_cut_beside_round_hole.json"));
    let (pocketed, loft) = slab_pocket_loft();
    let hole = std::f64::consts::PI * 17.5 * 17.5 * 25.0;
    let want = pocketed - loft - hole;
    let got = volume(&s, "Body1");
    assert!((got - want).abs() < 1e-6 * want, "{got} vs {want}");
}

#[test]
fn shallow_loft_cut_from_the_top_face() {
    let s = replay(include_str!("../../../examples/regressions/shallow_loft_cut_from_top_face.json"));
    let (_, loft) = slab_pocket_loft();
    let want = 900.0 * 480.0 * 160.0 - loft;
    let got = volume(&s, "Body1");
    assert!((got - want).abs() < 1e-9 * want, "{got} vs {want}");
}

#[test]
fn oblique_cut_through_a_small_arc_matches_a_split() {
    let scripts = [
        include_str!("../../../examples/regressions/oblique_cut_of_cylinder.json"),
        include_str!("../../../examples/regressions/oblique_cut_of_cylinder_hang.json"),
    ];
    // The same bar split by the prism's tilted top: the piece above it is the cut's result.
    let mut split = Session::default();
    let script: Value = serde_json::from_str(scripts[0]).unwrap();
    let steps = script.get("commands").and_then(Value::as_array).unwrap();
    split.run_script(&Value::Array(steps[..5].to_vec())).unwrap();
    let whole = volume(&split, "Tool");
    split.execute("solid.split_body", &json!({"body": "Tool", "plane": {"origin": [0, -472, 853.91], "normal": [0, 15.44, 40]}})).unwrap();
    let above = volume(&split, "Tool");
    assert!(above < whole - 10.0, "{above} of {whole}");
    for script in scripts {
        let s = replay(script);
        let got = volume(&s, "Tool");
        assert!((got - above).abs() < 1e-6 * above, "{got} vs {above}");
    }
}

/// Joining struts that end on a bore wall reached a truck-topology panic (#268). The guard caught it,
/// but no kernel call should panic at all (wasm aborts on a panic).
#[test]
fn struts_ending_on_a_bore_wall_join_without_a_panic() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let me = std::thread::current().id();
    let count = Arc::new(AtomicUsize::new(0));
    let c2 = count.clone();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |_| {
        if std::thread::current().id() == me {
            c2.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let mut s = Session::default();
    let script: Value = serde_json::from_str(include_str!("../../../examples/regressions/join_struts_on_bore_wall.json")).unwrap();
    let result = s.run_script(&script);
    std::panic::set_hook(prev);
    assert_eq!(count.load(Ordering::SeqCst), 0, "kernel panics during the join");
    result.unwrap();
    assert_eq!(s.model.state().bodies.len(), 1);
}

/// A blind Ø2 hole into the flat back of a long D-profile bar ends 2 mm short of the round front,
/// yet the cut was refused (#544). It removes π · 1² · 10 mm³ (within 5 %: the measure tessellates
/// the small bore) and adds the bore's wall and floor to the bar's six faces.
#[test]
fn blind_hole_into_a_long_d_profile_bar_stops_short_of_the_round_front() {
    let script: Value = serde_json::from_str(include_str!("../../../examples/regressions/blind_hole_in_long_d_profile.json")).unwrap();
    let steps = script.get("commands").and_then(Value::as_array).unwrap();
    let mut s = Session::default();
    s.run_script(&Value::Array(steps[..5].to_vec())).unwrap();
    let bar = volume(&s, "Slat");
    s.run_script(&Value::Array(steps[5..].to_vec())).unwrap();
    let hole = std::f64::consts::PI * 10.0;
    let removed = bar - volume(&s, "Slat");
    assert!((removed - hole).abs() < 0.05 * hole, "removed {removed} vs {hole}");
    let faces = s.model.state().body("Slat").map(|b| solidcraft_kernel::measure(&b.body).unwrap().merged.faces);
    assert_eq!(faces, Some(8));
}

/// A ring pocket cut from below a recess leaves a 0.5 mm floor joining the outer wall to the inner
/// block: one connected part, yet the cut left two bodies and lost volume (#545).
#[test]
fn ring_cut_below_a_recess_keeps_the_part_in_one_body() {
    let s = replay(include_str!("../../../examples/regressions/ring_cut_below_recess_thin_floor.json"));
    let bodies: Vec<(String, f64)> = s.model.state().bodies.iter().map(|b| (b.name.clone(), volume(&s, &b.name))).collect();
    let want = 100.0 * 100.0 * 30.0 - 88.0 * 88.0 * 12.0 - (95.0 * 95.0 - 68.0 * 68.0) * 17.5;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    let got = volume(&s, "Frame");
    assert!((got - want).abs() < 1e-9 * want, "{got} vs {want}");
}

/// Four ribs joined on a rounded lid, standing on the plate over the skirt and flush with the
/// window's sides: each adds exactly its own material and nothing changes away from it. The
/// joins displaced the tool and one left a slit through the far side of the skirt and plate
/// (#616), which no volume shows: the area does. The reply of each join carries its warning, as
/// the timeline does (an agent placing parts over MCP saw none).
#[test]
fn ribs_flush_with_a_window_change_nothing_away_from_them() {
    let script: Value = serde_json::from_str(include_str!("../../../examples/regressions/lid_ribs_flush_with_window.json")).unwrap();
    let steps = script.get("commands").and_then(Value::as_array).unwrap();
    let (lid, ribs) = steps.split_at(steps.len() - 4);
    let mut s = Session::default();
    s.run_script(&Value::Array(lid.to_vec())).unwrap();
    let measure = |s: &Session| s.model.state().body("Lid").map(|b| solidcraft_kernel::measure(&b.body).unwrap()).unwrap();
    let (v0, a0) = (measure(&s).volume, measure(&s).area);
    assert!((v0 - 6703.5222).abs() < 1e-3, "{v0}");
    for (i, rib) in ribs.iter().enumerate() {
        let reply = s.execute(rib["command"].as_str().unwrap(), &rib["params"]).unwrap();
        let id = reply["feature"].as_u64().unwrap();
        let result = s.model.result(id).unwrap();
        assert_eq!(reply["warning"], json!(result.warning), "the reply says what the timeline says");
        assert!(!result.warning.as_deref().unwrap_or_default().contains("tool displacement"), "{:?}", result.warning);
        let m = measure(&s);
        let n = (i + 1) as f64;
        // 1.6 x 6 x 11.7 less the 1.35 x 6 x 3 inside the skirt.
        assert!((m.volume - (v0 + n * 88.02)).abs() < 1e-6, "rib {}: {}", i + 1, m.volume - v0);
        // Top 9.6, outer side 6 x 8.7 above the skirt, inner side 6 x 11.7 (flush with the
        // window's side), ends 2 x (1.6 x 11.7 - 1.35 x 3); the skirt top 8.1, its inner side
        // 18 and 1.5 of the plate go under the rib.
        let area = 9.6 + 52.2 + 70.2 + 29.34 - 8.1 - 18.0 - 1.5;
        assert!((m.area - (a0 + n * area)).abs() < 1e-6, "rib {}: area {} (no slit adds any)", i + 1, m.area - a0);
    }
    let bounds = s.model.state().body("Lid").unwrap().body.tessellate(0.01).unwrap().bounds();
    assert!((bounds.max.z - 14.1).abs() < 1e-9, "{}", bounds.max.z);
}

/// #679: one sketch of a 2 × 2 grid of 10 mm squares; three cells extruded up 10 mm (XO/XX) and
/// three down 10 mm, joined (XX/OX). The two L's meet back to back over two whole cells, along
/// an edge and at the centre: one solid of 6 cells × 10 mm × 100 mm².
#[test]
fn ls_extruded_up_and_down_from_one_grid_join() {
    let mut s = Session::default();
    let run = |s: &mut Session, id: &str, p: Value| s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Grid"}));
    for (p0, p1) in [([-10, -10], [0, 0]), ([0, -10], [10, 0]), ([-10, 0], [0, 10]), ([0, 0], [10, 10])] {
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": p0, "p1": p1}));
    }
    run(&mut s, "sketch.finish", json!({}));
    let cells = |pts: &[[f64; 2]]| Value::Array(pts.iter().map(|p| json!({"point": p})).collect());
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "Grid", "profiles": cells(&[[-5., 5.], [-5., -5.], [5., -5.]]), "distance": 10, "body_name": "Logo"}),
    );
    let reply = run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "Grid", "profiles": cells(&[[-5., 5.], [5., 5.], [5., -5.]]), "distance": "10 mm", "direction": "negative", "operation": "join"}),
    );
    assert!(reply["warning"].as_str().is_none_or(|w| !w.contains("tool displacement")), "{reply}");
    let st = s.model.state();
    assert_eq!(st.bodies.len(), 1);
    let body = &st.body("Logo").unwrap().body;
    assert_eq!(body.shell_count(), 1);
    assert!(body.validity().is_empty(), "{:?}", body.validity());
    let v = volume(&s, "Logo");
    assert!((v - 6000.0).abs() < 1e-6, "{v}");
}

/// This process's CPU time (all threads) and peak resident memory, where the system says
/// (Linux: /proc/self); `None` elsewhere.
fn cpu_and_peak_memory() -> Option<(std::time::Duration, u64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // utime and stime, in clock ticks (100 a second on Linux), after the command's name.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = status.lines().find_map(|l| l.strip_prefix("VmHWM:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some((std::time::Duration::from_millis(ticks * 10), kb * 1024))
}

/// Press/pull on the sensor enclosure's lid top: joining the pulled face's prism imprints the
/// contact (#825), and boxing the lid's faces by dividing their curved edges down to the
/// contact tolerance ran away (over 24 GB and many minutes). It takes about 9 s of CPU here and
/// no memory beyond the design's. The budgets are work, not wall time, which a loaded runner
/// stretches (61-87 s for the same work on CI).
#[test]
fn press_pull_on_the_enclosure_lid_is_prompt() {
    let mut s = Session::default();
    let script: Value = serde_json::from_str(include_str!("../../../examples/acceptance/sensor_enclosure.json")).unwrap();
    s.run_script(&script).unwrap();
    let total = |s: &Session| -> f64 { s.model.state().bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).sum() };
    let before = total(&s);
    let start = cpu_and_peak_memory();
    s.execute("solid.press_pull", &json!({"face": [-12.0, 27.4, 23.0], "distance": 5.0})).unwrap();
    if let (Some((cpu0, peak0)), Some((cpu1, peak1))) = (start, cpu_and_peak_memory()) {
        let grew = peak1.saturating_sub(peak0);
        assert!(grew < 1 << 30, "the press/pull grew the peak memory by {} MB", grew >> 20);
        // Other tests in this process (cargo test) count too, so the bound is generous.
        assert!(cpu1 - cpu0 < std::time::Duration::from_secs(300), "the press/pull took {:?} of CPU", cpu1 - cpu0);
    }
    assert!(total(&s) > before, "{before} -> {}", total(&s));
}

/// A lip swept round a block's rim, flush with its sides and round its corners, joins it as one
/// solid of both volumes: the block (50 × 40 with R4.5 corners, 10 high) and the lip (1 × 1 along
/// 41 + 31 + 41 mm of straight rim and two quarter turns between radii 3.5 and 4.5).
#[test]
fn lip_swept_flush_round_a_corner_joins() {
    let s = replay(include_str!("../../../examples/regressions/lip_swept_flush_round_corner.json"));
    let bodies = &s.model.state().bodies;
    assert_eq!(bodies.len(), 1);
    let b = &bodies[0];
    assert!(b.body.validity().is_empty(), "{:?}", b.body.validity());
    assert_eq!(b.body.shell_count(), 1);
    let block = (50.0 * 40.0 - (4.0 - std::f64::consts::PI) * 4.5 * 4.5) * 10.0;
    let lip = 41.0 + 31.0 + 41.0 + 2.0 * std::f64::consts::PI * (4.5 * 4.5 - 3.5 * 3.5) / 4.0;
    let volume = b.body.tessellate(1e-5).unwrap().measure().volume;
    assert!((volume - block - lip).abs() < 0.02, "{volume} expected {}", block + lip);
}

/// #1275: an Extrude Cut of a whole face of a rounded-rectangle block with a bottom chamfer. The
/// tool's corner cylinders lay on the block's; they are grown off them now, as straight edges
/// were, and the cut takes 5 mm off the top.
#[test]
fn cut_of_a_whole_face_beside_round_corners_after_a_chamfer() {
    let s = replay(include_str!("../../../examples/regressions/cut_whole_face_after_bottom_chamfer.json"));
    let bodies = &s.model.state().bodies;
    assert_eq!(bodies.len(), 1);
    let b = &bodies[0];
    assert!(b.body.validity().is_empty(), "{:?}", b.body.validity());
    let pi = std::f64::consts::PI;
    let block = (40.0 * 40.0 - (4.0 - pi) * 8.0 * 8.0) * 15.0;
    // The chamfer's 2 × 2 triangle swept along the loop (Pappus: its centroid is 2/3 mm in).
    let chamfer = 2.0 * (4.0 * 24.0 + 2.0 * pi * (8.0 - 2.0 / 3.0));
    let mesh = b.body.tessellate(1e-4).unwrap();
    let volume = mesh.measure().volume;
    assert!((volume - (block - chamfer)).abs() < 0.05, "{volume} expected {}", block - chamfer);
    let top = mesh.bounds().max.z;
    assert!((top - 15.0).abs() < 1e-6, "top at {top}");
}

/// The display tessellation of loft bodies joined into a bike frame plans at every display
/// quality and every refinement step. The edge nodes on an intersection of two lofts once
/// stepped back along the curve (the edge's fit rang where truck's parameter changes speed, and
/// took a leader point off the curve past a vertex), so a face's boundary crossed itself and
/// the body fell back to the reference mesher, which drew a sheet across the down tube.
#[test]
fn joined_loft_frames_plan_their_display_at_every_quality() {
    use solidcraft_doc::display_quality::{DisplayQuality, MAX_REFINE};
    for (script, name) in [
        (include_str!("../../../examples/regressions/loft_join_bracket_hull_triangle.json"), "DT2"),
        (include_str!("../../../examples/regressions/front_triangle_hull_triangle.json"), "FrontTriangle"),
    ] {
        let s = replay(script);
        let state = s.model.state();
        let body = &state.body(name).unwrap().body;
        for q in DisplayQuality::ALL {
            for k in 0..=MAX_REFINE {
                let plan = body.display_plan_angular(q.refined_chord(body.size(), k), q.refined_angle(k));
                assert!(plan.is_ok(), "{name} at {} + {k}: {:?}", q.id(), plan.err());
            }
        }
    }
}

/// A chamfered 240° ring cut by two prisms whose outlines have a corner exactly on its bore and
/// one in its end face, then notched into its top outer chamfer: each cut is exact (no retry
/// moved the tool), the two mirrored cuts take the same material, the notch takes what its
/// integral says, and the result meshes to its volume at a coarse tolerance too. The prism cuts
/// went to the last-resort nudge (seconds each, a face of no width left at each corner) and the
/// notch was refused for its volume: its hole in the chamfer meshed several mm² too large.
#[test]
fn oblique_cuts_through_a_chamfered_ring_are_exact() {
    let script: Value = serde_json::from_str(include_str!("../../../examples/regressions/oblique_cuts_through_chamfered_ring.json")).unwrap();
    let steps = script.get("commands").and_then(Value::as_array).unwrap();
    let mut s = Session::default();
    let mut volumes = Vec::new();
    for step in steps {
        let reply = s.execute(step["command"].as_str().unwrap(), &step["params"]).unwrap();
        if step["command"] == "solid.extrude" {
            assert!(reply["warning"].is_null(), "{reply}");
        }
        if matches!(step["command"].as_str(), Some("solid.move" | "solid.extrude")) {
            volumes.push(s.model.state().body("R").unwrap().body.tessellate(1e-5).unwrap().measure().volume);
        }
    }
    // Pappus: the profile's moment about the axis turned through 240°.
    let profile = [(16.6, 0.0), (17.5, 0.0), (18.5, 1.0), (18.5, 7.0), (17.5, 8.0), (16.6, 8.0), (16.4, 7.8), (16.4, 0.2)];
    let moment: f64 = (0..profile.len())
        .map(|i| {
            let ((x0, y0), (x1, y1)) = (profile[i], profile[(i + 1) % profile.len()]);
            (x0 * y1 - x1 * y0) * (x0 + x1) / 6.0
        })
        .sum();
    let ring = moment.abs() * 4.0 * std::f64::consts::PI / 3.0;
    let [whole, first, second, notched] = volumes[..] else { panic!("{volumes:?}") };
    // Volumes from meshes at 1e-5 mm: within a few millionths of the ring's.
    assert!((whole - ring).abs() < 1e-5 * ring, "{whole} vs {ring}");
    // The ring and the two prisms are mirror images across XZ.
    let (cut1, cut2) = (whole - first, first - second);
    assert!(cut1 > 5.0 && (cut1 - cut2).abs() < 1e-6, "{cut1} vs {cut2}");
    let notch = ring_notch();
    assert!(((second - notched) - notch).abs() < 1e-4, "{} vs {notch}", second - notched);
    let state = s.model.state();
    let body = &state.body("R").unwrap().body;
    assert!(body.validity().is_empty(), "{:?}", body.validity());
    // Faces meshed coarsely keep to their surfaces: chords cut a little area off round faces,
    // and nothing adds any (triangles fanning across a chamfer from the notch's hole or from the
    // cuts' trims, folded over each other, added a few mm² each).
    let (coarse, fine) = (body.tessellate(0.05).unwrap().measure().area, body.tessellate(1e-5).unwrap().measure().area);
    assert!(coarse < fine && coarse > fine - 2.0, "area meshed at 0.05: {coarse} vs {fine}");
    println!(
        "ring {whole:.6} / {ring:.6} mm³; cuts {cut1:.6}, {cut2:.6}; notch {:.6} / {notch:.6}; area meshed at 0.05 {coarse:.4} / {fine:.4}",
        second - notched
    );
}

/// The ring's notch alone: its hole in the top outer chamfer meshed several mm² too large, and
/// the cut was refused for an implausible volume after more than a minute; it takes what its
/// integral says.
#[test]
fn notch_into_a_ring_chamfer_is_cut_exactly() {
    let script: Value = serde_json::from_str(include_str!("../../../examples/regressions/oblique_cuts_through_chamfered_ring.json")).unwrap();
    let steps = script.get("commands").and_then(Value::as_array).unwrap();
    let mut s = Session::default();
    let measure = |s: &Session| s.model.state().body("R").unwrap().body.tessellate(1e-5).unwrap().measure().volume;
    s.run_script(&Value::Array(steps[..5].to_vec())).unwrap();
    let before = measure(&s);
    // The notch's sketch and its cut.
    for step in &steps[steps.len() - 4..] {
        let reply = s.execute(step["command"].as_str().unwrap(), &step["params"]).unwrap();
        assert!(reply["warning"].is_null(), "{reply}");
    }
    let (got, want) = (before - measure(&s), ring_notch());
    assert!((got - want).abs() < 1e-4, "{got} vs {want}");
}

/// What the ring's notch takes: a band 0.9 wide about a radius, from 17.6 out along it, and from
/// z 7.1 up to the top outer chamfer (r = 25.5 - z). Across the band (s), along it from 17.6 to
/// where r = 18.4, the height 18.4 - r: along it in closed form, across by Simpson's rule.
fn ring_notch() -> f64 {
    let along = |s: f64| {
        let x1 = (18.4f64 * 18.4 - s * s).sqrt();
        let g = |x: f64| {
            let r = (x * x + s * s).sqrt();
            18.4 * x - 0.5 * (x * r + s * s * (x + r).ln())
        };
        g(x1) - g(17.6)
    };
    let n = 2000;
    let h = 0.9 / n as f64;
    let weight = |i: usize| {
        if i == 0 || i == n {
            1.0
        } else if i % 2 == 1 {
            4.0
        } else {
            2.0
        }
    };
    (0..=n).map(|i| weight(i) * along(-0.45 + i as f64 * h)).sum::<f64>() * h / 3.0
}

/// The wedge of #1617: a side profile in (y, z) extruded 138 mm along x, its back face leaning
/// by `lean` (the tangent of its angle), with the issue's rounds. The last one runs along the
/// outline of both ends. The issue asked for R1 with `fit: reduce`, which reduces it to 0.854 mm;
/// that radius is asked for here directly, which builds the same body without the fit's search
/// (about a minute).
fn raked_wedge(lean: f64) -> Session {
    let mut s = Session::default();
    let top = 60.0 - 60.0 * lean;
    let profile = [[-60.0, 0.0], [60.0, 0.0], [top, 60.0], [38.0, 60.0], [-22.0, 22.0], [-60.0, 22.0]];
    let back = (60.0 + top) / 2.0;
    let ends: Vec<[f64; 3]> = [-69.0, 69.0].iter().flat_map(|&x| [[x, -41.0, 22.0], [x, 8.0, 41.0], [x, (38.0 + top) / 2.0, 60.0]]).collect();
    let steps = [
        ("sketch.create", json!({"plane": {"origin": [0, 0, 0], "x_dir": [0, 1, 0], "y_dir": [0, 0, 1]}, "name": "Profile"})),
        ("sketch.line", json!({"points": profile, "closed": true})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance": 138, "direction": "symmetric", "operation": "new", "body_name": "Wedge"})),
        ("solid.fillet", json!({"body": "Wedge", "radius": 1.2, "edges": [[0, -60, 22]]})),
        ("solid.fillet", json!({"body": "Wedge", "radius": 1.5, "edges": [[0, -22, 22]]})),
        ("solid.fillet", json!({"body": "Wedge", "radius": 1.5, "edges": [[0, 38, 60]]})),
        ("solid.fillet", json!({"body": "Wedge", "radius": 4, "edges": [[-69, -60, 12], [69, -60, 12], [-69, back, 30], [69, back, 30]]})),
        ("solid.fillet", json!({"body": "Wedge", "radius": 0.854, "edges": ends})),
    ];
    for (command, params) in steps {
        s.execute(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
    }
    s
}

/// A box intersect taking the end slice x >= 55 off the wedge of #1617 failed with "no result"
/// once its ends were rounded, when its back face leaned 10 degrees (a vertical one worked).
/// The leaning back shortens the top edge the end round runs along to 11.4 mm. Its strip there
/// borrowed sections from that short neighbour and folded back on itself past the joint, and
/// the boolean could not intersect the folded faces (fixed with #1667). The slice and the
/// rest, cut off by the same box, are sound and add up to the wedge, with the cut plane where
/// the issue had it and moved by 0.02 mm, as it also tried.
#[test]
fn box_intersect_splits_the_end_off_a_raked_wedge_after_its_end_rounds() {
    for lean in [10f64.to_radians().tan(), 0.0] {
        let mut s = raked_wedge(lean);
        let whole = volume(&s, "Wedge");
        for (i, x0) in [55.0, 55.02].into_iter().enumerate() {
            let mut piece = |op: &str| -> (f64, solidcraft_geom::Aabb3) {
                let copy = s.execute("solid.move", &json!({"bodies": ["Wedge"], "translate": [0, 0, 0], "copy": true})).unwrap();
                let copy = copy["created_bodies"][0].as_str().unwrap().to_string();
                let cap = format!("Cap{i}{op}");
                let length = 80.0 - x0;
                let cap_box = json!({"center": [x0 + length / 2.0, 0, 40], "length": length, "width": 160, "height": 120, "operation": "new", "body_name": cap});
                s.execute("solid.box", &cap_box).unwrap();
                let (target, tool) = if op == "intersect" { (cap, copy) } else { (copy, cap) };
                s.execute("solid.combine", &json!({"target": target, "tools": [tool], "operation": op}))
                    .unwrap_or_else(|e| panic!("lean {lean}, x >= {x0}, {op}: {e}"));
                let state = s.model.state();
                let body = &state.body(&target).unwrap().body;
                assert!(body.validity().is_empty(), "lean {lean}, x >= {x0}, {op}: {:?}", body.validity());
                (solidcraft_kernel::measure(body).unwrap().volume, body.tessellate(1e-3).unwrap().bounds())
            };
            let (slice, at) = piece("intersect");
            let (rest, _) = piece("cut");
            println!("#1617 lean {lean:.4}, x >= {x0}: slice {slice:.6} + rest {rest:.6} = {:.6}, wedge {whole:.6} mm³", slice + rest);
            assert!((slice + rest - whole).abs() < 1e-6 * whole, "{slice} + {rest} vs {whole}");
            assert!((at.min.x - x0).abs() < 1e-6 && (at.max.x - 69.0).abs() < 1e-3, "slice spans x {} to {}", at.min.x, at.max.x);
        }
    }
}

/// #1853: half a slot drawn on a plate's top face, cut along its centreline, revolved about a
/// construction line on that centreline as a cut. Half a turn into the plate and a full turn
/// both failed with "no result": the revolve came out wrong where its arcs end on the axis (a
/// fifth too much or too little), and the cut could not intersect it there. Both now take the
/// half of the capsule below the face.
#[test]
fn revolve_cut_of_a_half_slot_about_its_centreline_in_the_face() {
    let pi = std::f64::consts::PI;
    let inside = (pi * 9.0 * 14.0 + 4.0 / 3.0 * pi * 27.0) / 2.0;
    for angle in ["-180 deg", "360 deg"] {
        let mut s = Session::default();
        let steps = [
            ("solid.box", json!({"corner": [0, 0, 0], "length": 40, "width": 20, "height": 10, "body_name": "Plate"})),
            ("sketch.create", json!({"plane": {"origin": [0, 0, 10], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Half"})),
            ("sketch.line", json!({"points": [[10, 10], [30, 10]]})),
            ("sketch.line", json!({"points": [[27, 13], [13, 13]]})),
            ("sketch.arc.center_point", json!({"center": [13, 10], "start": [13, 13], "end": [10, 10]})),
            ("sketch.arc.center_point", json!({"center": [27, 10], "start": [30, 10], "end": [27, 13]})),
            ("sketch.line", json!({"points": [[5, 10], [35, 10]], "construction": true, "ids": ["axis"]})),
            ("sketch.finish", json!({})),
        ];
        for (command, params) in steps {
            s.execute(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
        }
        let whole = volume(&s, "Plate");
        s.execute("solid.revolve", &json!({"sketch": "Half", "axis": "axis", "angle": angle, "operation": "cut"}))
            .unwrap_or_else(|e| panic!("revolve {angle}: {e}"));
        let bad = s.model.state().body("Plate").map(|b| b.body.validity()).unwrap_or_default();
        assert!(bad.is_empty(), "{angle}: {bad:?}");
        let removed = whole - volume(&s, "Plate");
        println!("#1853 revolve {angle}: removed {removed:.6} mm³ of the {inside:.6} below the face");
        assert!((removed - inside).abs() < 1e-3 * inside, "{angle}: removed {removed} instead of {inside}");
    }
}

/// #2005: changing a circular sweep from a separate body to a half-turn cut must create the
/// internal channel, rather than failing the Boolean on coincident end caps.
#[test]
fn closed_sweep_redefines_to_a_half_turn_torus_cut() {
    let s = replay(include_str!("../../../examples/regressions/closed_sweep_torus_cut.json"));
    assert_eq!(s.model.state().bodies.len(), 1);
    let body = &s.model.state().bodies[0].body;
    assert!(body.validity().is_empty(), "{:?}", body.validity());
    let mesh = body.tessellate(0.02).unwrap();
    let expected = (std::f64::consts::PI * 64.0 - 64.0) * std::f64::consts::TAU * 30.0;
    assert!((mesh.measure().volume - expected).abs() < expected * 0.015);
    for (point, material) in [(solidcraft_geom::Vec3::new(30.0, 0.0, 0.0), false), (solidcraft_geom::Vec3::new(30.0, 0.0, 7.0), true)] {
        assert_eq!(mesh.contains(point), material, "material at {point:?}");
    }
}

#[test]
fn many_extrusion_profiles_preserve_material_and_body_identity() {
    for operation in ["join", "cut", "new"] {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 40, "width": 30, "height": 20, "body_name": "Stock"})).unwrap();
        s.execute("sketch.create", &json!({"plane": {"origin": [0, 0, 20], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}})).unwrap();
        for i in 0..6 {
            for j in 0..4 {
                let (x, y) = (2 + i * 6, 2 + j * 7);
                s.execute("sketch.rectangle.two_point", &json!({"p0": [x, y], "p1": [x + 4, y + 4]})).unwrap();
            }
        }
        s.execute("sketch.finish", &json!({})).unwrap();
        let distance = if operation == "cut" { -5 } else { 5 };
        let before = s.revision;
        s.execute("solid.extrude", &json!({"distance": distance, "operation": operation})).unwrap();
        let state = s.model.state();
        assert_eq!(state.bodies.len(), if operation == "new" { 25 } else { 1 });
        let expected = 24_000.0
            + match operation {
                "join" => 1920.0,
                "cut" => -1920.0,
                _ => 0.0,
            };
        assert!((volume(&s, "Stock") - expected).abs() < 1e-6);
        let stock = state.body("Stock").unwrap();
        for (point, present) in [([3.0, 3.0, 22.0], operation == "join"), ([3.0, 3.0, 18.0], operation != "cut"), ([7.0, 3.0, 18.0], true)] {
            assert_eq!(stock.mesh().contains(solidcraft_geom::Vec3::from(point)), present, "{operation}: {point:?}");
        }
        assert!(stock.body.validity().is_empty());
        assert!(s.revision > before);
        s.execute("edit.undo", &json!({})).unwrap();
        assert!((volume(&s, "Stock") - 24_000.0).abs() < 1e-6);
        s.execute("edit.redo", &json!({})).unwrap();
        assert!((volume(&s, "Stock") - expected).abs() < 1e-6);
    }
}

#[test]
fn many_circular_extrusion_profiles_cut_a_curved_target() {
    let mut s = Session::default();
    s.execute("solid.cylinder", &json!({"radius": 20, "height": 20, "body_name": "Stock"})).unwrap();
    s.execute("sketch.create", &json!({"plane": {"origin": [0, 0, -1], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}})).unwrap();
    for x in [-9, -3, 3, 9] {
        for y in [-9, -3, 3, 9] {
            s.execute("sketch.circle.center", &json!({"center": [x, y], "radius": 1})).unwrap();
        }
    }
    s.execute("sketch.finish", &json!({})).unwrap();
    s.execute("solid.extrude", &json!({"distance": 22, "operation": "cut", "targets": ["Stock"]})).unwrap();
    // The rebuilt caps mesh more finely than the original cylinder. Compare with the
    // analytic material volume, rather than subtracting two differently faceted meshes.
    let expected = (400.0 - 16.0) * std::f64::consts::PI * 20.0;
    let actual = volume(&s, "Stock");
    assert!((actual - expected).abs() < expected * 1e-5, "{actual} vs {expected}");
    let state = s.model.state();
    assert_eq!(state.bodies.len(), 1);
    let stock = state.body("Stock").unwrap();
    assert!(stock.body.validity().is_empty());
    let mesh = stock.mesh();
    assert!(mesh.contains(solidcraft_geom::Vec3::new(0.0, 0.0, 10.0)));
    for x in [-9.0, -3.0, 3.0, 9.0] {
        for y in [-9.0, -3.0, 3.0, 9.0] {
            assert!(!mesh.contains(solidcraft_geom::Vec3::new(x, y, 10.0)), "hole at {x}, {y}");
        }
    }
}

#[test]
fn coplanar_body_faces_extrude_together_and_replay_their_links() {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length":64,"width":64,"height":20,"body_name":"Plate"})).unwrap();
    s.execute("sketch.create", &json!({"plane":{"origin":[0,0,20],"x_dir":[1,0,0],"y_dir":[0,1,0]}})).unwrap();
    let mut faces = Vec::new();
    for i in 0..8 {
        for j in 0..8 {
            let (x, y) = (2 + i * 7, 2 + j * 7);
            s.execute("sketch.rectangle.two_point", &json!({"p0":[x,y],"p1":[x+4,y+4]})).unwrap();
            faces.push(json!([x + 2, y + 2, 26]));
        }
    }
    s.execute("sketch.finish", &json!({})).unwrap();
    s.execute("solid.extrude", &json!({"distance":6,"operation":"join"})).unwrap();
    let (features, undo) = (s.doc.features.len(), s.undo.len());
    s.execute("solid.extrude", &json!({"faces":faces,"distance":-7,"operation":"cut"})).unwrap();
    assert_eq!(s.doc.features.len(), features + 2, "one linked sketch and one extrusion");
    assert_eq!(s.undo.len(), undo + 1);
    let actual = volume(&s, "Plate");
    let state = s.model.state();
    let sketch = state.sketch(4).unwrap();
    assert!((actual - 80_896.0).abs() < 1e-5, "volume {actual}; plane {:?}; profiles {}", sketch.plane, sketch.profiles.len());
    assert_eq!(s.model.state().bodies.len(), 1);
    let document = serde_json::from_value(serde_json::to_value(s.doc.as_ref()).unwrap()).unwrap();
    let mut reopened = Session { doc: std::sync::Arc::new(document), ..Session::default() };
    reopened.refresh();
    assert!((volume(&reopened, "Plate") - 80_896.0).abs() < 1e-5);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc.features.len(), features);
    s.execute("edit.redo", &json!({})).unwrap();
    assert!((volume(&s, "Plate") - 80_896.0).abs() < 1e-5);
}

#[test]
fn unsupported_face_groups_are_atomic() {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length":10,"width":10,"height":10})).unwrap();
    let before = serde_json::to_value(s.doc.as_ref()).unwrap();
    let (revision, undo) = (s.revision, s.undo.len());
    for faces in [
        json!([]),
        json!([[5, 5, 10], [5, 5, 0]]),
        json!([[5, 5, 10], [5, 0, 5]]),
        json!([[5, 5, 10], [1000, 1000, 1000]]),
        json!(vec![[5, 5, 10]; 1025]),
    ] {
        assert!(s.execute("solid.extrude", &json!({"faces":faces,"distance":5,"operation":"join"})).is_err());
        assert_eq!(serde_json::to_value(s.doc.as_ref()).unwrap(), before);
        assert_eq!((s.revision, s.undo.len()), (revision, undo));
    }
}

#[test]
fn imported_rotated_face_extrusions_preserve_other_operations_and_bodies() {
    use solidcraft_geom::Vec3;
    let mut source = Session::default();
    source.execute("solid.box", &json!({"corner":[-16,-16,-3],"length":32,"width":32,"height":1,"body_name":"Other"})).unwrap();
    source.execute("solid.cylinder", &json!({"radius":14,"height":8,"body_name":"Stock"})).unwrap();
    let mut outlines = Vec::new();
    let mut centers = Vec::new();
    for i in 0..12 {
        let angle = i as f64 * std::f64::consts::TAU / 12.;
        let (c, s) = (angle.cos(), angle.sin());
        let outline: Vec<_> =
            [(-2., -2.), (2., -2.), (2., 2.), (-2., 2.)].into_iter().map(|(x, y)| [10.7 * c + x * c - y * s, 10.7 * s + x * s + y * c]).collect();
        outlines.push(outline);
        centers.push([10.7 * c, 10.7 * s, 12.]);
    }
    let draw = |session: &mut Session, height: f64| {
        session.execute("sketch.create", &json!({"plane":{"origin":[0,0,height],"x_dir":[1,0,0],"y_dir":[0,1,0]}})).unwrap();
        for outline in &outlines {
            session.execute("sketch.line", &json!({"points":outline,"closed":true,"infer":false})).unwrap();
        }
        session.execute("sketch.finish", &json!({})).unwrap();
    };
    draw(&mut source, 8.);
    source.execute("solid.extrude", &json!({"distance":4,"operation":"join"})).unwrap();
    // An original public-safe STEP example, with rotated full-face seams and a
    // curved target. No vendor/user design is part of this regression.
    let path = std::env::temp_dir().join(format!("solidcraft-face-extrusion-{}.step", std::process::id()));
    source.execute("file.export", &json!({"path":path.to_string_lossy()})).unwrap();
    let mut imported = Session::default();
    imported.execute("doc.open", &json!({"path":path.to_string_lossy()})).unwrap();
    std::fs::remove_file(path).unwrap();
    let original = imported.model.state();
    let other = original.body("Other").unwrap().body.clone();
    for (operation, distance) in [("join", 2.), ("cut", -1.), ("new", 2.), ("intersect", -1.)] {
        let mut grouped = imported.scratch();
        grouped.execute("solid.extrude", &json!({"faces":centers,"distance":distance,"operation":operation})).unwrap();
        let mut reference = imported.scratch();
        draw(&mut reference, 12.);
        if matches!(operation, "join" | "cut") {
            // Individual tools retain the original Boolean and participant path.
            for index in 0..12 {
                reference
                    .execute("solid.extrude", &json!({"sketch":"Sketch1","profiles":[index],"distance":distance,"operation":operation}))
                    .unwrap();
            }
        } else {
            reference.execute("solid.extrude", &json!({"distance":distance,"operation":operation})).unwrap();
        }
        let actual = grouped.model.state();
        let expected = reference.model.state();
        assert_eq!(actual.bodies.len(), expected.bodies.len(), "{operation}");
        for body in &actual.bodies {
            let reference = expected.body(&body.name).unwrap();
            // Fixed fine chord tolerance: cap subdivision can change the default
            // adaptive mesh's cylinder faceting without changing its analytic surface.
            let (a, b) = (body.body.tessellate(1e-5).unwrap().measure().volume, reference.body.tessellate(1e-5).unwrap().measure().volume);
            assert!((a - b).abs() < b.max(1.) * 1e-6, "{operation}: {} {a} vs {b}", body.name);
            assert!(body.body.validity().is_empty(), "{operation}: {}", body.name);
            let (a, b) = (body.mesh(), reference.mesh());
            for center in &centers {
                for z in [7., 11.5, 13.] {
                    let point = Vec3::new(center[0], center[1], z);
                    assert_eq!(a.contains(point), b.contains(point), "{operation}: {} {point:?}", body.name);
                }
            }
        }
        if operation != "intersect" {
            let unchanged = &actual.body("Other").unwrap().body;
            assert_eq!(unchanged.face_count(), other.face_count());
            assert_eq!(unchanged.color(), other.color());
            assert_eq!(unchanged.tessellate(0.01).unwrap().positions, other.tessellate(0.01).unwrap().positions, "unaffected imported body");
        }
    }
}

#[test]
fn multi_target_join_keeps_nearby_bodies_and_original_absorption_order() {
    for (second_x, expected_count) in [(10.0015, 2), (9., 1)] {
        let mut session = Session::default();
        session.execute("solid.box", &json!({"length":10,"width":10,"height":10,"body_name":"Stock"})).unwrap();
        session.execute("solid.box", &json!({"corner":[second_x,8,10],"length":1,"width":1,"height":2,"body_name":"Other"})).unwrap();
        session.execute("sketch.create", &json!({"plane":{"origin":[0,0,10],"x_dir":[1,0,0],"y_dir":[0,1,0]}})).unwrap();
        for origin in [[0., 0.], [8., 8.]] {
            session.execute("sketch.rectangle.two_point", &json!({"p0":origin,"p1":[origin[0]+2.,origin[1]+2.]})).unwrap();
        }
        session.execute("sketch.finish", &json!({})).unwrap();
        let mut reference = session.scratch();
        for index in 0..2 {
            reference.execute("solid.extrude", &json!({"sketch":"Sketch1","profiles":[index],"distance":2,"operation":"join"})).unwrap();
        }
        session.execute("solid.extrude", &json!({"distance":2,"operation":"join"})).unwrap();
        assert_eq!(session.model.state().bodies.len(), expected_count);
        assert_eq!(
            session.model.state().bodies.iter().map(|body| body.name.clone()).collect::<Vec<_>>(),
            reference.model.state().bodies.iter().map(|body| body.name.clone()).collect::<Vec<_>>()
        );
        for body in &session.model.state().bodies {
            assert!((volume(&session, &body.name) - volume(&reference, &body.name)).abs() < 1e-6);
        }
    }
}
