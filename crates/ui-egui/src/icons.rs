//! Stable UI rendering boundary for the icon artwork. Compiled only at L5.
//!
//! Command, menu, browser and timeline icons are the SVG masters in `assets/icons/ui/design/svg`,
//! rendered by [`crate::icon_svg`]: colours come from `palette.toml` (light and dark, and the
//! accents), never from the caller. An icon with no master yet draws the placeholder.
//! The glyph, marker, navigation and pointer sources below are not command icons.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egui::{Color32, Painter, Rect, TextureHandle, TextureOptions, pos2};

use crate::icon_svg::{self, Look};

#[path = "../../../assets/icons/ui/source/glyphs.rs"]
pub(crate) mod glyphs;
#[path = "../../../assets/icons/ui/source/markers.rs"]
pub(crate) mod markers;
#[path = "../../../assets/icons/ui/source/navigation.rs"]
pub(crate) mod navigation;
#[path = "../../../assets/icons/ui/source/pointer.rs"]
pub(crate) mod pointer;

/// Textures already rasterised: one per (master, look, pixel size).
#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<(&'static str, Look, u32), TextureHandle>>>);

/// Draw the icon `name` in `r`. The ink, fill and accent colours the callers pass are no longer
/// used: the palette decides. Only the ink's opacity is kept, so a dimmed (disabled) button dims
/// its icon.
pub fn paint(p: &Painter, r: Rect, name: &str, ink: Color32, _fill: Color32, _accent: Color32) {
    paint_icon(p, r, name, f32::from(ink.a()) / 255.0);
}

/// [`paint`] with an explicit opacity (0 to 1).
pub fn paint_icon(p: &Painter, r: Rect, name: &str, opacity: f32) {
    paint_on(p, r, name, None, opacity);
}

/// Draw the icon `name` for a surface that is always dark (the application bar), whatever the
/// theme: `dark: Some(true)`. `None` follows the theme.
pub fn paint_on(p: &Painter, r: Rect, name: &str, dark: Option<bool>, opacity: f32) {
    let ctx = p.ctx();
    let ppp = ctx.pixels_per_point().max(0.5);
    let side = r.width().min(r.height());
    let px = (side * ppp).round().clamp(8.0, 512.0) as u32;
    let resolved = icon_svg::resolve(name);
    let look = Look { dark: dark.unwrap_or_else(|| ctx.global_style().visuals.dark_mode), accent: resolved.accent, vision: crate::theme::vision() };
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("sc_icon_cache"), Cache::default).clone());
    let Ok(mut map) = cache.0.lock() else { return };
    let key = (resolved.master, look, px);
    if let std::collections::hash_map::Entry::Vacant(slot) = map.entry(key) {
        let Some(doc) = icon_svg::doc_for(resolved.master, px) else { return };
        let Some(pm) = icon_svg::render(doc, look, px) else { return };
        let image = egui::ColorImage::from_rgba_premultiplied([px as usize, px as usize], pm.data());
        slot.insert(ctx.load_texture(format!("icon:{}:{px}", resolved.master), image, TextureOptions::LINEAR));
    }
    let Some(tex) = map.get(&key) else { return };
    // Snap the image to whole physical pixels so it stays crisp.
    let size = px as f32 / ppp;
    let min = pos2(((r.center().x - size / 2.0) * ppp).round() / ppp, ((r.center().y - size / 2.0) * ppp).round() / ppp);
    let tint = Color32::from_white_alpha((opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
    p.image(tex.id(), Rect::from_min_size(min, egui::vec2(size, size)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{self, ColorVision};

    #[test]
    fn actual_uploaded_icon_pixels_follow_semantics_and_vision_cache_changes() {
        let ctx = egui::Context::default();
        let previous = theme::vision();
        for dark in [false, true] {
            for vision in [ColorVision::Off, ColorVision::Protan, ColorVision::Tritan] {
                theme::set_vision(vision);
                for icon in ["line", "c_parallel", "construction", "centerline", "surface_extrude", "mesh_export", "contact_new"] {
                    let expected = if icon == "construction" {
                        icon_svg::role_colour("guide", Look { dark, accent: None, vision })
                    } else {
                        icon_svg::primary(icon, dark, vision)
                    };
                    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                        let r = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0));
                        paint_on(ui.painter(), r, icon, Some(dark), 1.0);
                    });
                    assert!(
                        output.textures_delta.set.iter().any(|(_, deltas)| deltas.iter().any(|delta| match &delta.image {
                            egui::ImageData::Color(image) => image.pixels.contains(&expected),
                        })),
                        "{icon}, {dark}, {vision:?}: actual texture misses its semantic stroke or reused a stale cache entry"
                    );
                    if icon == "contact_new" {
                        let image = output
                            .textures_delta
                            .set
                            .values()
                            .flatten()
                            .find_map(|delta| match &delta.image {
                                egui::ImageData::Color(image) if image.size == [64, 64] => Some(image),
                                _ => None,
                            })
                            .unwrap();
                        assert_eq!(image.pixels.get(48 * 64 + 48), Some(&Color32::TRANSPARENT), "uploaded plus must stay transparent");
                    }
                    // This headless test inspects uploads instead of sending them
                    // to a GPU; acknowledge the deltas before they are dropped.
                    output.textures_delta.clear();
                }
            }
        }
        theme::set_vision(previous);
    }
}
