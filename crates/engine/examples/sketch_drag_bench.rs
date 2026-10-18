//! Engine time for live frames and release, independent of rendering/input scheduling.
use std::{error::Error, io::Write, path::PathBuf, sync::Arc, time::Instant};

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_geom::Vec2;
use solidcraft_sketch::{Analysis, ConstraintKind, Hold, Sketch, profiling::capture, solve_with};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn prepare(size: usize, pinned: bool, cold: bool) -> Result<Session> {
    let mut s = Session::default();
    s.execute("sketch.create", &json!({"plane": "XY"}))?;
    let id = s.active_sketch.ok_or("missing sketch")?;
    let mut sk = Sketch::new();
    let mut points = Vec::new();
    let mut seed = 99u64;
    let mut jitter = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    for y in 0..size {
        for x in 0..size {
            let p = sk.add_point(Vec2::new((x + 1) as f64 * 10.0 + jitter(), (y + 1) as f64 * 10.0 + jitter()), None)?;
            points.push(p);
            if pinned && x == 0 && y == 0 {
                sk.add_constraint(ConstraintKind::Fix { p }, None)?;
            }
            for (other, horizontal) in [((x > 0).then(|| points.len() - 2), true), ((y > 0).then(|| points.len() - 1 - size), false)] {
                if let Some(q) = other.and_then(|i| points.get(i)).copied() {
                    let l = sk.add_line_pts(q, p, None)?;
                    sk.add_constraint(if horizontal { ConstraintKind::Horizontal { l } } else { ConstraintKind::Vertical { l } }, None)?;
                    sk.add_constraint(ConstraintKind::Distance { p, q, value: 10.0 }, None)?;
                }
            }
        }
    }
    if !solve_with(&mut sk, &Hold::default(), None, Analysis::StatusOnly, None).ok() {
        return Err("grid preparation failed".into());
    }
    *Arc::make_mut(&mut s.doc).sketch_mut(id)? = sk;
    if cold {
        s.model = Default::default();
    } else {
        s.refresh();
    }
    Ok(s)
}

fn frame(s: &mut Session) -> Result<Value> {
    let id = s.active_sketch.ok_or("missing sketch")?;
    let sk = s.doc.sketch(id)?;
    let p = sk.points.get(1).ok_or("missing point")?;
    let params = json!({"entity": p.id, "from": [p.pos.x, p.pos.y], "to": [p.pos.x + 0.25, p.pos.y], "live": true});
    Ok(s.execute("sketch.drag", &params)?)
}

fn main() -> Result<()> {
    let mut size = 14usize;
    let mut repeats = 10usize;
    let mut out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(key) = args.next() {
        let value = args.next().ok_or("missing option value")?;
        match key.as_str() {
            "--size" => size = value.parse()?,
            "--repeats" => repeats = value.parse()?,
            "--out" => out = Some(value.into()),
            _ => return Err(format!("unknown option {key}").into()),
        }
    }
    // The floating variant must stay within the solver's 400-variable redundancy cap so
    // that this benchmark can require and compare nonempty published diagnostics.
    if !(2..=14).contains(&size) || !(1..=100).contains(&repeats) {
        return Err("size must be in 2..14 and repeats in 1..100".into());
    }
    let mut cases = Vec::new();
    for (name, pinned, cold, release) in [
        ("live-floating", false, false, false),
        ("live-pinned", true, false, false),
        ("release-floating", false, false, true),
        ("first-frame", false, true, false),
    ] {
        let mut samples = Vec::new();
        for i in 0..repeats + 2 {
            let mut s = prepare(size, pinned, cold)?;
            if release {
                frame(&mut s)?;
            }
            let start = Instant::now();
            let (response, profile) = capture(|| if release { s.execute("sketch.drag_end", &json!({})).map_err(Into::into) } else { frame(&mut s) });
            let elapsed_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let response: Value = response?;
            let id = s.active_sketch.ok_or("missing sketch")?;
            let state = s.world_state();
            let report = &state.sketch(id).ok_or("missing published sketch")?.report;
            if !report.ok() || report.redundant.is_empty() {
                return Err(format!("{name}: missing full diagnostics: {report:?}; response {response}; counters {:?}", profile.counters).into());
            }
            if i >= 2 {
                samples
                    .push(json!({"elapsed_ns": elapsed_ns, "response": response, "profile": profile, "report": report, "sketch": s.doc.sketch(id)?}));
            }
        }
        cases.push(json!({"case": name, "samples": samples}));
    }
    let output = json!({"schema": "solidcraft/sketch-drag-bench/1", "size": size, "seed": 99, "repeats": repeats, "warmup": 2, "cases": cases});
    let mut writer: Box<dyn Write> = match out {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::stdout().lock()),
    };
    serde_json::to_writer_pretty(&mut writer, &output)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}
