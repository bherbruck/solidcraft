//! Hidden-line investigation harness (#2105, Refs #860 W05). Ignored tests that build the
//! fixtures, run the exact visibility prototype (`solidcraft_geom::hlr`), compare it with the
//! mesh-based `file.export_view_dxf`, and write SVG/JSON evidence to `$HLR_OUT`.
//! Run: `HLR_OUT=<dir> cargo test --release -p solidcraft-engine --lib hlr_investigation -- --ignored --nocapture`

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};
use solidcraft_geom::Vec3;
use solidcraft_geom::hlr::{self, Drawing, Options, Placement, Prepared, Source, View, Visibility};
use solidcraft_render::Camera;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn out_dir() -> PathBuf {
    let dir = std::env::var("HLR_OUT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("solidcraft-hlr"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn camera(yaw: f64, pitch: f64) -> (Camera, View) {
    let cam = Camera { target: Vec3::ZERO, yaw, pitch, roll: 0.0, distance: 500.0, fov: 0.0, up: Default::default() };
    let (right, up, back) = cam.basis();
    (cam, View { x: right, y: up, d: -back })
}

const IDENTITY: [Vec3; 3] = [Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.), Vec3::new(0., 0., 1.)];

/// Every visible body of the session, prepared once each, at identity placements (world bodies).
fn scene(s: &Session, opts: &Options) -> (Vec<Prepared>, Vec<Placement>, Vec<Value>, f64) {
    let started = Instant::now();
    let world = s.world_state();
    let mut defs = Vec::new();
    let mut placements = Vec::new();
    let mut reports = Vec::new();
    for b in world.bodies.iter().filter(|b| s.body_visible(&b.name)) {
        let brep = solidcraft_kernel::portable_body(&b.body).unwrap();
        if let Ok(name) = std::env::var("HLR_DUMP") {
            std::fs::write(out_dir().join(format!("{name}-{}.brep.json", defs.len())), serde_json::to_string(&brep).unwrap()).unwrap();
        }
        let def = hlr::prepare(&brep, opts).unwrap();
        reports.push(json!({"body": b.name, "report": def.report}));
        placements.push(Placement { body: defs.len(), rotation: IDENTITY, translation: Vec3::ZERO });
        defs.push(def);
    }
    (defs, placements, reports, started.elapsed().as_secs_f64() * 1e3)
}

fn never() -> bool {
    false
}

/// What is drawn: pieces covered by a coincident visible line are not (visible wins).
fn effective(p: &hlr::Piece) -> Option<Visibility> {
    (!p.covered).then_some(p.visibility)
}

/// SVG with screen y up; visible solid, hidden dashed, uncertain red.
fn svg(d: &Drawing, dxf: Option<&[(bool, [f64; 2], [f64; 2])]>) -> String {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in d.lines.iter().flat_map(|l| l.pieces.iter()).flat_map(|p| p.points.iter()) {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let pad = 0.05 * (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.);
    let (w, h) = (hi[0] - lo[0] + 2. * pad, hi[1] - lo[1] + 2. * pad);
    let panels = if dxf.is_some() { 2. } else { 1. };
    let sw = 0.0035 * w.max(h);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {:.4} {:.4}\" width=\"{:.0}\" height=\"{:.0}\"><rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n",
        w * panels,
        h,
        600. * panels,
        600. * h / w
    );
    let map = |p: [f64; 2], dx: f64| format!("{:.5},{:.5}", p[0] - lo[0] + pad + dx, hi[1] - p[1] + pad);
    let mut ordered: Vec<(&hlr::Line, &hlr::Piece)> = d.lines.iter().flat_map(|l| l.pieces.iter().map(move |p| (l, p))).collect();
    ordered.retain(|(_, p)| effective(p).is_some());
    ordered.sort_by_key(|(_, p)| match effective(p) {
        Some(Visibility::Hidden) | None => 0,
        Some(Visibility::Visible) => 1,
        Some(Visibility::Uncertain) => 2,
    });
    for (l, p) in ordered {
        let sil = matches!(l.source, Source::Silhouette { .. });
        {
            let (colour, dash) = match p.visibility {
                Visibility::Visible => (if sil { "#1f4e9e" } else { "black" }, String::new()),
                Visibility::Hidden => ("#888", format!(" stroke-dasharray=\"{:.4} {:.4}\"", 4. * sw, 2. * sw)),
                Visibility::Uncertain => ("#d00", String::new()),
            };
            let pts: Vec<String> = p.points.iter().map(|q| map(*q, 0.)).collect();
            out += &format!("<polyline fill=\"none\" stroke=\"{colour}\" stroke-width=\"{sw:.4}\"{dash} points=\"{}\"/>\n", pts.join(" "));
        }
    }
    if let Some(lines) = dxf {
        for (hidden, a, b) in lines {
            let (colour, dash) =
                if *hidden { ("#888", format!(" stroke-dasharray=\"{:.4} {:.4}\"", 4. * sw, 2. * sw)) } else { ("black", String::new()) };
            out +=
                &format!("<polyline fill=\"none\" stroke=\"{colour}\" stroke-width=\"{sw:.4}\"{dash} points=\"{} {}\"/>\n", map(*a, w), map(*b, w));
        }
    }
    out + "</svg>\n"
}

/// LINE and LWPOLYLINE segments of the VISIBLE/HIDDEN layers: (hidden, a, b).
fn dxf_segments(text: &str) -> Vec<(bool, [f64; 2], [f64; 2])> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut kind = String::new();
    let mut layer = String::new();
    let mut pts: Vec<[f64; 2]> = Vec::new();
    let mut end: [f64; 2] = [0., 0.];
    let mut x = 0.;
    let flush = |kind: &str, layer: &str, pts: &[[f64; 2]], end: [f64; 2], out: &mut Vec<(bool, [f64; 2], [f64; 2])>| {
        let hidden = layer == "HIDDEN";
        if kind == "LINE" {
            if let Some(a) = pts.first() {
                out.push((hidden, *a, end));
            }
        } else if kind == "LWPOLYLINE" {
            for w in pts.windows(2) {
                out.push((hidden, w[0], w[1]));
            }
        }
    };
    while i + 1 < lines.len() {
        let (code, val) = (lines[i], lines[i + 1]);
        match code {
            "0" => {
                flush(&kind, &layer, &pts, end, &mut out);
                kind = val.to_string();
                pts.clear();
            }
            "8" => layer = val.to_string(),
            "10" => x = val.parse().unwrap_or(0.),
            "20" => pts.push([x, val.parse().unwrap_or(0.)]),
            "11" => x = val.parse().unwrap_or(0.),
            "21" => end = [x, val.parse().unwrap_or(0.)],
            _ => {}
        }
        i += 2;
    }
    flush(&kind, &layer, &pts, end, &mut out);
    out
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let l2 = ex * ex + ey * ey;
    let t = if l2 > 0. { (((p[0] - a[0]) * ex + (p[1] - a[1]) * ey) / l2).clamp(0., 1.) } else { 0. };
    ((a[0] + t * ex - p[0]).powi(2) + (a[1] + t * ey - p[1]).powi(2)).sqrt()
}

fn length(polys: &[Vec<[f64; 2]>]) -> f64 {
    polys.iter().flat_map(|p| p.windows(2)).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum()
}

/// Agreement between the exact drawing (pieces that are drawn) and the mesh DXF, by length:
/// each side's lines, sampled every 0.02 mm, are matched to the other side's lines within
/// 0.05 mm (five times the DXF's default chord tolerance) of the same class, of the other
/// class, or of none. DXF draws hidden lines under coincident visible ones; the exact drawing
/// drops them, so DXF "hidden on exact visible" is expected there.
fn compare(d: &Drawing, dxf: &[(bool, [f64; 2], [f64; 2])]) -> Value {
    const STEP: f64 = 0.02;
    const TOL: f64 = 0.05;
    let ours: Vec<(bool, [f64; 2], [f64; 2])> = d
        .lines
        .iter()
        .flat_map(|l| l.pieces.iter())
        .filter_map(|p| match effective(p) {
            Some(Visibility::Visible) => Some((false, p)),
            Some(Visibility::Hidden) => Some((true, p)),
            _ => None,
        })
        .flat_map(|(h, p)| p.points.windows(2).map(move |w| (h, w[0], w[1])))
        .collect();
    let uncertain_len: f64 = d
        .lines
        .iter()
        .flat_map(|l| l.pieces.iter())
        .filter(|p| effective(p) == Some(Visibility::Uncertain))
        .map(|p| length(std::slice::from_ref(&p.points)))
        .sum();
    // Distance from q to the nearest segment of each class: [visible, hidden].
    let nearest = |segs: &[(bool, [f64; 2], [f64; 2])], q: [f64; 2]| {
        let mut best = [f64::MAX; 2];
        for (h, a, b) in segs {
            let k = usize::from(*h);
            best[k] = best[k].min(seg_dist(q, *a, *b));
        }
        best
    };
    // Uncertain chords of the exact drawing (uncovered: covered ones lie on visible lines).
    let unsure: Vec<([f64; 2], [f64; 2])> = d
        .lines
        .iter()
        .flat_map(|l| l.pieces.iter())
        .filter(|p| effective(p) == Some(Visibility::Uncertain))
        .flat_map(|p| p.points.windows(2).map(|w| (w[0], w[1])))
        .collect();
    let tally = |from: &[(bool, [f64; 2], [f64; 2])], to: &[(bool, [f64; 2], [f64; 2])]| {
        let mut out = json!({});
        for (name, hidden) in [("visible", false), ("hidden", true)] {
            let polys: Vec<Vec<[f64; 2]>> = from.iter().filter(|s| s.0 == hidden).map(|s| vec![s.1, s.2]).collect();
            // Midpoints of pieces at most STEP long, each weighted by its length.
            let mut pts: Vec<([f64; 2], f64)> = Vec::new();
            for p in &polys {
                for w in p.windows(2) {
                    let l = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
                    let n = (l / STEP).ceil().max(1.) as usize;
                    for k in 0..n {
                        let f = (k as f64 + 0.5) / n as f64;
                        pts.push(([w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f], l / n as f64));
                    }
                }
            }
            let (mut same, mut other, mut on_unsure, mut none) = (0f64, 0f64, 0f64, 0f64);
            let mut dev: Vec<f64> = Vec::new();
            for (q, w) in &pts {
                let near = nearest(to, *q);
                let (s, o) = (near[usize::from(hidden)], near[usize::from(!hidden)]);
                if s <= TOL {
                    same += w;
                    dev.push(s);
                } else if o <= TOL {
                    other += w;
                } else if unsure.iter().any(|(a, b)| seg_dist(*q, *a, *b) <= TOL) {
                    on_unsure += w;
                } else {
                    none += w;
                }
            }
            dev.sort_by(f64::total_cmp);
            let total = (same + other + on_unsure + none).max(1e-300);
            out[name] = json!({
                "length": length(&polys),
                "same_class_share": same / total, "other_class_share": other / total,
                "on_exact_uncertain_share": on_unsure / total, "unmatched_share": none / total,
                "matched_distance_mm": {"median": dev.get(dev.len() / 2).copied().unwrap_or(0.), "max": dev.last().copied().unwrap_or(0.)},
            });
        }
        out
    };
    // Every classified piece, drawn or not: covered hidden pieces lie under visible lines, where
    // the DXF also draws its hidden lines, so classification is compared against these.
    // Uncertain pieces carry no classification, covered or not (their length is reported).
    let classified: Vec<(bool, [f64; 2], [f64; 2])> = d
        .lines
        .iter()
        .flat_map(|l| l.pieces.iter())
        .filter_map(|p| match p.visibility {
            Visibility::Visible => Some((false, p)),
            Visibility::Hidden => Some((true, p)),
            Visibility::Uncertain => None,
        })
        .flat_map(|(h, p)| p.points.windows(2).map(move |w| (h, w[0], w[1])))
        .collect();
    json!({
        "exact_vs_dxf": tally(&ours, dxf),
        "dxf_vs_exact_drawn": tally(dxf, &ours),
        "dxf_vs_exact_classified": tally(dxf, &classified),
        "exact_uncertain_length": uncertain_len,
    })
}

struct ViewCase {
    name: &'static str,
    yaw: f64,
    pitch: f64,
}

/// Runs the exact drawing and the DXF export for each view, writes SVG/JSON, returns the report.
fn study(name: &str, s: &mut Session, views: &[ViewCase], check: &dyn Fn(&str, &Drawing) -> Value) -> Value {
    if std::env::var("HLR_ONLY").is_ok_and(|only| !only.split(',').any(|o| o == name)) {
        return json!({"fixture": name, "skipped": true});
    }
    let opts = Options::default();
    eprintln!("[{name}] prepare");
    let (defs, placements, reports, prepare_ms) = scene(s, &opts);
    let mut out = json!({"fixture": name, "prepare_ms": prepare_ms, "bodies": reports, "views": []});
    for v in views {
        let (cam, view) = camera(v.yaw, v.pitch);
        eprintln!("[{name}/{}] exact", v.name);
        let started = Instant::now();
        let d = hlr::hidden_lines(&defs, &placements, &view, &opts, &never).unwrap();
        let exact_ms = started.elapsed().as_secs_f64() * 1e3;
        eprintln!("[{name}/{}] dxf", v.name);
        let started = Instant::now();
        let r = run(s, "file.export_view_dxf", json!({"camera": serde_json::to_value(cam).unwrap(), "hidden": "dashed"}));
        let dxf_ms = started.elapsed().as_secs_f64() * 1e3;
        let segs = dxf_segments(r["dxf"].as_str().unwrap_or(""));
        let file = format!("{name}-{}", v.name);
        std::fs::write(out_dir().join(format!("{file}.svg")), svg(&d, Some(&segs))).unwrap();
        std::fs::write(out_dir().join(format!("{file}.json")), serde_json::to_string_pretty(&d).unwrap()).unwrap();
        let entry = json!({
            "view": v.name, "yaw": v.yaw, "pitch": v.pitch, "exact_ms": exact_ms, "dxf_ms": dxf_ms,
            "stats": d.stats, "dxf_stats": r["stats"], "compare_with_dxf": compare(&d, &segs), "checks": check(v.name, &d),
        });
        println!("{name}/{}: {}", v.name, serde_json::to_string(&entry).unwrap());
        out["views"].as_array_mut().unwrap().push(entry);
    }
    out
}

/// Screen points of pieces with the given visibility from the given sources.
fn points(d: &Drawing, vis: Visibility, keep: &dyn Fn(&Source) -> bool) -> Vec<[f64; 2]> {
    d.lines
        .iter()
        .filter(|l| keep(&l.source))
        .flat_map(|l| l.pieces.iter())
        .filter(|p| effective(p) == Some(vis))
        .flat_map(|p| p.points.iter().copied())
        .collect()
}

fn max_of(v: impl Iterator<Item = f64>) -> f64 {
    v.fold(0., f64::max)
}

#[test]
#[ignore = "investigation: writes evidence"]
fn fixtures() {
    let half_pi = std::f64::consts::FRAC_PI_2;
    let mut all = Vec::new();

    // Stepped block: 60 x 40 x 30 with the back-bottom step (y 20..40, z 0..15) removed.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 30, "body_name": "B"}));
    run(&mut s, "solid.box", json!({"length": 60, "width": 20, "height": 15, "corner": [0, 20, 0], "operation": "cut", "targets": ["B"]}));
    all.push(study(
        "stepped",
        &mut s,
        &[ViewCase { name: "front", yaw: 0., pitch: 0. }, ViewCase { name: "top", yaw: 0., pitch: half_pi }, ViewCase { name: "iso", yaw: 0.6, pitch: 0.5 }],
        &|view, d| {
            let hid = points(d, Visibility::Hidden, &|_| true);
            match view {
                "front" => json!({"hidden_max_abs_z_minus_15": max_of(hid.iter().map(|p| (p[1] - 15.).abs())), "hidden_x_range": [hid.iter().map(|p| p[0]).fold(f64::MAX, f64::min), max_of(hid.iter().map(|p| p[0]))]}),
                "top" => json!({"hidden_max_abs_y_minus_20": max_of(hid.iter().map(|p| (p[1] - 20.).abs()))}),
                _ => json!({}),
            }
        },
    ));

    // Cylinder R10 x 30 with a cross hole R3 along Y at z = 15.
    let cross = |s: &mut Session| {
        run(s, "solid.cylinder", json!({"radius": 10, "height": 30, "body_name": "C"}));
        run(s, "solid.cylinder", json!({"radius": 3, "height": 40, "base": [0, -20, 15], "axis": "Y", "operation": "cut", "targets": ["C"]}));
    };
    let mut s = Session::default();
    cross(&mut s);
    all.push(study(
        "crosshole",
        &mut s,
        &[
            ViewCase { name: "side", yaw: -half_pi, pitch: 0. },
            ViewCase { name: "front", yaw: 0., pitch: 0. },
            ViewCase { name: "iso", yaw: 0.6, pitch: 0.5 },
        ],
        &|view, d| match view {
            "side" => {
                let sil = |l: &Source| matches!(l, Source::Silhouette { .. });
                let hid = points(d, Visibility::Hidden, &sil);
                let vis = points(d, Visibility::Visible, &sil);
                json!({
                    "hidden_silhouettes_max_abs_z_minus_12_or_18": max_of(hid.iter().map(|p| (p[1] - 12.).abs().min((p[1] - 18.).abs()))),
                    "hidden_silhouette_x_range": [hid.iter().map(|p| p[0]).fold(f64::MAX, f64::min), max_of(hid.iter().map(|p| p[0]))],
                    "visible_silhouettes_max_abs_x_minus_10": max_of(vis.iter().map(|p| (p[0].abs() - 10.).abs())),
                    "visible_silhouette_points_inside_hole_band": vis.iter().filter(|p| p[1] > 12. + 1e-9 && p[1] < 18. - 1e-9).count(),
                })
            }
            "front" => {
                let edges = |l: &Source| matches!(l, Source::Edge { .. });
                let pts = points(d, Visibility::Visible, &edges);
                let ring: Vec<f64> = pts.iter().map(|p| (p[0].hypot(p[1] - 15.) - 3.).abs()).filter(|e| *e < 1.).collect();
                json!({"hole_outline_max_radius_error": max_of(ring.iter().copied()), "hole_outline_points": ring.len()})
            }
            _ => json!({}),
        },
    ));

    // Cone: a right triangle (0,0), (10,0), (0,20) on XZ revolved about Z.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XZ"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 0], [0, 20]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.revolve", json!({"axis": "Z"}));
    all.push(study(
        "cone",
        &mut s,
        &[ViewCase { name: "side", yaw: 0., pitch: 0. }, ViewCase { name: "oblique", yaw: 0.4, pitch: 0.5 }],
        &|view, d| {
            let sil = |l: &Source| matches!(l, Source::Silhouette { .. });
            let vis = points(d, Visibility::Visible, &sil);
            match view {
                // Apex at z = +-20 depending on the sketch's XZ orientation; lines |x| = 10 (1 - |z|/20).
                "side" => json!({"silhouette_max_line_error": max_of(vis.iter().map(|p| (p[0].abs() - 10. * (1. - p[1].abs() / 20.)).abs()))}),
                _ => json!({"visible_silhouette_points": vis.len()}),
            }
        },
    ));

    // Torus R20 r5.
    let mut s = Session::default();
    run(&mut s, "solid.torus", json!({"major": 20, "minor": 5}));
    all.push(study(
        "torus",
        &mut s,
        &[ViewCase { name: "faceon", yaw: 0., pitch: half_pi }, ViewCase { name: "oblique", yaw: 0.3, pitch: 0.4 }, ViewCase { name: "side", yaw: 0., pitch: 0. }],
        &|view, d| {
            let sil = |l: &Source| matches!(l, Source::Silhouette { .. });
            let vis = points(d, Visibility::Visible, &sil);
            match view {
                "faceon" => json!({
                    "silhouette_max_error_from_r25_or_r15": max_of(vis.iter().map(|p| { let r = p[0].hypot(p[1]); (r - 25.).abs().min((r - 15.).abs()) })),
                    "points": vis.len(),
                }),
                _ => json!({"visible_silhouette_points": vis.len()}),
            }
        },
    ));

    // Sphere R10 at the origin, any view: a silhouette circle of radius 10.
    let mut s = Session::default();
    run(&mut s, "solid.sphere", json!({"radius": 10}));
    all.push(study("sphere", &mut s, &[ViewCase { name: "oblique", yaw: 0.3, pitch: 0.7 }], &|_, d| {
        let sil = |l: &Source| matches!(l, Source::Silhouette { .. });
        let vis = points(d, Visibility::Visible, &sil);
        json!({"silhouette_max_radius_error": max_of(vis.iter().map(|p| (p[0].hypot(p[1]) - 10.).abs())), "points": vis.len()})
    }));

    // Tangent edges: a box with one rounded edge.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.fillet", json!({"edges": [[20, 0, 20]], "radius": 5}));
    all.push(study(
        "fillet",
        &mut s,
        &[ViewCase { name: "iso", yaw: 0.6, pitch: 0.5 }, ViewCase { name: "side", yaw: -half_pi, pitch: 0. }],
        &|_, _| json!({}),
    ));

    // Coincident faces: two separate boxes touching on x = 10.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [10, 0, 0], "operation": "new"}));
    all.push(study("touching", &mut s, &[ViewCase { name: "iso", yaw: 0.6, pitch: 0.5 }], &|_, _| json!({})));

    // Geometry behind a cut: the cross-hole cylinder split on XZ, the y > 0 half seen from -y.
    let mut s = Session::default();
    cross(&mut s);
    run(&mut s, "solid.split_body", json!({"body": "C", "plane": "XZ"}));
    let world = s.world_state();
    let names: Vec<(String, f64)> = world
        .bodies
        .iter()
        .map(|b| {
            let brep = solidcraft_kernel::portable_body(&b.body).unwrap();
            let ys: Vec<f64> = brep.shells.iter().flat_map(|sh| sh.vertices.iter().map(|v| v[1])).collect();
            (b.name.clone(), ys.iter().sum::<f64>() / ys.len().max(1) as f64)
        })
        .collect();
    for (n, y) in &names {
        if *y < 0. {
            run(&mut s, "browser.visibility", json!({"items": [n], "visible": false}));
        }
    }
    all.push(study("cut", &mut s, &[ViewCase { name: "front", yaw: 0., pitch: 0. }, ViewCase { name: "iso", yaw: 0.6, pitch: 0.5 }], &|view, d| {
        if view != "front" {
            return json!({});
        }
        let vis = points(d, Visibility::Visible, &|_| true);
        let ring: Vec<f64> = vis.iter().map(|p| (p[0].hypot(p[1] - 15.) - 3.).abs()).filter(|e| *e < 1.).collect();
        json!({"cut_hole_max_radius_error": max_of(ring.iter().copied()), "points": ring.len()})
    }));

    // A spline face the recogniser does not know: the rounded top edge of a cylinder.
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 30}));
    run(&mut s, "solid.fillet", json!({"edges": [[10, 0, 30]], "radius": 3}));
    all.push(study("roundcyl", &mut s, &[ViewCase { name: "side", yaw: 0., pitch: 0. }, ViewCase { name: "iso", yaw: 0.6, pitch: 0.5 }], &|_, _| {
        json!({})
    }));

    // Freeform faces: a loft from a circle R15 at z = 0 to a 20 x 10 rectangle at z = 30.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 0], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Low"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 15}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 30], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "High"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-10, -5], "p1": [10, 5]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "Low"}, {"sketch": "High"}]}));
    all.push(study("loft", &mut s, &[ViewCase { name: "side", yaw: 0., pitch: 0. }, ViewCase { name: "iso", yaw: 0.6, pitch: 0.5 }], &|_, _| {
        json!({})
    }));

    std::fs::write(out_dir().join("fixtures.json"), serde_json::to_string_pretty(&all).unwrap()).unwrap();
}

#[test]
#[ignore = "investigation: the same part far from the origin"]
fn far_from_origin() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 30, "body_name": "C"}));
    run(&mut s, "solid.cylinder", json!({"radius": 3, "height": 40, "base": [0, -20, 15], "axis": "Y", "operation": "cut", "targets": ["C"]}));
    let opts = Options::default();
    let (defs, _, _, _) = scene(&s, &opts);
    let (_, view) = camera(0.6, 0.5);
    let at = |t: Vec3| hlr::hidden_lines(&defs, &[Placement { body: 0, rotation: IDENTITY, translation: t }], &view, &opts, &never).unwrap();
    let base = at(Vec3::ZERO);
    let mut rows = Vec::new();
    for t in [Vec3::new(1e3, 2e3, -3e3), Vec3::new(1e5, 2e5, -3e5), Vec3::new(1e7, 2e7, -3e7)] {
        let d = at(t);
        let shift = view.screen(t);
        let states =
            |d: &Drawing| -> Vec<(Visibility, bool)> { d.lines.iter().flat_map(|l| l.pieces.iter().map(|p| (p.visibility, p.covered))).collect() };
        let same_states = states(&base) == states(&d);
        // Largest move of a piece end once the placement's own shift is removed.
        let mut moved = 0f64;
        for (a, b) in base.lines.iter().flat_map(|l| l.pieces.iter()).zip(d.lines.iter().flat_map(|l| l.pieces.iter())) {
            for (p, q) in [(a.points.first(), b.points.first()), (a.points.last(), b.points.last())] {
                if let (Some(p), Some(q)) = (p, q) {
                    moved = moved.max((q[0] - shift[0] - p[0]).hypot(q[1] - shift[1] - p[1]));
                }
            }
        }
        let row = json!({"translation": [t.x, t.y, t.z], "pieces": [base.stats.pieces, d.stats.pieces], "same_states": same_states, "max_end_move_mm": moved, "uncertain": d.stats.uncertain_uncovered});
        println!("{row}");
        rows.push(row);
    }
    std::fs::write(out_dir().join("far.json"), serde_json::to_string_pretty(&rows).unwrap()).unwrap();
}

#[test]
#[ignore = "investigation: cost with repeated placements of one prepared body"]
fn assembly_cost() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 30, "body_name": "C"}));
    run(&mut s, "solid.cylinder", json!({"radius": 3, "height": 40, "base": [0, -20, 15], "axis": "Y", "operation": "cut", "targets": ["C"]}));
    let world = s.world_state();
    let brep = solidcraft_kernel::portable_body(&world.bodies[0].body).unwrap();
    std::fs::write(out_dir().join("crosshole.brep.json"), serde_json::to_string(&brep).unwrap()).unwrap();
    let opts = Options::default();
    let started = Instant::now();
    let def = hlr::prepare(&brep, &opts).unwrap();
    let prepare_ms = started.elapsed().as_secs_f64() * 1e3;
    let defs = [def];
    let (_, view) = camera(0.6, 0.5);
    let mut rows = Vec::new();
    for n in [1usize, 2, 3, 4, 5, 6] {
        let mut placements = Vec::new();
        for i in 0..n {
            for j in 0..n {
                for k in 0..n {
                    // Spacing 25 mm: neighbours overlap in projection, so they occlude each other.
                    let rot = if (i + j + k) % 2 == 0 { IDENTITY } else { [Vec3::new(0., 1., 0.), Vec3::new(-1., 0., 0.), Vec3::new(0., 0., 1.)] };
                    placements.push(Placement { body: 0, rotation: rot, translation: Vec3::new(25. * i as f64, 25. * j as f64, 35. * k as f64) });
                }
            }
        }
        let started = Instant::now();
        let d = hlr::hidden_lines(&defs, &placements, &view, &opts, &never).unwrap();
        let ms = started.elapsed().as_secs_f64() * 1e3;
        let scene = json!({"view": {"x": [view.x.x, view.x.y, view.x.z], "y": [view.y.x, view.y.y, view.y.z], "d": [view.d.x, view.d.y, view.d.z]},
            "placements": placements.iter().map(|p| json!({"r": p.rotation.iter().map(|v| [v.x, v.y, v.z]).collect::<Vec<_>>(), "t": [p.translation.x, p.translation.y, p.translation.z]})).collect::<Vec<_>>()});
        std::fs::write(out_dir().join(format!("assembly-{}.scene.json", placements.len())), serde_json::to_string(&scene).unwrap()).unwrap();
        if n == 3 {
            std::fs::write(out_dir().join("assembly-27.svg"), svg(&d, None)).unwrap();
        }
        // Cancellation: stop after half the measured time and see how long the return takes.
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_at = std::sync::Arc::new(std::sync::Mutex::new(None::<Instant>));
        let (f2, c2) = (flag.clone(), cancel_at.clone());
        let half = std::time::Duration::from_secs_f64(ms / 2e3);
        let setter = std::thread::spawn(move || {
            std::thread::sleep(half);
            *c2.lock().unwrap() = Some(Instant::now());
            f2.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let cancel = || flag.load(std::sync::atomic::Ordering::Relaxed);
        let r = hlr::hidden_lines(&defs, &placements, &view, &opts, &cancel);
        let returned = Instant::now();
        setter.join().unwrap();
        let latency = cancel_at.lock().unwrap().map(|t| returned.saturating_duration_since(t).as_secs_f64() * 1e3);
        let row = json!({"placements": placements.len(), "ms": ms, "stats": d.stats, "cancelled": r.is_err(), "cancel_latency_ms": latency});
        println!("{row}");
        rows.push(row);
    }
    let out = json!({"prepare_ms": prepare_ms, "report": defs[0].report, "rows": rows});
    std::fs::write(out_dir().join("assembly.json"), serde_json::to_string_pretty(&out).unwrap()).unwrap();
}

fn describe(name: &str, s: &Session) {
    let world = s.world_state();
    for body in &world.bodies {
        let brep = solidcraft_kernel::portable_body(&body.body).unwrap();
        println!("== {name} / {}", body.name);
        for (i, shell) in brep.shells.iter().enumerate() {
            println!(" shell {i}: {} vertices, {} edges, {} faces", shell.vertices.len(), shell.edges.len(), shell.faces.len());
            for (e, edge) in shell.edges.iter().enumerate() {
                let c = serde_json::to_value(&edge.curve).unwrap();
                println!("  e{e} {:?} {}", edge.vertices, c["kind"]);
            }
            for (f, face) in shell.faces.iter().enumerate() {
                let sf = serde_json::to_value(&face.surface).unwrap();
                let loops: Vec<Vec<String>> =
                    face.loops.iter().map(|l| l.iter().map(|u| format!("{}{}", if u.forward { "+" } else { "-" }, u.edge)).collect()).collect();
                println!("  f{f} {} profile={} forward={} loops={loops:?}", sf["kind"], sf["profile"]["kind"], face.forward);
            }
        }
    }
}

/// Prints the classification log of every uncertain piece that is not covered.
fn probe_uncertain(name: &str, s: &Session, yaw: f64, pitch: f64) {
    let opts = Options::default();
    let (defs, placements, _, _) = scene(s, &opts);
    let (_, view) = camera(yaw, pitch);
    let d = hlr::hidden_lines(&defs, &placements, &view, &opts, &never).unwrap();
    for l in &d.lines {
        if matches!(l.source, Source::Edge { edge: 0 | 1, .. }) {
            for p in &l.pieces {
                println!(
                    "   all: {:?} t {:?} {:?} covered {} first {:?} last {:?}",
                    l.source,
                    p.t,
                    p.visibility,
                    p.covered,
                    p.points.first(),
                    p.points.last()
                );
            }
        }
    }
    for l in &d.lines {
        let def = &defs[placements[l.placement].body];
        for p in l.pieces.iter().filter(|p| p.visibility == Visibility::Uncertain && !p.covered) {
            let m = Vec3::new(p.mid[0], p.mid[1], p.mid[2]);
            let own = def.own_faces(&l.source);
            let (v, log) = hlr::debug_classify(&defs, &placements, &view, l.placement, &own, m).unwrap();
            println!("== {name} {:?} own {own:?} t {:?} mid {m:?} -> {v:?}", l.source, p.t);
            for line in log {
                println!("   {line}");
            }
        }
    }
}

#[test]
#[ignore = "investigation probe: uncertain pieces"]
fn probe_rays() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 30, "body_name": "C"}));
    run(&mut s, "solid.cylinder", json!({"radius": 3, "height": 40, "base": [0, -20, 15], "axis": "Y", "operation": "cut", "targets": ["C"]}));
    probe_uncertain("crosshole/side", &s, -std::f64::consts::FRAC_PI_2, 0.);
    probe_uncertain("crosshole/front", &s, 0., 0.);
}

#[test]
#[ignore = "investigation probe: prints the portable records of the fixtures"]
fn probe_portable_records() {
    for (name, cmds) in [
        ("cylinder", vec![("solid.cylinder", json!({"radius": 10, "height": 30}))]),
        ("sphere", vec![("solid.sphere", json!({"radius": 10}))]),
        ("torus", vec![("solid.torus", json!({"major": 20, "minor": 5}))]),
        (
            "fillet",
            vec![("solid.box", json!({"length": 40, "width": 30, "height": 20})), ("solid.fillet", json!({"edges": [[20, 0, 20]], "radius": 5}))],
        ),
    ] {
        let mut s = Session::default();
        for (id, p) in cmds {
            run(&mut s, id, p);
        }
        describe(name, &s);
    }
}
