use super::*;
use crate::scenario::{Harness, rect_of};
use serde_json::json;

#[test]
fn modes_are_transient_and_available_to_control_and_scenarios() {
    let mut h = Harness::new();
    assert_eq!(h.app.ui.layout, LayoutMode::Floating);
    let prefs = serde_json::to_value(&h.app.preferences).unwrap();
    let doc = h.app.session.doc.to_json();
    h.step(&json!({"call":"ui.set", "params":{"layout":"docked"}})).unwrap();
    assert_eq!(h.app.ui.layout, LayoutMode::Docked);
    h.step(&json!({"call":"ui.set", "params":{"layout":"invalid"},"fail":true})).unwrap();
    assert_eq!(h.app.ui.layout, LayoutMode::Docked);
    h.call("ui.set", json!({"layout":"floating"}));
    assert_eq!(h.app.ui.layout, LayoutMode::Floating);
    assert_eq!(h.app.session.doc.to_json(), doc);
    assert_eq!(serde_json::to_value(&h.app.preferences).unwrap(), prefs);
}

#[test]
fn both_modes_keep_their_geometry_and_fit_clear_of_covered_rects() {
    for mode in [LayoutMode::Docked, LayoutMode::Floating] {
        let mut h = Harness::new();
        h.call("ui.resize", json!({"width":1400,"height":900}));
        h.call("ui.set", json!({"layout":mode}));
        h.app.run("solid.box", json!({"length":40,"width":30,"height":20})).unwrap();
        h.settle();
        let viewport = h.app.viewport.rect.unwrap();
        let workspace = rect_of("ribbon:workspace").unwrap();
        let toolbar = rect_of("toolbar").unwrap();
        if mode == LayoutMode::Floating {
            let ribbon = rect_of("ribbon:card").unwrap();
            assert_eq!(viewport.left(), 0.0);
            assert_eq!(viewport.top(), toolbar::APP_BAR_HEIGHT);
            assert_eq!(workspace.left(), SHELL_GAP);
            assert_eq!(workspace.top(), viewport.top() + SHELL_GAP);
            assert_eq!(ribbon.left() - workspace.right(), SHELL_GAP);
            assert_eq!(ribbon.right(), viewport.right() - SHELL_GAP);
            assert!(!covered(&h.app, pos2(4.0, workspace.center().y)));
            assert!(!covered(&h.app, pos2(workspace.right() + 2.0, workspace.center().y)));
            assert!(covered(&h.app, workspace.center()));
            assert!(covered(&h.app, ribbon.center()));
        } else {
            assert_eq!(toolbar.top(), toolbar::APP_BAR_HEIGHT);
            assert_eq!(viewport.top(), toolbar.bottom());
            assert_eq!(toolbar.height(), toolbar::RIBBON_HEIGHT);
            assert_eq!(workspace.left(), 0.0);
            assert!(covered_rects(&h.app).is_empty());
        }
        h.app.fit_view();
        h.settle();
        let layout = h.app.viewport.layout.as_ref().unwrap();
        for covered in covered_rects(&h.app) {
            assert!(!layout.drawable.intersect(covered).is_positive(), "{layout:?} {covered:?}");
        }
        let projection = crate::viewport::projection(&h.app, viewport);
        for point in [solidcraft_engine::geom::Vec3::ZERO, solidcraft_engine::geom::Vec3::new(40.0, 30.0, 20.0)] {
            assert!(layout.drawable.contains(projection.to_screen(point).unwrap()));
        }
        h.app.start("solid.extrude");
        h.settle();
        if mode == LayoutMode::Floating {
            assert!(h.app.viewport.dialog_rect.unwrap().top() >= h.app.viewport.layout.as_ref().unwrap().axis_view.bottom());
        }
    }
}

#[test]
fn cards_block_model_hover_wheel_and_drag_but_the_gap_remains_viewport() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":40,"width":30,"height":20})).unwrap();
    h.settle();
    let ribbon = rect_of("ribbon:card").unwrap();
    // Blank space at the right, away from the search and command buttons.
    let at = pos2(ribbon.right() - 12.0, ribbon.bottom() - 28.0);
    let camera = h.app.cam;
    let doc = h.app.session.doc.to_json();
    h.call("ui.move", json!({"x":at.x,"y":at.y}));
    assert!(h.app.viewport.hover.is_none());
    assert!(h.app.viewport.mouse.is_none());
    h.call("ui.scroll", json!({"x":at.x,"y":at.y,"delta":120}));
    h.call("ui.drag", json!({"x0":at.x,"y0":at.y,"x1":at.x-30.0,"y1":at.y+15.0,"button":"middle","steps":6}));
    assert_eq!(h.app.cam, camera);
    assert!(h.app.viewport.boxsel.is_none());
    assert_eq!(h.app.session.doc.to_json(), doc);
    let gap = pos2(4.0, ribbon.center().y);
    h.call("ui.move", json!({"x":gap.x,"y":gap.y}));
    assert_eq!(h.app.viewport.mouse, Some(gap));
}

#[test]
fn floating_sketch_palette_and_compass_share_measured_clearance() {
    for style in crate::prefs::NavigatorStyle::ALL {
        for (width, height, scale) in [(1400.0, 900.0, 1.0), (700.0, 720.0, 2.0)] {
            let mut h = Harness::new();
            h.app.preferences.navigator_style = style;
            h.call("ui.resize", json!({"width":width,"height":height}));
            h.call("ui.prefs", json!({"ui_scale":scale}));
            h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
            h.app.run("sketch.line", json!({"points":[[0,0],[20,0]]})).unwrap();
            h.settle();
            h.app.fit_view();
            h.settle();
            let layout = h.app.viewport.layout.as_ref().unwrap();
            let palette = h.app.viewport.palette_rect.unwrap();
            assert!(palette.top() >= layout.axis_view.bottom(), "{style:?} {palette:?} {layout:?}");
            assert!(layout.viewport.expand(1.0).contains_rect(palette), "{palette:?} {layout:?}");
            assert!(palette.contains(crate::scenario::handle_at("sketch_palette:finish").unwrap()));
            let proj = crate::viewport::projection(&h.app, layout.viewport);
            for point in [solidcraft_engine::geom::Vec3::ZERO, solidcraft_engine::geom::Vec3::new(20.0, 0.0, 0.0)] {
                assert!(layout.drawable.contains(proj.to_screen(point).unwrap()), "{layout:?}");
            }
            if width > 1000.0 {
                let content = layout.content;
                h.app.start("solid.extrude");
                h.settle();
                let dialog = h.app.viewport.dialog_rect.unwrap();
                let expected_top = (content.top() + crate::viewport::AXIS_VIEW_CLEARANCE).min((content.bottom() - 360.0).max(content.top()));
                assert!((dialog.left() - content.left()).abs() < 1.0);
                assert!((dialog.top() - expected_top).abs() < 1.0, "left dialogs retain their existing top: {dialog:?}");
            }
        }
    }
}

/// Strict compass clearance conflicts with the palette's existing minimum content height.
/// Keep a reproducer for the later palette layout work instead of changing that layout here.
#[test]
#[ignore = "short floating palette needs a decision about whole-panel scrolling or compass clearance"]
fn short_floating_palette_stays_in_viewport() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width":700,"height":475}));
    h.call("ui.prefs", json!({"ui_scale":2.0}));
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.settle();
    let viewport = h.app.viewport.rect.unwrap();
    let palette = h.app.viewport.palette_rect.unwrap();
    assert!(viewport.contains_rect(palette), "{palette:?} extends beyond {viewport:?}");
}

#[test]
fn selection_drag_released_over_a_card_leaves_no_rubber_band() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width":1400,"height":900}));
    h.settle();
    let ribbon = rect_of("ribbon:card").unwrap();
    let viewport = h.app.viewport.rect.unwrap();
    h.call(
        "ui.drag",
        json!({"x0":viewport.right()-20.0,"y0":viewport.bottom()-100.0,
        "x1":ribbon.right()-12.0,"y1":ribbon.bottom()-28.0,"steps":12}),
    );
    assert!(h.app.viewport.boxsel.is_none());
    assert!(h.app.session.selection.is_empty());
}
