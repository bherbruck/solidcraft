//! Colours and widget style: a dark theme (the default, a blue-grey in the spirit of desktop CAD
//! tools, viewport #2e3440) and a light one, each with colour vision modes. Every colour the UI
//! uses comes from [`Tokens`].

use std::cell::Cell;

#[path = "../../../assets/styles/ui/palettes.rs"]
mod palettes;

use egui::{Color32, Stroke};
use serde::{Deserialize, Serialize};

thread_local! {
    /// Theme state belongs to the UI thread drawing the frame. Concurrent app/test renderers
    /// must not change one another's palette while a frame or icon texture is being built.
    static DARK: Cell<bool> = const { Cell::new(true) };
    /// The colour vision mode, per thread. Each app
    /// sets it from its own preferences at the start of every frame (`SolveApp::logic`), on the
    /// thread that draws it. Tests run many apps side by side on their own threads in one process:
    /// with one global, a test in a mode changed the colours under another one
    /// (`hover_stays_off_during_spin_and_animated_views` failed that way). No worker thread reads
    /// it: the preview thread is handed its colours (`preview::Colors`) and thumbnails are drawn by
    /// the engine in the model's colours, so a new thread starts Off and stays Off.
    static VISION: Cell<ColorVision> = const { Cell::new(ColorVision::Off) };
}

/// Is the dark theme on?
pub fn is_dark() -> bool {
    DARK.get()
}

/// The common conversion boundary for compiled icon palette data.
pub(crate) const fn icon_color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// Colour vision modes (#778): a second set of values for the colours that carry a meaning, for
/// a weak or missing red (protan), green (deutan) or blue (tritan) sense. Not a filter: Off gives
/// back the theme's colours unchanged. Protan and Deutan share one red-green table, measured
/// clear for both (`colour_vision_tests`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorVision {
    #[default]
    Off,
    Protan,
    Deutan,
    Tritan,
}

impl ColorVision {
    pub const ALL: [ColorVision; 4] = [ColorVision::Off, ColorVision::Protan, ColorVision::Deutan, ColorVision::Tritan];

    /// The saved value (`ui.prefs` `color_vision`).
    pub fn id(self) -> &'static str {
        match self {
            ColorVision::Off => "off",
            ColorVision::Protan => "protan",
            ColorVision::Deutan => "deutan",
            ColorVision::Tritan => "tritan",
        }
    }

    pub fn from_id(id: &str) -> Option<ColorVision> {
        ColorVision::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Its name in the Preferences (a message id).
    pub fn label_id(self) -> &'static str {
        match self {
            ColorVision::Off => "ui.colour_vision_off",
            ColorVision::Protan => "ui.colour_vision_protan",
            ColorVision::Deutan => "ui.colour_vision_deutan",
            ColorVision::Tritan => "ui.colour_vision_tritan",
        }
    }

    /// Who it is for, in one line (its tooltip; a message id).
    pub fn help_id(self) -> &'static str {
        match self {
            ColorVision::Off => "ui.colour_vision_off_tip",
            ColorVision::Protan => "ui.colour_vision_protan_tip",
            ColorVision::Deutan => "ui.colour_vision_deutan_tip",
            ColorVision::Tritan => "ui.colour_vision_tritan_tip",
        }
    }
}

/// From saved preferences: a value this version doesn't know (a later version's mode, a hand
/// edit) is Off rather than an error, which would lose every other saved preference with it.
/// `ui.prefs` refuses such a value before it gets here (`prefs::set`).
impl<'de> Deserialize<'de> for ColorVision {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(v.as_str().and_then(ColorVision::from_id).unwrap_or_default())
    }
}

/// The colour vision mode on now (on this thread).
pub fn vision() -> ColorVision {
    VISION.with(Cell::get)
}

/// Switch the colour vision mode (the visuals follow on the next [`apply`]).
pub fn set_vision(mode: ColorVision) {
    VISION.with(|v| v.set(mode));
}

/// An sRGB colour given as 0..1 (the analysis colours), for drawing in the UI.
pub fn srgb(c: [f32; 3]) -> Color32 {
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(byte(c[0]), byte(c[1]), byte(c[2]))
}

/// Surface analysis colours, straight sRGB 0..1 as the shader takes them (exact, so they read
/// the same as when they were literals in the WGSL). The light theme's are the headless default.
pub use solidcraft_engine::render::AnalysisColors;

/// The colours icons draw of their own (`icons::paint` takes ink, fill and accent from its
/// caller). Every icon colour is a token so a colour vision mode can reach it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconColors {
    /// Surface sheets: a sheet, a second sheet beside it, a lighter sheet, iso lines and teeth.
    pub sheet: Color32,
    pub sheet_2: Color32,
    pub sheet_light: Color32,
    pub sheet_line: Color32,
    /// Highlights (and the white ink of markers and the agent pointer), and the sphere's glint.
    pub highlight: Color32,
    pub glint: Color32,
    pub folder: Color32,
    pub plane: Color32,
    pub fold: Color32,
    /// Zebra stripes, and the environment map's sky and floor.
    pub stripe: Color32,
    pub sky: Color32,
    pub floor: Color32,
    /// Appearance: red, blue and yellow paint dabs.
    pub paint: [Color32; 3],
    /// Origin: X, Y and Z arrows.
    pub axes: [Color32; 3],
    /// Accessibility: reachable, blocked. Draft: positive, between. Curvature: flat, middle,
    /// tight.
    pub access: [Color32; 2],
    pub draft: [Color32; 2],
    pub curvature: [Color32; 3],
    pub interference: Color32,
    pub warning: Color32,
    /// The pointer an AI agent moves.
    pub agent: Color32,
    /// The dark halo under viewport markers (snap points).
    pub marker_halo: Color32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub app_bar: Color32,
    pub toolbar: Color32,
    pub panel: Color32,
    pub panel_header: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub sketch_accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub icon: Color32,
    pub icon_fill: Color32,
    pub timeline: Color32,
    pub hover: Color32,
    // ---- viewport selection feedback ----
    /// Selected face (flat fill).
    pub sel_face: Color32,
    /// Selected edge core and its lighter rim.
    pub sel_edge: Color32,
    pub sel_edge_rim: Color32,
    /// Hovered edge: dark core with a light halo.
    pub hover_edge: Color32,
    pub hover_edge_halo: Color32,
    /// How much lighter a hovered face is (grey levels).
    pub hover_face_lift: u8,
    /// Selected profile fill and outline.
    pub sel_profile: Color32,
    pub sel_profile_edge: Color32,
    /// Hovered profile: faint tint and a bright outline.
    pub hover_profile: Color32,
    pub hover_profile_edge: Color32,
    /// Selected vertex dot.
    pub sel_vertex: Color32,
    // ---- origin ----
    pub origin_plane: Color32,
    pub origin_plane_edge: Color32,
    pub origin_plane_hover: Color32,
    pub origin_point: Color32,
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
    /// Construction planes.
    pub construction_plane: Color32,
    /// Construction axes and points.
    pub construction_geom: Color32,
    // ---- bodies and live previews ----
    pub body: Color32,
    pub body_edge: Color32,
    /// Command manipulators (direction arrows, handles).
    pub manipulator: Color32,
    /// New surface in a feature preview.
    pub preview_add: Color32,
    /// The surface of a body a feature preview creates (New Body), apart from a join (#1805).
    pub preview_new: Color32,
    /// Proposed sketch geometry, distinct from blue selections.
    pub preview_sketch: Color32,
    /// Removed material in a feature preview (translucent).
    pub preview_cut: Color32,
    /// Navigation cube faces, outline and labels.
    pub cube_face: Color32,
    pub cube_border: Color32,
    pub cube_text: Color32,
    /// Light blue hover fill, readable on the pale cube in either theme.
    pub cube_hover: Color32,
    // ---- viewport ----
    /// Viewport background (top and bottom of the gradient; equal for a flat fill).
    pub viewport_top: Color32,
    pub viewport_bottom: Color32,
    pub grid: Color32,
    pub grid_major: Color32,
    /// A whole selected body.
    pub body_selected: Color32,
    /// Sketch curves: under-constrained, fully constrained (and finished sketches).
    pub sketch: Color32,
    pub sketch_fixed: Color32,
    /// Fixed (locked) sketch geometry.
    pub sketch_locked: Color32,
    /// Sketch dimensions: driving, driven (reference).
    pub dimension: Color32,
    pub dimension_driven: Color32,
    /// Sketch points: fully constrained, free.
    pub sketch_point: Color32,
    pub sketch_point_free: Color32,
    /// Command dialogs: panel, border, and the filled selection chip.
    pub dialog_bg: Color32,
    pub dialog_border: Color32,
    pub chip: Color32,
    /// Band behind a panel's heading and, lighter, behind its section headings (both set off
    /// from `dialog_bg`).
    pub heading_bg: Color32,
    pub subheading_bg: Color32,
    /// Inside of bodies cut by a section.
    pub section_cap: Color32,
    /// Closed sketch profiles (translucent fill).
    pub profile_fill: Color32,
    /// Constraint glyph boxes.
    pub glyph_bg: Color32,
    pub glyph_edge: Color32,
    pub glyph_text: Color32,
    /// Chips drawn over the viewport (status, navigation bar, dimension labels).
    pub overlay: Color32,
    /// Rubber band of the shape being drawn.
    pub rubber_band: Color32,
    /// Light text on the application bar.
    pub app_bar_text: Color32,
    /// A hovered tab on the application bar (keeps `app_bar_text` readable).
    pub app_bar_hover: Color32,
    /// Timeline items: normal and failed.
    pub timeline_item: Color32,
    pub timeline_item_error: Color32,
    pub timeline_marker: Color32,
    /// Button and field backgrounds on panels.
    pub field: Color32,
    // ---- box selection ----
    pub box_window: Color32,
    pub box_crossing: Color32,
    // ---- colours that came from the engine or were drawn in place ----
    /// Sketch curves the engine tags (`view::colors`): projected, lost link, construction,
    /// centerline.
    pub sketch_projected: Color32,
    pub sketch_lost: Color32,
    pub sketch_construction: Color32,
    pub sketch_centerline: Color32,
    /// The view triad's X, Y and Z (and the CPU scene's origin axes).
    pub triad: [Color32; 3],
    /// The move gizmo's X, Y and Z.
    pub gizmo: [Color32; 3],
    /// Sheet metal flat pattern: bend up, bend down.
    pub bend_up: Color32,
    pub bend_down: Color32,
    /// Interference: the clash fill, before its shading.
    pub interference: Color32,
    /// Continuity Analysis seams, best to worst as `inspect.continuity` judges them: smooth (G2),
    /// tangent with a curvature jump (G1), a crease (G0) and a gap. Bluish green, blue, yellow
    /// and reddish purple stay apart in every colour vision mode, so no mode changes them.
    pub continuity: [Color32; 4],
    pub analysis: AnalysisColors,
    pub icons: IconColors,
    /// The application bar's buttons: ink, fill and accent of their icons.
    pub app_bar_icon: [Color32; 3],
    /// The SolidCraft mark on the application bar: light, mid and dark faces, and its edge.
    pub logo: [Color32; 4],
}

impl Tokens {
    /// The tokens of the current theme and colour vision mode.
    pub fn get() -> Tokens {
        Tokens::themed(is_dark(), vision())
    }

    /// The tokens of a theme and colour vision mode.
    pub fn themed(dark: bool, mode: ColorVision) -> Tokens {
        (if dark { Tokens::dark() } else { Tokens::light() }).with_vision(dark, mode)
    }

    /// These tokens with a colour vision mode's values on top (the tables in `palettes.rs`); Off
    /// gives them back unchanged.
    pub fn with_vision(self, dark: bool, mode: ColorVision) -> Tokens {
        match mode {
            ColorVision::Off => self,
            ColorVision::Protan | ColorVision::Deutan => self.red_green(dark),
            ColorVision::Tritan => self.blue_yellow(dark),
        }
    }

    /// Fade idle cube colours into the viewport while retaining opaque facets.
    pub fn navigation_cube(self, active: bool) -> Tokens {
        if active {
            return self;
        }
        let mut t = self;
        t.cube_face = t.cube_face.lerp_to_gamma(t.viewport_top, 0.55);
        t.cube_border = t.cube_border.lerp_to_gamma(t.viewport_top, 0.55);
        t.cube_text = t.cube_text.lerp_to_gamma(t.viewport_top, 0.55);
        t.axis_x = t.axis_x.lerp_to_gamma(t.viewport_top, 0.55);
        t.axis_y = t.axis_y.lerp_to_gamma(t.viewport_top, 0.55);
        t.axis_z = t.axis_z.lerp_to_gamma(t.viewport_top, 0.55);
        t
    }
}

/// How a physical material looks (the default appearance follows the material).
pub fn material_color(name: &str) -> Option<Color32> {
    solidcraft_engine::doc::material_color(name).map(|[r, g, b]| Color32::from_rgb(r, g, b))
}

/// Switch themes (the visuals follow on the next [`apply`]).
pub fn set_dark(dark: bool) {
    DARK.set(dark);
}

fn visuals(dark: bool) -> egui::Visuals {
    let t = Tokens::themed(dark, vision());
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.panel_header;
    v.override_text_color = Some(t.text);
    v.widgets.inactive.weak_bg_fill = t.panel_header;
    v.widgets.inactive.bg_fill = t.panel_header;
    v.widgets.active.weak_bg_fill = t.accent_soft;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_fill = t.hover;
    // egui uses the open-state fill for a movable window's active title bar.
    v.widgets.open.weak_bg_fill = t.panel_header;
    v.window_corner_radius = egui::CornerRadius::same(6);
    v
}

pub fn apply(ctx: &egui::Context) {
    let dark = is_dark();
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_visuals(visuals(dark));
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(6.0, 3.0);
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn concurrent_ui_renderers_keep_their_own_theme_and_vision() {
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for (dark, vision) in [(false, super::ColorVision::Off), (true, super::ColorVision::Protan)] {
                let barrier = &barrier;
                scope.spawn(move || {
                    super::set_dark(dark);
                    super::set_vision(vision);
                    barrier.wait();
                    assert_eq!(super::is_dark(), dark);
                    assert_eq!(super::vision(), vision);
                    assert_eq!(super::Tokens::get().sketch_accent, super::Tokens::themed(dark, vision).sketch_accent);
                });
            }
        });
    }

    /// Title and section bands stand out from a dialog, the title more than its sections.
    #[test]
    fn heading_bands_are_darker_than_the_dialog_in_order() {
        let luma = |c: egui::Color32| u32::from(c.r()) * 3 + u32::from(c.g()) * 6 + u32::from(c.b());
        for t in [super::Tokens::light(), super::Tokens::dark()] {
            assert!(luma(t.heading_bg) < luma(t.subheading_bg), "{t:?}");
            assert!(luma(t.subheading_bg) < luma(t.dialog_bg), "{t:?}");
        }
    }

    #[test]
    fn active_movable_window_header_uses_theme_panel_header() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
            ctx.set_visuals(super::visuals(dark));
            let mut output = egui::FullOutput::default();
            for _ in 0..3 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                    ..Default::default()
                };
                output = ctx.run_ui(raw, |ui| {
                    crate::frame::window(ui.ctx(), crate::i18n::tr_label("Parameters"), crate::frame::Width::Normal).fade_in(false).show(
                        ui.ctx(),
                        |ui| {
                            ui.label(crate::i18n::tr_label("Parameter values"));
                        },
                    );
                });
                output.textures_delta.clear();
            }
            let title = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Parameters" => Some(text.pos + text.galley.rect.center().to_vec2()),
                    _ => None,
                })
                .expect("window title must be drawn");
            let header = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill.a() > 0 && rect.rect.contains(title) => Some(rect),
                    _ => None,
                })
                .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
                .expect("window header must be drawn behind its title");
            let expected = if dark { super::Tokens::dark() } else { super::Tokens::light() };
            assert_eq!(header.fill, expected.panel_header, "active title bar should follow the {dark} palette");
        }
    }
}
