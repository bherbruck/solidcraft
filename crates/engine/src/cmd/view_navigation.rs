//! Stateless camera commands shared by the navigation cube and programmatic callers.
use serde_json::{Value, json};
use solidcraft_render::{Camera, StandardView};

use super::CommandSpec;
use crate::params::{bad, num, req_vec2, req_vec3, vec3};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
CommandSpec::new("view.graphics_preset", "Graphics Preset", graphics_preset)
    .icon("settings").noundo().doc_only().params("preset: performance|quality → preset, enabled, display_quality, supersampling, supersampling_adaptive; Quality: AO, Fine, adaptive 2x SSAA; Performance: no AO, Normal, no SSAA"),
CommandSpec::new("view.ambient_occlusion", "Ambient Occlusion", ambient_occlusion)
    .icon("settings").noundo().doc_only().params("enabled: bool → enabled; apply soft contact shading in the GPU viewport"),
CommandSpec::new("view.navigate", "Navigate View", navigate)
    .icon("orbit")
    .noundo()
    .doc_only()
    .params("camera: Camera; direction?: [x,y,z] in the camera's up-axis frame; or action: left|right|up|down|clockwise|counterclockwise|home|fit; or motion: pan|orbit|zoom, delta?: [dx,dy] (pixels, each ±10000), viewport?: [width,height] (1..1000000, required for pan or pivot), pivot?: [x,y,z], factor?: 0.05..20 (required for zoom), anchor?: [x,y,z] → camera, fit, continuous?")];

fn navigate(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "view.navigate";
    let mut cam: Camera =
        serde_json::from_value(p.get("camera").cloned().ok_or_else(|| bad(cmd, "camera is required"))?).map_err(|_| bad(cmd, "invalid camera"))?;
    if !cam.is_valid()
        || !cam.fov.is_finite()
        || !(0.0..std::f64::consts::PI).contains(&cam.fov)
        || cam.yaw.abs() > 1e6
        || cam.roll.abs() > 1e6
        || cam.pitch.abs() > std::f64::consts::FRAC_PI_2
    {
        return Err(bad(cmd, "invalid camera"));
    }
    if p.get("motion").is_some() {
        return continuous(cam, p);
    }
    let mut fit = false;
    if let Some(value) = p.get("direction") {
        let dir = vec3(value)
            .filter(|v| v.is_finite() && v.len() <= 2.0)
            .and_then(|v| v.normalized())
            .ok_or_else(|| bad(cmd, "direction must be a finite nonzero vector of length at most 2"))?;
        let heading = cam.yaw;
        cam = cam.looking_from(cam.from_frame(dir));
        if dir.x.abs() < 1e-9 && dir.y.abs() < 1e-9 {
            cam.yaw = square_heading(heading);
        }
    } else {
        use std::f64::consts::FRAC_PI_2;
        let (r, u, _) = cam.basis();
        match p.get("action").and_then(Value::as_str) {
            Some("left") => cam = cam.looking_from(-r),
            Some("right") => cam = cam.looking_from(r),
            Some("up") => cam = cam.looking_from(u),
            Some("down") => cam = cam.looking_from(-u),
            Some("clockwise") => cam.roll = (cam.roll + FRAC_PI_2).rem_euclid(std::f64::consts::TAU),
            Some("counterclockwise") => cam.roll = (cam.roll - FRAC_PI_2).rem_euclid(std::f64::consts::TAU),
            Some("home") => {
                cam.set_view(StandardView::Iso);
                fit = true;
            }
            Some("fit") => fit = true,
            _ => return Err(bad(cmd, "unknown navigation action")),
        }
    }
    Ok(json!({"camera": cam, "fit": fit}))
}

/// Direct manipulation is stateless, bounded and independent of document evaluation.
fn continuous(mut cam: Camera, p: &Value) -> Result<Value> {
    let cmd = "view.navigate";
    let viewport = || {
        let size = req_vec2(cmd, p, "viewport")?;
        if !(1.0..=1e6).contains(&size.x) || !(1.0..=1e6).contains(&size.y) {
            return Err(bad(cmd, "viewport dimensions must be in 1..1000000"));
        }
        Ok(size)
    };
    match p.get("motion").and_then(Value::as_str) {
        Some(motion @ ("pan" | "orbit")) => {
            let delta = req_vec2(cmd, p, "delta")?;
            if delta.x.abs() > 1e4 || delta.y.abs() > 1e4 {
                return Err(bad(cmd, "motion deltas must be within ±10000 pixels"));
            }
            if motion == "pan" {
                cam.pan(delta.x, delta.y, viewport()?.y);
            } else if p.get("pivot").is_some() {
                let pivot = req_vec3(cmd, p, "pivot")?;
                let size = viewport()?;
                let before = cam.to_screen(pivot, size.x, size.y, 1.0);
                cam.orbit(delta.x, delta.y);
                let after = cam.to_screen(pivot, size.x, size.y, 1.0);
                if let (Some((ax, ay, _)), Some((bx, by, _))) = (before, after) {
                    // Pan operates at target depth. Perspective screen correction must instead
                    // use the pivot's depth after orbiting; lateral pan leaves that depth fixed.
                    let scale = if cam.fov > 0.0 { (cam.eye() - pivot).dot(cam.back()) / cam.distance } else { 1.0 };
                    cam.pan((ax - bx) * scale, (ay - by) * scale, size.y);
                }
            } else {
                cam.orbit(delta.x, delta.y);
            }
        }
        Some("zoom") => {
            let factor = num(p, "factor").filter(|f| (0.05..=20.0).contains(f)).ok_or_else(|| bad(cmd, "factor must be in 0.05..20"))?;
            let anchor = p.get("anchor").map(|_| req_vec3(cmd, p, "anchor")).transpose()?;
            cam.zoom_at(factor, anchor);
        }
        _ => return Err(bad(cmd, "unknown navigation motion")),
    }
    if !cam.is_valid() {
        return Err(bad(cmd, "motion produced an invalid camera"));
    }
    Ok(json!({"camera":cam, "fit":false, "continuous":true}))
}

/// At a halfway tie prefer the heading nearer the front plan view.
pub(crate) fn square_heading(yaw: f64) -> f64 {
    if !yaw.is_finite() {
        return 0.0;
    }
    let q = std::f64::consts::FRAC_PI_2;
    let lo = (yaw / q).floor() * q;
    let hi = lo + q;
    if ((yaw - lo) - (hi - yaw)).abs() < 1e-6 {
        let off_front = |a: f64| ((a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI).abs();
        if off_front(lo) <= off_front(hi) { lo } else { hi }
    } else if yaw - lo < hi - yaw {
        lo
    } else {
        hi
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_geom::Vec3;
    use solidcraft_render::UpAxis;

    #[test]
    fn continuous_navigation_moves_the_camera_without_editing_the_design() {
        let mut s = Session::default();
        let before = s.doc.to_json();
        let cam = Camera { distance: 100.0, ..Default::default() };
        for (motion, extra) in
            [("pan", json!({"delta":[30,20],"viewport":[800,600]})), ("orbit", json!({"delta":[30,20]})), ("zoom", json!({"factor":0.5}))]
        {
            let mut params = extra;
            params["camera"] = json!(cam);
            params["motion"] = json!(motion);
            let result = s.execute("view.navigate", &params).unwrap();
            let to: Camera = serde_json::from_value(result["camera"].clone()).unwrap();
            assert_eq!(result["continuous"], true);
            match motion {
                "pan" => {
                    assert_ne!(to.target, cam.target);
                    assert_eq!((to.yaw, to.pitch, to.distance), (cam.yaw, cam.pitch, cam.distance));
                }
                "orbit" => {
                    assert!((to.yaw - cam.yaw - 0.24).abs() < 1e-9);
                    assert!((to.pitch - cam.pitch - 0.16).abs() < 1e-9);
                    assert_eq!((to.target, to.distance), (cam.target, cam.distance));
                }
                _ => assert_eq!(to.distance, 50.0),
            }
        }
        assert_eq!(s.doc.to_json(), before);
        assert!(s.undo.is_empty());
    }

    #[test]
    fn continuous_perspective_orbit_keeps_near_and_far_pivots_on_screen() {
        let mut s = Session::default();
        let cam = Camera { yaw: 0.0, pitch: 0.0, distance: 100.0, fov: 0.6, ..Default::default() };
        for pivot in [Vec3::new(15.0, -50.0, 0.0), Vec3::new(15.0, 50.0, 0.0)] {
            let result =
                s.execute("view.navigate", &json!({"camera":cam,"motion":"orbit","delta":[20,10],"viewport":[800,600],"pivot":pivot})).unwrap();
            let to: Camera = serde_json::from_value(result["camera"].clone()).unwrap();
            let (ax, ay, _) = cam.to_screen(pivot, 800.0, 600.0, 200.0).unwrap();
            let (bx, by, _) = to.to_screen(pivot, 800.0, 600.0, 200.0).unwrap();
            assert!((ax - bx).abs() < 1e-6 && (ay - by).abs() < 1e-6, "pivot {pivot:?} drifted from ({ax},{ay}) to ({bx},{by})");
        }
    }

    #[test]
    fn continuous_orbit_keeps_its_pivot_on_screen_and_rejects_hostile_motion() {
        let mut s = Session::default();
        let cam = Camera { distance: 100.0, ..Default::default() };
        let pivot = Vec3::new(12.0, 5.0, 0.0);
        let result = s.execute("view.navigate", &json!({"camera":cam,"motion":"orbit","delta":[20,10],"viewport":[800,600],"pivot":pivot})).unwrap();
        let to: Camera = serde_json::from_value(result["camera"].clone()).unwrap();
        let (ax, ay, _) = cam.to_screen(pivot, 800.0, 600.0, 1.0).unwrap();
        let (bx, by, _) = to.to_screen(pivot, 800.0, 600.0, 1.0).unwrap();
        assert!((ax - bx).abs() < 1e-6 && (ay - by).abs() < 1e-6);
        for extra in [
            json!({"motion":"pan","delta":[1,2],"viewport":[800,0]}),
            json!({"motion":"pan","delta":[1e100,2],"viewport":[800,600]}),
            json!({"motion":"orbit","delta":[1]}),
            json!({"motion":"orbit","delta":[1,2],"pivot":[0,0,0]}),
            json!({"motion":"zoom","factor":0}),
            json!({"motion":"zoom","factor":1e100}),
            json!({"motion":"zoom","factor":1,"anchor":[1e100,0,0]}),
            json!({"motion":"unknown"}),
        ] {
            let mut p = extra;
            p["camera"] = json!(cam);
            assert!(s.execute("view.navigate", &p).is_err(), "{p}");
        }
    }

    #[test]
    fn every_cube_direction_preserves_framing_without_editing_the_design() {
        for up in [UpAxis::Z, UpAxis::Y] {
            let mut s = Session::default();
            let before = s.doc.to_json();
            let cam = Camera { up, target: Vec3::new(10.0, 20.0, 30.0), distance: 123.0, ..Default::default() };
            for x in [-1.0, 0.0, 1.0] {
                for y in [-1.0, 0.0, 1.0] {
                    for z in [-1.0, 0.0, 1.0] {
                        let Some(dir) = Vec3::new(x, y, z).normalized() else { continue };
                        let result = s.execute("view.navigate", &json!({"camera":cam,"direction":[x,y,z]})).unwrap();
                        let to: Camera = serde_json::from_value(result["camera"].clone()).unwrap();
                        assert!(to.back().dist(cam.from_frame(dir)) < 2e-6);
                        assert_eq!((to.target, to.distance, to.fov, to.up), (cam.target, cam.distance, cam.fov, cam.up));
                    }
                }
            }
            assert_eq!(s.doc.to_json(), before);
            assert!(s.undo.is_empty());
        }
    }

    #[test]
    fn arrows_roll_and_home_are_programmatic_and_reject_hostile_input() {
        let mut s = Session::default();
        let cam = Camera::default();
        let (r, u, _) = cam.basis();
        for (action, dir) in [("left", -r), ("right", r), ("up", u), ("down", -u)] {
            let result = navigate(&mut s, &json!({"camera":cam,"action":action})).unwrap();
            let to: Camera = serde_json::from_value(result["camera"].clone()).unwrap();
            assert!(to.back().dist(dir) < 2e-6);
        }
        let result = navigate(&mut s, &json!({"camera":cam,"action":"clockwise"})).unwrap();
        let to: Camera = serde_json::from_value(result["camera"].clone()).unwrap();
        assert_eq!(to.back(), cam.back());
        assert!((to.basis().0 - u).len() < 1e-9);
        for action in ["home", "fit"] {
            assert_eq!(navigate(&mut s, &json!({"camera":cam,"action":action})).unwrap()["fit"], true);
        }
        for params in [
            json!({}),
            json!({"camera":cam,"direction":[0,0,0]}),
            json!({"camera":cam,"direction":[1e308,0,0]}),
            json!({"camera":cam,"action":"unknown"}),
        ] {
            assert!(s.execute("view.navigate", &params).is_err());
        }
    }
}

fn ambient_occlusion(_s: &mut Session, p: &Value) -> Result<Value> {
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad("view.ambient_occlusion", "enabled must be a boolean"))?;
    Ok(json!({"enabled": enabled}))
}

#[cfg(test)]
mod ambient_occlusion_tests {
    use super::*;
    #[test]
    fn validates_display_setting_without_editing_design_or_history() {
        let mut s = Session::default();
        let before = s.doc.to_json();
        for enabled in [true, false] {
            assert_eq!(s.execute("view.ambient_occlusion", &json!({"enabled": enabled})).unwrap(), json!({"enabled": enabled}));
        }
        for p in [json!({}), json!({"enabled": 1}), json!({"enabled": "true"}), json!({"enabled": null})] {
            assert!(s.execute("view.ambient_occlusion", &p).is_err());
        }
        assert_eq!(s.doc.to_json(), before);
        assert!(s.undo.is_empty());
    }
}

/// One preset for contact shading, tessellation and supersampling. MSAA stays independent.
fn graphics_preset(_s: &mut Session, p: &Value) -> Result<Value> {
    let preset = p.get("preset").and_then(Value::as_str);
    let enabled = match preset {
        Some("quality") => true,
        Some("performance") => false,
        _ => return Err(bad("view.graphics_preset", "preset must be performance or quality")),
    };
    Ok(json!({"preset": preset, "enabled": enabled, "display_quality": if enabled { "fine" } else { "normal" },
        "supersampling": if enabled { 2.0 } else { 1.0 }, "supersampling_adaptive": true}))
}

#[cfg(test)]
mod graphics_preset_tests {
    use super::*;

    #[test]
    fn presets_validate_without_changing_design_or_history() {
        let mut s = Session::default();
        let before = s.doc.to_json();
        for (preset, enabled) in [("quality", true), ("performance", false)] {
            assert_eq!(
                s.execute("view.graphics_preset", &json!({"preset":preset})).unwrap(),
                json!({"preset":preset,"enabled":enabled,"display_quality":if enabled { "fine" } else { "normal" },
                    "supersampling":if enabled { 2.0 } else { 1.0 },"supersampling_adaptive":true})
            );
        }
        for params in [json!({}), json!({"preset":"fast"}), json!({"preset":true}), json!({"preset":null})] {
            assert!(s.execute("view.graphics_preset", &params).is_err());
        }
        assert_eq!(s.doc.to_json(), before);
        assert!(s.undo.is_empty());
    }
}
