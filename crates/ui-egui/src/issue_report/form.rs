//! The reviewed report form. User answers stay separate from collected diagnostics.

use egui::{Align, Align2, Id, Layout, RichText, Ui, Vec2, vec2};

use super::{Draft, MAX_ANSWER, MAX_BODY, draft_url};
use crate::{SolveApp, i18n::tr_label};

fn editor_width(ui: &Ui) -> f32 {
    // Keep physical-pixel rounding from growing the window at fractional scales.
    (ui.available_width() - ui.ctx().pixels_per_point().recip()).max(24.0)
}

fn answer(ui: &mut Ui, label: &str, hint: &str, text: &mut String, id: &str, rows: usize) -> egui::Response {
    let label = ui.label(tr_label(label));
    let response = ui.add(
        egui::TextEdit::multiline(text)
            .id(Id::new(id))
            .hint_text(tr_label(hint))
            .char_limit(MAX_ANSWER)
            .desired_rows(rows)
            .desired_width(editor_width(ui)),
    );
    let response = response.labelled_by(label.id);
    crate::scenario::publish_handle(id, response.rect.center());
    super::truncate(text, MAX_ANSWER);
    ui.add_space(12.0);
    response
}

fn screenshot_actions(app: &mut SolveApp, draft: &mut Draft, ui: &mut Ui) -> bool {
    let mut retake = false;
    ui.horizontal_wrapped(|ui| {
        retake = ui.button(tr_label("Retake screenshot")).clicked();
        if ui.add_enabled(draft.image.is_some(), egui::Button::new(tr_label("Remove screenshot"))).clicked() {
            draft.image = None;
            draft.texture = None;
            draft.note = "Screenshot removed from this draft.".into();
        }
    });
    if draft.image.is_some() {
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr_label("Copy screenshot")).clicked()
                && let Some(image) = draft.image.as_ref()
            {
                ui.ctx().copy_image((**image).clone());
                draft.opened_github = false;
                draft.note = "Paste the image into the GitHub draft, or save the PNG if image copying is unavailable.".into();
            }
            if ui.add_enabled(app.services.pick_save.is_some(), egui::Button::new(tr_label("Save screenshot…"))).clicked()
                && let Some(image) = draft.image.clone()
            {
                app.pick_save(&format!("solidcraft-issue-{}.png", app.shot_token), &["png"], move |app, path| {
                    let Some(path) = path else { return };
                    let result = crate::control::save_screenshot(&image, Some(&path));
                    if let Some(draft) = app.issue_report.draft.as_mut() {
                        draft.note = if result.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                            "Screenshot saved. Attach the PNG in the GitHub draft.".into()
                        } else {
                            format!("Could not save screenshot: {}", super::debug(&result.get("error"), 1000))
                        };
                    }
                });
            }
        });
    }
    if !draft.note.is_empty() {
        ui.label(tr_label(&draft.note));
    }
    retake
}

fn screenshot(app: &mut SolveApp, draft: &mut Draft, ui: &mut Ui) -> bool {
    ui.label(RichText::new(tr_label("Screenshot (optional)")).strong());
    let preview = draft.image.as_ref().map(|image| {
        draft.texture.get_or_insert_with(|| ui.ctx().load_texture("issue-report-frame", image.clone(), egui::TextureOptions::LINEAR)).clone()
    });
    let mut retake = false;
    egui::Frame::group(ui.style()).inner_margin(12).show(ui, |ui| {
        let guidance = |ui: &mut Ui| {
            ui.add(egui::Label::new(tr_label("This image is not attached to GitHub yet. Copy or save it, then attach it on GitHub.")).wrap());
        };
        if let Some(texture) = preview {
            if ui.available_width() >= 500.0 {
                ui.horizontal_top(|ui| {
                    ui.add(egui::Image::new(&texture).max_size(vec2(208.0, 130.0)).maintain_aspect_ratio(true));
                    ui.vertical(|ui| {
                        guidance(ui);
                        retake = screenshot_actions(app, draft, ui);
                    });
                });
            } else {
                ui.add(egui::Image::new(&texture).max_size(vec2(editor_width(ui), 130.0)).maintain_aspect_ratio(true));
                guidance(ui);
                retake = screenshot_actions(app, draft, ui);
            }
        } else {
            retake = screenshot_actions(app, draft, ui);
        }
    });
    retake
}

/// Returns whether to retake the screenshot and whether the user closed the form.
pub(super) fn show(app: &mut SolveApp, draft: &mut Draft, ctx: &egui::Context) -> (bool, bool) {
    let screen = ctx.content_rect();
    let available = (screen.size() - vec2(32.0, 32.0)).max(vec2(100.0, 100.0));
    let size = available.min(vec2(680.0, 760.0));
    let mut open = true;
    let mut close = false;
    let mut retake = false;
    let mut focus_description = false;
    crate::frame::window(ctx, tr_label("Report a bug"), crate::frame::Width::Wide)
        .id(Id::new("sc_issue_report"))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .constrain_to(screen.shrink(16.0))
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(16))
        .fixed_size(size)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            // Lay out the footer first, so wrapping and UI scaling reserve its actual height.
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.horizontal_wrapped(|ui| {
                    let t = crate::theme::Tokens::get();
                    let primary = crate::frame::add_outlined(
                        ui,
                        egui::Button::new(RichText::new(tr_label("Continue on GitHub")).strong().color(t.text))
                            .fill(t.accent_soft)
                            .stroke(egui::Stroke::new(1.0, t.accent)),
                        1.0,
                    );
                    crate::scenario::publish_rect("report:continue", primary.rect);
                    crate::scenario::publish_handle("report:continue", primary.rect.center());
                    if primary.clicked() {
                        if draft.description.trim().is_empty() {
                            draft.missing_description = true;
                            focus_description = true;
                        } else {
                            let body = draft.report_body();
                            let title = draft.report_title();
                            let (url, copy_needed) = draft_url(&title, &body);
                            if copy_needed {
                                ctx.copy_text(body);
                            }
                            ctx.open_url(egui::OpenUrl::new_tab(url));
                            draft.opened_github = true;
                        }
                    }
                    let response = ui.button(tr_label("Close draft"));
                    crate::scenario::publish_rect("report:close", response.rect);
                    crate::scenario::publish_handle("report:close", response.rect.center());
                    close = response.clicked();
                });
                let body = draft.report_body();
                let (_, copy_needed) = draft_url(&draft.report_title(), &body);
                let guidance = if draft.opened_github {
                    if copy_needed {
                        "Report text copied. Paste it into GitHub and submit your report there."
                    } else {
                        "Review your report on GitHub and click Submit new issue to publish it."
                    }
                } else if copy_needed {
                    "Opens your browser and copies the report text for you to paste into GitHub."
                } else {
                    "Opens your browser. Review and submit your report on GitHub."
                };
                ui.label(tr_label(guidance));
                ui.label(tr_label("Reports are public. A GitHub account is required."));
                ui.separator();
                let height = ui.available_height().max(0.0);
                egui::ScrollArea::vertical()
                    .id_salt("issue-report-review")
                    .auto_shrink([false, false])
                    .min_scrolled_height(0.0)
                    .max_height(height)
                    .show(ui, |ui| {
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            let label = ui.label(tr_label("Summary"));
                            let response = ui
                                .add(
                                    egui::TextEdit::singleline(&mut draft.title)
                                        .id(Id::new("issue_report_title"))
                                        .char_limit(160)
                                        .desired_width(editor_width(ui)),
                                )
                                .labelled_by(label.id);
                            if response.changed() {
                                draft.opened_github = false;
                            }
                            super::truncate(&mut draft.title, 160);
                            ui.add_space(12.0);
                            let response =
                                answer(ui, "What happened?", "Describe the problem you saw", &mut draft.description, "issue_report_body", 3);
                            if response.changed() {
                                draft.opened_github = false;
                            }
                            crate::scenario::publish_rect("report:description", response.rect);
                            if !draft.focus_requested || focus_description {
                                response.scroll_to_me(Some(Align::Center));
                                response.request_focus();
                                draft.focus_requested = true;
                            }
                            if draft.missing_description {
                                if draft.description.trim().is_empty() {
                                    ui.colored_label(crate::theme::Tokens::get().error, tr_label("Describe what happened before continuing."));
                                } else {
                                    draft.missing_description = false;
                                }
                            }
                            let steps = answer(
                                ui,
                                "Steps to reproduce (optional)",
                                "What did you do just before the problem? A short list is enough",
                                &mut draft.steps,
                                "issue_report_steps",
                                2,
                            );
                            let expected = answer(
                                ui,
                                "Expected result (optional)",
                                "What did you expect to see?",
                                &mut draft.expected,
                                "issue_report_expected",
                                2,
                            );
                            if steps.changed() || expected.changed() {
                                draft.opened_github = false;
                            }
                            retake = screenshot(app, draft, ui);
                            ui.add_space(16.0);
                            ui.label(tr_label("App/system information and command settings are included. Review private details before sharing."));
                            egui::CollapsingHeader::new(tr_label("Technical details")).id_salt("issue-report-technical").show(ui, |ui| {
                                let response = ui.add(
                                    egui::TextEdit::multiline(&mut draft.body)
                                        .id(Id::new("issue_report_technical"))
                                        .char_limit(MAX_BODY)
                                        .desired_rows(8)
                                        .desired_width(editor_width(ui)),
                                );
                                if response.changed() {
                                    draft.opened_github = false;
                                }
                                super::truncate(&mut draft.body, MAX_BODY);
                                if ui.button(tr_label("Copy report")).clicked() {
                                    ctx.copy_text(draft.report_body());
                                    draft.note = "Report copied. Paste it into the GitHub draft if needed.".into();
                                }
                            });
                        });
                    });
            });
        });
    (retake, close || !open)
}
