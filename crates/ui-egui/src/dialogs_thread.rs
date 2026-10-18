//! Editable metric thread dimensions and optional presets write the existing designation
//! parameter. Browsing choices does not edit the model or freeze an automatic feature.

use solidcraft_engine::doc::{metric_thread_sizes, parse_metric_thread};

use crate::dialogs::{FIELD_W, check, row_label};
use crate::i18n::tr;

/// Common pitch presets, not an exhaustive catalogue of standard diameter/pitch pairs.
/// The coarse and current pitches are included separately; custom pairs remain editable.
const PITCHES: &[f64] =
    &[0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.6, 0.7, 0.75, 0.8, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0];

// Additional coarse diameter/pitch pairs from ISO 261:1998, table 2:
// https://www.iso.org/standard/4165.html
// Keep UI presets separate from the document's historical automatic sizing table:
// expanding that table would change the geometry of existing automatic features.
const EXTRA_COARSE: &[(f64, f64)] = &[
    (1.0, 0.25),
    (1.1, 0.25),
    (1.2, 0.25),
    (1.4, 0.3),
    (1.8, 0.35),
    (2.2, 0.45),
    (3.5, 0.6),
    (4.5, 0.75),
    (7.0, 1.0),
    (9.0, 1.25),
    (11.0, 1.5),
    (33.0, 3.5),
    (39.0, 4.0),
    (45.0, 4.5),
    (52.0, 5.0),
    (60.0, 5.5),
    (64.0, 6.0),
    (68.0, 6.0),
];

// Whole-millimetre convenience values avoid gaps left by a coarse-only catalogue.
// This is a browsable range, never a limit or a list of standard diameter/pitch pairs.
fn diameter_presets() -> Vec<f64> {
    let mut diameters: Vec<f64> = (1..=100).map(f64::from).chain(metric_thread_sizes().iter().chain(EXTRA_COARSE).map(|(d, _)| *d)).collect();
    diameters.sort_by(f64::total_cmp);
    diameters.dedup();
    diameters
}

fn coarse_pitch(diameter: f64) -> Option<f64> {
    metric_thread_sizes().iter().chain(EXTRA_COARSE).find(|(d, _)| *d == diameter).map(|(_, p)| *p)
}

/// New external threads snap to the nearest fitting preset. Persist a resolved pair
/// when the historical document default differs, so preview, OK and reload agree.
/// Internal faces retain tap-drill sizing: their measured diameter is not the major size.
pub(crate) fn suggestion(s: &solidcraft_engine::Session, pick: &solidcraft_engine::Sel) -> Option<(String, bool)> {
    let solidcraft_engine::Sel::Face { body, point, .. } = pick else { return None };
    let state = s.world_state();
    let legacy = solidcraft_engine::doc::suggested_thread(&state, *point).ok();
    let cylinder = solidcraft_engine::kernel::cylinder_face_at(&state.body(body)?.body, *point)?;
    if cylinder.internal {
        return legacy.map(|size| (size, false));
    }
    // Remove sub-micron roundoff from analytic surfaces (e.g. 43.999999999999986).
    let diameter = (cylinder.radius * 2.0 * 1e6).round() / 1e6;
    let diameter = diameter_presets()
        .into_iter()
        .min_by(|a, b| (a - diameter).abs().total_cmp(&(b - diameter).abs()))
        .filter(|d| (d - diameter).abs() <= 0.15 * diameter && (d - diameter).abs() < 0.15 * d)
        .unwrap_or(diameter);
    let pitch = coarse_pitch(diameter).unwrap_or(1.5_f64.min(diameter / 2.0));
    let size = format!("M{diameter}x{pitch}");
    let pair = parse_metric_thread(&size)?;
    if let Some(legacy) = legacy
        && parse_metric_thread(&legacy) == Some(pair)
    {
        Some((legacy, false))
    } else {
        Some((size, true))
    }
}

/// Keep incomplete input while typing. The feature still stores only its designation;
/// suggestions, presets and direct designation edits refresh these fields.
#[derive(Clone, Debug, Default)]
pub struct Dimensions {
    source: String,
    diameter: String,
    pitch: String,
}

impl Dimensions {
    fn sync(&mut self, designation: &str) {
        if self.source == designation {
            return;
        }
        self.source = designation.into();
        (self.diameter, self.pitch) = parse_metric_thread(designation).map(|(d, p)| (format!("{d} mm"), format!("{p} mm"))).unwrap_or_default();
    }

    fn write(&mut self, designation: &mut String, suggested: &mut Option<String>) {
        let number = |text: &str| crate::prefs::parse_length_mm(text, 1.0).map(|n| n.to_string()).unwrap_or_else(|| text.trim().into());
        *designation = format!("M{}x{}", number(&self.diameter), number(&self.pitch));
        // Even incomplete or invalid input must be sent to validation, never replaced by
        // an automatic size or the last valid pair when the user presses OK.
        self.source = designation.clone();
        *suggested = None;
    }
}

/// A directly editable metric length with a separate menu of optional presets.
/// Returns (text changed, preset chosen, Enter pressed).
fn dimension(ui: &mut egui::Ui, label: &str, text: &mut String, presets: &[f64]) -> (bool, Option<f64>, bool) {
    row_label(ui, label);
    let result = ui.horizontal(|ui| {
        let width = FIELD_W - 28.0 - ui.spacing().item_spacing.x;
        let input = ui.add(egui::TextEdit::singleline(text).id_salt((label, "value")).desired_width(width).hint_text("mm"));
        crate::scenario::publish_handle(&format!("dialog:{label}"), input.rect.center());
        let mut chosen = None;
        let was_open = egui::ComboBox::is_open(ui.ctx(), ui.make_persistent_id((label, "presets")));
        let menu = egui::ComboBox::from_id_salt((label, "presets")).width(24.0).height(480.0).selected_text("").show_ui(ui, |ui| {
            let current = crate::prefs::parse_length_mm(text, 1.0);
            for &value in presets {
                let label_value = format!("{value} mm");
                let item = ui.selectable_label(current == Some(value), &label_value);
                if !was_open && current == Some(value) {
                    item.scroll_to_me_animation(Some(egui::Align::Center), egui::style::ScrollAnimation::none());
                }
                crate::scenario::publish_handle(&format!("dialog:{label}:{label_value}"), item.rect.center());
                if item.clicked() {
                    chosen = Some(value);
                    ui.close();
                }
            }
        });
        crate::scenario::publish_handle(&format!("dialog:{label} presets"), menu.response.rect.center());
        menu.response.on_hover_text(crate::i18n::tr_label("Preset"));
        (input.changed(), chosen, input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
    });
    ui.end_row();
    result.inner
}

/// Returns (Enter pressed, automatic sizing requested).
pub(crate) fn rows(ui: &mut egui::Ui, designation: &mut String, suggested: &mut Option<String>, dimensions: &mut Dimensions) -> (bool, bool) {
    row_label(ui, "Profile");
    ui.label(tr("ISO metric"));
    ui.end_row();

    row_label(ui, "Automatic size");
    let before = designation.trim().is_empty() || suggested.as_deref() == Some(designation.as_str());
    let mut automatic = before;
    ui.add_enabled_ui(!designation.trim().is_empty(), |ui| check(ui, &mut automatic));
    let reset = automatic && !before;
    if automatic != before {
        *suggested = None;
        if automatic {
            designation.clear();
        }
    }
    ui.end_row();

    dimensions.sync(designation);
    let current = parse_metric_thread(designation);
    let mut diameters = diameter_presets();
    if let Some((d, _)) = current
        && !diameters.contains(&d)
    {
        diameters.push(d);
    }
    diameters.sort_by(f64::total_cmp);
    diameters.dedup();
    let (changed, selected, mut enter) = dimension(ui, "Size", &mut dimensions.diameter, &diameters);
    if let Some(d) = selected {
        dimensions.diameter = format!("{d} mm");
        // Start with coarse pitch where defined, otherwise a convenient fine pitch.
        // Fine pitch selection: ISO 261:1998, section 5.4; M44x1.5 is covered by DIN 13-6:1999-11.
        // The two menus are independent values, not a catalogue of standard pairs.
        // Typing a diameter keeps the authored pitch; neither field is limited to presets.
        if current.is_none_or(|(n, _)| n != d) {
            let pitch = coarse_pitch(d).unwrap_or(1.5_f64.min(d / 2.0));
            dimensions.pitch = format!("{pitch} mm");
        }
    }
    if changed || selected.is_some() {
        dimensions.write(designation, suggested);
    }

    let current = parse_metric_thread(designation);
    let diameter = crate::prefs::parse_length_mm(&dimensions.diameter, 1.0);
    let coarse = diameter.and_then(coarse_pitch);
    let mut pitches: Vec<f64> = PITCHES.iter().copied().filter(|p| diameter.is_none_or(|d| *p < d) && coarse.is_none_or(|c| *p <= c)).collect();
    pitches.extend(coarse);
    pitches.extend(current.map(|(_, p)| p));
    pitches.sort_by(|a, b| b.total_cmp(a));
    pitches.dedup();
    let (changed, selected, submitted) = dimension(ui, "Pitch", &mut dimensions.pitch, &pitches);
    enter |= submitted;
    if let Some(p) = selected {
        dimensions.pitch = format!("{p} mm");
    }
    if changed || selected.is_some() {
        dimensions.write(designation, suggested);
    }

    row_label(ui, "Designation");
    let response = ui.add(egui::TextEdit::singleline(designation).hint_text(tr("fits the face, e.g. M8")));
    crate::scenario::publish_handle("dialog:thread_size", response.rect.center());
    if response.changed() {
        *suggested = None;
    }
    enter |= response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.end_row();
    (enter, reset)
}
