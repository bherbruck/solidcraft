use super::*;

fn strip() -> led_strip::LedStripOptions {
    led_strip::LedStripOptions {
        center_mm: [0.0, 0.0, 10.0],
        length_mm: 8.0,
        width_mm: 0.5,
        length_axis: [1.0, 0.0, 0.0],
        width_axis: [0.0, -1.0, 0.0],
        color: [1.0, 0.5, 0.25],
        intensity: 2.0,
    }
}
fn full(n: u32) -> RaytraceOptions {
    let mut options = RaytraceOptions {
        primary_samples: 16,
        lights: Some(vec![DirectionalLightOptions { direction: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0 }; 8]),
        punctual_lights: Some(vec![
            PunctualLightOptions {
                position_mm: [0.0, 0.0, 20.0],
                color: [1.0; 3],
                intensity: 1.0,
                reference_distance_mm: 20.0,
                minimum_distance_mm: 0.001,
                cone: None
            };
            8
        ]),
        reflections: Some(ReflectionOptions {
            bodies: vec![ReflectionBody { body: "receiver".into(), mode: ReflectionMode::RoughSpecular }],
            ..Default::default()
        }),
        ..Default::default()
    };
    for _ in 0..4 {
        strip().append_to(&mut options, n).unwrap();
    }
    options
}
#[test]
fn full_union_admits_twenty_sources_and_bounded_attempts_without_scaling_power() {
    for n in [1, 4, 16, 64] {
        let options = full(n);
        options.validate().unwrap();
        assert_eq!(
            options.lights.as_ref().unwrap().len() + options.punctual_lights.as_ref().unwrap().len() + options.area_lights.as_ref().unwrap().len(),
            20
        );
        for area in options.area_lights.as_ref().unwrap() {
            assert_eq!(area.intensity, 2.0);
            assert_eq!(area.color, [1.0, 0.5, 0.25]);
        }
        let expected = 8 * std::mem::size_of::<DirectionalLightOptions>()
            + 8 * std::mem::size_of::<PunctualLightOptions>()
            + 4 * std::mem::size_of::<RectangleLightOptions>()
            + std::mem::size_of::<ReflectionBody>()
            + 8;
        assert_eq!(options.retained_bytes().unwrap(), expected as u64);
        assert_eq!(serde_json::from_value::<RaytraceOptions>(serde_json::to_value(&options).unwrap()).unwrap(), options);
    }
}
#[test]
fn lowering_preserves_aa_reflections_anisotropy_and_punctual_sources_atomically() {
    let mut options = full(64);
    options.area_lights.as_mut().unwrap().pop();
    options.anisotropic_faces[0] = Some(AnisotropicFaceOptions {
        body_index: 0,
        body_name: "receiver".into(),
        face: 0,
        tangent: [1.0, 0.0, 0.0],
        orientation: AnisotropicOrientation::QuarterTurn,
        roughness_x: 0.2,
        roughness_y: 0.5,
    });
    let before = options.clone();
    strip().append_to(&mut options, 64).unwrap();
    assert_eq!(options.primary_samples, before.primary_samples);
    assert_eq!(options.punctual_lights, before.punctual_lights);
    assert_eq!(options.reflections, before.reflections);
    assert_eq!(options.anisotropic_faces, before.anisotropic_faces);
    let full = options.clone();
    let cap = options.area_lights.as_ref().unwrap().capacity();
    assert!(strip().append_to(&mut options, 64).is_err());
    assert_eq!(options, full);
    assert_eq!(options.area_lights.as_ref().unwrap().capacity(), cap);
}
#[test]
fn sequence_null_defaults_and_original_overcapacity_refuse_before_normalization() {
    let defaults = RaytraceOptions::default();
    assert_eq!(serde_json::from_value::<RaytraceOptions>(serde_json::json!({"area_lights":null,"punctual_lights":null})).unwrap(), defaults);
    for kind in ["lights", "punctual_lights", "area_lights"] {
        let mut options = full(64);
        match kind {
            "lights" => options.lights = Some(Vec::with_capacity(9)),
            "punctual_lights" => options.punctual_lights = Some(Vec::with_capacity(9)),
            _ => options.area_lights = Some(Vec::with_capacity(5)),
        }
        let before = options.clone();
        assert!(options.retained_bytes().is_err());
        assert!(strip().append_to(&mut options, 64).is_err());
        assert_eq!(options, before);
    }
    for kind in ["lights", "punctual_lights", "area_lights"] {
        let mut value = serde_json::to_value(full(64)).unwrap();
        let list = value[kind].as_array_mut().unwrap();
        list.push(list[0].clone());
        assert!(serde_json::from_value::<RaytraceOptions>(value).is_err());
    }
}

#[test]
fn source_kind_boundaries_remain_independent_with_presets_and_full_rectangle_budget() {
    for preset in [LightingPreset::Neutral, LightingPreset::WarmCool, LightingPreset::KeyRim] {
        let mut options = full(64);
        options.lights = None;
        options.preset = Some(preset);
        options.validate().unwrap();
        assert_eq!(options.area_lights.as_ref().unwrap().len(), 4);
        assert_eq!(options.punctual_lights.as_ref().unwrap().len(), 8);
    }
    for kind in ["directional", "punctual", "rectangle"] {
        let mut options = full(64);
        match kind {
            "directional" => {
                let lights = options.lights.as_mut().unwrap();
                lights.push(lights[0].clone());
            }
            "punctual" => {
                let lights = options.punctual_lights.as_mut().unwrap();
                lights.push(lights[0]);
            }
            _ => {
                let lights = options.area_lights.as_mut().unwrap();
                lights.push(lights[0].clone());
            }
        }
        assert!(options.validate().is_err());
        assert!(options.retained_bytes().is_err());
    }
}
