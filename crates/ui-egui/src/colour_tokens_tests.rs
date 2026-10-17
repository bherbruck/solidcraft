//! Every colour that carries a meaning comes from `theme::Tokens` (#778): the engine's sketch and
//! axis tags, the surface analysis colours and every colour an icon draws. These tests hold that
//! the tokens equal what was drawn before, and that a new colour cannot skip them.

use egui::Color32;
use solidcraft_engine::render::Rgb;
use solidcraft_engine::view::colors;

use crate::theme::Tokens;

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// The new tokens hold exactly the values that were drawn in place before they were tokens, so
/// with no colour vision mode on, nothing on screen changes.
#[test]
fn moved_colours_keep_the_values_they_were_drawn_with() {
    for t in [Tokens::light(), Tokens::dark()] {
        assert_eq!(t.sketch_projected, rgb(colors::SKETCH_PROJECTED));
        assert_eq!(t.sketch_lost, rgb(colors::SKETCH_LOST));
        assert_eq!(t.sketch_construction, rgb(colors::SKETCH_CONSTRUCTION));
        assert_eq!(t.sketch_centerline, rgb(colors::SKETCH_CENTERLINE));
        assert_eq!(t.triad, [rgb(colors::AXIS_X), rgb(colors::AXIS_Y), rgb(colors::AXIS_Z)]);
        assert_eq!(t.gizmo, [Color32::from_rgb(225, 70, 60), Color32::from_rgb(80, 190, 80), Color32::from_rgb(70, 130, 240)]);
        assert_eq!(t.interference, Color32::from_rgb(235, 45, 40));
        // The shader's former literals, bit for bit.
        assert_eq!(t.analysis.draft, [[0.20, 0.75, 0.30], [0.95, 0.80, 0.15], [0.85, 0.22, 0.20]]);
        assert_eq!(t.analysis.access, [[0.20, 0.75, 0.30], [0.85, 0.22, 0.20]]);
        assert_eq!(t.analysis.curvature, [[0.15, 0.35, 0.95], [0.10, 0.80, 0.25], [0.90, 0.15, 0.10]]);
        assert!(!t.analysis.hatch);
        assert_eq!(t.app_bar_icon, [Color32::from_rgb(225, 230, 238), Color32::from_rgb(120, 140, 170), Color32::from_rgb(120, 190, 255)]);
        assert_eq!(t.logo, [Color32::from_rgb(96, 158, 236), Color32::from_rgb(58, 136, 238), Color32::from_rgb(55, 108, 197), Color32::WHITE]);
        assert_eq!(t.icons.glint, Color32::from_white_alpha(160));
        assert_eq!(t.icons.marker_halo, Color32::from_black_alpha(150));
        assert_eq!(t.icons.agent, Color32::from_rgb(168, 85, 247));
    }
    let (l, d) = (Tokens::light(), Tokens::dark());
    assert_eq!((l.bend_up, l.bend_down), (Color32::from_rgb(30, 140, 50), Color32::from_rgb(200, 90, 20)));
    assert_eq!((d.bend_up, d.bend_down), (Color32::from_rgb(110, 200, 120), Color32::from_rgb(240, 150, 80)));
}

/// The analysis colours reach the shader in the order its `ana_c` array reads them.
#[test]
fn analysis_colours_reach_the_shader_in_its_order() {
    let a = Tokens::light().analysis;
    let v = crate::gpu::analysis_colors(&a);
    let at = |i: usize| [v[4 * i], v[4 * i + 1], v[4 * i + 2]];
    assert_eq!([at(0), at(1), at(2)], a.draft);
    assert_eq!([at(3), at(4)], a.access);
    assert_eq!([at(5), at(6), at(7)], a.curvature);
    assert!((0..8).all(|i| v[4 * i + 3] == 1.0));
    assert_eq!(v[32], 0.0, "no stripes without a colour vision mode");
    let mut striped = a;
    striped.hatch = true;
    assert_eq!(crate::gpu::analysis_colors(&striped)[32], 1.0);
}

// Icons no longer take their colours from the caller or from `IconColors`: the SVG masters name
// palette roles (`assets/icons/ui/design/palette.toml`, light and dark, plus accents) that
// `icon_svg` resolves. `icon_svg`'s own tests hold that every master parses and that only the
// palette's colours exist; the colour vision modes do not reach them yet.

/// The most colour literals each UI file may hold, and why. None of them carries a meaning that a
/// colour vision mode must change. A file with more fails here: make the new colour a token in
/// `theme::Tokens` (and `assets/styles/ui/palettes.rs`) instead, or, if it carries no meaning,
/// raise the file's count here and give the reason. Fewer is always fine. The icon sources may
/// hold none at all.
const ALLOWED_LITERALS: &[(&str, usize, &str)] = &[
    ("navigation.rs", 1, "the navigation cube's face shading, made from a token"),
    ("canvas.rs", 3, "white rings around manipulator handles"),
    ("context_menu.rs", 1, "the radial menu's shadow"),
    ("dialogs_appearance.rs", 1, "a design appearance: the colour is the fact"),
    ("dialogs_save_mesh.rs", 1, "an installed slicer's own icon, drawn untinted"),
    ("dialogs_library.rs", 1, "a library appearance's swatch: the colour is the fact"),
    ("dialogs_assembly.rs", 1, "the interference fill, shaded from its token"),
    ("dialogs_motion.rs", 1, "a white outline"),
    ("documents.rs", 4, "white hover overlays on document tabs"),
    ("frame.rs", 1, "the window shadow"),
    ("gizmo.rs", 2, "the gizmo's neutral grey ball"),
    ("icons.rs", 1, "the white tint that carries an icon image's opacity"),
    ("home.rs", 4, "white ink on accent buttons, and thumbnail tints"),
    ("ref_images.rs", 1, "a reference image's opacity tint"),
    ("sketch_palette.rs", 1, "white text on the accent button"),
    ("sketch_text.rs", 1, "white text on the selection chip"),
    ("titlebar.rs", 4, "the window close button's red hover (the system's convention), white hovers and ink"),
    ("toolbar.rs", 3, "white hover overlays and the version text"),
];

const COLOUR_LITERALS: &[&str] = &[
    "Color32::from_rgb(",
    "Color32::from_rgba_unmultiplied(",
    "Color32::from_rgba_unmultiplied_const(",
    "Color32::from_rgba_premultiplied(",
    "Color32::from_gray(",
    "Color32::from_black_alpha(",
    "Color32::from_white_alpha(",
    "Color32::from_hex(",
    "Color32::WHITE",
    "Color32::BLACK",
    "Color32::RED",
    "Color32::GREEN",
    "Color32::BLUE",
    "Color32::YELLOW",
    "Color32::GOLD",
    "Color32::ORANGE",
    "Color32::BROWN",
    "Color32::KHAKI",
    "Color32::GRAY",
    "Color32::LIGHT_GRAY",
    "Color32::DARK_GRAY",
    "Color32::DARK_RED",
    "Color32::LIGHT_RED",
    "Color32::DARK_BLUE",
    "Color32::LIGHT_BLUE",
    "Color32::DARK_GREEN",
    "Color32::LIGHT_GREEN",
    "Color32::LIGHT_YELLOW",
];

#[test]
fn colours_outside_the_theme_are_only_the_listed_neutral_ones() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dirs = [root.join("src"), root.join("../../assets/icons/ui/source")];
    let mut found = Vec::new();
    for dir in &dirs {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "theme.rs" {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            let code = src.split("#[cfg(test)]").next().unwrap_or(&src);
            let n: usize = COLOUR_LITERALS.iter().map(|p| code.matches(p).count()).sum();
            let allowed = ALLOWED_LITERALS.iter().find(|(f, _, _)| *f == name).map_or(0, |(_, n, _)| *n);
            if n > allowed {
                found.push(format!("{name}: {n} colour literal(s), at most {allowed} allowed"));
            }
        }
    }
    assert!(found.is_empty(), "colour literals outside theme::Tokens: {found:?}");
}

/// The analysis shader takes its colours from the uniform (and so from the tokens). Only the
/// zebra and environment map neutrals and direction vectors stay as literals.
#[test]
fn analysis_shader_colours_come_from_the_uniform() {
    let src = include_str!("../../../assets/shaders/viewport.wgsl");
    let start = src.find("fn analysis(").unwrap_or(0);
    let end = src.find("fn stripe(").unwrap_or(src.len());
    let body = &src[start..end];
    let mut literals = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("vec3<f32>(") {
        let tail = &rest[i..];
        let close = tail.find(')').unwrap_or(tail.len() - 1);
        literals.push(&tail[..=close]);
        rest = &tail[close..];
    }
    assert_eq!(
        literals,
        [
            "vec3<f32>(0.0, 0.0, 1.0)",
            "vec3<f32>(1.0, 0.0, 0.0)",
            "vec3<f32>(c)",
            "vec3<f32>(0.32, 0.34, 0.38)",
            "vec3<f32>(0.80, 0.86, 0.95)",
            "vec3<f32>(0.6)",
            "vec3<f32>(0.9)",
            "vec3<f32>(0.55, 0.56, 0.60)",
            "vec3<f32>(0.20, 0.20, 0.22)",
            "vec3<f32>(0.35)",
            "vec3<f32>(1.0)",
        ],
        "a new colour in the analysis shader must come from `ana_c` (theme::AnalysisColors)"
    );
    assert_eq!(body.matches("u.ana_c[").count(), 9, "every analysis colour reads `ana_c`");
}

/// Every colour the engine tags a scene line with is mapped to a token by the viewport, or carries
/// no meaning a colour vision mode must change. A new engine tag fails here until it is placed.
#[test]
fn every_engine_colour_tag_is_placed() {
    const MAPPED: &[&str] = &[
        "SKETCH",
        "SKETCH_DONE",
        "SKETCH_FIXED",
        "SKETCH_LOCKED",
        "SKETCH_PROJECTED",
        "SKETCH_LOST",
        "SKETCH_CONSTRUCTION",
        "SKETCH_CENTERLINE",
        "AXIS_X",
        "AXIS_Y",
        "AXIS_Z",
    ];
    // Bodies, edges, profiles, the grid and the background: the GPU viewport draws these from
    // their own tokens; the engine's values are only the headless snapshot's.
    const NEUTRAL: &[&str] = &["BODY", "BODY_SELECTED", "EDGE", "PROFILE", "GRID", "GRID_MAJOR", "BG_TOP", "BG_BOTTOM", "SECTION_CAP"];
    let src = include_str!("../../engine/src/view.rs");
    let start = src.find("pub mod colors {").unwrap_or(0);
    let block = &src[start..];
    let block = &block[..block.find("\n}").unwrap_or(block.len())];
    let mut names: Vec<&str> = block.lines().filter_map(|l| l.trim().strip_prefix("pub const ")).filter_map(|l| l.split(':').next()).collect();
    names.sort_unstable();
    let mut placed: Vec<&str> = MAPPED.iter().chain(NEUTRAL).copied().collect();
    placed.sort_unstable();
    assert_eq!(names, placed, "an engine colour tag must be mapped to a token in `viewport::sketch_color` or listed as neutral");
}
