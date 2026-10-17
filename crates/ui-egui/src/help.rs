//! The Help menu (the ? button on the application bar): Documentation (the online book), About
//! SolidCraft (version, commit, build date, licences, attributions, fonts), the keyboard shortcuts
//! (read only), and Report an Issue, which opens a local editable report draft.

use egui::{RichText, vec2};

use crate::SolveApp;
use crate::theme::Tokens;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("SOLIDCRAFT_COMMIT");
pub const BUILD_DATE: &str = env!("SOLIDCRAFT_BUILD_DATE");
/// The user and developer guide (docs/book, published by .github/workflows/pages.yml).
pub const DOCS_URL: &str = "https://docs.solidcraft.io/";
const ATTRIBUTION: &str = include_str!("../../../ATTRIBUTION.md");
use solidcraft_engine::licences::{LICENSE_APACHE, LICENSE_MIT, NOTICE, THIRD_PARTY};
/// Fonts: what this build carries.
pub const FONTS: &str = "The interface uses egui's built-in fonts (Ubuntu Light and Hack, under the Ubuntu Font Licence and \
the MIT/Bitstream Vera licence; Noto Emoji and the emoji icon font, under the OFL and MIT). Sketch text uses Liberation Sans \
(SIL OFL 1.1). The Crafting Apps' optional font pack, craft-fonts (BIZ UDPGothic, Shippori Mincho, BIZ UDMincho, Noto Sans \
CJK SC, Noto Sans Arabic; SIL OFL 1.1), is not bundled in SolidCraft builds.";

#[derive(Default)]
pub struct HelpState {
    pub menu: bool,
    pub about: bool,
    /// The GPU the window renders with (set by the host).
    pub gpu: Option<String>,
}

/// One line per fact, for bug reports.
pub fn diagnostics(app: &SolveApp) -> String {
    let renderer = if app.viewport.gpu.is_some() { "GPU" } else { "CPU" };
    format!(
        "SolidCraft {VERSION} ({COMMIT}, built {BUILD_DATE})\nOS: {} {}\nRenderer: {renderer}{}\nWindow: {}\nDesign: {} ({} features, {} bodies)",
        std::env::consts::OS,
        std::env::consts::ARCH,
        app.help.gpu.as_ref().map(|g| format!(", {g}")).unwrap_or_default(),
        if app.custom_titlebar { "custom title bar" } else { "system title bar" },
        app.session.doc.name,
        app.session.doc.features.len(),
        app.session.world_state().bodies.len(),
    )
}

/// Run a Help menu item: `docs`, `about`, `shortcuts`, `report`, `crashes`.
pub fn run(app: &mut SolveApp, ctx: &egui::Context, item: &str) {
    app.help.menu = false;
    match item {
        "docs" => ctx.open_url(egui::OpenUrl::new_tab(DOCS_URL)),
        "about" => app.help.about = true,
        "shortcuts" => {
            app.keymap.open = true;
            app.keymap.read_only = true;
        }
        "crashes" => crate::crash_ui::open(app),
        "report" => {
            let dialog = app.dialog.clone();
            let error = dialog.as_ref().and_then(|d| d.error.clone()).or_else(|| app.preview.error.clone());
            crate::issue_report::begin(app, ctx, dialog.as_ref(), error.as_deref());
        }
        _ => {}
    }
}

/// The Help menu under the ? button, and the About window.
pub fn show(app: &mut SolveApp, ctx: &egui::Context, anchor: egui::Pos2) {
    if app.help.menu {
        let mut pick: Option<&str> = None;
        let resp =
            crate::frame::area(egui::Id::new("sc_help_menu")).fixed_pos(anchor).constrain(true).order(egui::Order::Foreground).show(ctx, |ui| {
                crate::context_menu::menu_frame(ui, |ui| {
                    for (id, label) in [
                        ("docs", "Documentation"),
                        ("about", "About SolidCraft"),
                        ("shortcuts", "Keyboard Shortcuts"),
                        ("report", "Report an Issue…"),
                        ("crashes", "Crash Reports…"),
                    ] {
                        if crate::context_menu::list_row(ui, &crate::context_menu::Item::action(id, label, "")).is_some() {
                            pick = Some(id);
                        }
                    }
                })
            });
        if let Some(p) = pick {
            run(app, ctx, p);
        } else if ctx.input(|i| i.pointer.any_pressed() || i.key_pressed(egui::Key::Escape)) && !resp.response.contains_pointer() {
            // Pressed elsewhere (the ? button toggles it itself).
            if !ctx.data(|d| d.get_temp::<bool>(egui::Id::new("sc_help_opened")).unwrap_or(false)) {
                app.help.menu = false;
            }
        }
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("sc_help_opened"), false));
    }
    about(app, ctx);
    crate::crash_ui::show(app, ctx);
}

fn about(app: &mut SolveApp, ctx: &egui::Context) {
    if !app.help.about {
        return;
    }
    let t = Tokens::get();
    let mut open = true;
    crate::frame::window(ctx, crate::i18n::tr_label("About SolidCraft"), crate::frame::Width::Normal)
        .id(egui::Id::new("sc_about"))
        .open(&mut open)
        .default_size(vec2(440.0, 560.0))
        .show(ctx, |ui| {
            ui.label(RichText::new("SolidCraft").size(22.0).strong());
            ui.label(format!("Version {VERSION} · commit {COMMIT} · built {BUILD_DATE}"));
            ui.label(RichText::new(crate::i18n::tr_label("Open-source parametric 3D CAD in Rust. Licensed MIT OR Apache-2.0.")).color(t.text_dim));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::tr_label("Copy diagnostic information")).clicked() {
                    ctx.copy_text(diagnostics(app));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::CollapsingHeader::new("Attributions (ATTRIBUTION.md)").show(ui, |ui| {
                    ui.label(RichText::new(ATTRIBUTION).monospace().size(11.0));
                });
                egui::CollapsingHeader::new("Fonts").show(ui, |ui| {
                    ui.label(FONTS);
                });
                egui::CollapsingHeader::new("Licences").show(ui, |ui| {
                    egui::CollapsingHeader::new("NOTICE").show(ui, |ui| {
                        ui.label(RichText::new(NOTICE).monospace().size(11.0));
                    });
                    egui::CollapsingHeader::new("MIT licence").show(ui, |ui| {
                        ui.label(RichText::new(LICENSE_MIT).monospace().size(11.0));
                    });
                    egui::CollapsingHeader::new("Apache licence 2.0").show(ui, |ui| {
                        ui.label(RichText::new(LICENSE_APACHE).monospace().size(11.0));
                    });
                    egui::CollapsingHeader::new("Third-party licences").show(ui, |ui| {
                        if ui.button(crate::i18n::tr_label("Copy all licences")).clicked() {
                            ctx.copy_text(solidcraft_engine::licences::all());
                        }
                        third_party(ui);
                    });
                });
            });
        });
    if !open {
        app.help.about = false;
    }
}

/// THIRD-PARTY-LICENSES.txt (over 10 000 lines): only the lines in view are laid out.
fn third_party(ui: &mut egui::Ui) {
    let lines: Vec<&str> = THIRD_PARTY.lines().collect();
    let font = egui::FontId::monospace(11.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    egui::ScrollArea::both().id_salt("sc_third_party").max_height(360.0).show_rows(ui, row_h, lines.len(), |ui, rows| {
        for line in lines.get(rows).unwrap_or_default() {
            // One row per line (show_rows needs equal rows): long lines scroll sideways.
            ui.add(egui::Label::new(RichText::new(*line).font(font.clone())).extend());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_knows_its_build_and_diagnostics_say_where_it_runs() {
        assert!(!COMMIT.is_empty() && BUILD_DATE.len() == 10, "{COMMIT} {BUILD_DATE}");
        assert!(ATTRIBUTION.contains("ATTRIBUTION") || ATTRIBUTION.contains("Attribution"));
        let app = SolveApp::new(solidcraft_engine::Session::default(), Default::default());
        let d = diagnostics(&app);
        assert!(d.contains(VERSION) && d.contains(std::env::consts::OS) && d.contains("Renderer: CPU"), "{d}");
    }
}
