use crate::{Result, Session, params::bad, render_jobs::job_error};
use serde::Deserialize;
use serde_json::{Value, json};
use solidcraft_doc::render_settings::RenderSetup;
use solidcraft_render::{
    RenderOperation, RenderRequest,
    job::{ImageRequest, Integrator, JobId, RaytraceOptions},
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Start {
    integrator: Integrator,
    environment: Option<EnvironmentRequest>,
    raytrace: Option<RaytraceOptions>,
    led_strip: Option<solidcraft_render::job::led_strip::LedStripOptions>,
    led_samples: Option<u32>,
    setup: Option<String>,
    settings: Option<RenderSetup>,
    transparent_background: bool,
    replace: bool,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EnvironmentRequest {
    path: String,
    rotation_radians: f64,
    intensity: f64,
    samples: u32,
}
impl Default for EnvironmentRequest {
    fn default() -> Self {
        Self { path: String::new(), rotation_radians: 0.0, intensity: 1.0, samples: 16 }
    }
}

type Prepared =
    (Start, solidcraft_render::ProductScene, ImageRequest, Value, Option<solidcraft_render::lighting::environment_input::EnvironmentInput>);

fn lower_led(input: &mut Start, command: &str) -> Result<()> {
    if input.led_strip.is_none() && input.led_samples.is_some() {
        return Err(bad(command, "led_samples requires led_strip"));
    }
    if let Some(strip) = input.led_strip {
        if input.integrator != Integrator::Raytrace {
            return Err(bad(command, "led_strip requires integrator: raytrace"));
        }
        strip.append_to(input.raytrace.get_or_insert_default(), input.led_samples.unwrap_or(16)).map_err(job_error)?;
        input.led_strip = None;
        input.led_samples = None;
    }
    Ok(())
}

#[cfg(test)]
mod union_lowering_tests {
    use super::*;
    #[test]
    fn led_lowering_keeps_hdr_reflection_aa_and_anisotropy_inputs_without_reading_hdr() {
        let wire = json!({"integrator":"raytrace","environment":{"path":"unopened.hdr","rotation_radians":0.75,"intensity":2,"samples":64},"raytrace":{"primary_samples":4,"reflections":{"bodies":[{"body":"receiver","mode":"rough_specular"}],"samples":16,"max_bounces":3},"anisotropic_faces":[{"body_index":0,"body_name":"receiver","face":0,"tangent":[1,0,0],"orientation":"quarter_turn","roughness_x":0.2,"roughness_y":0.5},null],"punctual_lights":[{"position_mm":[0,0,20],"color":[1,1,1],"intensity":1,"reference_distance_mm":20,"minimum_distance_mm":0.001}]},"led_strip":{"center_mm":[0,0,10],"length_mm":8,"width_mm":0.5,"length_axis":[1,0,0],"width_axis":[0,-1,0],"color":[1,0.5,0.25],"intensity":2},"led_samples":64});
        let mut input: Start = serde_json::from_value(wire.clone()).unwrap();
        let before = input.raytrace.clone().unwrap();
        lower_led(&mut input, "render.preflight").unwrap();
        let after = input.raytrace.unwrap();
        assert_eq!(before.primary_samples, after.primary_samples);
        assert_eq!(before.reflections, after.reflections);
        assert_eq!(before.anisotropic_faces, after.anisotropic_faces);
        assert_eq!(before.punctual_lights, after.punctual_lights);
        assert_eq!(after.area_lights.as_ref().unwrap().len(), 1);
        assert_eq!(after.area_samples, Some(64));
        let environment = input.environment.unwrap();
        assert_eq!(environment.path, "unopened.hdr");
        assert_eq!(environment.rotation_radians, 0.75);
        assert_eq!(environment.intensity, 2.0);
        assert_eq!(environment.samples, 64);
        assert!(input.led_strip.is_none() && input.led_samples.is_none());
        assert_eq!(wire["environment"]["path"], json!("unopened.hdr"));
    }
}

fn prepare(s: &Session, params: &Value, command: &str) -> Result<Prepared> {
    let mut input: Start = serde_json::from_value(params.clone()).map_err(|e| bad(command, e.to_string()))?;
    if input.setup.is_some() && input.settings.is_some() {
        return Err(bad(command, "choose either setup or settings"));
    }
    // Reject unavailable execution before capturing authoritative product meshes.
    // Runtime services are independent of the document and can be checked cold.
    let host = s.render_job_host().ok_or_else(|| bad(command, "no image host is installed"))?;
    if !host.is_available() {
        return Err(bad(command, "image host is unavailable"));
    }
    host.validate_integrator(input.integrator).map_err(job_error)?;
    lower_led(&mut input, command)?;
    if input.integrator == Integrator::Raster && input.raytrace.is_some() {
        return Err(bad(command, "raytrace options require integrator: raytrace"));
    }
    if input.integrator == Integrator::Raytrace && input.transparent_background {
        return Err(bad(command, "raytrace supports opaque backgrounds only"));
    }
    if let Some(options) = &input.raytrace {
        options.retained_bytes().map_err(job_error)?;
    }
    let environment = if let Some(environment) = &input.environment {
        if input.integrator != Integrator::Raytrace
            || environment.path.is_empty()
            || environment.path.len() > 4096
            || environment.path.capacity() > 4096
            || environment.path.chars().any(char::is_control)
        {
            return Err(bad(command, "environment requires raytrace and a bounded HDR input path"));
        }
        // The established scoped, bounded file reader and HDR decoder remain the only input path.
        let bytes =
            solidcraft_io::vfs::read(&environment.path, solidcraft_render::lighting::MAX_HDR_BYTES).map_err(|e| bad(command, e.to_string()))?;
        let source = solidcraft_render::lighting::HdrSource::new(bytes).map_err(|e| bad(command, e.to_string()))?;
        Some(
            solidcraft_render::lighting::environment_input::EnvironmentInput::new(
                source,
                environment.rotation_radians,
                environment.intensity,
                environment.samples,
            )
            .map_err(|e| bad(command, e.to_string()))?,
        )
    } else {
        None
    };
    let scene = crate::product_scene::extract(s)?;
    if environment.is_some() {
        input.raytrace.as_ref().unwrap_or(&RaytraceOptions::default()).validate_environment_reflections(&scene).map_err(job_error)?;
    }
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
    if settings.exposure_ev != 0.0 || settings.environment_rotation != 0.0 || settings.environment_intensity != 1.0 {
        return Err(bad(command, "exposure and environment controls are not supported by the R4a CPU image executor"));
    }
    let descriptor = host.descriptor_for(input.integrator).map_err(job_error)?;
    let expected_executor = descriptor.id.clone();
    let mut backend = s.render_backend();
    backend.executors = vec![descriptor];
    let mut features = scene.required_features();
    if environment.is_some() {
        features.push(solidcraft_render::RenderFeature::Hdri);
    }
    let environment_info = environment.as_ref().map(|e| e.metadata());
    let resolved = solidcraft_render::resolve_request(
        &[backend],
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
    if resolved.executor != expected_executor {
        return Err(bad(command, "resolved executor is not installed as an image job host"));
    }
    let request = ImageRequest {
        integrator: input.integrator,
        raytrace: input.raytrace.take(),
        camera: settings.camera,
        width: settings.width,
        height: settings.height,
        background: settings.background,
        transparent_background: input.transparent_background,
    };
    let mut admission = solidcraft_render::job::admit(&scene, &request).map_err(job_error)?;
    let environment_bytes = environment.as_ref().map_or(Ok(0), |e| e.required_bytes()).map_err(|e| bad(command, e.to_string()))?;
    let retained_lights = request.raytrace.as_ref().map_or(Ok(0), |o| o.retained_bytes()).map_err(job_error)?;
    let retained_environment =
        if environment_info.is_some() { std::mem::size_of::<solidcraft_render::lighting::environment_input::EnvironmentInfo>() as u64 } else { 0 };
    let retained_request_bytes = retained_lights.checked_add(retained_environment).ok_or_else(|| bad(command, "request metadata overflow"))?;

    admission.estimated_total_bytes = admission
        .estimated_total_bytes
        .checked_add(environment_bytes)
        .and_then(|v| v.checked_add(retained_request_bytes))
        .filter(|v| *v <= 384 * 1024 * 1024)
        .ok_or_else(|| bad(command, "environment job exceeds per-job memory limit"))?;
    admission.preparation_bytes =
        admission.preparation_bytes.checked_add(environment_bytes).ok_or_else(|| bad(command, "environment admission overflow"))?;
    let transparency = match input.integrator {
        Integrator::Raster => "sorted_alpha_approximation",
        Integrator::Raytrace => "opaque_only",
    };
    let reply = json!({"environment":environment_info,"retained_request_bytes":retained_request_bytes,"resolved":resolved,"admission":admission,"request":request,"scene_generation":s.revision,"host":host.kind(),"transparency":transparency,"refraction":false});
    Ok((input, scene, request, reply, environment))
}

pub(super) fn preflight(s: &mut Session, params: &Value) -> Result<Value> {
    prepare(s, params, "render.preflight").map(|(_, _, _, reply, _)| reply)
}

pub(super) fn start(s: &mut Session, params: &Value) -> Result<Value> {
    let (input, scene, request, _, environment) = prepare(s, params, "render.start")?;
    let status = s.render_jobs.start_with_environment(s.revision, scene, request, input.replace, environment).map_err(job_error)?;
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
