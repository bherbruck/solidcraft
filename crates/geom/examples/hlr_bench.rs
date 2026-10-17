//! Hidden-line investigation timing (#2105): `hlr_bench <body.brep.json> <scene.json>...`.
//! Builds natively and for `wasm32-wasip1`, so the same scenes can be timed in both.

use std::time::Instant;

use serde_json::Value;
use solidcraft_geom::Vec3;
use solidcraft_geom::brep::Brep;
use solidcraft_geom::hlr::{self, Options, Placement, View};

fn v(x: &Value) -> Option<Vec3> {
    Some(Vec3::new(x.get(0)?.as_f64()?, x.get(1)?.as_f64()?, x.get(2)?.as_f64()?))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // HLR_STACK (bytes) runs the work on a thread with that stack, to measure stack needs.
    match std::env::var("HLR_STACK").ok().and_then(|v| v.parse::<usize>().ok()) {
        Some(size) => std::thread::Builder::new()
            .stack_size(size)
            .spawn(|| run().map_err(|e| e.to_string()))?
            .join()
            .map_err(|_| "worker panicked")?
            .map_err(Into::into),
        None => run(),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (body, scenes) = args.split_first().ok_or("usage: hlr_bench body.json scene.json...")?;
    let brep: Brep = serde_json::from_str(&std::fs::read_to_string(body)?)?;
    let opts = Options::default();
    let started = Instant::now();
    let def = hlr::prepare(&brep, &opts)?;
    let prepare_ms = started.elapsed().as_secs_f64() * 1e3;
    let defs = [def];
    for path in scenes {
        let scene: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        let view = &scene["view"];
        let view = View { x: v(&view["x"]).ok_or("view")?, y: v(&view["y"]).ok_or("view")?, d: v(&view["d"]).ok_or("view")? };
        let mut placements = Vec::new();
        for p in scene["placements"].as_array().ok_or("placements")? {
            let r = &p["r"];
            placements.push(Placement {
                body: 0,
                rotation: [v(&r[0]).ok_or("r")?, v(&r[1]).ok_or("r")?, v(&r[2]).ok_or("r")?],
                translation: v(&p["t"]).ok_or("t")?,
            });
        }
        let started = Instant::now();
        let d = hlr::hidden_lines(&defs, &placements, &view, &opts, &|| false)?;
        let ms = started.elapsed().as_secs_f64() * 1e3;
        println!(
            "{{\"scene\":\"{path}\",\"placements\":{},\"prepare_ms\":{prepare_ms:.1},\"ms\":{ms:.1},\"pieces\":{},\"rays\":{}}}",
            placements.len(),
            d.stats.pieces,
            d.stats.rays
        );
    }
    Ok(())
}
