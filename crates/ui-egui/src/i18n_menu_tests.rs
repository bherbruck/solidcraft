//! The context menu and the browser build `Item`s out of English literals, and
//! `Locale::label()` can only translate a literal that exists as an English
//! message. A label the catalog has never seen comes straight back as English
//! and stays English in every locale, with nothing in the build to say so.
//! This reads every menu the app can open and fails on the ones it cannot
//! translate.

use std::collections::BTreeSet;

use crate::browser;
use crate::context_menu::{self, Item, Target};
use crate::{Services, SolveApp};
use serde_json::json;
use solidcraft_engine::Session;

fn sample_app() -> SolveApp {
    let mut s = Session::default();
    s.run_script(&solidcraft_engine::sample::script()).expect("sample script");
    SolveApp::new(s, Services::default())
}

/// Every English text `label()` can be asked to translate.
fn english_texts() -> BTreeSet<String> {
    const EN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/locales/en.json"));
    let messages: serde_json::Map<String, serde_json::Value> = serde_json::from_str(EN).expect("en.json is valid JSON");
    messages.values().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

/// Separators carry no text; section headings are real text and are checked.
fn translatable(items: &[Item]) -> impl Iterator<Item = &str> {
    items.iter().map(|i| i.label.as_str()).filter(|l| !l.is_empty())
}

fn take(app: &SolveApp, out: &mut BTreeSet<String>) {
    let state = app.session.model.state();
    let bodies: Vec<String> = state.bodies.iter().map(|b| b.name.clone()).collect();
    let features: Vec<u64> = app.session.doc.features.iter().map(|f| f.id).collect();
    let components: Vec<u64> = app.session.doc.components.iter().map(|c| c.id).collect();
    let groups: Vec<u64> = app.session.doc.browser_groups.iter().map(|g| g.id).collect();

    for target in [Target::Viewport, Target::Origin, Target::Units, Target::Toolbar, Target::Workspace, Target::NamedViews] {
        take_items(out, &context_menu::items(app, &target));
    }
    for name in &bodies {
        take_items(out, &context_menu::items(app, &Target::Body { name: name.clone() }));
    }
    for name in ["Named View1", "Front detail"] {
        take_items(out, &context_menu::items(app, &Target::NamedView { name: name.to_string() }));
    }
    for id in &features {
        take_items(out, &context_menu::items(app, &Target::Sketch { id: *id }));
        take_items(out, &context_menu::items(app, &Target::Plane { id: *id }));
    }
    for id in &components {
        take_items(out, &context_menu::items(app, &Target::Component { id: *id }));
        for folder in ["bodies", "sketches", "construction"] {
            take_items(out, &context_menu::items(app, &Target::Folder { component: *id, folder: folder.to_string() }));
            take_items(out, &browser::folder_items(app, *id, folder));
        }
    }
    for id in &groups {
        take_items(out, &context_menu::items(app, &Target::Group { id: *id }));
        take_items(out, &browser::group_items(app, *id));
    }
    for spec in solidcraft_engine::command_specs().iter() {
        let target = Target::ToolbarCommand { id: spec.id.to_string() };
        take_items(out, &context_menu::items(app, &target));
    }
}

fn take_items(out: &mut BTreeSet<String>, items: &[Item]) {
    for label in translatable(items) {
        out.insert(label.to_string());
    }
}

/// A label the catalog has never seen can never translate, and the item's own
/// text is the only witness. Every menu label the app can draw must therefore
/// exist as an English message. The sample design's names are data, not text,
/// and are left alone.
#[test]
fn every_menu_label_is_in_the_english_catalog() {
    let mut app = sample_app();
    let mut seen = BTreeSet::new();
    take(&app, &mut seen);

    // The labels that depend on state, so their other branch is seen too.
    let names: Vec<String> = app.session.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    app.set_origin_shown(false);
    take(&app, &mut seen);
    for name in &names {
        app.session.visibility.locked_bodies.push(name.clone());
        app.session.visibility.hidden_bodies.push(name.clone());
    }
    take(&app, &mut seen);
    app.session.visibility.locked_bodies.clear();
    app.session.visibility.hidden_bodies.clear();
    take(&app, &mut seen);

    // A menu that stops yielding labels would empty `seen` and turn the
    // whole test into a tautology, so the sweep has to stay broad.
    assert!(seen.len() > 40, "the sweep only reached {} label(s)", seen.len());

    let texts = english_texts();
    let mut missing: Vec<&str> = seen
        .iter()
        // Unit symbols are the same word in every language: `mm` is `mm` in
        // Chinese and in German, so they are not prose to translate.
        .filter(|l| !texts.contains(l.as_str()) && !crate::prefs::UNITS.contains(&l.as_str()))
        .map(String::as_str)
        .collect();
    missing.sort_unstable();
    assert!(missing.is_empty(), "{} menu label(s) have no English catalog entry, so they stay English in every locale: {:?}", missing.len(), missing);
}

/// The control channel and scripts call `run()` outside a render language scope, so the
/// "command done" echo must follow the saved preference rather than the thread-local scope
/// (otherwise its template falls back to English: "撤销 done" instead of "撤销 完成").
#[test]
fn command_done_echo_follows_the_language_preference_without_a_render_scope() {
    let mut app = SolveApp::new(Session::default(), Services::default());
    app.preferences.language = "zh".into();
    app.run("solid.box", json!({ "length": 5, "width": 5, "height": 5 })).expect("box");
    let echo = app.session.log.last().expect("the echo is logged");
    let zh = crate::i18n::Locale::for_language("zh");
    let expected = zh.format("ui.command_done", &[("command", zh.label(crate::command_label("solid.box")))]);
    assert_eq!(echo, &expected);
    assert!(echo.contains("完成"), "{echo}");
    assert!(!echo.ends_with("done"), "{echo}");
}
