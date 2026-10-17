use crate::Session;
use serde_json::{Value, json};
#[cfg(all(feature = "raytrace-cpu", not(target_arch = "wasm32")))]
use solidcraft_render::job::RaytraceOptions;
use solidcraft_render::job::{CpuHost, DirectionalLightOptions, PunctualLightOptions, RectangleLightOptions};

fn params() -> Value {
    let area = RectangleLightOptions {
        center_mm: [0.0, 0.0, 10.0],
        edge_u_mm: [8.0, 0.0, 0.0],
        edge_v_mm: [0.0, -0.5, 0.0],
        color: [1.0; 3],
        intensity: 2.0,
    };
    let point = PunctualLightOptions {
        position_mm: [0.0, 0.0, 10.0],
        color: [1.0; 3],
        intensity: 1.0,
        reference_distance_mm: 10.0,
        minimum_distance_mm: 0.001,
        cone: None,
    };
    let directional = DirectionalLightOptions { direction: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 };
    json!({"integrator":"raytrace","settings":{"width":8,"height":8},"raytrace":{"primary_samples":16,"lights":vec![directional;8],"punctual_lights":vec![point;8],"area_lights":vec![area;3],"area_samples":64},"led_strip":{"center_mm":[0,0,10],"length_mm":8,"width_mm":0.5,"length_axis":[1,0,0],"width_axis":[0,-1,0],"color":[1,0.5,0.25],"intensity":2},"led_samples":64})
}

#[cfg(all(feature = "raytrace-cpu", not(target_arch = "wasm32")))]
#[test]
fn normalized_twenty_source_preflight_has_one_metadata_authority_and_no_reservation() {
    let host = CpuHost::for_platform().unwrap();
    let mut s = Session::default();
    s.set_render_job_host(Some(host.clone())).unwrap();
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.undo.len(), s.redo.len());
    let input = params();
    let unchanged = input.clone();
    let reply = s.execute("render.preflight", &input).unwrap();
    let options: RaytraceOptions = serde_json::from_value(reply["request"]["raytrace"].clone()).unwrap();
    assert_eq!(options.primary_samples, 16);
    assert_eq!(options.area_lights.as_ref().unwrap().len(), 4);
    assert_eq!(options.punctual_lights.as_ref().unwrap().len(), 8);
    assert_eq!(reply["retained_request_bytes"], json!(options.retained_bytes().unwrap()));
    assert_eq!(host.admitted_bytes(), 0);
    assert_eq!(input, unchanged);
    assert!(std::sync::Arc::ptr_eq(&doc, &s.doc));
    assert_eq!(before, (s.revision, s.evaluations, s.undo.len(), s.redo.len()));
}

#[test]
fn invalid_or_unavailable_union_preserves_stale_scene_before_capture_or_replacement() {
    let host = CpuHost::for_platform().unwrap();
    let mut s = Session::default();
    s.set_render_job_host(Some(host.clone())).unwrap();
    for (command, input) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.rectangle.two_point", json!({"p0":[5,5],"p1":[25,20]})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance":5})),
        ("sketch.edit", json!({"sketch":"Sketch1"})),
        ("sketch.drag", json!({"entity":"l1.start","from":[5,5],"to":[4,5],"live":true})),
    ] {
        s.execute(command, &input).unwrap();
    }
    assert!(s.model.is_stale());
    let doc = s.doc.clone();
    let before = (s.revision, s.evaluations, s.undo.len(), s.redo.len());
    for command in ["render.preflight", "render.start"] {
        let mut input = params();
        input["replace"] = json!(true);
        input["led_samples"] = json!(4);
        let error = s.execute(command, &input).unwrap_err().to_string();
        assert!(error.contains("area_samples") || error.contains("raytrac"), "{error}");
        assert!(s.model.is_stale());
        assert!(std::sync::Arc::ptr_eq(&doc, &s.doc));
        assert_eq!(before, (s.revision, s.evaluations, s.undo.len(), s.redo.len()));
        assert_eq!(host.admitted_bytes(), 0);
    }
}
