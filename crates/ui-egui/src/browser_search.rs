//! The browser's search (#1413): a field above the tree that keeps only the rows whose names
//! match, each with its parents (dimmed, opened for the search), chips that narrow it to one
//! kind, and Select all / Hide others for what it found. The matching is the engine's
//! ([`browser_find`]), the same as the `browser.find` command's.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use egui::{Pos2, RichText, Sense, vec2};
use serde_json::json;
use solidcraft_engine::Sel;
use solidcraft_engine::browser_find::{self, Kind, MAX_PATTERN_CHARS, Pattern, Search};
use solidcraft_engine::doc::{Document, FeatureKind};

use crate::SolveApp;
use crate::theme::Tokens;

/// The search, kept with the browser's state.
#[derive(Default)]
pub struct BrowserSearch {
    /// What is typed.
    pub text: String,
    /// The chip picked (`None`: All).
    pub kind: Option<Kind>,
    /// Rows closed during this search; the tree's own folds are left as they were.
    closed: BTreeSet<String>,
    /// What the search found (`None`: no search).
    hits: Option<Hits>,
    /// Ctrl+F: the field takes the keyboard on the next frame.
    focus: bool,
}

impl BrowserSearch {
    /// A search is on: there is text, or a chip is picked.
    pub fn on(&self) -> bool {
        self.hits.is_some()
    }

    pub(crate) fn hits(&self) -> Option<&Hits> {
        self.hits.as_ref()
    }

    /// Ends the search: the tree is shown as it was.
    pub fn clear(&mut self) {
        self.text.clear();
        self.kind = None;
        self.closed.clear();
        self.hits = None;
    }

    pub(crate) fn closed(&self, key: &str) -> bool {
        self.closed.contains(key)
    }

    pub(crate) fn toggle_closed(&mut self, key: &str) {
        if !self.closed.remove(key) {
            self.closed.insert(key.to_string());
        }
    }

    /// Gives the field the keyboard (Ctrl+F).
    pub fn focus(&mut self) {
        self.focus = true;
    }
}

/// What a search found, arranged for drawing the tree.
pub(crate) struct Hits {
    /// The text and chip it was found for.
    text: String,
    kind: Option<Kind>,
    /// The design it looked through: its document and revision, and the sketch being edited
    /// (that hides bodies removed later).
    doc: Arc<Document>,
    revision: u64,
    sketch: Option<u64>,
    pub(crate) search: Search,
    /// The matching rows of each placement (the design itself at the empty path): their keys
    /// in Bodies, Sketches and Construction.
    rows: HashMap<Vec<u64>, [HashSet<String>; 3]>,
    /// The placements listed: the matching ones and every one that holds a match.
    placements: HashSet<Vec<u64>>,
    /// The placements that match themselves.
    matched: HashSet<Vec<u64>>,
    /// The placements with something listed beneath them (a match inside, or a placement that
    /// holds one): only those fold during a search.
    holds: HashSet<Vec<u64>>,
    joints: HashSet<u64>,
}

fn folder_index(folder: &str) -> usize {
    match folder {
        "bodies" => 0,
        "sketches" => 1,
        _ => 2,
    }
}

impl Hits {
    fn new(text: String, kind: Option<Kind>, app: &SolveApp, search: Search) -> Hits {
        let mut hits = Hits {
            text,
            kind,
            doc: app.session.doc.clone(),
            revision: app.session.revision,
            sketch: app.session.active_sketch,
            search: Search::default(),
            rows: HashMap::new(),
            placements: HashSet::new(),
            matched: HashSet::new(),
            holds: HashSet::new(),
            joints: HashSet::new(),
        };
        for f in &search.found {
            for depth in 1..=f.path.len() {
                if let Some(at) = f.path.get(..depth) {
                    hits.placements.insert(at.to_vec());
                    if depth < f.path.len() {
                        hits.holds.insert(at.to_vec());
                    }
                }
            }
            let folder = match f.kind {
                Kind::Component => {
                    hits.matched.insert(f.path.clone());
                    continue;
                }
                Kind::Joint => {
                    hits.joints.extend(f.key.parse::<u64>().ok());
                    continue;
                }
                Kind::Body => 0,
                Kind::Sketch => 1,
                Kind::Construction => 2,
            };
            hits.holds.insert(f.path.clone());
            if let Some(keys) = hits.rows.entry(f.path.clone()).or_default().get_mut(folder) {
                keys.insert(f.key.clone());
            }
        }
        hits.search = search;
        hits
    }

    fn is_for(&self, text: &str, kind: Option<Kind>, app: &SolveApp) -> bool {
        self.text == text
            && self.kind == kind
            && self.revision == app.session.revision
            && self.sketch == app.session.active_sketch
            && Arc::ptr_eq(&self.doc, &app.session.doc)
    }

    pub(crate) fn total(&self) -> usize {
        self.search.total
    }

    /// Is the row with `key` in a folder (bodies, sketches or construction) of the placement at
    /// `path` (empty: the design itself) a match?
    pub(crate) fn row(&self, path: &[u64], folder: &str, key: &str) -> bool {
        self.rows.get(path).and_then(|f| f.get(folder_index(folder))).is_some_and(|keys| keys.contains(key))
    }

    /// Is the placement at `path` listed: it matches, or holds a match?
    pub(crate) fn lists(&self, path: &[u64]) -> bool {
        self.placements.contains(path)
    }

    pub(crate) fn matches(&self, path: &[u64]) -> bool {
        self.matched.contains(path)
    }

    /// Does the placement at `path` list anything beneath it?
    pub(crate) fn holds(&self, path: &[u64]) -> bool {
        self.holds.contains(path)
    }

    pub(crate) fn joint(&self, id: u64) -> bool {
        self.joints.contains(&id)
    }

    /// Something Hide others can keep shown: a body, or a placement (with its bodies).
    fn shows_bodies(&self) -> bool {
        self.search.found.iter().any(|f| matches!(f.kind, Kind::Body | Kind::Component))
    }
}

/// Brings what the search found up to date with the text, the chip and the design: a new
/// search only when one of them changed.
pub(crate) fn refresh(app: &mut SolveApp) {
    let pattern = Pattern::new(&app.tree.search.text);
    let kind = app.tree.search.kind;
    if pattern.is_none() && kind.is_none() {
        app.tree.search.hits = None;
        app.tree.search.closed.clear();
        return;
    }
    if app.tree.search.hits.as_ref().is_some_and(|h| h.is_for(&app.tree.search.text, kind, app)) {
        return;
    }
    // Another text or chip is another search: the rows it closed open again.
    if app.tree.search.hits.as_ref().is_none_or(|h| h.text != app.tree.search.text || h.kind != kind) {
        app.tree.search.closed.clear();
    }
    let kinds: Vec<Kind> = kind.into_iter().collect();
    let search = browser_find::find(&app.session, pattern.as_ref(), &kinds);
    app.tree.search.hits = Some(Hits::new(app.tree.search.text.clone(), kind, app, search));
}

/// The kinds the design has (a chip for each), and the picked one even when it has none left.
fn kinds_present(app: &SolveApp) -> Vec<Kind> {
    let doc = &app.session.doc;
    let has = |k: Kind| match k {
        Kind::Component => !doc.occurrences.is_empty(),
        Kind::Body => !app.session.model.state().bodies.is_empty(),
        Kind::Sketch => doc.features.iter().any(|f| matches!(f.kind, FeatureKind::Sketch { .. })),
        Kind::Construction => doc.features.iter().any(|f| matches!(f.kind, FeatureKind::ConstructionPlane { .. })),
        Kind::Joint => !doc.assembly.joints.is_empty(),
    };
    Kind::ALL.into_iter().filter(|k| has(*k) || app.tree.search.kind == Some(*k)).collect()
}

fn chip_label(kind: Option<Kind>) -> &'static str {
    match kind {
        None => crate::i18n::tr("ui.all"),
        Some(Kind::Component) => crate::i18n::tr("ui.browser_search_components"),
        Some(Kind::Body) => crate::i18n::tr("ui.bodies"),
        Some(Kind::Sketch) => crate::i18n::tr("ui.sketches"),
        Some(Kind::Construction) => crate::i18n::tr("ui.browser_search_construction"),
        Some(Kind::Joint) => crate::i18n::tr("ui.joints"),
    }
}

/// The search field with its count, the kind chips, and the actions on what it found: above
/// the browser's rows, in both browser layouts.
pub(crate) fn bar(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    refresh(app);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
        crate::icons::glyphs::magnifier(ui.painter(), r.center(), t.text_dim);
        let count = app.tree.search.hits().map(|h| {
            let id = if h.search.incomplete { "ui.browser_search_found_more" } else { "ui.browser_search_found" };
            crate::i18n::format(id, &[("count", &h.total().to_string())])
        });
        let count_w =
            count.as_ref().map_or(0.0, |c| ui.painter().layout_no_wrap(c.clone(), egui::FontId::proportional(11.0), t.text_dim).size().x + 4.0);
        let clear_w = if app.tree.search.on() { 18.0 } else { 0.0 };
        let width = (ui.available_width() - count_w - clear_w - 4.0).max(60.0);
        let field = egui::TextEdit::singleline(&mut app.tree.search.text)
            .hint_text(crate::i18n::tr("ui.browser_search_hint"))
            .char_limit(MAX_PATTERN_CHARS)
            .desired_width(width);
        let resp = ui.add(field);
        crate::scenario::publish_handle("browser:search", resp.rect.center());
        if std::mem::take(&mut app.tree.search.focus) {
            resp.request_focus();
        }
        // Esc in the field ends the search (egui gives up the focus on Esc).
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.tree.search.clear();
        }
        resp.on_hover_text(crate::i18n::tr("ui.browser_search_tip"));
        if let Some(count) = count {
            ui.label(RichText::new(count).size(11.0).color(t.text_dim));
        }
        if app.tree.search.on() {
            let (r, resp) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::click());
            let ink = if resp.hovered() { t.text } else { t.text_dim };
            crate::icons::glyphs::cross(ui.painter(), egui::Rect::from_center_size(r.center(), vec2(7.0, 7.0)), egui::Stroke::new(1.4, ink));
            crate::scenario::publish_handle("browser:search:clear", r.center());
            if resp.on_hover_text(crate::i18n::tr("ui.browser_search_clear")).clicked() {
                app.tree.search.clear();
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
        for kind in std::iter::once(None).chain(kinds_present(app).into_iter().map(Some)) {
            let r = ui.selectable_label(app.tree.search.kind == kind, RichText::new(chip_label(kind)).size(11.0));
            crate::scenario::publish_handle(&format!("browser:search:kind:{}", kind.map_or("all", Kind::id)), r.rect.center());
            if r.clicked() {
                app.tree.search.kind = kind;
            }
        }
    });
    // This frame's typing and chips.
    refresh(app);
    let Some(hits) = app.tree.search.hits() else {
        ui.add_space(4.0);
        return;
    };
    let (total, hide) = (hits.total(), hits.shows_bodies());
    if total == 0 {
        ui.add_space(6.0);
        ui.label(RichText::new(crate::i18n::tr("ui.browser_search_none")).size(12.0).color(t.text_dim));
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label = crate::i18n::format("ui.browser_select_all", &[("count", &total.to_string())]);
        let r = ui.small_button(label);
        crate::scenario::publish_handle("browser:search:select-all", r.rect.center());
        if r.on_hover_text(crate::i18n::tr("ui.browser_select_all_tip")).clicked() {
            select_found(app);
        }
        let r = ui.add_enabled(hide, egui::Button::new(crate::i18n::tr("ui.browser_hide_others")).small());
        crate::scenario::publish_handle("browser:search:hide-others", r.rect.center());
        if r.on_hover_text(crate::i18n::tr("ui.browser_hide_others_tip")).clicked() {
            hide_others(app);
        }
    });
    ui.add_space(4.0);
}

/// Select all: everything the search found (joints are picked in the tree).
pub(crate) fn select_found(app: &mut SolveApp) {
    let Some(hits) = app.tree.search.hits() else { return };
    let mut items: Vec<Sel> = Vec::new();
    // A sketch or plane of a component is listed in every placement of it, as one item.
    let (mut features, mut planes) = (HashSet::new(), HashSet::new());
    let mut joints = Vec::new();
    for f in &hits.search.found {
        match &f.item {
            Some(Sel::Feature { id }) if !features.insert(*id) => {}
            Some(Sel::Plane { name }) if !planes.insert(name.clone()) => {}
            Some(item) => items.push(item.clone()),
            None => joints.extend(f.key.parse::<u64>().ok()),
        }
    }
    app.tree.picked_components.clear();
    app.tree.picked_canvases.clear();
    let _ = if items.is_empty() { app.run("select.clear", json!({})) } else { app.run("select.set", json!({ "items": items })) };
    app.tree.picked_joints = joints;
}

/// Hide others: only the bodies the search found, and the bodies of the placements it found,
/// stay shown (the browser's Isolate, on what was found).
pub(crate) fn hide_others(app: &mut SolveApp) {
    let Some(hits) = app.tree.search.hits() else { return };
    let mut keep: Vec<String> = Vec::new();
    for f in &hits.search.found {
        match (&f.kind, &f.item) {
            (Kind::Body, Some(Sel::Body { name })) => keep.push(name.clone()),
            (Kind::Component, _) => keep.extend(app.session.occurrence_bodies(&f.path, true).unwrap_or_default()),
            _ => {}
        }
    }
    if keep.is_empty() {
        return;
    }
    keep.sort_unstable();
    keep.dedup();
    let isolate = crate::context_menu::Item::action("ui.isolate", "Isolate", "").with(json!({ "bodies": keep }));
    crate::context_menu::run_item(app, &isolate, Pos2::ZERO);
}
