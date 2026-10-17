//! The existing length display policy, shared by windowed and headless consumers.
//!
//! Values are stored in millimetres and displayed in the five design units. Unknown unit
//! names retain the millimetre fallback; expression units have a separate, broader vocabulary.
//! These functions format values, not validate them: non-finite inputs retain Rust's `NaN`,
//! `inf` and `-inf` text. Callers that require finite measurements must validate them first.

/// Units per millimetre and the unit's name. Unknown names use millimetres.
pub fn unit_scale(units: &str) -> (f64, &'static str) {
    match units {
        "cm" => (0.1, "cm"),
        "m" => (0.001, "m"),
        "in" => (1.0 / 25.4, "in"),
        "ft" => (1.0 / 304.8, "ft"),
        _ => (1.0, "mm"),
    }
}

/// Inch labels retain a meaningful fourth digit without promoting floating-point noise.
/// `value` is already converted to the requested display unit, not a millimetre value.
pub fn length_precision(value: f64, units: &str, decimals: u8) -> usize {
    let base = usize::from(decimals.min(8));
    if units != "in" {
        return base;
    }
    let needs_four = ((value * 10_000.0).round() - (value * 1000.0).round() * 10.0).abs() >= 0.5;
    base.max(if needs_four { 4 } else { 3 })
}

/// A millimetre length in the requested display unit, without a unit suffix.
pub fn length_number(mm: f64, units: &str, decimals: u8) -> String {
    let value = mm * unit_scale(units).0;
    format!("{:.*}", length_precision(value, units, decimals), value)
}

/// A length in millimetres in `units`, with `decimals` (for display).
pub fn format_length(mm: f64, units: &str, decimals: u8) -> String {
    let (_, u) = unit_scale(units);
    format!("{} {u}", length_number(mm, units, decimals))
}

#[cfg(test)]
mod tests {
    use super::{format_length, length_number, length_precision, unit_scale};

    #[test]
    fn all_design_units_keep_bare_and_suffixed_labels() {
        for (mm, units, bare, suffixed) in [
            (12.5, "mm", "12.50", "12.50 mm"),
            (125.0, "cm", "12.50", "12.50 cm"),
            (12_500.0, "m", "12.50", "12.50 m"),
            (317.5, "in", "12.500", "12.500 in"),
            (3810.0, "ft", "12.50", "12.50 ft"),
        ] {
            assert_eq!(length_number(mm, units, 2), bare, "{units}");
            assert_eq!(format_length(mm, units, 2), suffixed, "{units}");
        }
    }

    #[test]
    fn inch_labels_keep_three_or_four_digits_without_promoting_noise() {
        for (mm, decimals, expected) in [
            (25.4, 0, "1.000"),
            (25.4 * 1.234, 2, "1.234"),
            (25.4 * 1.2345, 0, "1.2345"),
            (-25.4 * 1.2345, 3, "-1.2345"),
            (25.4 * 0.001, 2, "0.001"),
            (25.4 * (1.0 + 1e-12), 2, "1.000"),
            (25.4, 4, "1.0000"),
            (25.4, 5, "1.00000"),
            (0.0, 0, "0.000"),
        ] {
            assert_eq!(length_number(mm, "in", decimals), expected, "{mm} mm, {decimals} places");
        }
    }

    #[test]
    fn precision_uses_the_converted_value_and_caps_requested_places() {
        assert_eq!(length_precision(1.2345, "in", 2), 4);
        assert_eq!(length_precision(1.234, "in", 2), 3);
        assert_eq!(length_precision(1.2345, "mm", 2), 2);
        assert_eq!(length_precision(1.2345, "in", 6), 6);
        assert_eq!(length_precision(1.2345, "in", u8::MAX), 8);
        assert_eq!(length_precision(1.2345, "mm", u8::MAX), 8);
        assert_eq!(length_number(12.5, "mm", u8::MAX), "12.50000000");
        assert_eq!(length_number(25.4, "in", u8::MAX), "1.00000000");
    }

    #[test]
    fn rounding_and_signed_zero_keep_the_existing_display_policy() {
        for (mm, decimals, expected) in [
            (0.0, 2, "0.00"),
            (-0.0, 2, "-0.00"),
            (-0.0001, 2, "-0.00"),
            (-12.5, 2, "-12.50"),
            (12.5, 0, "12"),
            (13.5, 0, "14"),
            (-12.5, 0, "-12"),
            (-13.5, 0, "-14"),
            (1.25, 1, "1.2"),
            (1.75, 1, "1.8"),
        ] {
            assert_eq!(length_number(mm, "mm", decimals), expected, "{mm} mm, {decimals} places");
        }
        assert_eq!(format_length(-0.0, "in", 2), "-0.000 in");
    }

    #[test]
    fn unknown_display_units_keep_the_millimetre_fallback() {
        for units in ["", "inch", " in ", "IN", "unsupported"] {
            assert_eq!(unit_scale(units), (1.0, "mm"), "{units}");
            assert_eq!(length_number(25.4, units, 2), "25.40", "{units}");
            assert_eq!(format_length(25.4, units, 2), "25.40 mm", "{units}");
        }
    }

    #[test]
    fn non_finite_text_is_preserved_without_claiming_validation() {
        assert_eq!(length_number(f64::NAN, "mm", 2), "NaN");
        assert_eq!(format_length(f64::INFINITY, "in", u8::MAX), "inf in");
        assert_eq!(format_length(f64::NEG_INFINITY, "cm", 2), "-inf cm");
    }
}
