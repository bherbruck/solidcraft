use super::*;
use std::f64::consts::PI;

fn lambert() -> OpaqueSurface {
    // At normal incidence IOR 1 gives f = base_colour / pi exactly.
    OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.25), 0.0, 1.0, 1.0).unwrap()
}

#[test]
fn analytic_lambert_separates_linear_colour_intensity_and_cosine() {
    let light = DirectionalLight::new(Vec3::Z, Vec3::new(1.0, 0.5, 0.25), 2.0).unwrap();
    let lights = Lighting::new(vec![light]).unwrap();
    let actual = lights.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap();
    let expected = Vec3::new(0.8, 0.25, 0.0625) * (2.0 / PI);
    assert!((actual - expected).len() < 1e-14);
    let twice = Lighting::new(vec![DirectionalLight::new(light.direction(), light.color(), 4.0).unwrap()]).unwrap();
    assert!((twice.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap() - actual * 2.0).len() < 1e-14);
    // White, fully rough conductor has f = 1 / (2*pi*(1+cos(theta)))
    // at a normal view. This independently checks the incident cosine.
    let conductor = OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap();
    let angled = Lighting::new(vec![DirectionalLight::new(Vec3::new(0.6, 0.0, 0.8), light.color(), 2.0).unwrap()]).unwrap();
    let actual = angled.shade_linear(conductor, Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap();
    assert!((actual - light.color() * (0.8 / (1.8 * PI))).len() < 1e-14);
}

#[test]
fn shadow_visibility_is_independent_for_each_coloured_light() {
    let lights = Lighting::new(vec![
        DirectionalLight::new(Vec3::Z, Vec3::new(1.0, 0.0, 0.0), 2.0).unwrap(),
        DirectionalLight::new(Vec3::Z, Vec3::new(0.0, 0.0, 1.0), 3.0).unwrap(),
    ])
    .unwrap();
    let mut visited = Vec::new();
    let blue = lights
        .shade_linear(lambert(), Vec3::Z, Vec3::Z, |i, direction| {
            visited.push((i, direction));
            Ok(i == 1)
        })
        .unwrap();
    assert_eq!(visited, vec![(0, Vec3::Z), (1, Vec3::Z)]);
    assert!((blue - Vec3::new(0.0, 0.0, 0.75 / PI)).len() < 1e-14);
    assert_eq!(lights.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| Ok(false)).unwrap(), Vec3::ZERO);
    for error in [TraceError::Cancelled, TraceError::WorkLimit, TraceError::MemoryLimit] {
        assert_eq!(lights.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| Err(error)), Err(error));
    }
}

#[test]
fn neutral_default_preserves_existing_studio_linear_response() {
    let lighting = Lighting::studio_default();
    assert_eq!(lighting.lights().len(), 3);
    for (light, (direction, intensity)) in lighting.lights().iter().zip(crate::studio::LIGHTS) {
        assert_eq!(light.direction(), direction);
        assert_eq!(light.intensity(), intensity);
        assert_eq!(light.color(), Vec3::new(1.0, 1.0, 1.0));
    }
    for metallic in [0.0, 0.5, 1.0] {
        let surface = OpaqueSurface::new(Vec3::new(0.8, 0.3, 0.2), metallic, 0.5, 1.5).unwrap();
        let expected = crate::studio::shade(surface, Vec3::Z, Vec3::Z).unwrap();
        let actual = lighting.shade_linear(surface, Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap();
        assert!((expected - actual).len() < 1e-12);
    }
}

#[test]
fn exposure_is_applied_after_the_sum_before_one_tone_map() {
    let linear = Vec3::new(0.25, 0.5, 1.0);
    let mapped = map_to_display(linear, 1.0).unwrap();
    assert!((mapped - Vec3::new(1.0 / 3.0, 0.5, 2.0 / 3.0)).len() < 1e-14);
    assert_eq!(map_to_display(Vec3::ZERO, -16.0).unwrap(), Vec3::ZERO);
    assert_eq!(map_to_display(linear, 0.0).unwrap(), crate::studio::tone_map(linear).unwrap());
    let high = map_to_display(Vec3::new(MAX_LINEAR_RADIANCE, MAX_LINEAR_RADIANCE, MAX_LINEAR_RADIANCE), 16.0).unwrap();
    assert!(high.is_finite() && high.x <= 1.0);
}

#[test]
fn dark_backfacing_and_unlit_surfaces_do_not_trace_shadow_rays() {
    let muted = DirectionalLight::new(Vec3::Z, Vec3::new(1.0, 1.0, 1.0), 0.0).unwrap();
    let behind = DirectionalLight::new(-Vec3::Z, Vec3::new(1.0, 1.0, 1.0), 1.0).unwrap();
    let black = DirectionalLight::new(Vec3::Z, Vec3::ZERO, 1.0).unwrap();
    let lighting = Lighting::new(vec![muted, behind, black]).unwrap();
    assert_eq!(lighting.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| panic!("no contributing light")).unwrap(), Vec3::ZERO);
    assert_eq!(Lighting::default().shade_linear(lambert(), Vec3::Z, -Vec3::Z, |_, _| panic!("backfacing view")).unwrap(), Vec3::ZERO);
    assert_eq!(Lighting::new(Vec::new()).unwrap().shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| panic!("no lights")).unwrap(), Vec3::ZERO);
}

#[test]
fn invalid_inputs_are_refused_and_extreme_direction_scales_normalize() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 64.01] {
        assert!(DirectionalLight::new(Vec3::Z, Vec3::new(1.0, 1.0, 1.0), bad).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
        assert!(DirectionalLight::new(Vec3::Z, Vec3::new(bad, 1.0, 1.0), 1.0).is_err());
    }
    for bad in [Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 1.0)] {
        assert!(DirectionalLight::new(bad, Vec3::ZERO, 1.0).is_err());
        assert!(Lighting::default().shade_linear(lambert(), bad, Vec3::Z, |_, _| Ok(true)).is_err());
        assert!(Lighting::default().shade_linear(lambert(), Vec3::Z, bad, |_, _| Ok(true)).is_err());
    }
    let white = Vec3::new(1.0, 1.0, 1.0);
    for magnitude in [1e-300, 1e308] {
        let direction = DirectionalLight::new(Vec3::new(magnitude, magnitude, magnitude), white, 1.0).unwrap().direction();
        assert!((direction.len() - 1.0).abs() < 1e-14);
    }
    let light = DirectionalLight::new(Vec3::Z, white, MAX_LIGHT_INTENSITY).unwrap();
    assert!(Lighting::new(vec![light; MAX_LIGHTS]).is_ok());
    assert!(Lighting::new(vec![light; MAX_LIGHTS + 1]).is_err());
    let excess_capacity = Vec::<DirectionalLight>::with_capacity(MAX_LIGHTS + 1);
    assert_eq!(excess_capacity.len(), 0);
    assert!(Lighting::new(excess_capacity).is_err(), "empty vectors cannot retain an unbounded light allocation");
    for bad in [f64::NAN, f64::INFINITY, -16.01, 16.01] {
        assert!(map_to_display(Vec3::ZERO, bad).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, -1.0, MAX_LINEAR_RADIANCE * 2.0] {
        assert!(map_to_display(Vec3::new(bad, 0.0, 0.0), 0.0).is_err());
    }
}

#[test]
fn original_presets_have_distinct_bounded_real_direct_responses() {
    let white = Lighting::preset(StudioPreset::Neutral).unwrap();
    assert_eq!(white, Lighting::default());
    let warm = Lighting::preset(StudioPreset::WarmCool).unwrap();
    let rim = Lighting::preset(StudioPreset::KeyRim).unwrap();
    for lights in [&white, &warm, &rim] {
        assert!(lights.lights().len() <= MAX_LIGHTS);
        let response = lights.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap();
        assert!(response.is_finite() && response.x > 0.0 && response.y > 0.0 && response.z > 0.0);
    }
    assert_ne!(
        white.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap(),
        warm.shade_linear(lambert(), Vec3::Z, Vec3::Z, |_, _| Ok(true)).unwrap()
    );
}
