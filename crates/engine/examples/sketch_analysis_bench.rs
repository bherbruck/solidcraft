//! Time the actual AutoConstrain command on the solve_profile grid, with no UI or test-only solves.

use std::{error::Error, io::Write, path::PathBuf, sync::Arc, time::Instant};

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_geom::Vec2;
use solidcraft_sketch::{ConstraintKind, Sketch, profiling::capture};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn grid(size: usize) -> Result<Sketch> {
    let mut sk = Sketch::new();
    let mut points = Vec::new();
    let mut seed = 99u64;
    let mut jitter = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    for y in 0..size {
        for x in 0..size {
            let p = sk.add_point(Vec2::new(x as f64 * 10.0 + jitter(), y as f64 * 10.0 + jitter()), None)?;
            points.push(p);
            if x == 0 && y == 0 {
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
    Ok(sk)
}

fn prepare(size: usize) -> Result<Session> {
    let mut s = Session::default();
    s.execute("sketch.create", &json!({"plane": "XY"}))?;
    let id = s.active_sketch.ok_or("missing active sketch")?;
    *Arc::make_mut(&mut s.doc).sketch_mut(id)? = grid(size)?;
    Ok(s)
}

fn run(s: &mut Session) -> Result<Value> {
    Ok(s.execute("sketch.auto_constrain", &json!({"dimensions": false}))?)
}

fn percentile(samples: &[u64], percent: usize) -> u64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted.get((sorted.len() * percent).div_ceil(100).saturating_sub(1)).copied().unwrap_or(0)
}

fn main() -> Result<()> {
    let mut size = 10usize;
    let mut repeats = 10usize;
    let mut warmup = 2usize;
    let mut out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(key) = args.next() {
        if key == "--help" {
            println!("sketch_analysis_bench [--size 2..20] [--repeats 1..100] [--warmup 0..10] [--out FILE]");
            return Ok(());
        }
        let value = args.next().ok_or_else(|| format!("missing value for {key}"))?;
        match key.as_str() {
            "--size" => size = value.parse()?,
            "--repeats" => repeats = value.parse()?,
            "--warmup" => warmup = value.parse()?,
            "--out" => out = Some(value.into()),
            _ => return Err(format!("unknown option {key}").into()),
        }
    }
    if !(2..=20).contains(&size) || !(1..=100).contains(&repeats) || warmup > 10 {
        return Err("size, repeats or warmup outside supported range (see --help)".into());
    }
    for _ in 0..warmup {
        run(&mut prepare(size)?)?;
    }
    let mut samples = Vec::new();
    let mut times = Vec::new();
    let mut reference = None;
    for _ in 0..repeats {
        // Session creation, fixture generation and serialization are outside the timer.
        let mut session = prepare(size)?;
        let start = Instant::now();
        let (response, profile) = capture(|| run(&mut session));
        let elapsed_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let response = response?;
        if response["dof"] != 0 || response["added"] != json!([]) || response["sketch"]["solved"] != true {
            return Err(format!("unexpected AutoConstrain result: {response}").into());
        }
        let id = session.active_sketch.ok_or("missing active sketch")?;
        let final_sketch = session.doc.sketch(id)?;
        if let Some(expected) = &reference {
            if final_sketch != expected {
                return Err("AutoConstrain geometry changed between samples".into());
            }
        } else {
            reference = Some(final_sketch.clone());
        }
        times.push(elapsed_ns);
        let state = session.model.state();
        let published_report = &state.sketch(id).ok_or("missing published sketch")?.report;
        samples.push(json!({"elapsed_ns": elapsed_ns, "response": response, "profile": profile, "published_report": published_report}));
    }
    let output = json!({
        "schema": "solidcraft/sketch-analysis-bench/1", "command": "sketch.auto_constrain",
        "params": {"dimensions": false}, "size": size, "seed": 99, "repeats": repeats, "warmup": warmup,
        "median_ns": percentile(&times, 50), "p95_ns": percentile(&times, 95),
        "final_sketch": reference, "samples": samples,
    });
    let mut writer: Box<dyn Write> = match out {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::stdout().lock()),
    };
    serde_json::to_writer_pretty(&mut writer, &output)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}
