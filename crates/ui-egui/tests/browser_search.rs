//! The browser's search box lists only the items whose names contain the text (#1413).
use solidcraft_ui_egui::scenario::{Harness, handle_at};

/// The sample plate: body Plate, sketches Base, Holes, Boss and Bore.
fn sample() -> Harness {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.app.session.run_script(&solidcraft_engine::sample::script()).unwrap();
    h.frames(3);
    h
}

fn shown(name: &str) -> bool {
    handle_at(&format!("row:{name}")).is_some()
}

#[test]
fn search_lists_matching_items_in_opened_folders() {
    let mut h = sample();
    for name in ["Plate", "Base", "Holes", "Boss", "Bore", "Origin", "Document Settings"] {
        assert!(shown(name), "{name} before searching");
    }
    // A closed Sketches folder opens to show its matches.
    let key = h.app.tree.collapsed.insert("c0/sketches".into());
    assert!(key);
    h.frames(2);
    assert!(!shown("Boss"));
    h.app.tree.search.text = "bo".into();
    h.frames(2);
    assert!(shown("Boss") && shown("Bore"), "the matches show");
    for name in ["Plate", "Base", "Holes", "Origin", "Document Settings"] {
        assert!(!shown(name), "{name} doesn't match `bo`");
    }
    // Case doesn't matter; a body matches too.
    h.app.tree.search.text = "PLATE".into();
    h.frames(2);
    assert!(shown("Plate") && !shown("Boss"));
    // Clearing the search puts everything back, the folder closed as the user left it.
    h.app.tree.search.clear();
    h.frames(2);
    assert!(shown("Plate") && shown("Origin") && !shown("Boss"));
}
