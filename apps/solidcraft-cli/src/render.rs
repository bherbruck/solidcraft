//! Complete headless image-job workflow through the same engine commands as the UI/MCP.
use serde_json::json;
use solidcraft_engine::{
    doc::render_settings::RenderSetup,
    render::job::{CpuHost, JobId, Phase},
};

pub fn run(args: &[String]) -> Result<(), String> {
    let input = args.first().filter(|p| !p.starts_with('-')).ok_or("render requires an input design or script")?;
    let mut out = None;
    let mut setup = None;
    let mut width = None;
    let mut height = None;
    let mut progressive = false;
    let mut samples = None;
    let mut max_depth = None;
    let mut seed = None;
    let mut exposure = None;
    let mut lighting = None;
    let mut tone_map = None;
    let mut environment = None;
    let mut transparent = false;
    let mut overwrite = false;
    let mut timeout = 120u64;
    let mut i = 1;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--progressive" => progressive = true,
            "--transparent" => transparent = true,
            "--overwrite" => overwrite = true,
            "--out" | "--setup" | "--width" | "--height" | "--timeout" | "--samples" | "--max-depth" | "--seed" | "--exposure" | "--lighting"
            | "--tone-map" | "--environment" => {
                i += 1;
                let value = args.get(i).ok_or_else(|| format!("missing value for {arg}"))?;
                match arg.as_str() {
                    "--out" => out = Some(value.clone()),
                    "--setup" => setup = Some(value.clone()),
                    "--width" => width = Some(value.parse::<u32>().map_err(|_| "invalid width")?),
                    "--height" => height = Some(value.parse::<u32>().map_err(|_| "invalid height")?),
                    "--samples" => samples = Some(value.parse::<u32>().map_err(|_| "invalid samples")?),
                    "--max-depth" => max_depth = Some(value.parse::<u32>().map_err(|_| "invalid depth")?),
                    "--seed" => seed = Some(value.parse::<u32>().map_err(|_| "invalid seed")?),
                    "--exposure" => exposure = Some(value.parse::<f64>().map_err(|_| "invalid exposure")?),
                    "--environment" => {
                        let text =
                            solidcraft_engine::io::vfs::read_to_string(value, 65536).map_err(|e| format!("could not read environment: {e}"))?;
                        environment = Some(serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())?);
                    }
                    "--lighting" => {
                        let text = solidcraft_engine::io::vfs::read_to_string(value, 65536).map_err(|e| format!("could not read lighting: {e}"))?;
                        lighting = Some(serde_json::from_str::<solidcraft_engine::render::job::StudioLighting>(&text).map_err(|e| e.to_string())?);
                    }
                    "--tone-map" => {
                        tone_map = Some(match value.as_str() {
                            "reinhard" => solidcraft_engine::render::job::ToneMap::Reinhard,
                            "aces_fitted" => solidcraft_engine::render::job::ToneMap::AcesFitted,
                            _ => return Err("tone-map must be reinhard or aces_fitted".into()),
                        })
                    }
                    _ => timeout = value.parse::<u64>().map_err(|_| "invalid timeout")?,
                }
            }
            _ => return Err(format!("unknown render option: {arg}")),
        }
        i += 1;
    }
    if !(1..=3600).contains(&timeout) {
        return Err("render timeout must be 1..3600 seconds".into());
    }
    let out = out.ok_or("render requires --out FILE.png")?;
    let mut session = crate::load(input)?;
    session.set_render_job_host(Some(CpuHost::for_platform().map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    if progressive {
        session.register_render_job_host(solidcraft_gpu::path::host().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    session.execute("render.scene", &json!({})).map_err(|e| e.to_string())?;
    let mut settings = if let Some(name) = setup {
        session.doc.render.setups.get(&name).cloned().ok_or("no saved render setup with that name")?
    } else {
        let mut settings = RenderSetup::default();
        if let Some(camera) = session.camera {
            settings.camera = camera;
        } else {
            settings.camera.fit(&solidcraft_engine::product_scene::extract(&session).map_err(|e| e.to_string())?.bounds);
        }
        settings
    };
    if let Some(width) = width {
        settings.width = width;
    }
    if let Some(height) = height {
        settings.height = height;
    }
    if let Some(exposure) = exposure {
        settings.exposure_ev = exposure;
    }
    let params = json!({"settings":settings,"transparent_background":transparent,"progressive":progressive,"samples":samples,"max_depth":max_depth,"seed":seed,"lighting":lighting,"tone_map":tone_map,"environment":environment});
    let preflight = session.execute("render.preflight", &params).map_err(|e| e.to_string())?;
    let started = session.execute("render.start", &params).map_err(|e| e.to_string())?;
    let id: JobId = serde_json::from_value(started["job"].clone()).map_err(|e| e.to_string())?;
    let handle = session.render_job_handle(id).map_err(|e| e.to_string())?;
    let status = match handle.wait_timeout(std::time::Duration::from_secs(timeout)) {
        Ok(status) => status,
        Err(e) => {
            let _ = session.execute("render.cancel", &json!({"job":id}));
            return Err(e.to_string());
        }
    };
    if status.progress.phase != Phase::Completed {
        return Err(status.error.map_or_else(|| "render cancelled".into(), |e| e.to_string()));
    }
    let exported = session.execute("render.export", &json!({"job":id,"path":out,"overwrite":overwrite})).map_err(|e| e.to_string())?;
    println!("{}", crate::pretty(&json!({"preflight":preflight,"export":exported})));
    Ok(())
}
