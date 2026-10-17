use super::*;
use crate::{Body, BoolOp, ExportBody, StepHeader, boolean, box_solid, cylinder, fillet, iges_export_bodies, measure, sphere, torus};
use solidcraft_geom::Vec3;

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-9)
}

/// Export a body to IGES and read it back: same volume and area, its name and colour.
fn round_trip(b: &Body) -> crate::StepImport {
    let body = ExportBody { name: "Bracket left 1".into(), body: b, color: Some([1.0, 0.5, 0.0]) };
    let text = iges_export_bodies(&[body], &StepHeader { file_name: "t.igs".into(), ..Default::default() }).unwrap();
    for l in text.lines() {
        assert_eq!(l.chars().count(), 80, "{l}");
    }
    let imp = iges_import(&text).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    assert_eq!(imp.bodies.len(), 1);
    let (m0, m1) = (measure(b).unwrap(), measure(&imp.bodies[0].body).unwrap());
    assert!(rel(m1.volume, m0.volume) < 1e-3, "volume {} vs {}", m1.volume, m0.volume);
    assert!(rel(m1.area, m0.area) < 1e-3, "area {} vs {}", m1.area, m0.area);
    assert!(imp.bodies[0].closed);
    assert_eq!(imp.bodies[0].name, "Bracket left 1");
    assert_eq!(imp.bodies[0].color, Some([1.0, 0.5, 0.0]));
    imp
}

#[test]
fn primitives_round_trip_through_iges() {
    round_trip(&box_solid(Vec3::ZERO, Vec3::new(10.0, 20.0, 30.0)).unwrap());
    round_trip(&cylinder(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, 1.0, 1.0), 5.0, 12.0).unwrap());
    round_trip(&sphere(Vec3::new(1.0, 2.0, 3.0), 10.0).unwrap());
    round_trip(&torus(Vec3::ZERO, 20.0, 4.0).unwrap());
}

#[test]
fn features_round_trip_through_iges() {
    let plate = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
    let pin = cylinder(Vec3::new(20.0, 15.0, -1.0), Vec3::Z, 5.0, 12.0).unwrap();
    let holed = boolean(&plate, &pin, BoolOp::Cut).unwrap().unwrap();
    round_trip(&holed);
    round_trip(&fillet(&plate, &[Vec3::new(20.0, 0.0, 10.0)], 3.0).unwrap());
}

/// Hostile files fail cleanly: garbage, truncation, missing and circular references, huge
/// counts.
#[test]
fn hostile_iges_never_panics() {
    let good = iges_export_bodies(
        &[ExportBody { name: "B".into(), body: &box_solid(Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0)).unwrap(), color: None }],
        &StepHeader::default(),
    )
    .unwrap();
    let mut cases: Vec<String> = vec![String::new(), "garbage".into(), "\u{0}\u{ff}".into(), good[..good.len() / 2].to_string()];
    // Every parameter line with its numbers bumped, scrambled or blown up.
    for (k, f) in [|s: &str| s.replace('1', "9"), |s: &str| s.replace(',', ";"), |s: &str| s.replace("0,", "999999999,")].iter().enumerate() {
        let t: String = good
            .lines()
            .map(|l| if l.as_bytes().get(72) == Some(&b'P') && l.len() >= 64 { format!("{}{}", f(&l[..64]), &l[64..]) } else { l.to_string() })
            .collect::<Vec<_>>()
            .join("\n");
        let _ = k;
        cases.push(t);
    }
    // A transformation and a composite curve that refer to themselves.
    cases.push(
        [
            format!("{:<72}S{:>7}", "", 1),
            format!("{:<72}G{:>7}", "1H,,1H;;", 1),
            format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", 124, 1, 0, 0, 0, 0, 1, 0, "00000000", 1),
            format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", 124, 0, 0, 1, 0, 0, 0, "", 0, 2),
            format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", 102, 2, 0, 0, 0, 0, 1, 0, "00000000", 3),
            format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", 102, 0, 0, 1, 0, 0, 0, "", 0, 4),
            format!("{:<64}{:>8}P{:>7}", "124,1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.,0.;", 1, 1),
            format!("{:<64}{:>8}P{:>7}", "102,1,3;", 3, 2),
            format!("{:<72}T{:>7}", "S      1G      1D      4P      2", 1),
        ]
        .join("\n"),
    );
    for c in cases {
        let r = std::panic::catch_unwind(|| iges_import(&c).map(|i| i.bodies.len()));
        assert!(r.is_ok(), "panicked on a hostile file");
    }
}

/// An IGES file from (type, form, parameters) entities, in order (pointers are 2k + 1).
fn iges_file(ents: &[(i64, i64, String)]) -> String {
    let mut out = vec![format!("{:<72}S{:>7}", "test", 1), format!("{:<72}G{:>7}", "1H,,1H;,,,,,,,,,,,1.,2,2HMM,,,,1.E-6;", 1)];
    let mut p_lines: Vec<(usize, String)> = Vec::new();
    let mut starts = Vec::new();
    for (k, (t, _, p)) in ents.iter().enumerate() {
        starts.push(p_lines.len() + 1);
        let text = format!("{t},{p};");
        let chars: Vec<char> = text.chars().collect();
        for c in chars.chunks(64) {
            p_lines.push((2 * k + 1, c.iter().collect()));
        }
    }
    for (k, (t, form, _)) in ents.iter().enumerate() {
        let count = p_lines.iter().filter(|(d, _)| *d == 2 * k + 1).count();
        out.push(format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", t, starts[k], 0, 0, 0, 0, 0, 0, "00000000", 2 * k + 1));
        out.push(format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", t, 0, 0, count, form, 0, 0, "", 0, 2 * k + 2));
    }
    for (i, (d, l)) in p_lines.iter().enumerate() {
        out.push(format!("{l:<64}{d:>8}P{:>7}", i + 1));
    }
    out.push(format!("{:<72}T{:>7}", "", 1));
    out.join("\n")
}

/// An exported box inside a named group and nested subfigures, placed twice. The second
/// placement composes its translation and scale with a directory transformation.
fn assembly_file(form: i64, scale: f64) -> String {
    let body = box_solid(Vec3::ZERO, Vec3::new(2.0, 3.0, 4.0)).unwrap();
    let text = iges_export_bodies(&[ExportBody { name: "Body1".into(), body: &body, color: None }], &StepHeader::default()).unwrap();
    let f = parse(&text).unwrap();
    let mut ids: Vec<_> = f.des.keys().copied().collect();
    ids.sort_unstable();
    let solid = *ids.iter().find(|&&s| f.des[&s].typ == 186).unwrap();
    let mut transforms: HashMap<_, _> = f.des.iter().map(|(seq, d)| (*seq, d.xform)).collect();
    let mut ents: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(k, seq)| {
            assert_eq!(*seq, 2 * k + 1);
            let d = &f.des[seq];
            let p = f.params[seq]
                .iter()
                .skip(1)
                .map(|s| if s.is_empty() || num(s).is_some() { s.clone() } else { format!("{}H{s}", s.chars().count()) })
                .collect::<Vec<_>>()
                .join(",");
            (d.typ, d.form, p)
        })
        .collect();
    let definition = 2 * ents.len() + 1;
    ents.push((308, 0, format!("0,8HLead rod,1,{solid}")));
    let first = 2 * ents.len() + 1;
    ents.push((408, 0, format!("{definition},10.,0.,0.,1.")));
    let rotation = 2 * ents.len() + 1;
    ents.push((124, 0, "0.,-1.,0.,100.,1.,0.,0.,0.,0.,0.,1.,0.".into()));
    let second = 2 * ents.len() + 1;
    ents.push((408, 0, format!("{definition},0.,20.,0.,{scale}")));
    transforms.insert(second, rotation as i64);
    let frame = 2 * ents.len() + 1;
    ents.push((308, 0, format!("1,5HFrame,2,{first},{second}")));
    let property = 2 * ents.len() + 1;
    ents.push((406, 15, "1,6HMotion".into()));
    ents.push((402, form, format!("1,{frame},0,1,{property}")));
    iges_file(&ents)
        .lines()
        .map(|line| {
            if line.as_bytes()[72] != b'D' {
                return line.to_string();
            }
            let seq: usize = line[73..80].trim().parse().unwrap();
            match transforms.get(&seq) {
                Some(xf) => format!("{}{:>8}{}", &line[..48], xf, &line[56..]),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn groups_and_subfigure_instances_keep_names_structure_and_placements() {
    for (form, scale) in [(1, 1.0), (7, 2.0), (14, 1.0), (15, 2.0)] {
        let imp = iges_import(&assembly_file(form, scale)).unwrap();
        assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
        assert_eq!(imp.bodies.len(), 2);
        assert_eq!(imp.tree.len(), 1);
        let motion = &imp.tree[0].children[0];
        assert_eq!(motion.name, "Motion");
        let frame = &motion.children[0];
        assert_eq!(frame.name, "Frame");
        assert_eq!(frame.children.len(), 2);
        assert!(frame.children.iter().all(|n| n.name == "Lead rod"));
        assert!(imp.bodies.iter().all(|b| b.path == ["Import", "Motion", "Frame", "Lead rod"]));
        assert_eq!(imp.bodies[0].source_item, imp.bodies[1].source_item);
        let a = measure(&imp.bodies[0].body).unwrap();
        let b = measure(&imp.bodies[1].body).unwrap();
        assert!(rel(a.volume, 24.0) < 1e-6 && rel(b.volume, 24.0 * scale.powi(3)) < 1e-6);
        assert!(a.bbox.min.dist(Vec3::new(10.0, 0.0, 0.0)) < 1e-6 && a.bbox.max.dist(Vec3::new(12.0, 3.0, 4.0)) < 1e-6);
        assert!(b.bbox.min.dist(Vec3::new(80.0 - 3.0 * scale, 0.0, 0.0)) < 1e-6);
        assert!(b.bbox.max.dist(Vec3::new(80.0, 2.0 * scale, 4.0 * scale)) < 1e-6);
    }
}

#[test]
fn invalid_assembly_references_and_counts_fail_before_geometry() {
    for (params, message) in [
        ("0,4HLoop,1,1".into(), "cyclic"),
        ("0,4HPart,1,999".into(), "missing entity"),
        ("0,4HPart,-1".into(), "negative IGES group size"),
        (format!("0,4HPart,{}", MAX_LIST + 1), "work limit"),
    ] {
        let error = iges_import(&iges_file(&[(308, 0, params)])).unwrap_err();
        assert!(matches!(&error, crate::KernelError::Invalid(text) if text.contains(message)), "{error}");
    }
    let mut chain = Vec::new();
    for k in 0..=MAX_DEPTH {
        chain.push((308, 0, if k == MAX_DEPTH { "0,4HPart,0".into() } else { format!("0,4HPart,1,{}", 2 * k + 3) }));
    }
    let error = iges_import(&iges_file(&chain)).unwrap_err();
    assert!(matches!(&error, crate::KernelError::Invalid(text) if text.contains("nesting is too deep")), "{error}");
    let members = "1,".repeat(100_000);
    let many = [(308, 0, "0,4HPart,0".into()), (308, 0, format!("1,5HFrame,100001,{members}1"))];
    let error = iges_import(&iges_file(&many)).unwrap_err();
    assert!(matches!(&error, crate::KernelError::Invalid(text) if text.contains("too many IGES assembly placements")), "{error}");
}

/// Six trimmed bilinear patches with line boundaries, forming a 10 × 20 × 30 box.
fn trimmed_box(ents: &mut Vec<(i64, i64, String)>, x: f64) -> Vec<usize> {
    let c = |i: usize| -> [f64; 3] {
        [x + if i & 1 != 0 { 10.0 } else { 0.0 }, if i & 2 != 0 { 20.0 } else { 0.0 }, if i & 4 != 0 { 30.0 } else { 0.0 }]
    };
    // Faces as corner quads (counter-clockwise seen from outside).
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut faces = Vec::new();
    let fmt = |p: [f64; 3]| format!("{:?},{:?},{:?}", p[0], p[1], p[2]);
    for q in quads {
        let (a, b, cc, d) = (c(q[0]), c(q[1]), c(q[2]), c(q[3]));
        // Patch: u from a to b, v from a to d (normal (b - a) × (d - a) outward).
        let surf = format!("1,1,1,1,0,0,1,0,0,0.,0.,1.,1.,0.,0.,1.,1.,1.,1.,1.,1.,{},{},{},{},0.,1.,0.,1.", fmt(a), fmt(b), fmt(d), fmt(cc));
        ents.push((128, 0, surf));
        let s_de = 2 * ents.len() - 1;
        let mut lines = Vec::new();
        for (p0, p1) in [(a, b), (b, cc), (cc, d), (d, a)] {
            ents.push((110, 0, format!("{},{}", fmt(p0), fmt(p1))));
            lines.push(2 * ents.len() - 1);
        }
        ents.push((102, 0, format!("4,{}", lines.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))));
        let comp = 2 * ents.len() - 1;
        ents.push((142, 0, format!("0,{s_de},0,{comp},2")));
        let cos = 2 * ents.len() - 1;
        ents.push((144, 0, format!("{s_de},1,0,{cos}")));
        faces.push(2 * ents.len() - 1);
    }
    faces
}

#[test]
fn trimmed_surfaces_sew_into_a_solid() {
    let mut ents = Vec::new();
    trimmed_box(&mut ents, 0.0);
    let imp = iges_import(&iges_file(&ents)).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    assert!(imp.bodies[0].closed);
    let m = measure(&imp.bodies[0].body).unwrap();
    assert!(rel(m.volume, 6000.0) < 1e-6 && rel(m.area, 2200.0) < 1e-6, "{} {}", m.volume, m.area);
}

#[test]
fn surfaces_in_separate_groups_remain_separate_parts() {
    let mut ents = Vec::new();
    for (name, x) in [("Left", 0.0), ("Right", 100.0)] {
        let faces = trimmed_box(&mut ents, x);
        let members = faces.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
        ents.push((308, 0, format!("0,{}H{name},{},{members}", name.len(), faces.len())));
    }
    let imp = iges_import(&iges_file(&ents)).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    assert_eq!(imp.bodies.len(), 2);
    assert_eq!(imp.tree[0].children.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Left", "Right"]);
    for (body, x) in imp.bodies.iter().zip([0.0, 100.0]) {
        let m = measure(&body.body).unwrap();
        assert!(body.closed && rel(m.volume, 6000.0) < 1e-6 && rel(m.area, 2200.0) < 1e-6);
        assert!(m.bbox.min.dist(Vec3::new(x, 0.0, 0.0)) < 1e-6 && m.bbox.max.dist(Vec3::new(x + 10.0, 20.0, 30.0)) < 1e-6);
    }
}

#[test]
fn a_drill_point_round_trips_through_iges() {
    // A blind hole with a cone tip: the tip's face closes at its apex (its seam runs out to the
    // apex and straight back).
    use solidcraft_geom::{Loop2, Plane, Region2, Vec2};
    let plate = box_solid(Vec3::new(-20.0, -15.0, 0.0), Vec3::new(20.0, 15.0, 10.0)).unwrap();
    let profile =
        Region2 { outer: Loop2::polygon(&[Vec2::new(0.0, 2.0), Vec2::new(4.0, 5.0), Vec2::new(4.0, 11.0), Vec2::new(0.0, 11.0)]), holes: vec![] };
    let plane = Plane { y: Vec3::Z, ..Plane::XZ }; // Positive profile heights are world +Z.
    let hole = crate::revolve(&plane, &[profile], Vec2::ZERO, Vec2::new(0.0, 1.0), std::f64::consts::TAU).unwrap().pop().unwrap();
    let drilled = boolean(&plate, &hole, BoolOp::Cut).unwrap().unwrap();
    let v = 12000.0 - std::f64::consts::PI * 16.0 * (5.0 + 1.0);
    assert!(rel(measure(&drilled).unwrap().volume, v) < 1e-3);
    round_trip(&drilled);
}

/// A composite curve (102) names its children by pointer, and nothing stops a tree whose every
/// node names the same child twice. Twenty levels under the depth cap is a million leaf visits
/// and thirty is 2^30; only counting the work sees that, because the depth never grows past the
/// cap and a node that yields no segment trips no bound on the output.
#[test]
fn composite_curve_fan_out_is_bounded() {
    let quad = |z: f64| {
        let c = [[0.0, 0.0, z], [10.0, 0.0, z], [10.0, 20.0, z], [0.0, 20.0, z]];
        let fmt = |p: [f64; 3]| format!("{:?},{:?},{:?}", p[0], p[1], p[2]);
        format!("1,1,1,1,0,0,1,0,0,0.,0.,1.,1.,0.,0.,1.,1.,1.,1.,1.,1.,{},{},{},{},0.,1.,0.,1.", fmt(c[0]), fmt(c[1]), fmt(c[3]), fmt(c[2]))
    };
    let mut ents: Vec<(i64, i64, String)> = vec![(128, 0, quad(0.0)), (110, 0, "0.,0.,0.,10.,0.,0.".into())];
    let (surface, mut child) = (1, 3);
    // Twenty composites, each naming the one below it twice.
    for _ in 0..20 {
        ents.push((102, 0, format!("2,{child},{child}")));
        child = 2 * ents.len() - 1;
    }
    ents.push((142, 0, format!("0,{surface},0,{child},2")));
    let cos = 2 * ents.len() - 1;
    ents.push((144, 0, format!("{surface},1,0,{cos}")));

    let reported = match iges_to_step(&iges_file(&ents)) {
        Ok((_, warnings)) => warnings.join(" "),
        Err(error) => error,
    };
    // Check the work-budget rejection, not elapsed time: CI runs test binaries concurrently.
    assert!(reported.contains("too complex") || reported.contains("too many segments"), "{reported}");
}

/// Nothing stops directory entries from all naming the same parameter range, and each one
/// materialises it as its own string and token list. Ten thousand entries over a million
/// shared lines retained about a gigabyte each.
#[test]
fn directory_entries_cannot_share_their_way_past_the_file_size() {
    let mut out = vec![format!("{:<72}S{:>7}", "test", 1), format!("{:<72}G{:>7}", "1H,,1H;,,,,,,,,,,,1.,2,2HMM,,,,1.E-6;", 1)];
    // 1,000 entries, each claiming the same 2,000 parameter lines.
    for k in 0..1_000 {
        out.push(format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", 110, 1, 0, 0, 0, 0, 0, 0, "00000000", 2 * k + 1));
        out.push(format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}D{:>7}", 110, 0, 0, 2_000, 0, 0, 0, "", 0, 2 * k + 2));
    }
    for i in 0..2_000 {
        out.push(format!("{:<64}{:>8}P{:>7}", "0.,0.,0.,1.,0.,0.;", 1, i + 1));
    }
    out.push(format!("{:<72}T{:>7}", "", 1));

    let t = std::time::Instant::now();
    let error = iges_to_step(&out.join("\n")).unwrap_err();
    assert!(error.contains("parameter data is too large"), "{error}");
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "took {:?}", t.elapsed());
}
