use crate::{Result, Session, params::bad, render_jobs::job_error};
use serde::Deserialize;
use serde_json::{Value, json};
use solidcraft_doc::render_settings::RenderSetup;
use solidcraft_render::{
    RenderOperation, RenderRequest,
    job::{ImageRequest, JobId},
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Start {
    setup: Option<String>,
    settings: Option<RenderSetup>,
    transparent_background: bool,
    replace: bool,
    progressive: bool,
    samples: Option<u32>,
    max_depth: Option<u32>,
    seed: Option<u32>,
}

struct Prepared {
    input: Start,
    scene: solidcraft_render::ProductScene,
    request: ImageRequest,
    reply: Value,
    executor: solidcraft_render::ExecutorId,
    options: solidcraft_render::job::JobOptions,
}

fn prepare(s: &Session, params: &Value, command: &str) -> Result<Prepared> {
    let input: Start = serde_json::from_value(params.clone()).map_err(|e| bad(command, e.to_string()))?;
    if input.setup.is_some() && input.settings.is_some() {
        return Err(bad(command, "choose either setup or settings"));
    }
    let scene = crate::product_scene::extract(s)?;
    let settings = if let Some(name) = &input.setup {
        s.doc.render.setups.get(name).cloned().ok_or_else(|| bad(command, "no saved render setup with that name"))?
    } else if let Some(settings) = &input.settings {
        settings.clone()
    } else {
        let mut settings = RenderSetup::default();
        if let Some(camera) = s.camera {
            settings.camera = camera;
        } else {
            settings.camera.fit(&scene.bounds);
        }
        settings
    };
    settings.validate().map_err(|e| bad(command, e.to_string()))?;
    if settings.environment_rotation != 0.0 || settings.environment_intensity != 1.0 {
        return Err(bad(command, "environment controls are not supported by the installed image executors"));
    }
    if !input.progressive && (input.samples.is_some() || input.max_depth.is_some() || input.seed.is_some() || settings.exposure_ev != 0.0) {
        return Err(bad(command, "sampling and exposure controls require progressive rendering"));
    }
    let mut options = solidcraft_render::job::JobOptions::default();
    if let Some(samples) = input.samples {
        options.samples = samples;
    }
    if let Some(depth) = input.max_depth {
        options.max_depth = depth;
    }
    if let Some(seed) = input.seed {
        options.seed = seed;
    }
    options.exposure_ev = settings.exposure_ev;
    options.validate().map_err(job_error)?;
    let mut features = scene.required_features();
    if input.progressive {
        features.push(solidcraft_render::RenderFeature::Progressive);
    }
    let resolved = solidcraft_render::resolve_request(
        &[s.render_backend()],
        &RenderRequest {
            backend: Some(settings.backend.clone()),
            mode: settings.mode,
            operation: RenderOperation::Image,
            width: u64::from(settings.width),
            height: u64::from(settings.height),
            format: Some(settings.format),
            features,
            policy: Default::default(),
        },
    )
    .map_err(|e| bad(command, e.to_string()))?;
    let host = s.render_jobs.host_for(&resolved.executor).ok_or_else(|| bad(command, "resolved executor is not installed as an image job host"))?;
    if !host.is_available() {
        return Err(bad(command, "image host is unavailable"));
    }
    let request = ImageRequest {
        camera: settings.camera,
        width: settings.width,
        height: settings.height,
        background: settings.background,
        transparent_background: input.transparent_background,
    };
    let admission = host.admit(&scene, &request, options).map_err(job_error)?;
    let reply = json!({"resolved":resolved,"admission":admission,"request":request,"scene_generation":s.revision,"host":host.kind(),"transparency":if input.progressive {"opaque_only"} else {"sorted_alpha_approximation"},"options":options,"refraction":false});
    Ok(Prepared { input, scene, request, reply, executor: resolved.executor, options })
}

pub(super) fn preflight(s: &mut Session, params: &Value) -> Result<Value> {
    prepare(s, params, "render.preflight").map(|prepared| prepared.reply)
}

pub(super) fn start(s: &mut Session, params: &Value) -> Result<Value> {
    let p = prepare(s, params, "render.start")?;
    let status = s.render_jobs.start(s.revision, p.scene, p.request, p.input.replace, &p.executor, p.options).map_err(job_error)?;
    Ok(json!(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    job: JobId,
}

pub(super) fn status(s: &mut Session, params: &Value) -> Result<Value> {
    let input: Job = serde_json::from_value(params.clone()).map_err(|e| bad("render.status", e.to_string()))?;
    Ok(json!(s.render_jobs.status(input.job).map_err(job_error)?))
}

pub(super) fn cancel(s: &mut Session, params: &Value) -> Result<Value> {
    let input: Job = serde_json::from_value(params.clone()).map_err(|e| bad("render.cancel", e.to_string()))?;
    let status = s.render_jobs.status(input.job).map_err(job_error)?;
    if status.progress.phase.terminal() {
        return Ok(json!(status));
    }
    Ok(json!(s.render_jobs.handle(input.job).map_err(job_error)?.cancel().map_err(job_error)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    job: JobId,
    path: String,
    #[serde(default)]
    overwrite: bool,
}

pub(super) fn export(s: &mut Session, params: &Value) -> Result<Value> {
    let input: Export = serde_json::from_value(params.clone()).map_err(|e| bad("render.export", e.to_string()))?;
    let path = std::path::Path::new(&input.path);
    if input.path.len() > 4096 || input.path.chars().any(char::is_control) || !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
        return Err(bad("render.export", "a bounded PNG destination path is required"));
    }
    // Atomic native writers are also used for the app's own files and do not
    // apply a request's path scope. Check the caller's destination explicitly.
    solidcraft_io::vfs::check(&input.path).map_err(|e| bad("render.export", e.to_string()))?;
    let frame = s.render_jobs.frame(input.job).map_err(job_error)?;
    // Replace atomically without reading an arbitrarily large previous target into memory.
    // Design saves retain their independent backup policy.
    let result = if input.overwrite {
        solidcraft_io::write_atomic(path, frame.png(), false)
    } else {
        solidcraft_io::safe_write::create_atomic(path, frame.png())
    };
    result.map_err(|e| bad("render.export", format!("could not export PNG: {e}")))?;
    Ok(json!({"job":input.job,"path":input.path,"bytes":frame.png().len(),"frame":frame.info}))
}
