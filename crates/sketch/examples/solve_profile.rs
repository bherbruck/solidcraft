//! Reproducible numerical traces and paired wall-time samples. Use `--help` for cases and bounds.

use std::{error::Error, io::Write, path::PathBuf, time::Instant};

use serde::Serialize;
use solidcraft_geom::Vec2;
use solidcraft_sketch::{
    Analysis, ConstraintKind, DrivingValues, Hold, Sketch, SolveReport,
    profiling::{Counters, SolveProfile, Timings, capture},
    solve_after_value_edit, solve_with,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const CASES: [&str; 6] = ["distance-zero", "distance-offset", "x-zero", "linkage", "linkage-edit", "grid"];

struct Options {
    case: String,
    repeats: usize,
    warmup: usize,
    size: usize,
    seed: u64,
    scale: f64,
    offset: f64,
    analysis: Analysis,
    cpu_loops: Option<usize>,
    out: Option<PathBuf>,
}

impl Options {
    fn read() -> Result<Option<Self>> {
        let mut opt = Self {
            case: "all".into(),
            repeats: 5,
            warmup: 1,
            size: 10,
            seed: 99,
            scale: 1.0,
            offset: 0.0,
            analysis: Analysis::Full,
            cpu_loops: None,
            out: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(key) = args.next() {
            if key == "--help" {
                println!(
                    "solve_profile [--case all|{}] [--repeats 1..100] [--warmup 0..10] [--size 2..30] [--seed U64] [--scale 1e-6..1e6] [--offset MM] [--analysis full|freedom|status] [--cpu-loops 1..1000000] [--out FILE]",
                    CASES.join("|")
                );
                return Ok(None);
            }
            let value = args.next().ok_or_else(|| format!("missing value for {key}"))?;
            match key.as_str() {
                "--case" => opt.case = value,
                "--repeats" => opt.repeats = value.parse()?,
                "--warmup" => opt.warmup = value.parse()?,
                "--size" => opt.size = value.parse()?,
                "--seed" => opt.seed = value.parse()?,
                "--cpu-loops" => opt.cpu_loops = Some(value.parse()?),
                "--scale" => opt.scale = value.parse()?,
                "--offset" => opt.offset = value.parse()?,
                "--analysis" => match value.as_str() {
                    "full" => opt.analysis = Analysis::Full,
                    "freedom" => opt.analysis = Analysis::Freedom,
                    "status" => opt.analysis = Analysis::StatusOnly,
                    _ => return Err("analysis must be full, freedom or status".into()),
                },
                "--out" => opt.out = Some(value.into()),
                _ => return Err(format!("unknown option {key}").into()),
            }
        }
        if !(1..=100).contains(&opt.repeats) || opt.warmup > 10 || !(2..=30).contains(&opt.size) {
            return Err("repeats, warmup or size outside supported range (see --help)".into());
        }
        if opt.cpu_loops.is_some_and(|n| !(1..=1_000_000).contains(&n)) {
            return Err("cpu-loops must be in 1..1000000".into());
        }
        if !opt.scale.is_finite() || !(1e-6..=1e6).contains(&opt.scale) || !opt.offset.is_finite() || opt.offset.abs() > 1e8 {
            return Err("scale must be finite in 1e-6..1e6 and offset finite within +/-1e8 mm".into());
        }
        if opt.case != "all" && !CASES.contains(&opt.case.as_str()) {
            return Err(format!("unknown case {}", opt.case).into());
        }
        if opt.analysis != Analysis::Full && (opt.case == "all" || opt.case == "linkage-edit") {
            return Err("linkage-edit uses full analysis; select another --case for reduced analysis".into());
        }
        Ok(Some(opt))
    }

    fn at(&self, x: f64, y: f64) -> Vec2 {
        Vec2::new(self.offset + x * self.scale, self.offset + y * self.scale)
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

struct Case {
    sketch: Sketch,
    old: Option<DrivingValues>,
}

fn make_case(name: &str, opt: &Options) -> Result<Case> {
    let mut sk = Sketch::new();
    let mut old = None;
    let mut rnd = Lcg(opt.seed);
    if name == "grid" {
        let mut points = Vec::new();
        for y in 0..opt.size {
            for x in 0..opt.size {
                let p = sk.add_point(opt.at(x as f64 * 10.0 + rnd.next(), y as f64 * 10.0 + rnd.next()), None)?;
                points.push(p);
                if x == 0 && y == 0 {
                    sk.add_constraint(ConstraintKind::Fix { p }, None)?;
                }
                for (other, horizontal) in [((x > 0).then(|| points.len() - 2), true), ((y > 0).then(|| points.len() - 1 - opt.size), false)] {
                    if let Some(q) = other.and_then(|i| points.get(i)).copied() {
                        let l = sk.add_line_pts(q, p, None)?;
                        sk.add_constraint(if horizontal { ConstraintKind::Horizontal { l } } else { ConstraintKind::Vertical { l } }, None)?;
                        sk.add_constraint(ConstraintKind::Distance { p, q, value: 10.0 * opt.scale }, None)?;
                    }
                }
            }
        }
    } else if name.starts_with("linkage") {
        let truth = [(0.0, 0.0), (40.0, 0.0), (18.0, 30.0), (8.0, 55.0), (52.0, 48.0), (30.0, 80.0)];
        let mut ids = Vec::new();
        for (i, (x, y)) in truth.iter().copied().enumerate() {
            let jitter = if i == 0 { (0.0, 0.0) } else { (rnd.next() * 2.0, rnd.next() * 2.0) };
            ids.push(sk.add_point(opt.at(x + jitter.0, y + jitter.1), None)?);
        }
        let p = *ids.first().ok_or("missing base point")?;
        let q = *ids.get(1).ok_or("missing base endpoint")?;
        sk.add_constraint(ConstraintKind::Fix { p }, None)?;
        let l = sk.add_line_pts(p, q, None)?;
        sk.add_constraint(ConstraintKind::Horizontal { l }, None)?;
        for (a, b) in [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (0, 3), (1, 4), (2, 5)] {
            let (ax, ay) = *truth.get(a).ok_or("missing endpoint")?;
            let (bx, by) = *truth.get(b).ok_or("missing endpoint")?;
            let p = *ids.get(a).ok_or("missing point")?;
            let q = *ids.get(b).ok_or("missing point")?;
            sk.add_constraint(ConstraintKind::Distance { p, q, value: (ax - bx).hypot(ay - by) * opt.scale }, None)?;
        }
        if name == "linkage-edit" {
            let report = solve_with(&mut sk, &Hold::default(), None, Analysis::Full, None);
            if !report.ok() {
                return Err("could not prepare the solved linkage for the dimension edit".into());
            }
            old = Some(sk.driving_values());
            for c in &mut sk.constraints {
                if let Some(value) = c.kind.value() {
                    c.kind.set_value(value * (1.0 + rnd.next() * 0.15));
                }
            }
        }
    } else {
        let p = sk.add_point(opt.at(0.0, 0.0), None)?;
        let q = sk.add_point(opt.at(if name == "distance-offset" { 1e-5 } else { 0.0 }, if name == "x-zero" { 5.0 } else { 0.0 }), None)?;
        sk.add_constraint(ConstraintKind::Fix { p }, None)?;
        let k = if name == "x-zero" {
            ConstraintKind::DistanceX { p, q, value: 10.0 * opt.scale }
        } else {
            ConstraintKind::Distance { p, q, value: 10.0 * opt.scale }
        };
        sk.add_constraint(k, None)?;
    }
    Ok(Case { sketch: sk, old })
}

fn solve(case: &Case, sk: &mut Sketch, opt: &Options) -> SolveReport {
    match &case.old {
        Some(old) => solve_after_value_edit(sk, old),
        None => solve_with(sk, &Hold::default(), None, opt.analysis, None),
    }
}

#[derive(Serialize)]
struct Sample {
    elapsed_ns: u64,
    report: SolveReport,
    counters: Counters,
    timings_ns: Timings,
}

#[derive(Serialize)]
struct CaseResult {
    case: String,
    points: usize,
    constraints: usize,
    profiled_median_ns: u64,
    profiled_p95_ns: u64,
    uncaptured_median_ns: u64,
    uncaptured_p95_ns: u64,
    uncaptured_samples_ns: Vec<u64>,
    samples: Vec<Sample>,
    last_run_trace: SolveProfile,
}

fn elapsed(t: Instant) -> u64 {
    t.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

fn percentile(samples: &[u64], percent: usize) -> u64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted.get((sorted.len() * percent).div_ceil(100).saturating_sub(1)).copied().unwrap_or(0)
}

fn measure(name: &str, opt: &Options) -> Result<CaseResult> {
    let case = make_case(name, opt)?;
    for _ in 0..opt.warmup {
        solve(&case, &mut case.sketch.clone(), opt);
    }
    let mut samples = Vec::new();
    let mut baseline = Vec::new();
    let mut last = SolveProfile::default();
    for i in 0..opt.repeats {
        // Clones and comparison are outside both timers; alternate ordering to reduce drift.
        let mut measured = case.sketch.clone();
        let mut plain = case.sketch.clone();
        let unprofiled = |sk: &mut Sketch| {
            let t = Instant::now();
            let r = solve(&case, sk, opt);
            (r, elapsed(t))
        };
        let profiled = |sk: &mut Sketch| {
            let t = Instant::now();
            let (r, p) = capture(|| solve(&case, sk, opt));
            (r, p, elapsed(t))
        };
        let ((a, plain_ns), (b, profile, ns)) = if i % 2 == 0 {
            let a = unprofiled(&mut plain);
            (a, profiled(&mut measured))
        } else {
            let b = profiled(&mut measured);
            (unprofiled(&mut plain), b)
        };
        if measured != plain || serde_json::to_value(&a)? != serde_json::to_value(&b)? {
            return Err(format!("instrumentation changed the result for {name}").into());
        }
        baseline.push(plain_ns);
        samples.push(Sample { elapsed_ns: ns, report: b, counters: profile.counters.clone(), timings_ns: profile.timings_ns.clone() });
        last = profile;
    }
    let times: Vec<_> = samples.iter().map(|s| s.elapsed_ns).collect();
    Ok(CaseResult {
        case: name.into(),
        points: case.sketch.points.len(),
        constraints: case.sketch.constraints.len(),
        profiled_median_ns: percentile(&times, 50),
        profiled_p95_ns: percentile(&times, 95),
        uncaptured_median_ns: percentile(&baseline, 50),
        uncaptured_p95_ns: percentile(&baseline, 95),
        uncaptured_samples_ns: baseline,
        samples,
        last_run_trace: last,
    })
}

fn main() -> Result<()> {
    let Some(opt) = Options::read()? else { return Ok(()) };
    let names = if opt.case == "all" { CASES.to_vec() } else { vec![opt.case.as_str()] };
    let cases: Vec<serde_json::Value> = names
        .iter()
        .map(|name| {
            if let Some(loops) = opt.cpu_loops {
                // Sampling profilers see a sustained workload without trace allocation, rho
                // calculation or JSON serialization in the hot loop. Each solve starts fresh.
                let case = make_case(name, &opt)?;
                for _ in 0..opt.warmup {
                    solve(&case, &mut case.sketch.clone(), &opt);
                }
                let (mut solved, mut failed) = (0usize, 0usize);
                let t = Instant::now();
                for _ in 0..loops {
                    let report = std::hint::black_box(solve(&case, &mut case.sketch.clone(), &opt));
                    if report.ok() { solved += 1 } else { failed += 1 }
                }
                Ok(serde_json::json!({ "case": name, "loops": loops, "solved": solved, "failed": failed, "elapsed_ns": elapsed(t) }))
            } else {
                Ok(serde_json::to_value(measure(name, &opt)?)?)
            }
        })
        .collect::<Result<_>>()?;
    let output = serde_json::json!({
        "schema": "solidcraft/sketch-profile/1", "package_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "debug_assertions": cfg!(debug_assertions),
        "seed": opt.seed, "scale": opt.scale, "offset_mm": opt.offset, "size": opt.size,
        "repeats": opt.repeats, "warmup": opt.warmup, "analysis": match opt.analysis { Analysis::Full => "full", Analysis::Freedom => "freedom", Analysis::StatusOnly => "status" },
        "mode": if opt.cpu_loops.is_some() { "cpu" } else { "trace" },
        "cases": cases,
    });
    let mut writer: Box<dyn Write> = match opt.out {
        Some(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(path)?)),
        None => Box::new(std::io::stdout().lock()),
    };
    serde_json::to_writer_pretty(&mut writer, &output)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}
