use super::*;
use std::collections::BTreeSet;

#[test]
fn radial_strata_and_open_domains_hold_for_every_supported_count_and_extreme_keys() {
    for samples in 1..=64 {
        for pixel in [0, 17, u64::MAX] {
            for bounce in [0, 3, u64::MAX] {
                for sample in 0..samples {
                    let u = variates(pixel, sample, bounce, samples).unwrap();
                    assert!(u.into_iter().all(|v| v.is_finite() && v > 0.0 && v < 1.0));
                    assert_eq!((u[0] * samples as f64).floor() as u64, sample);
                    assert_eq!(u, variates(pixel, sample, bounce, samples).unwrap());
                }
            }
        }
    }
}

#[test]
fn azimuth_visits_each_dyadic_bin_and_scramble_changes_with_pixel_and_bounce() {
    for samples in [1, 2, 4, 8, 16, 32, 64] {
        for (pixel, bounce) in [(0, 0), (73, 2), (u64::MAX, u64::MAX)] {
            let bins: BTreeSet<_> = (0..samples).map(|i| (variates(pixel, i, bounce, samples).unwrap()[1] * samples as f64).floor() as u64).collect();
            assert_eq!(bins, (0..samples).collect::<BTreeSet<_>>());
        }
    }
    let original = variates(0, 0, 0, 16).unwrap();
    assert_ne!(original, variates(1, 0, 0, 16).unwrap());
    assert_ne!(original, variates(0, 0, 1, 16).unwrap());
    assert_ne!(original, variates(0, 0, 0, 32).unwrap(), "R is part of the sequence, not a progressive-prefix contract");
}

#[test]
fn malformed_count_or_ordinal_refuses_without_wrapping_or_retrying() {
    for count in [0, 65, u64::MAX] {
        assert_eq!(variates(0, 0, 0, count), Err(TraceError::InvalidInput("reflection sample ordinal/count outside 1..64 strata")));
    }
    for count in 1..=64 {
        for sample in [count, u64::MAX] {
            assert!(variates(u64::MAX, sample, u64::MAX, count).is_err());
        }
    }
}

#[test]
fn analytic_white_furnace_mean_second_moment_and_null_events_keep_full_sample_divisor() {
    let white = OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, 1.0, 1.5).unwrap();
    // At n=v, alpha=F=1: W(u)=2-1/(1-u) for u<1/2, zero otherwise.
    // Independent integrals: E[W]=1-ln2; E[W^2]=3-4ln2. Both integrands
    // decrease from1 to0: one point per equal stratum brackets the integral
    // between left/right Riemann sums, whose difference is exactly1/R.
    let expected_mean = 1.0 - 2.0_f64.ln();
    let expected_second = 3.0 - 4.0 * 2.0_f64.ln();
    for samples in [2, 4, 8, 16, 32, 64] {
        for (pixel, bounce) in [(0, 0), (19, 3), (u64::MAX, 7)] {
            let (mut sum, mut square_sum, mut nulls) = (0.0, 0.0, 0);
            for i in 0..samples {
                match rough_specular(white, Vec3::Z, Vec3::Z, variates(pixel, i, bounce, samples).unwrap()).unwrap() {
                    Some(sample) => {
                        sum += sample.weight.x;
                        square_sum += sample.weight.x * sample.weight.x;
                    }
                    None => nulls += 1,
                }
            }
            assert_eq!(nulls, samples / 2, "null events are retained, not retried");
            let bound = 1.0 / samples as f64;
            assert!((sum / samples as f64 - expected_mean).abs() < bound);
            assert!((square_sum / samples as f64 - expected_second).abs() < bound);
        }
    }
}

#[test]
fn original_hashed_sequence_remains_a_distinct_comparison_control() {
    for (pixel, sample, bounce) in [(0, 0, 0), (73, 7, 2), (u64::MAX, 63, u64::MAX)] {
        let old = hash_variates(pixel, sample, bounce);
        assert_eq!(old, hash_variates(pixel, sample, bounce));
        assert!(old.into_iter().all(|v| v > 0.0 && v < 1.0));
        assert_ne!(old, variates(pixel, sample, bounce, 64).unwrap());
    }
}
