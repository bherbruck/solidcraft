//! Reproducible evidence through the production command/job interfaces.
//! Usage: studio_evidence RECIPE.json RENDER-PARAMS.json OUTPUT.png
//! Writes the editable design, command timings and actual adapter/job metadata beside PNG.
use serde_json::{Value, json};
use solidcraft_engine::{
    Session,
    render::job::{CpuHost, ErrorCode, JobId, Phase},
};
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

fn read_json(path: &str) -> Result<Value, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?.take(16_777_217).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 16_777_216 {
        return Err("evidence input exceeds 16 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}"))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [recipe_path, params_path, output_path] = args.as_slice() else {
        return Err("usage: studio_evidence RECIPE.json RENDER-PARAMS.json OUTPUT.png".into());
    };
    let output = Path::new(output_path);
    if !output.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        return Err("output must have a .png extension".into());
    }
    let recipe = read_json(recipe_path)?;
    let params = read_json(params_path)?;
    let mut session = Session::default();
    eprintln!("Generating original CAD scene from {recipe_path}");
    let generation_start = Instant::now();
    let (commands, command_timings) = session.run_script_timed(&recipe, false).map_err(|e| e.to_string())?;
    let generation_seconds = generation_start.elapsed().as_secs_f64();
    let design_path = output.with_extension("solidcraft");
    session.execute("file.save_as", &json!({"path":design_path})).map_err(|e| e.to_string())?;
    let scene = solidcraft_engine::product_scene::extract(&session).map_err(|e| e.to_string())?;
    let bodies: Vec<Value> = scene
        .meshes
        .iter()
        .map(|mesh| json!({"name":mesh.name,"vertices":mesh.mesh.positions.len(),"triangles":mesh.mesh.triangles.len()}))
        .collect();
    drop(scene);

    let policy = solidcraft_gpu::select::Policy::from_env().map_err(|e| e.to_string())?;
    let (adapter, adapter_report) = solidcraft_gpu::select::pick(policy).ok_or("no adapter satisfies the requested GPU policy")?;
    let host_start = Instant::now();
    session.set_render_job_host(Some(CpuHost::for_platform().map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    session.register_render_job_host(solidcraft_gpu::path::host_on_adapter(&adapter).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let host_seconds = host_start.elapsed().as_secs_f64();
    let preflight = session.execute("render.preflight", &params).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let started = session.execute("render.start", &params).map_err(|e| e.to_string())?;
    let id: JobId = serde_json::from_value(started.get("job").cloned().ok_or("missing job id")?).map_err(|e| e.to_string())?;
    let handle = session.render_job_handle(id).map_err(|e| e.to_string())?;
    eprintln!("Rendering through production gpu-path host: {adapter_report:?}");
    let status = loop {
        match handle.wait_timeout(Duration::from_secs(10)) {
            Ok(status) => break status,
            Err(error) if error.code == ErrorCode::TimedOut && start.elapsed() < Duration::from_secs(3600) => {
                let status = handle.status().map_err(|e| e.to_string())?;
                eprintln!(
                    "Render progress: {:?} {}/{} after {:.1}s",
                    status.progress.phase,
                    status.progress.done,
                    status.progress.total,
                    start.elapsed().as_secs_f64()
                );
            }
            Err(error) => {
                let _ = handle.cancel();
                return Err(error.to_string());
            }
        }
    };
    let render_and_encode_seconds = start.elapsed().as_secs_f64();
    if status.progress.phase != Phase::Completed {
        return Err(status.error.map_or_else(|| "render did not complete".into(), |e| e.to_string()));
    }
    let exported = session.execute("render.export", &json!({"job":id,"path":output_path,"overwrite":true})).map_err(|e| e.to_string())?;
    let report = json!({
        "recipe":recipe_path,"render_params":params,"adapter":adapter_report,
        "environment":{"SOLIDCRAFT_GPU":std::env::var("SOLIDCRAFT_GPU").ok(),"WGPU_BACKEND":std::env::var("WGPU_BACKEND").ok()},
        "cad_generation_seconds":generation_seconds,"gpu_host_setup_seconds":host_seconds,
        "render_and_encode_seconds":render_and_encode_seconds,
        "timing_scope":"wall time from render.start through completed PNG encoding; includes job preparation and readback, excludes CAD generation and initial GPU pipeline setup",
        "bodies":bodies,"commands":commands,"command_timings":command_timings,
        "preflight":preflight,"export":exported,"editable_design":design_path
    });
    std::fs::write(output.with_extension("json"), serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    Ok(())
}

fn main() -> Result<(), String> {
    run()
}
