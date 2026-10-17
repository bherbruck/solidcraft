use super::*;
use crate::scenario::{Harness, handle_at, rect_of};
use crate::shell::{LayoutMode, SHELL_GAP};

#[test]
fn browser_card_shrinks_and_scrolls_without_covering_history() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width":1400,"height":900}));
    h.settle();
    let short = rect_of("browser:card").unwrap();
    let history = rect_of("history:card").unwrap();
    assert_eq!(short.left(), SHELL_GAP);
    assert_eq!(short.width(), 250.0);
    assert_eq!(short.top(), rect_of("ribbon:card").unwrap().bottom() + SHELL_GAP);
    assert!(short.bottom() < history.top() - SHELL_GAP);
    assert!(drawn_rows().iter().all(|(_, rect, _, _)| rect.height() == 26.0));

    for i in 0..35 {
        h.app.run("sketch.create", json!({"plane":"XY", "name":format!("Sketch{i}")})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
    }
    h.settle();
    let tall = rect_of("browser:card").unwrap();
    assert!(tall.height() > short.height());
    assert!((tall.bottom() - (history.top() - SHELL_GAP)).abs() < 1.0, "{tall:?} {history:?}");
    assert!(h.app.viewport.rect.unwrap().contains_rect(tall));
    assert!(handle_at("row:Sketch34").is_none(), "last row starts below the scroll clip");
    let camera = h.app.cam;
    h.call("ui.scroll", json!({"x":tall.center().x,"y":tall.center().y,"delta":-1800}));
    h.settle();
    assert!(handle_at("row:Sketch34").is_some(), "scrolling must reveal the last row");
    assert_eq!(h.app.cam, camera);
    assert_eq!(rect_of("browser:card").unwrap(), tall);

    h.call("ui.set", json!({"layout":"docked"}));
    h.settle();
    assert!(rect_of("browser:card").is_none());
    assert_eq!(h.app.viewport.rect.unwrap().left(), 250.0);
    assert!(drawn_rows().iter().all(|(_, rect, _, _)| rect.height() == 26.0));
    h.call("ui.set", json!({"layout":"floating"}));
    h.app.tree.collapsed.insert("c0".into());
    h.settle();
    assert!(rect_of("browser:card").unwrap().height() < short.height());
}

#[test]
fn selected_row_paints_tab_soft_fill_and_accent_text_in_both_layouts() {
    for mode in [LayoutMode::Floating, LayoutMode::Docked] {
        for tab in ["MODEL", "SURFACE", "SKETCH"] {
            let ctx = egui::Context::default();
            let mut app = SolveApp::new(Default::default(), Default::default());
            app.ui.layout = mode;
            app.ui.tab = tab.into();
            crate::theme::TabAccent::publish(&app, &ctx);
            let accent = crate::theme::TabAccent::for_tab(&Tokens::get(), tab);
            let mut row_rect = Rect::NOTHING;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let row = Row { selected: true, label: "Selected body", dim: true, color: Some(Tokens::get().error), ..Default::default() };
                row_rect = draw_row(ui, egui::Id::new("selected-row"), &row).rect.unwrap();
            });
            out.textures_delta.clear();
            assert_eq!(row_rect.height(), 26.0);
            assert!(out.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.rect == row_rect && r.fill == accent.soft)));
            assert!(
                out.shapes
                    .iter()
                    .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "Selected body" && t.fallback_color == accent.color))
            );
        }
    }
}

#[test]
fn card_and_column_keep_body_eyes_menus_and_group_drops() {
    for mode in [LayoutMode::Floating, LayoutMode::Docked] {
        let mut h = Harness::new();
        h.app.ui.layout = mode;
        for (name, x) in [("One", 0), ("Two", 20)] {
            h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"corner":[x,0,0],"operation":"new_body","body_name":name})).unwrap();
        }
        let group =
            h.app.run("browser.group", json!({"component":0,"folder":"bodies","items":["One"],"name":"Group"})).unwrap()["group"].as_u64().unwrap();
        h.settle();
        h.step(&json!({"click":{"handle":"eye:Two"}})).unwrap();
        assert!(h.app.session.visibility.hidden_bodies.iter().any(|name| name == "Two"));
        h.step(&json!({"click":{"handle":"eye:Two"}})).unwrap();
        assert!(!h.app.session.visibility.hidden_bodies.iter().any(|name| name == "Two"));
        h.step(&json!({"click":{"handle":"row:Two"},"button":"right"})).unwrap();
        assert!(matches!(&h.app.menu.target, crate::context_menu::Target::Body { name } if name == "Two"));
        h.step(&json!({"key":"Escape"})).unwrap();
        h.step(&json!({"drag":[{"handle":"row:Two"},{"handle":"row:Group (1)"}]})).unwrap();
        h.settle();
        let group = h.app.session.doc.browser_groups.iter().find(|g| g.id == group).unwrap();
        assert!(group.items.iter().any(|name| name == "Two"), "body drop into the folder failed in {mode:?}");
    }
}

#[test]
fn browser_header_folds_the_floating_card_to_its_header_and_back() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width":1400,"height":900}));
    h.settle();
    let open = rect_of("browser:card").unwrap();
    assert!(!h.app.tree.panel_collapsed);
    let at = handle_at("browser:header").unwrap();
    h.call("ui.click", json!({"x":at.x,"y":at.y}));
    h.settle();
    let folded = rect_of("browser:card").unwrap();
    assert!(h.app.tree.panel_collapsed);
    assert!(folded.height() < 60.0 && folded.height() < open.height(), "{folded:?} {open:?}");
    assert_eq!(folded.top(), open.top());
    assert!(drawn_rows().is_empty() || handle_at("row:Plate").is_none());
    h.call("ui.click", json!({"x":at.x,"y":at.y}));
    h.settle();
    assert!(!h.app.tree.panel_collapsed);
    assert!((rect_of("browser:card").unwrap().height() - open.height()).abs() < 1.0);
}
