//! Reviewed issue drafts with local evidence; never publishes or uploads a report.

use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{ColorImage, TextureHandle};

use crate::{SolveApp, dialogs, help};

mod form;

const MAX_ANSWER: usize = 2000;
const MAX_BODY: usize = 24_000;
// Room for all three answers and the original bounded diagnostic context.
const MAX_REPORT: usize = 32_000;
const MAX_IMAGE_PIXELS: usize = 16_000_000;
const ISSUE_URL: &str = "https://github.com/bherbruck/solidcraft/issues/new";

thread_local! {
    static CAD_INPUT_BLOCKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn cad_input_blocked() -> bool {
    CAD_INPUT_BLOCKED.with(std::cell::Cell::get)
}

/// Draw CAD behind a report or native picker without delivering input or focus to editors.
/// Fresh focus history also prevents inline editors from committing on report focus loss.
pub(crate) struct CadInput {
    ctx: egui::Context,
    input: Option<egui::InputState>,
    memory: Option<egui::Memory>,
    previous_blocked: bool,
}

impl CadInput {
    pub(crate) fn block(app: &SolveApp, ui: &mut egui::Ui, guard: &mut Option<Self>) {
        if guard.is_none() {
            *guard = Self::begin(app, ui.ctx());
        }
        if guard.is_some() {
            ui.visuals_mut().disabled_alpha = 1.0;
            ui.disable();
        }
    }

    pub(crate) fn begin(app: &SolveApp, ctx: &egui::Context) -> Option<Self> {
        if app.issue_report.draft.is_none() && app.file_picker.is_none() {
            return None;
        }
        let mut raw = ctx.input(|i| i.raw.clone());
        raw.events.clear();
        raw.dropped_files.clear();
        raw.hovered_files.clear();
        let neutral = egui::InputState::default().begin_pass(raw.clone(), true, ctx.pixels_per_point(), egui::InputOptions::default());
        let input = ctx.input_mut(|i| std::mem::replace(i, neutral));
        let memory = ctx.memory_mut(|m| {
            let mut saved = std::mem::take(m);
            let mut neutral = egui::Memory::default();
            neutral.options = saved.options.clone();
            Arc::make_mut(&mut neutral.options.dark_style).visuals.disabled_alpha = 1.0;
            Arc::make_mut(&mut neutral.options.light_style).visuals.disabled_alpha = 1.0;
            neutral.data = saved.data.clone();
            neutral.to_global = saved.to_global.clone();
            *neutral.areas_mut() = saved.areas().clone();
            neutral.caches = std::mem::take(&mut saved.caches);
            *m = neutral;
            saved
        });
        let previous_blocked = CAD_INPUT_BLOCKED.with(|blocked| blocked.replace(true));
        Some(Self { ctx: ctx.clone(), input: Some(input), memory: Some(memory), previous_blocked })
    }
}

impl Drop for CadInput {
    fn drop(&mut self) {
        CAD_INPUT_BLOCKED.with(|blocked| blocked.set(self.previous_blocked));
        if let Some(input) = self.input.take() {
            self.ctx.input_mut(|i| *i = input);
        }
        if let Some(mut memory) = self.memory.take() {
            self.ctx.memory_mut(|m| {
                // Keep current layout/widget state and caches; isolate only input/focus history.
                memory.data = std::mem::take(&mut m.data);
                memory.caches = std::mem::take(&mut m.caches);
                memory.to_global = std::mem::take(&mut m.to_global);
                *memory.areas_mut() = m.areas().clone();
                *m = memory;
            });
        }
    }
}

#[derive(Default)]
pub struct ReportState {
    pub draft: Option<Draft>,
    pending: Option<Pending>,
    cad_focus: Option<egui::Id>,
}

pub struct Draft {
    pub title: String,
    /// Collected technical context, separately reviewable and editable.
    pub body: String,
    description: String,
    steps: String,
    expected: String,
    image: Option<Arc<ColorImage>>,
    texture: Option<TextureHandle>,
    pub note: String,
    source_signature: u64,
    focus_requested: bool,
    missing_description: bool,
    opened_github: bool,
}

impl Draft {
    fn new(title: String, body: String, source_signature: u64) -> Self {
        Self {
            title,
            body,
            description: String::new(),
            steps: String::new(),
            expected: String::new(),
            image: None,
            texture: None,
            note: String::new(),
            source_signature,
            focus_requested: false,
            missing_description: false,
            opened_github: false,
        }
    }

    fn report_title(&self) -> String {
        if self.title.trim().is_empty() {
            limited(self.description.trim().lines().next().unwrap_or_default(), 160)
        } else {
            limited(&self.title, 160)
        }
    }

    fn report_body(&self) -> String {
        let answer = |text: &str| if text.trim().is_empty() { "Not provided".to_string() } else { limited(text, MAX_ANSWER) };
        let mut report = format!(
            "## Actual behavior\n{}\n\n## Reproduction\n{}\n\n## Expected behavior\n{}\n\n",
            answer(&self.description),
            answer(&self.steps),
            answer(&self.expected)
        );
        let room = MAX_REPORT.saturating_sub(report.len());
        if self.body.len() <= room {
            report.push_str(&self.body);
        } else {
            let marker = "\n[technical details truncated]\n";
            report.push_str(&limited(&self.body, room.saturating_sub(marker.len())));
            report.push_str(marker);
        }
        report
    }
}

struct Pending {
    token: u64,
    signature: u64,
    deadline: f64,
    window: egui::Rect,
    pixels_per_point: f32,
}

/// A bounded formatting sink also avoids allocating a full hostile Debug/JSON value first.
struct Text {
    value: String,
    limit: usize,
}

impl std::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = self.limit.saturating_sub(self.value.len());
        if s.len() <= room {
            self.value.push_str(s);
            return Ok(());
        }
        let mut end = room.min(s.len());
        while !s.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        self.value.push_str(s.get(..end).unwrap_or_default());
        Err(std::fmt::Error)
    }
}

fn debug(value: &impl std::fmt::Debug, limit: usize) -> String {
    let mut text = Text { value: String::new(), limit };
    if write!(&mut text, "{value:#?}").is_err() {
        text.value.push_str("\n[truncated]");
    }
    text.value
}

struct JsonBytes {
    value: Vec<u8>,
    limit: usize,
}
impl std::io::Write for JsonBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = bytes.len().min(self.limit.saturating_sub(self.value.len()));
        self.value.extend_from_slice(bytes.get(..n).unwrap_or_default());
        if n != bytes.len() {
            return Err(std::io::Error::other("report context limit"));
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn json_context(value: &impl serde::Serialize, limit: usize) -> String {
    let mut bytes = JsonBytes { value: Vec::new(), limit };
    let truncated = serde_json::to_writer_pretty(&mut bytes, value).is_err();
    let mut value = String::from_utf8_lossy(&bytes.value).into_owned();
    if truncated {
        value.push_str("\n[truncated]");
    }
    value
}

fn truncate(value: &mut String, limit: usize) {
    if value.len() > limit {
        let mut end = limit;
        while !value.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        value.truncate(end);
    }
}

fn limited(value: &str, limit: usize) -> String {
    let mut end = limit.min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value.get(..end).unwrap_or_default().to_string()
}

fn block(value: &str) -> String {
    value.lines().map(|line| format!("    {line}\n")).collect()
}

/// Hash directly through formatting, without constructing an unbounded Debug string.
struct Fingerprint(std::collections::hash_map::DefaultHasher);
impl std::fmt::Write for Fingerprint {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

fn source_signature(app: &SolveApp, dialog: Option<&dialogs::Dialog>) -> u64 {
    let mut h = Fingerprint(std::collections::hash_map::DefaultHasher::new());
    app.session.revision.hash(&mut h.0);
    app.docs.active.hash(&mut h.0);
    app.docs.count().hash(&mut h.0);
    app.session.doc.name.hash(&mut h.0);
    app.preview.key.hash(&mut h.0);
    app.preview.error.hash(&mut h.0);
    app.preview.busy.hash(&mut h.0);
    let _ = write!(&mut h, "{:?}{:?}", dialog, app.session.selection);
    h.0.finish()
}

fn signature(app: &SolveApp, dialog: Option<&dialogs::Dialog>) -> u64 {
    let mut h = Fingerprint(std::collections::hash_map::DefaultHasher::new());
    source_signature(app, dialog).hash(&mut h.0);
    let _ = write!(&mut h, "{:?}{:?}{:?}", app.cam, app.viewport.rect, app.ui.perspective);
    h.0.finish()
}

fn reportable(feature: &solidcraft_engine::doc::Feature) -> bool {
    !matches!(
        feature.kind,
        solidcraft_engine::doc::FeatureKind::Sketch { .. }
            | solidcraft_engine::doc::FeatureKind::Import { .. }
            | solidcraft_engine::doc::FeatureKind::MeshImport { .. }
    )
}

fn feature_context(feature: &solidcraft_engine::doc::Feature) -> String {
    format!("Feature {}: {}\n{}", feature.id, debug(&feature.name, 160), block(&json_context(&feature.kind, 2000)))
}

/// Read-only context excludes whole documents, sketch geometry and imported file contents.
pub fn draft_for(app: &SolveApp, dialog: Option<&dialogs::Dialog>, error: Option<&str>) -> Draft {
    let mut context = String::new();
    let label = dialog.map(dialogs::report_title).unwrap_or_else(|| "Modeling".into());
    if let Some(d) = dialog {
        match dialogs::apply_commands(app, d) {
            Ok(commands) => context.push_str(&block(&json_context(&commands, 10_000))),
            Err(e) => {
                context.push_str(&format!("Could not build command parameters: {}\n", debug(&e, 1000)));
                context.push_str(&block(&debug(&d.kind, 6000)));
                context.push_str(&block(&debug(&d.inputs, 3000)));
                context.push_str(&block(&debug(&d.extra, 1000)));
            }
        }
    }
    context.push_str("\nSelected items (including occurrence paths):\n");
    context.push_str(&block(&debug(&app.session.selection.iter().take(32).collect::<Vec<_>>(), 3000)));
    for item in app.session.selection.iter().take(8) {
        if let solidcraft_engine::Sel::Feature { id } = item
            && let Some(feature) = app.session.doc.feature(*id)
            && reportable(feature)
        {
            context.push_str(&feature_context(feature));
            if let Some(e) = app.session.model.result(*id).and_then(|r| r.error.as_deref()) {
                context.push_str(&format!("Feature error: {}\n", debug(&e, 1000)));
            }
        }
    }
    if dialog.is_none()
        && let Some(feature) = app.session.doc.features.iter().rev().find(|f| reportable(f))
    {
        context.push_str("\nLatest modeling operation:\n");
        context.push_str(&feature_context(feature));
        if let Some(e) = app.session.model.result(feature.id).and_then(|r| r.error.as_deref()) {
            context.push_str(&format!("Feature error: {}\n", debug(&e, 1000)));
        }
    }
    let actual = error.map(|e| debug(&e, 4000)).unwrap_or_else(|| "No command error was recorded.".into());
    let image_state = if error.is_some() {
        "A failed preview may display the last valid geometry; that geometry is not evidence that the failed operation succeeded."
    } else {
        "The screenshot shows the current view."
    };
    let mut body = format!(
        "## Operation context\n{actual}\n\n{image_state}\n\n## Settings and selection\n{context}\n## Environment\n{}\nRevision: {}\nCamera: {}\n\n## Visual context\nAttach the reviewed screenshot or a short example showing the problem.\n",
        block(&debug(&help::diagnostics(app), 2000)),
        app.session.revision,
        block(&debug(&app.cam, 1500))
    );
    if body.len() > MAX_BODY {
        truncate(&mut body, MAX_BODY - 32);
        body.push_str("\n[report context truncated]\n");
    }
    let mut title = format!("{label}: {}", if error.is_some() { "operation failed" } else { "unexpected result" });
    truncate(&mut title, 160);
    Draft::new(title, body, source_signature(app, dialog))
}

/// A draft from a crash report (`solidcraft_engine::crash`): what happened, where, the command
/// and how to replay it. The design stays on this computer; the draft says how to share it.
pub fn crash_draft(report: &serde_json::Value) -> Draft {
    let s = |k: &str| report.get(k).and_then(serde_json::Value::as_str).unwrap_or("?").to_string();
    let what = match report.get("kind").and_then(serde_json::Value::as_str) {
        Some("command_panic") => "An internal error stopped a command (the design was left unchanged)",
        Some("kernel_panic_recovered") => "The geometry kernel hit an internal error (caught; the operation failed)",
        Some("unclean_exit") => "SolidCraft closed unexpectedly (abort, stack overflow, a crash in a graphics driver or the process was killed)",
        _ => "SolidCraft hit an internal error",
    };
    let command = report.get("command").and_then(|c| c.get("command")).and_then(serde_json::Value::as_str);
    let frames: Vec<&str> =
        report.get("backtrace").and_then(serde_json::Value::as_array).into_iter().flatten().filter_map(serde_json::Value::as_str).take(25).collect();
    let recent: Vec<String> = report
        .get("recent_commands")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let id = c.get("command")?.as_str()?;
            Some(if c.get("ok").and_then(serde_json::Value::as_bool) == Some(false) { format!("{id} (failed)") } else { id.to_string() })
        })
        .collect();
    let app = report.get("app");
    let field = |o: Option<&serde_json::Value>, k: &str| o.and_then(|o| o.get(k)).and_then(serde_json::Value::as_str).unwrap_or("?").to_string();
    let system = report.get("system");
    let mut body = format!(
        "## What happened\n{what}{}.\n\nMessage: {}\nLocation: {}\nThread: {}\nHappened {} time(s), first {}\n\n## Backtrace\n{}\n## Command\n{}\n## Recent commands\n{}\n\n## Reproduction\nCrash report `{}` on the reporter's computer holds {}. Replay it with `solidcraft-cli crashes replay {}`.\nOnly attach the design if it can be shared publicly.\n\n## Environment\n{}\n",
        command.map(|c| format!(" while running `{c}`")).unwrap_or_default(),
        limited(&s("message"), 2000),
        s("location"),
        s("thread"),
        report.get("count").and_then(serde_json::Value::as_u64).unwrap_or(1),
        s("first_seen"),
        block(&frames.join("\n")),
        block(&json_context(&report.get("command"), 4000)),
        if recent.is_empty() { "(none recorded)".to_string() } else { recent.join(", ") },
        s("id"),
        match (report.get("design_path").is_some(), report.get("replay_path").is_some()) {
            (true, true) => "the design as it was before the crash (design.solidcraft) and the commands that crash it (replay.json)",
            (false, true) => "the commands that crash it on a new design (replay.json)",
            _ => "no replay (see its notes)",
        },
        s("id"),
        block(&format!(
            "SolidCraft {} ({}, built {})\nOS: {} {}\n{}",
            field(app, "version"),
            field(app, "commit"),
            field(app, "built"),
            field(system, "os"),
            field(system, "arch"),
            field(system, "environment")
        )),
    );
    if body.len() > MAX_BODY {
        truncate(&mut body, MAX_BODY - 32);
        body.push_str("\n[report context truncated]\n");
    }
    let mut title = format!("Crash: {}", s("message").lines().next().unwrap_or_default());
    truncate(&mut title, 160);
    let mut draft = Draft::new(title, body, 0);
    draft.description =
        limited(&format!("{what}{}.", command.map(|c| format!(" while running `{}`", limited(c, 160))).unwrap_or_default()), MAX_ANSWER);
    draft
}

/// Open a reviewed draft for a crash report (no screenshot: the view shows nothing of it).
pub fn begin_crash(app: &mut SolveApp, ctx: &egui::Context, report: &serde_json::Value) {
    if app.issue_report.draft.is_none() {
        app.issue_report.cad_focus = ctx.memory(|m| m.focused());
    }
    cancel_capture(app);
    let mut draft = crash_draft(report);
    // A screenshot can still be added of the view as it is now.
    draft.source_signature = source_signature(app, None);
    app.issue_report.draft = Some(draft);
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

/// Long drafts stay local and can be copied into the browser editor; never discard them.
pub fn draft_url(title: &str, body: &str) -> (String, bool) {
    let title = limited(title, 160);
    let base = format!("{ISSUE_URL}?title={}", encode(&title));
    if body.len() <= MAX_BODY {
        let url = format!("{base}&body={}", encode(body));
        if url.len() <= 2000 {
            return (url, false);
        }
    }
    (base, true)
}

pub fn begin(app: &mut SolveApp, ctx: &egui::Context, dialog: Option<&dialogs::Dialog>, error: Option<&str>) {
    if app.issue_report.draft.is_none() {
        app.issue_report.cad_focus = ctx.memory(|m| m.focused());
    }
    cancel_capture(app);
    app.issue_report.draft = Some(draft_for(app, dialog, error));
    if let Some(draft) = app.issue_report.draft.as_mut() {
        let rect = ctx.content_rect();
        let _ = write!(
            &mut draft.body,
            "\nWindow: {:.0}×{:.0} logical points; {:.2} physical pixels/point\n",
            rect.width(),
            rect.height(),
            ctx.pixels_per_point()
        );
        truncate(&mut draft.body, MAX_BODY);
    }
    capture(app, ctx, dialog);
}

fn cancel_capture(app: &mut SolveApp) {
    if let Some(pending) = app.issue_report.pending.take() {
        app.queued_shots.retain(|(token, ..)| *token != pending.token);
    }
}

fn capture(app: &mut SolveApp, ctx: &egui::Context, dialog: Option<&dialogs::Dialog>) {
    cancel_capture(app);
    let current_source = source_signature(app, dialog);
    let Some(draft) = app.issue_report.draft.as_mut() else { return };
    draft.image = None;
    draft.texture = None;
    if draft.source_signature != current_source {
        draft.note =
            "Settings or model changed. Close this draft and create a new report to capture matching context; your text is still available to copy."
                .into();
        return;
    }
    if app.preview.busy || app.queued_shots.len() >= 16 {
        draft.note = "Screenshot unavailable while the preview is updating. Reopen the report after it settles, or continue with text.".into();
        return;
    }
    let Some(token) = app.shot_token.checked_add(1) else {
        draft.note = "Screenshot request limit reached; restart the app to capture another frame.".into();
        return;
    };
    app.shot_token = token;
    draft.note = "Capturing the current app view locally…".into();
    let now = crate::now_ms();
    app.issue_report.pending = Some(Pending {
        token,
        signature: signature(app, dialog),
        deadline: now + 8000.0,
        window: ctx.content_rect(),
        pixels_per_point: ctx.pixels_per_point(),
    });
    app.queued_shots.push((token, now + 150.0, 0));
    ctx.request_repaint();
}

pub(crate) fn pending(app: &SolveApp) -> bool {
    app.issue_report.pending.is_some()
}

pub(crate) fn tick(app: &mut SolveApp, ctx: &egui::Context) {
    let problem = app.issue_report.pending.as_ref().and_then(|p| {
        if signature(app, app.dialog.as_ref()) != p.signature || p.window != ctx.content_rect() || p.pixels_per_point != ctx.pixels_per_point() {
            Some("The model, inputs or view changed during capture. Retake the current view.")
        } else if crate::now_ms() >= p.deadline {
            Some("No app frame was presented. Restore the window and retake, or continue with text.")
        } else {
            None
        }
    });
    if let Some(problem) = problem {
        cancel_capture(app);
        if let Some(draft) = app.issue_report.draft.as_mut() {
            draft.note = problem.into();
        }
    }
    if pending(app) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

pub(crate) fn receive(app: &mut SolveApp, token: u64, image: Arc<ColorImage>) {
    if !app.issue_report.pending.as_ref().is_some_and(|p| p.token == token) {
        return;
    }
    let stale = app.issue_report.pending.as_ref().is_some_and(|p| p.signature != signature(app, app.dialog.as_ref()));
    app.issue_report.pending = None;
    if let Some(draft) = app.issue_report.draft.as_mut() {
        if stale {
            draft.note = "The model, inputs or view changed during capture. Retake the current view.".into();
            return;
        }
        let pixels = image.width().checked_mul(image.height());
        if !pixels.is_some_and(|n| n > 0 && n <= MAX_IMAGE_PIXELS && n == image.pixels.len()) {
            draft.note = "Screenshot dimensions are unsupported. The text draft is still available.".into();
        } else {
            draft.image = Some(image);
            draft.note = "Screenshot captured locally. Review it before attaching it on GitHub.".into();
        }
    }
}

/// The report window is hidden only while a frame is captured, so it cannot cover the evidence.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    if pending(app) {
        return;
    }
    let Some(mut draft) = app.issue_report.draft.take() else { return };
    let (retake, close) = form::show(app, &mut draft, ctx);
    if !close {
        app.issue_report.draft = Some(draft);
        if retake {
            let dialog = app.dialog.clone();
            capture(app, ctx, dialog.as_ref());
        }
    } else {
        cancel_capture(app);
        // Pointer release may have happened during review; never finalize it against
        // the report's last cursor position when CAD interaction resumes.
        if app.viewport.text_drag.is_some() {
            let _ = app.session.execute("sketch.drag_end", &serde_json::json!({}));
            crate::sketch_text_move::end_drag(app);
        }
        app.viewport.point_drag = None;
        app.viewport.boxsel = None;
        app.viewport.axis_drag = None;
        app.viewport.orbit_free = None;
        app.viewport.orbit_snap = None;
        crate::dim_view::drag_end();
        crate::sketch3d::drag_end();
        crate::drag_snap::clear();
        crate::gizmo::cancel(ctx, egui::Id::new("sc_move_triad"));
        crate::gizmo::cancel(ctx, egui::Id::new("sc_occ_triad"));
        if let Some(focus) = app.issue_report.cad_focus.take() {
            ctx.memory_mut(|m| m.request_focus(focus));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use serde_json::json;
    use solidcraft_engine::{Sel, Session};

    fn app() -> SolveApp {
        SolveApp::new(Session::default(), Default::default())
    }

    fn combine(app: &mut SolveApp) {
        app.start("solid.combine");
        let dialog = app.dialog.as_mut().unwrap();
        dialog.kind = dialogs::Kind::Combine { operation: 2, keep_tools: true };
        dialog.inputs.get_mut(0).unwrap().items = vec![Sel::Body { name: "Stock".into() }];
        dialog.inputs.get_mut(1).unwrap().items = vec![Sel::Body { name: "Cutter".into() }];
    }

    fn click_commands(h: &mut crate::scenario::Harness, label: &str) -> Vec<egui::OutputCommand> {
        let [x, y] = h.widget(label).unwrap();
        assert_eq!(h.call("ui.move", json!({"x":x,"y":y}))["ok"], true);
        let (request, _reply) = crate::control::ControlRequest::new("ui.click", json!({"x":x,"y":y}));
        assert!(matches!(crate::control::handle(&mut h.app, &h.ctx, &request), crate::control::Outcome::Done(value) if value["ok"] == true));
        let mut commands = Vec::new();
        for _ in 0..12 {
            h.frame();
            commands.extend(h.output.commands.clone());
        }
        commands
    }

    #[test]
    fn issue_report_form_separates_user_answers_from_technical_context() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.issue_report.draft = Some(draft_for(&h.app, None, None));
        h.frames(4);
        for label in ["What happened?", "Steps to reproduce (optional)", "Expected result (optional)", "Technical details", "Continue on GitHub"] {
            assert!(h.widget(label).is_some(), "missing report form control: {label}");
        }
        assert!(h.widget("Describe the intended result.").is_none(), "Markdown instructions must not be editable answers");
    }

    #[test]
    fn issue_report_answers_preserve_the_reviewed_context_within_the_export_budget() {
        let mut draft = draft_for(&app(), None, None);
        draft.description = "a".repeat(MAX_ANSWER);
        draft.steps = "b".repeat(MAX_ANSWER);
        draft.expected = "c".repeat(MAX_ANSWER);
        draft.body = format!("{}END_CONTEXT", "d".repeat(MAX_BODY - 11));
        let report = draft.report_body();
        assert!(report.len() <= MAX_REPORT && report.ends_with(&draft.body));
        for answer in [&draft.description, &draft.steps, &draft.expected] {
            assert!(report.contains(answer));
        }
        draft.description = "現象🚀".repeat(MAX_REPORT);
        draft.body = "調査🚀".repeat(MAX_REPORT);
        let bounded = draft.report_body();
        assert!(bounded.len() <= MAX_REPORT && bounded.ends_with("[technical details truncated]\n"));
    }

    #[test]
    fn issue_report_requires_an_answer_before_browser_handoff_without_changing_the_design() {
        for (size, scale) in [([1600, 1000], 1.0), ([640, 480], 2.0)] {
            let mut h = crate::scenario::Harness::new();
            h.app.home.open = false;
            h.app.preferences.ui_scale = scale;
            h.call("ui.resize", json!({"width":size[0],"height":size[1]}));
            h.app.issue_report.draft = Some(draft_for(&h.app, None, None));
            h.frames(4);
            let before = (h.app.session.doc.to_json(), h.app.session.revision, h.app.session.undo.len());
            let commands = click_commands(&mut h, "Continue on GitHub");
            assert!(!commands.iter().any(|c| matches!(c, egui::OutputCommand::OpenUrl(_))));
            assert!(h.app.issue_report.draft.as_ref().unwrap().missing_description);
            assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("issue_report_body")));
            h.step(&json!({"text":"The preview disappeared"})).unwrap();
            let commands = click_commands(&mut h, "Continue on GitHub");
            assert!(commands.iter().any(|c| matches!(c, egui::OutputCommand::OpenUrl(url) if url.url.starts_with(ISSUE_URL))));
            assert!(h.app.issue_report.draft.as_ref().unwrap().description.contains("The preview disappeared"));
            assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.revision, h.app.session.undo.len()));
        }
    }

    #[test]
    fn issue_report_actual_pending_boolean_settings_and_failure_are_read_only() {
        let mut app = app();
        combine(&mut app);
        let before = (app.session.doc.to_json(), app.session.revision, app.session.undo.len(), app.session.redo.len(), app.cam);
        let draft = draft_for(&app, app.dialog.as_ref(), Some("Boolean tool is outside target"));
        for expected in
            ["solid.combine", "Stock", "Cutter", "cut", "keep_tools", "true", "Boolean tool is outside target", help::VERSION, "last valid geometry"]
        {
            assert!(draft.body.contains(expected), "missing {expected}: {}", draft.body);
        }
        assert_eq!(before, (app.session.doc.to_json(), app.session.revision, app.session.undo.len(), app.session.redo.len(), app.cam));
    }

    #[test]
    fn issue_report_hostile_context_and_url_are_bounded_without_new_query_parameters() {
        let mut app = app();
        combine(&mut app);
        let hostile = "錯誤 ```\n## fake &labels=secret%20tag 🚀".repeat(40_000);
        let draft = draft_for(&app, app.dialog.as_ref(), Some(&hostile));
        assert!(draft.body.len() <= MAX_BODY);
        assert!(draft.body.contains("[truncated]"));
        let (url, copy) = draft_url("Cut &labels=secret #? 🚀", "actual\n```\n&assignees=someone");
        assert!(!copy);
        assert!(url.starts_with(ISSUE_URL));
        assert_eq!(url.matches('&').count(), 1);
        assert!(url.contains("%26labels%3Dsecret") && url.contains("%F0%9F%9A%80"));
        let (short, copy) = draft_url(&hostile, &draft.body);
        assert!(copy && short.len() <= 2000 && !short.contains("&body="));
        assert!(draft.body.contains("Stock"), "fallback must retain the editable full report");
    }

    #[test]
    fn issue_report_help_supports_successful_geometry_without_mutating_it() {
        let mut app = app();
        app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Stock"})).unwrap();
        app.run("solid.box", json!({"length":5,"width":5,"height":5,"body_name":"Cutter"})).unwrap();
        app.run("solid.combine", json!({"target":"Stock","tools":["Cutter"],"operation":"cut","keep_tools":true})).unwrap();
        assert!(app.session.model.results.iter().all(|r| r.error.is_none()));
        let before = (app.session.doc.to_json(), app.session.undo.len(), app.session.redo.len(), app.cam);
        let ctx = egui::Context::default();
        help::run(&mut app, &ctx, "report");
        let draft = app.issue_report.draft.as_ref().unwrap();
        assert!(draft.description.is_empty(), "user answers start empty, without template instructions");
        assert!(draft.body.contains(help::COMMIT));
        assert!(draft.body.contains("combine") && draft.body.contains("keep_tools") && draft.body.contains("Cutter"));
        assert!(pending(&app), "Help must request real evidence, not just copy generic text");
        assert_eq!(before, (app.session.doc.to_json(), app.session.undo.len(), app.session.redo.len(), app.cam));
    }

    #[test]
    fn issue_report_selected_and_recent_sketch_geometry_is_not_exported() {
        let mut app = app();
        app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"PublicExample"})).unwrap();
        let created = app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        let sketch = created["sketch"].as_u64().unwrap();
        app.run("sketch.point", json!({"point":[123456.789123,987654.321987]})).unwrap();
        app.run("sketch.finish", json!({})).unwrap();
        app.session.selection = vec![Sel::Feature { id: sketch }];
        let stored = app.session.doc.to_json();
        assert!(stored.contains("123456.789123") && stored.contains("987654.321987"));
        let draft = draft_for(&app, None, None);
        assert!(
            draft.body.contains("\"type\": \"box\"") && draft.body.contains("\"length\""),
            "keep the relevant last modeling settings: {}",
            draft.body
        );
        assert!(!draft.body.contains("123456.789123") && !draft.body.contains("987654.321987"));
        assert!(!draft.body.contains("\"points\""), "never serialize the selected or recent sketch point list");
        assert_eq!(stored, app.session.doc.to_json());
    }

    #[test]
    fn issue_report_error_row_click_opens_the_actual_boolean_draft() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Stock"})).unwrap();
        h.app.home.open = false;
        combine(&mut h.app);
        let error = h.app.run("solid.combine", json!({"target":"Stock","tools":["Cutter"],"operation":"cut","keep_tools":true})).unwrap_err();
        h.app.dialog.as_mut().unwrap().error = Some(error.clone());
        h.frames(3);
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len());
        h.step(&json!({"widget":"Create Issue…"})).unwrap();
        let draft = h.app.issue_report.draft.as_ref().unwrap();
        assert!(draft.body.contains("solid.combine") && draft.body.contains("Cutter") && draft.body.contains(&format!("{error:?}")));
        assert!(h.app.dialog.is_some(), "reporting must not cancel the live feature dialog");
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len()));
    }

    #[test]
    fn issue_report_long_draft_open_click_copies_the_full_reviewed_text() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        let mut draft = draft_for(&h.app, None, None);
        draft.body = "Reviewed context 🚀 &labels=private\n".repeat(120);
        draft.description = "The preview disappeared after clicking Extrude.".into();
        draft.steps = "1. Select a face\n2. Start Extrude".into();
        draft.expected = "A preview of the extrusion".into();
        draft.image = Some(Arc::new(ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4])));
        let reviewed = draft.report_body();
        h.app.issue_report.draft = Some(draft);
        h.frames(4);
        assert!(h.widget("Opens your browser and copies the report text for you to paste into GitHub.").is_some());
        assert!(h.widget("Reports are public. A GitHub account is required.").is_some());
        let commands = click_commands(&mut h, "Continue on GitHub");
        assert!(commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == &reviewed)), "commands: {commands:?}");
        assert!(
            commands.iter().any(|c| matches!(c, egui::OutputCommand::OpenUrl(url) if url.url.starts_with(ISSUE_URL) && !url.url.contains("&body=")))
        );
        assert_eq!(h.app.issue_report.draft.as_ref().unwrap().report_body(), reviewed);
        assert!(h.app.issue_report.draft.as_ref().unwrap().opened_github);
        click_commands(&mut h, "Copy screenshot");
        assert!(!h.app.issue_report.draft.as_ref().unwrap().opened_github, "copying an image replaces the report text on the clipboard");
        assert!(h.widget("Opens your browser and copies the report text for you to paste into GitHub.").is_some());
        click_commands(&mut h, "Continue on GitHub");
        click_commands(&mut h, "Remove screenshot");
        assert!(h.app.issue_report.draft.as_ref().unwrap().image.is_none());
        assert!(h.widget("Screenshot removed from this draft.").is_some());
        assert!(h.widget("Report text copied. Paste it into GitHub and submit your report there.").is_some());
    }

    #[test]
    fn issue_report_blank_summary_uses_the_export_title_for_clipboard_guidance() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        let mut draft = draft_for(&h.app, None, None);
        draft.title.clear();
        draft.description = "d".repeat(160);
        draft.body = "x".repeat(1500);
        let reviewed = draft.report_body();
        assert!(!draft_url("", &reviewed).1, "the fallback title must be what crosses the URL limit");
        assert!(draft_url(&draft.report_title(), &reviewed).1);
        h.app.issue_report.draft = Some(draft);
        h.frames(4);
        assert!(h.widget("Opens your browser and copies the report text for you to paste into GitHub.").is_some());
        let commands = click_commands(&mut h, "Continue on GitHub");
        assert!(commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == &reviewed)));
        assert!(h.widget("Report text copied. Paste it into GitHub and submit your report there.").is_some());
    }

    #[test]
    fn issue_report_idle_window_does_not_grow_with_a_screenshot() {
        for size in [[1600, 1000], [900, 700], [640, 480]] {
            for scale in [1.0, 1.25, 1.4, 2.0] {
                let mut h = crate::scenario::Harness::new();
                h.app.preferences.ui_scale = scale;
                h.app.home.open = false;
                h.call("ui.resize", json!({"width":size[0],"height":size[1]}));
                let mut draft = draft_for(&h.app, None, None);
                draft.note = "Screenshot captured locally. Review it before attaching it on GitHub.".into();
                draft.image = Some(Arc::new(ColorImage::new([1600, 1000], vec![egui::Color32::GREEN; 1_600_000])));
                h.app.issue_report.draft = Some(draft);
                h.frames(20);
                let rect = |h: &crate::scenario::Harness| egui::containers::AreaState::load(&h.ctx, egui::Id::new("sc_issue_report")).unwrap().rect();
                let settled = rect(&h);
                h.frames(120);
                let idle = rect(&h);
                assert!((idle.width() - settled.width()).abs() < 1.0, "size={size:?}: {settled:?} grew to {idle:?}");
                assert!(idle.width() <= 790.0, "report must retain its width limit: {idle:?}");
                assert!(h.ctx.content_rect().shrink(8.0).contains_rect(idle), "size={size:?} scale={scale}: retain an outer margin: {idle:?}");
                for label in ["Continue on GitHub", "Close draft"] {
                    let [x, y] = h.widget(label).unwrap();
                    assert!(idle.contains(egui::pos2(x as f32, y as f32)), "{label} must stay visible: {idle:?}, {x}, {y}");
                }
            }
        }
    }

    #[test]
    fn issue_report_editing_enter_and_cad_shortcuts_leave_the_live_boolean_unchanged() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Stock"})).unwrap();
        h.app.run("solid.box", json!({"length":5,"width":5,"height":5,"body_name":"Cutter"})).unwrap();
        combine(&mut h.app);
        h.app.issue_report.draft = Some(draft_for(&h.app, h.app.dialog.as_ref(), None));
        h.frames(4);
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision);
        for field in ["issue_report_title", "issue_report_body"] {
            let id = egui::Id::new(field);
            h.ctx.memory_mut(|m| m.request_focus(id));
            h.frames(1);
            h.step(&json!({"text":"edited-report-marker"})).unwrap();
            h.step(&json!({"key":"Enter"})).unwrap();
            assert!(h.app.dialog.is_some(), "Enter in {field} must not apply the valid Boolean");
            assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
            h.ctx.memory_mut(|m| m.surrender_focus(id));
        }
        let draft = h.app.issue_report.draft.as_ref().unwrap();
        assert!(draft.title.contains("edited-report-marker") && draft.description.contains("edited-report-marker"));
        for key in [json!({"key":"Enter"}), json!({"key":"Escape"}), json!({"key":"Z","cmd":true})] {
            h.step(&key).unwrap();
            assert!(h.app.dialog.is_some() && h.app.issue_report.draft.is_some());
            assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
        }
    }

    #[test]
    fn issue_report_placed_sketch_text_cannot_steal_focus_apply_or_cancel_behind_draft() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        let start = h.app.session.undo.len();
        h.app.start("sketch.text");
        h.frames(3);
        h.step(&json!({"click":{"sketch":[0,0]}})).unwrap();
        h.step(&json!({"text":"Pending CAD text"})).unwrap();
        assert!(h.widget("Pending CAD text").is_some(), "the real placed Text editor must be open");
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision);
        h.app.help.menu = true;
        h.frames(2);
        h.step(&json!({"click":{"handle":"menu:report"}})).unwrap();
        h.frames(3);
        assert!(h.widget("Pending CAD text").is_some(), "capture must keep the underlying Text editor drawn");
        let token = h.app.issue_report.pending.as_ref().unwrap().token;
        receive(&mut h.app, token, Arc::new(ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4])));
        h.frames(3);
        for field in ["issue_report_title", "issue_report_body"] {
            let id = egui::Id::new(field);
            h.ctx.memory_mut(|m| m.request_focus(id));
            h.frames(2);
            assert_eq!(h.ctx.memory(|m| m.focused()), Some(id), "placed Text must not steal report focus");
            h.step(&json!({"text":"report-only-marker"})).unwrap();
            h.step(&json!({"key":"Enter"})).unwrap();
            h.step(&json!({"key":"Escape"})).unwrap();
            assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
            assert!(h.widget("Pending CAD text").is_some(), "report keys must not apply, edit or cancel the Text editor");
        }
        let draft = h.app.issue_report.draft.as_ref().unwrap();
        assert!(draft.title.contains("report-only-marker") && draft.description.contains("report-only-marker"));
        h.step(&json!({"widget":"Close draft"})).unwrap();
        assert!(h.app.issue_report.draft.is_none());
        h.frames(2);
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
        assert!(h.widget("Pending CAD text").is_some());
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::sketch_text::editing(&h.app), "closing the report must return control to the preserved CAD text");
        assert_eq!(h.app.session.undo.len(), start + 1, "placing and typing the text is one undo step");
        let sid = h.app.session.active_sketch.unwrap();
        let sk = h.app.session.doc.sketch(sid).unwrap();
        assert!(sk.links.iter().any(|l| matches!(&l.source, solidcraft_engine::sketch::LinkSource::Text { text, .. } if text == "Pending CAD text")));
    }

    #[test]
    fn issue_report_boundary_preserves_opacity_widget_state_and_real_focus() {
        let mut app = app();
        let ctx = egui::Context::default();
        let focus = egui::Id::new("report-review-focus");
        let state = egui::Id::new("cad-layout-state");
        begin(&mut app, &ctx, None, None);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
            focused: true,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            ctx.memory_mut(|m| m.request_focus(focus));
            let barrier = CadInput::begin(&app, &ctx).unwrap();
            ui.visuals_mut().disabled_alpha = 1.0;
            ui.disable();
            assert_eq!(ui.opacity(), 1.0, "capture must retain normal drawing opacity");
            assert!(!ui.is_enabled());
            crate::frame::area(egui::Id::new("cad-boundary-proof")).fade_in(false).show(&ctx, |ui| {
                assert!(!ui.is_enabled());
                assert_eq!(ui.opacity(), 1.0);
                ui.ctx().data_mut(|d| d.insert_temp(state, 42_u32));
                ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("underlying-editor")));
            });
            drop(barrier);
            assert_eq!(ctx.memory(|m| m.focused()), Some(focus));
            assert_eq!(ctx.data(|d| d.get_temp::<u32>(state)), Some(42));
            assert!(crate::frame::area(egui::Id::new("report-boundary-proof")).is_enabled());
        });
        output.textures_delta.clear();
        app.issue_report.draft = None;
        assert!(!cad_input_blocked());
    }

    #[test]
    fn issue_report_cached_point_drag_release_does_not_finish_cad_drag() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[0,0],[20,0]],"id":"line"})).unwrap();
        h.frames(3);
        let at = h.call("ui.at", json!({"sketch":[0,0]}));
        let p = egui::pos2(at["result"][0].as_f64().unwrap() as f32, at["result"][1].as_f64().unwrap() as f32);
        h.app.synthetic.push(egui::Event::PointerMoved(p));
        h.frame();
        h.app.synthetic.push(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.frame();
        let q = p + egui::vec2(40.0, -40.0);
        h.app.synthetic.push(egui::Event::PointerMoved(q));
        h.frames(2);
        let drag = h.app.viewport.point_drag.clone();
        assert!(drag.is_some(), "real viewport point drag must have started");
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision);
        help::run(&mut h.app, &h.ctx, "report");
        // egui calculates this release before the CAD input barrier begins.
        h.app.synthetic.push(egui::Event::PointerButton {
            pos: q,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.frame();
        assert_eq!(h.app.viewport.point_drag, drag, "cached release must not clear or constrain the CAD drag");
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
        let token = h.app.issue_report.pending.as_ref().unwrap().token;
        receive(&mut h.app, token, Arc::new(ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4])));
        h.frames(3);
        h.step(&json!({"widget":"Close draft"})).unwrap();
        assert!(h.app.viewport.point_drag.is_none());
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
    }

    #[test]
    fn issue_report_new_dimension_draft_resumes_and_commits_once_after_close() {
        crate::dim_view::cancel();
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[0,0],[20,0]]})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 80.0;
        h.app.cam.target = solidcraft_engine::geom::Vec3::new(10.0, 0.0, 0.0);
        h.settle();
        h.step(&json!({"start":"sketch.dimension"})).unwrap();
        for point in [[0, 0, 0], [20, 0, 0], [10, 5, 0]] {
            h.step(&json!({"click":{"world":point}})).unwrap();
        }
        h.step(&json!({"text":"30 mm"})).unwrap();
        assert!(crate::dim_view::editing());
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision);
        help::run(&mut h.app, &h.ctx, "report");
        h.frames(3);
        let token = h.app.issue_report.pending.as_ref().unwrap().token;
        receive(&mut h.app, token, Arc::new(ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4])));
        h.frames(3);
        h.ctx.memory_mut(|m| m.request_focus(egui::Id::new("issue_report_body")));
        h.step(&json!({"text":"Typing here describes the bug, not a CAD dimension."})).unwrap();
        h.step(&json!({"key":"Enter"})).unwrap();
        h.step(&json!({"key":"Escape"})).unwrap();
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
        assert!(crate::dim_view::editing() && h.widget("30 mm").is_some());
        assert!(h.app.issue_report.draft.as_ref().unwrap().description.contains("Typing here describes"));
        h.step(&json!({"widget":"Close draft"})).unwrap();
        h.frames(2);
        assert_eq!(h.ctx.memory(|m| m.focused()), Some(egui::Id::new("sc_dim_edit")));
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::dim_view::editing());
        assert_eq!(h.app.session.undo.len(), before.1 + 1);
        assert_eq!(h.app.session.doc.param("d1").unwrap().expr, "30 mm");
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), before.0);
    }

    #[test]
    fn issue_report_inline_dimension_retains_expression_and_resumes_after_close() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.circle.center", json!({"center":[0,0],"radius":10,"id":"circle"})).unwrap();
        let out = h.app.run("sketch.dimension", json!({"entities":["circle"],"type":"radius","value":10})).unwrap();
        let param = out["param"].as_str().unwrap().to_owned();
        h.frames(3);
        crate::dim_view::edit_param(&h.app, &param);
        h.frames(2);
        h.step(&json!({"text":"12"})).unwrap();
        assert!(crate::dim_view::editing() && h.widget("12").is_some());
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision);
        help::run(&mut h.app, &h.ctx, "report");
        h.frames(3);
        let token = h.app.issue_report.pending.as_ref().unwrap().token;
        receive(&mut h.app, token, Arc::new(ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4])));
        h.frames(3);
        h.ctx.memory_mut(|m| m.request_focus(egui::Id::new("issue_report_body")));
        h.step(&json!({"text":"report only"})).unwrap();
        h.step(&json!({"key":"Enter"})).unwrap();
        h.step(&json!({"key":"Escape"})).unwrap();
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
        assert!(crate::dim_view::editing() && h.widget("12").is_some());
        h.step(&json!({"widget":"Close draft"})).unwrap();
        h.frames(2);
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
        assert!(crate::dim_view::editing() && h.widget("12").is_some());
        h.step(&json!({"key":"Enter"})).unwrap();
        assert!(!crate::dim_view::editing());
        assert_eq!(h.app.session.doc.param(&param).unwrap().expr, "12");
        assert_eq!(h.app.session.undo.len(), before.1 + 1);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn issue_report_native_file_drop_cannot_import_behind_review() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
        h.frames(3);
        let tabs_before = h.app.docs.count();
        assert!(!h.app.file_load.busy());
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision);
        help::run(&mut h.app, &h.ctx, "report");
        let path = h.dir.join("private-design.json");
        std::fs::write(&path, h.app.session.doc.to_json()).unwrap();
        #[derive(Debug)]
        struct File(std::path::PathBuf);
        impl egui::DroppedFile for File {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                std::fs::read(&self.0).map_err(|e| e.to_string())
            }
        }
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            focused: true,
            dropped_files: vec![Arc::new(File(path))],
            ..Default::default()
        };
        let app = &mut h.app;
        let mut output = h.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        output.textures_delta.clear();
        assert_eq!(before, (h.app.session.doc.to_json(), h.app.session.undo.len(), h.app.session.redo.len(), h.app.session.revision));
        assert!(h.app.issue_report.draft.is_some());
        assert!(!h.app.file_load.busy(), "native drop must not queue an asynchronous open behind the draft");
        assert_eq!(h.app.docs.count(), tabs_before);
    }

    #[test]
    fn issue_report_capture_accepts_only_its_matching_unchanged_frame() {
        let mut app = app();
        let ctx = egui::Context::default();
        begin(&mut app, &ctx, None, None);
        let token = app.issue_report.pending.as_ref().unwrap().token;
        let (reply, _receiver) = std::sync::mpsc::channel();
        app.pending_shots.push((token + 1, None, reply, crate::now_ms() + 8000.0));
        app.shot_token = token + 1;
        let image = Arc::new(ColorImage::new([4, 3], vec![egui::Color32::GREEN; 12]));
        receive(&mut app, token + 1, image.clone());
        assert!(pending(&app) && app.issue_report.draft.as_ref().unwrap().image.is_none());
        receive(&mut app, token, image.clone());
        assert!(!pending(&app));
        assert_eq!(app.issue_report.draft.as_ref().unwrap().image.as_ref().unwrap().size, [4, 3]);
        assert_eq!(app.pending_shots.len(), 1, "report cannot consume the control request");
        begin(&mut app, &ctx, None, None);
        let token = app.issue_report.pending.as_ref().unwrap().token;
        app.cam.distance *= 2.0;
        receive(&mut app, token, image);
        let draft = app.issue_report.draft.as_ref().unwrap();
        assert!(draft.image.is_none() && draft.note.contains("changed"));
    }

    #[test]
    fn issue_report_capture_rejects_changed_inputs_timeout_and_malformed_images() {
        let mut app = app();
        combine(&mut app);
        let ctx = egui::Context::default();
        let dialog = app.dialog.clone();
        begin(&mut app, &ctx, dialog.as_ref(), Some("bad Boolean"));
        app.dialog.as_mut().unwrap().extra.insert("direction".into(), json!("negative"));
        tick(&mut app, &ctx);
        assert!(!pending(&app));
        assert!(app.issue_report.draft.as_ref().unwrap().note.contains("changed"));
        let dialog = app.dialog.clone();
        begin(&mut app, &ctx, dialog.as_ref(), None);
        app.issue_report.pending.as_mut().unwrap().deadline = -1.0;
        tick(&mut app, &ctx);
        assert!(app.issue_report.draft.as_ref().unwrap().note.contains("No app frame"));
        let dialog = app.dialog.clone();
        begin(&mut app, &ctx, dialog.as_ref(), None);
        let token = app.issue_report.pending.as_ref().unwrap().token;
        receive(&mut app, token, Arc::new(ColorImage::new([0, 0], vec![])));
        assert!(app.issue_report.draft.as_ref().unwrap().image.is_none());
        assert!(app.issue_report.draft.as_ref().unwrap().note.contains("dimensions"));
    }
}
