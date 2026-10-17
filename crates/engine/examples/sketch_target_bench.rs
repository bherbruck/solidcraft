//! Matched command timings and analytic cursor-distance error for the reported four-bar.
mod support {
    pub mod four_bar;
}
use serde_json::json;
use solidcraft_geom::Vec2;
use solidcraft_sketch::profiling::capture;
use std::{io::Write, time::Instant};
use support::four_bar::{self, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: sketch_target_bench OUTPUT.json [REPEATS]")?;
    let repeats: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(10);
    if !(1..=100).contains(&repeats) {
        return Err("repeats must be in 1..100".into());
    }
    let mut cases = Vec::new();
    for target in [Vec2::new(-10.0, 8.0), Vec2::new(-10.0, -8.0), Vec2::new(20.0, 20.0), Vec2::new(20.0, 0.0), Vec2::new(-8.0, 6.0)] {
        for events in [1, 16] {
            let mut samples = Vec::new();
            for i in 0..repeats + 2 {
                let mut s = four_bar::session()?;
                let from = four_bar::point(&s, "crank.end")?;
                let side = four_bar::branch(&s)?;
                let start = Instant::now();
                let (result, profile) = capture(|| -> Result<()> {
                    for k in 1..=events {
                        let q = from + (target - from) * (k as f64 / events as f64);
                        let at = four_bar::point(&s, "crank.end")?;
                        s.execute("sketch.drag", &json!({"entity":"crank.end","from":[at.x,at.y],"to":[q.x,q.y],"live":true}))?;
                    }
                    Ok(())
                });
                let elapsed_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                result?;
                let actual = four_bar::point(&s, "crank.end")?;
                let error = four_bar::length_error(&s)?;
                if error > 1e-6 || four_bar::branch(&s)? != side {
                    return Err("length or branch violation".into());
                }
                if i >= 2 {
                    samples.push(json!({"elapsed_ns":elapsed_ns,"actual":[actual.x,actual.y],"length_error":error,"cursor_distance":actual.dist(target),"nearest_error":actual.dist(target*(10.0/target.len())),"profile":profile}));
                }
            }
            cases.push(json!({"target":[target.x,target.y],"events":events,"samples":samples}));
        }
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    serde_json::to_writer_pretty(&mut out, &json!({"schema":"solidcraft/sketch-target-bench/1","repeats":repeats,"warmup":2,"cases":cases}))?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
