//! Public-safe 10/40/20/42 mm linkage, matching the reported one-degree-of-freedom sketch.
use serde_json::json;
use solidcraft_engine::Session;
use solidcraft_geom::Vec2;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn session() -> Result<Session> {
    let mut s = Session::default();
    s.execute("sketch.create", &json!({"plane":"XZ"}))?;
    let x = -10.0 + (1600.0 - 400.0 + 52.0 * 52.0) / 104.0;
    let y = -(1600.0_f64 - (x + 10.0) * (x + 10.0)).sqrt();
    s.execute("sketch.line", &json!({"points":[[0,0],[-10,0],[x,y],[42,0]],"ids":["crank","coupler","rocker"],"infer":false}))?;
    s.execute("sketch.line", &json!({"points":["crank.start","rocker.end"],"ids":["ground"],"infer":false}))?;
    for (line, length) in [("crank", 10.0), ("coupler", 40.0), ("rocker", 20.0), ("ground", 42.0)] {
        s.execute("sketch.dimension", &json!({"entities":[line],"value":length}))?;
    }
    s.execute("sketch.constraint.horizontal_vertical", &json!({"line":"ground"}))?;
    Ok(s)
}

pub fn point(s: &Session, name: &str) -> Result<Vec2> {
    let sk = s.doc.sketch(s.active_sketch.ok_or("missing active sketch")?)?;
    sk.resolve_point(name).and_then(|p| sk.point(p)).ok_or_else(|| format!("missing point {name}").into())
}

pub fn length_error(s: &Session) -> Result<f64> {
    let mut error = 0.0_f64;
    for (a, b, length) in [
        ("crank.start", "crank.end", 10.0),
        ("coupler.start", "coupler.end", 40.0),
        ("rocker.start", "rocker.end", 20.0),
        ("ground.start", "ground.end", 42.0),
    ] {
        error = error.max((point(s, a)?.dist(point(s, b)?) - length).abs());
    }
    error = error.max(point(s, "crank.start")?.len());
    error = error.max(point(s, "ground.end")?.dist(Vec2::new(42.0, 0.0)));
    Ok(error)
}

pub fn branch(s: &Session) -> Result<f64> {
    let a = point(s, "crank.end")?;
    Ok((point(s, "rocker.end")? - a).cross(point(s, "coupler.end")? - a).signum())
}
