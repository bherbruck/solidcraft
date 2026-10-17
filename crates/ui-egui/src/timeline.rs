//! The timeline (bottom): features in order, the rollback marker, errors and warnings.

pub(crate) use crate::icons::glyphs::transport_glyph;
use egui::{Rect, Sense, Stroke, UiBuilder, pos2, vec2};
use serde_json::json;
use solidcraft_engine::doc::{
    FeatureKind, PlaneRef,
    construct::{AxisDef, PointDef},
};

use crate::theme::Tokens;
use crate::{SolveApp, icons};

pub(crate) fn icon_of(k: &FeatureKind) -> &'static str {
    match k {
        FeatureKind::Sketch { .. } => "sketch",
        FeatureKind::Extrude { .. } => "extrude",
        FeatureKind::SurfaceExtrude { .. } => "surface_extrude",
        FeatureKind::Unstitch { .. } => "surface_unstitch",
        FeatureKind::ReverseNormal { .. } => "surface_reverse_normal",
        FeatureKind::SurfaceRevolve { .. } => "surface_revolve",
        FeatureKind::SurfaceSweep { .. } => "surface_sweep",
        FeatureKind::SurfaceLoft { .. } => "surface_loft",
        FeatureKind::SurfaceOffset { .. } => "surface_offset",
        FeatureKind::SurfaceRuled { .. } => "surface_ruled",
        FeatureKind::SurfaceUntrim { .. } => "surface_untrim",
        FeatureKind::SurfaceBlend { .. } => "surface_blend",
        FeatureKind::SurfaceBoundary { .. } => "surface_boundary",
        FeatureKind::SurfaceMatch { .. } => "continuity",
        FeatureKind::SurfaceFair { .. } => "surface_fair",
        FeatureKind::Revolve { .. } => "revolve",
        FeatureKind::Fillet { .. } => "fillet",
        FeatureKind::Chamfer { .. } => "chamfer",
        FeatureKind::Box { .. } => "box",
        FeatureKind::Cylinder { .. } => "cylinder",
        FeatureKind::Sphere { .. } => "sphere",
        FeatureKind::Torus { .. } => "torus",
        FeatureKind::Combine { .. } => "combine",
        FeatureKind::Pattern { pattern: solidcraft_engine::doc::PatternKind::Rectangular { .. }, .. } => "pattern_rect",
        FeatureKind::Pattern { pattern: solidcraft_engine::doc::PatternKind::Path { .. }, .. } => "pattern_path",
        FeatureKind::Pattern { .. } => "pattern_circ",
        FeatureKind::Mirror { .. } => "mirror",
        FeatureKind::ConstructionPlane { plane } => construction_plane_icon(plane),
        FeatureKind::ConstructionAxis { def } => match def {
            AxisDef::ThroughFace { .. } => "axis_through_cylinder",
            AxisDef::NormalToFace { .. } => "axis_normal_to_face",
            AxisDef::TwoPlanes { .. } => "axis_two_planes",
            AxisDef::TwoPoints { .. } => "axis_two_points",
            AxisDef::Edge { .. } => "axis_edge",
        },
        FeatureKind::ConstructionPoint { def } => match def {
            PointDef::At { .. } => "point_vertex",
            PointDef::TwoEdges { .. } => "point_two_edges",
            PointDef::ThreePlanes { .. } => "point_three_planes",
            PointDef::Center { .. } => "point_center",
            PointDef::EdgeAndPlane { .. } => "point_edge_plane",
            PointDef::AlongPath { .. } => "point_along_path",
        },
        FeatureKind::Hole { .. } => "hole",
        FeatureKind::Loft { .. } => "loft",
        FeatureKind::Sweep { .. } => "sweep",
        FeatureKind::Shell { .. } => "shell",
        FeatureKind::Draft { .. } => "draft",
        FeatureKind::Split { .. } => "split",
        FeatureKind::Move { .. } => "move",
        FeatureKind::Import { .. } | FeatureKind::MeshImport { .. } | FeatureKind::MeshConvert { .. } => "import",
        FeatureKind::BodyTransfer { .. } => "move",
        FeatureKind::Thread { .. } => "thread",
        FeatureKind::SplitFace { .. } => "split_face",
        FeatureKind::Scale { .. } => "scale",
        FeatureKind::OffsetFace { .. } => "offset_face",
        FeatureKind::DeleteFace { .. } => "delete_face",
        FeatureKind::Remove { .. } => "delete",
        FeatureKind::BoundingSolid { .. } => "bounding_solid",
        FeatureKind::Pipe { .. } => "pipe",
        FeatureKind::Emboss { .. } => "emboss",
        FeatureKind::ReplaceFace { .. } => "replace_face",
        FeatureKind::Align { .. } => "align",
        FeatureKind::Coil { .. } => "coil",
        FeatureKind::SheetPerforate { .. } => "perforate",
        FeatureKind::BoundaryFill { .. } => "boundary_fill",
        FeatureKind::Patch { .. } => "surface_patch",
        FeatureKind::Stitch { .. } => "surface_stitch",
        FeatureKind::Thicken { .. } => "surface_thicken",
        FeatureKind::SurfaceExtend { .. } => "surface_extend",
        FeatureKind::SurfaceTrim { .. } => "surface_trim",
        other => crate::workspace::feature_icon(other).unwrap_or("feature"),
    }
}

fn construction_plane_icon(plane: &PlaneRef) -> &'static str {
    match plane {
        PlaneRef::AtAngle { .. } => "plane_angle",
        PlaneRef::Midplane { .. } => "plane_midplane",
        PlaneRef::TwoEdges { .. } => "plane_two_edges",
        PlaneRef::ThreePoints { .. } => "plane_three_points",
        PlaneRef::Tangent { .. } => "plane_tangent",
        PlaneRef::AlongPath { .. } => "plane_along_path",
        PlaneRef::Perpendicular { .. } => "plane_perpendicular",
        _ => "plane",
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    Marker,
    Feature(u64),
}

/// Slot index (0..=n) nearest to screen x, given the left edges of the feature buttons.
fn slot_at(xs: &[f32], end: f32, x: f32) -> usize {
    let mut best = (0usize, f32::INFINITY);
    for (i, sx) in xs.iter().chain(std::iter::once(&end)).enumerate() {
        let d = (sx - 2.0 - x).abs();
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// Ask before deleting a feature others depend on; delete right away otherwise.
pub(crate) fn delete_feature(app: &mut SolveApp, id: u64) {
    delete_features(app, vec![id]);
}

pub(crate) fn delete_features(app: &mut SolveApp, ids: Vec<u64>) {
    let mut with = std::collections::BTreeSet::new();
    let mut fail = std::collections::BTreeSet::new();
    for id in &ids {
        let deps = app.session.execute("timeline.dependents", &json!({ "feature": id })).unwrap_or_default();
        for (key, output) in [("deleted_with_it", &mut with), ("would_fail", &mut fail)] {
            output
                .extend(deps.get(key).and_then(|value| value.as_array()).into_iter().flatten().filter_map(|value| value.as_str().map(String::from)));
        }
    }
    if with.is_empty() && fail.is_empty() {
        let _ = app.run_heavy("timeline.delete", json!({ "features": ids }));
    } else {
        app.dialog = Some(crate::dialogs::Dialog::confirm_delete_many(ids, with.into_iter().collect(), fail.into_iter().collect()));
    }
}

/// The selected timeline features, in timeline order.
pub(crate) fn selected_feature_ids(app: &SolveApp) -> Vec<u64> {
    app.session
        .doc
        .features
        .iter()
        .filter(|feature| {
            app.session.selection.iter().any(|selection| matches!(selection, solidcraft_engine::Sel::Feature { id } if *id == feature.id))
        })
        .map(|feature| feature.id)
        .collect()
}

/// What Copy in a feature's menu copies: the selected features when the right-clicked one is
/// among them, else that feature alone.
pub(crate) fn copy_targets(app: &SolveApp, id: u64) -> Vec<u64> {
    let selected = selected_feature_ids(app);
    if selected.contains(&id) { selected } else { vec![id] }
}

/// Copy and Paste in a feature's timeline menu. Paste puts the copies at the history marker.
pub(crate) fn clipboard_buttons(app: &mut SolveApp, ui: &mut egui::Ui, id: u64) {
    if ui.button(crate::i18n::tr_label("Copy")).clicked() {
        let features = copy_targets(app, id);
        let _ = app.run("timeline.copy", json!({ "features": features }));
        ui.close();
    }
    if ui.add_enabled(!app.session.clipboard.is_empty(), egui::Button::new(crate::i18n::tr_label("Paste"))).clicked() {
        let _ = app.run("timeline.paste", json!({}));
        ui.close();
    }
}

/// What a feature's error or warning mark says, as its tooltip shows it (name and type, then
/// the error and the warning), for the clipboard. `None` when it has neither.
pub(crate) fn feature_message(app: &SolveApp, id: u64) -> Option<String> {
    let result = app.session.model.result(id)?;
    if result.error.is_none() && result.warning.is_none() {
        return None;
    }
    let feature = app.session.doc.features.iter().find(|f| f.id == id)?;
    let mut text = format!("{}  ({})", feature.name, feature.kind.type_name());
    if let Some(error) = &result.error {
        text.push_str(&format!("\nError: {error}"));
    }
    if let Some(warning) = &result.warning {
        text.push_str(&format!("\nWarning: {warning}"));
    }
    Some(text)
}

/// Copy Error Message (or Copy Warning Message) in the menu of a feature that has one.
pub(crate) fn message_button(app: &SolveApp, ui: &mut egui::Ui, id: u64) {
    let Some(text) = feature_message(app, id) else { return };
    let failed = app.session.model.result(id).is_some_and(|r| r.error.is_some());
    let label = if failed { "Copy Error Message" } else { "Copy Warning Message" };
    if ui.button(crate::i18n::tr_label(label)).clicked() {
        ui.ctx().copy_text(text);
        ui.close();
    }
}

/// Ctrl+click on a feature: add it to the selection, or take it out.
fn toggle_feature(app: &mut SolveApp, id: u64) {
    let selected = selected_feature_ids(app);
    if selected.contains(&id) {
        let rest: Vec<_> =
            app.session.selection.iter().filter(|s| !matches!(s, solidcraft_engine::Sel::Feature { id: f } if *f == id)).cloned().collect();
        let _ = app.run("select.set", json!({ "items": rest }));
    } else {
        let _ = app.run("select.set", json!({ "items": [{"type": "feature", "id": id}], "add": true }));
    }
}

pub fn timeline(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    egui::Panel::bottom("sc_timeline").exact_size(40.0).frame(egui::Frame::NONE.fill(t.timeline)).show(ui, |ui| {
        let r = ui.max_rect();
        let p = ui.painter().clone();
        p.line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, t.border));
        let n = app.session.doc.features.len();
        let marker = app.session.doc.marker.unwrap_or(n);
        // Playback controls.
        let mut x = r.left() + 10.0;
        let controls = [("first", Some(0usize)), ("back", Some(marker.saturating_sub(1))), ("forward", Some((marker + 1).min(n))), ("last", None)];
        for (what, target) in controls {
            let br = Rect::from_center_size(pos2(x + 11.0, r.center().y), vec2(22.0, 22.0));
            let resp = ui.interact(br, ui.id().with(("tl", what)), Sense::click());
            if resp.hovered() {
                p.rect_filled(br, 3.0, t.hover);
            }
            transport_glyph(&p, br.center(), what, t.icon);
            if resp
                .on_hover_text(match what {
                    "first" => "Roll back to the start",
                    "back" => "Step back one feature",
                    "forward" => "Step forward one feature",
                    _ => "Play to the end",
                })
                .clicked()
            {
                let pos = match target {
                    Some(k) if k < n => json!({"position": k}),
                    _ => json!({}),
                };
                let _ = app.run_heavy("timeline.roll_to", pos);
            }
            x += 24.0;
        }
        x += 14.0;
        // The features scroll horizontally in the space right of the playback controls.
        let strip = Rect::from_min_max(pos2(x, r.top()), r.right_bottom());
        ui.scope_builder(UiBuilder::new().max_rect(strip), |ui| {
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                let p = ui.painter().clone();
                let mut x = ui.max_rect().left();
                let origin = x;
                let feats: Vec<_> = app
                    .session
                    .doc
                    .features
                    .iter()
                    .map(|f| (f.id, f.name.clone(), icon_of(&f.kind), f.suppressed, f.kind.type_name(), f.kind.clone()))
                    .collect();
                // The feature whose body is under the cursor in the viewport.
                let st = app.session.world_state();
                let from_view = match &app.viewport.hover {
                    Some(
                        crate::viewport::Hit::Face { body, .. } | crate::viewport::Hit::Edge { body, .. } | crate::viewport::Hit::Vertex { body, .. },
                    ) => st.body(body).map(|b| b.feature),
                    _ => None,
                };
                let drag_id = ui.id().with("tl_drag");
                let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
                let pointer = ui.input(|i| i.pointer.hover_pos());
                let mut xs: Vec<f32> = Vec::new();
                let mut hovered: Option<u64> = None;
                let mut edit: Option<u64> = None;
                let mut marker_rect: Option<Rect> = None;
                for (i, (id, name, icon, suppressed, ty, kind)) in feats.iter().enumerate() {
                    if i == marker {
                        marker_rect = Some(Rect::from_min_size(pos2(x, r.top() + 5.0), vec2(6.0, r.height() - 10.0)));
                        x += 10.0;
                    }
                    xs.push(x);
                    let br = Rect::from_min_size(pos2(x, r.top() + 6.0), vec2(28.0, 28.0));
                    crate::scenario::publish_handle(&format!("timeline:{name}"), br.center());
                    let resp = ui.interact(br, ui.id().with(("feat", *id)), Sense::click_and_drag());
                    let res = app.session.model.result(*id);
                    let err = res.and_then(|r| r.error.clone());
                    let warn = res.and_then(|r| r.warning.clone());
                    let rolled = i >= marker;
                    let is_it = |s: &solidcraft_engine::Sel| matches!(s, solidcraft_engine::Sel::Feature { id: f } if f == id);
                    let sel = app.session.selection.iter().any(is_it) || app.dialog.as_ref().is_some_and(|d| d.items().iter().any(is_it));
                    if resp.hovered() {
                        hovered = Some(*id);
                    }
                    let bg = if err.is_some() {
                        t.timeline_item_error
                    } else if sel {
                        t.accent_soft
                    } else if resp.hovered() || from_view == Some(*id) {
                        t.hover
                    } else {
                        t.timeline_item
                    };
                    let edge = if err.is_some() {
                        t.error
                    } else if from_view == Some(*id) {
                        t.accent
                    } else {
                        t.border
                    };
                    p.rect(br, 3.0, bg, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
                    let ink = if rolled || *suppressed { t.border } else { t.icon };
                    icons::paint(
                        &p,
                        br.shrink(4.0),
                        icon,
                        ink,
                        if rolled { t.panel_header } else { t.icon_fill },
                        if rolled { t.border } else { t.accent },
                    );
                    if *suppressed {
                        p.line_segment([br.left_bottom() + vec2(4.0, -4.0), br.right_top() + vec2(-4.0, 4.0)], Stroke::new(1.5, t.text_dim));
                    }
                    // A failed feature has a mark of its own, not only its colour (#972).
                    if err.is_some() {
                        icons::paint(&p, Rect::from_min_size(br.right_top() - vec2(10.0, 0.0), vec2(10.0, 10.0)), "error", t.panel, t.error, t.error);
                    }
                    if warn.is_some() && err.is_none() {
                        icons::paint(
                            &p,
                            Rect::from_min_size(br.right_top() - vec2(10.0, 0.0), vec2(10.0, 10.0)),
                            "warning",
                            t.icon,
                            t.warning,
                            t.warning,
                        );
                    }
                    let mut tip = format!("{name}  ({ty})");
                    if let Some(e) = &err {
                        tip += &format!("\nError: {e}");
                    }
                    if let Some(w) = &warn {
                        tip += &format!("\nWarning: {w}");
                    }
                    if let Some(r) = res {
                        tip += &format!("\n{:.1} ms", r.ms);
                    }
                    tip += "\nDouble-click to edit · drag to reorder · right-click for more";
                    if resp.drag_started() {
                        drag = Some(Drag::Feature(*id));
                    }
                    let resp = resp.on_hover_text(tip);
                    if resp.clicked() {
                        // A dialog input that takes features (pattern objects) gets the click.
                        let takes = app.dialog.as_ref().and_then(|d| d.active_input()).is_some_and(|i| i.accept & crate::selection::FEATURES != 0);
                        if takes {
                            if let Some(mut d) = app.dialog.take() {
                                d.pick(&app.session, solidcraft_engine::Sel::Feature { id: *id });
                                app.dialog = Some(d);
                            }
                        } else if ui.input(|i| i.modifiers.command || i.modifiers.ctrl) {
                            toggle_feature(app, *id);
                        } else {
                            let _ = app.run("select.set", json!({"items": [{"type": "feature", "id": id}]}));
                        }
                    }
                    if resp.double_clicked() {
                        edit = Some(*id);
                    }
                    let sketch_of = match kind {
                        FeatureKind::Extrude { sketch, .. } | FeatureKind::Revolve { sketch, .. } | FeatureKind::Sweep { sketch, .. } => {
                            Some(*sketch)
                        }
                        _ => None,
                    };
                    resp.context_menu(|ui| {
                        if ui.button(crate::i18n::tr_label("Edit Feature")).clicked() {
                            edit = Some(*id);
                            ui.close();
                        }
                        if let Some(sk) = sketch_of
                            && ui.button(crate::i18n::tr_label("Edit Profile Sketch")).clicked()
                        {
                            app.edit_sketch(sk.0);
                            ui.close();
                        }
                        if ui.button(crate::i18n::tr_label("Rename")).clicked() {
                            app.dialog = Some(crate::dialogs::Dialog::rename(*id, name));
                            ui.close();
                        }
                        if ui.button(if *suppressed { "Unsuppress Features" } else { "Suppress Features" }).clicked() {
                            let _ = app.run_heavy("timeline.suppress", json!({"feature": id}));
                            ui.close();
                        }
                        if ui.button(crate::i18n::tr_label("Roll History Marker Here")).clicked() {
                            let _ = app.run_heavy("timeline.roll_to", json!({ "feature": id }));
                            ui.close();
                        }
                        if ui.button(crate::i18n::tr_label("Find in Browser")).clicked() {
                            find_in_browser(app, *id);
                            ui.close();
                        }
                        ui.separator();
                        clipboard_buttons(app, ui, *id);
                        message_button(app, ui, *id);
                        ui.separator();
                        if ui.button("Delete").clicked() {
                            delete_feature(app, *id);
                            ui.close();
                        }
                    });
                    x += 32.0;
                }
                let end = x;
                if marker >= feats.len() {
                    marker_rect = Some(Rect::from_min_size(pos2(x, r.top() + 5.0), vec2(6.0, r.height() - 10.0)));
                }
                // The history marker: drag it to roll the model back or forward.
                if let Some(mr) = marker_rect {
                    let resp = ui.interact(mr.expand2(vec2(3.0, 0.0)), ui.id().with("tl_marker"), Sense::drag());
                    if resp.drag_started() {
                        drag = Some(Drag::Marker);
                    }
                    let col = if resp.hovered() || drag == Some(Drag::Marker) { t.accent } else { t.timeline_marker };
                    p.rect_filled(mr, 1.0, col);
                    resp.on_hover_text("History marker: drag to roll the model back; new features go here");
                }
                let content_end = crate::dialogs_assembly::timeline_joints(app, ui, &p, end + 10.0, r);
                // The scroll range follows the drawn content (the buttons don't take layout space themselves).
                ui.allocate_exact_size(vec2((content_end - origin).max(0.0), r.height()), Sense::hover());
                // Drag feedback, and the drop.
                if let (Some(dg), Some(pp)) = (drag.as_ref(), pointer) {
                    let slot = slot_at(&xs, end, pp.x);
                    let sx = xs.get(slot).copied().unwrap_or(end) - 4.0;
                    p.line_segment(
                        [pos2(sx, r.top() + 3.0), pos2(sx, r.bottom() - 3.0)],
                        Stroke::new(2.0, if matches!(dg, Drag::Marker) { t.accent } else { t.sketch_accent }),
                    );
                }
                let released = ui.input(|i| i.pointer.any_released());
                // (Taking the drag ends it even when the pointer has left the window.)
                if released
                    && let Some(dg) = drag.take()
                    && let Some(pp) = pointer
                {
                    let slot = slot_at(&xs, end, pp.x);
                    match dg {
                        Drag::Marker => {
                            let _ = app.run_heavy("timeline.roll_to", if slot >= n { json!({}) } else { json!({ "position": slot }) });
                        }
                        Drag::Feature(id) => {
                            let from = app.session.doc.feature_index(id).unwrap_or(0);
                            // Dropping after itself means the slot shifts by one.
                            let to = if slot > from { slot - 1 } else { slot };
                            if to != from {
                                let _ = app.run_heavy("timeline.reorder", json!({ "feature": id, "position": to }));
                            }
                        }
                    }
                }
                ui.data_mut(|d| match drag {
                    Some(dg) => {
                        d.insert_temp(drag_id, dg.clone());
                    }
                    None => d.remove::<Drag>(drag_id),
                });
                app.viewport.hover_feature = hovered;
                if let Some(id) = edit {
                    app.edit_feature(id);
                }
            });
        });
    });
}

/// Select what a feature made, so the browser shows it.
fn find_in_browser(app: &mut SolveApp, id: u64) {
    let st = app.session.world_state();
    let bodies: Vec<serde_json::Value> = st.bodies.iter().filter(|b| b.feature == id).map(|b| json!({"type": "body", "name": b.name})).collect();
    let items = if bodies.is_empty() { vec![json!({"type": "feature", "id": id})] } else { bodies };
    let _ = app.run("select.set", json!({ "items": items }));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::scenario::Harness;

    #[test]
    fn construction_history_matches_the_tool_that_created_it() {
        use solidcraft_engine::doc::{
            FeatureKind, PlaneRef,
            construct::{AxisDef, GeoRef, PointDef},
        };
        use solidcraft_engine::geom::Vec3;
        let point = || GeoRef::Point { p: Vec3::ZERO };
        let reference = || Box::new(point());
        let mut cases = Vec::new();
        for (id, def) in [
            ("through_cylinder", AxisDef::ThroughFace { face: point() }),
            ("normal_to_face", AxisDef::NormalToFace { face: point(), point: point() }),
            ("two_planes", AxisDef::TwoPlanes { a: point(), b: point() }),
            ("two_points", AxisDef::TwoPoints { a: point(), b: point() }),
            ("edge", AxisDef::Edge { edge: point() }),
        ] {
            cases.push((format!("construct.axis.{id}"), FeatureKind::ConstructionAxis { def }));
        }
        for (id, def) in [
            ("vertex", PointDef::At { point: point() }),
            ("two_edges", PointDef::TwoEdges { a: point(), b: point() }),
            ("three_planes", PointDef::ThreePlanes { a: point(), b: point(), c: point() }),
            ("center", PointDef::Center { of: point() }),
            ("edge_plane", PointDef::EdgeAndPlane { edge: point(), plane: point() }),
            ("along_path", PointDef::AlongPath { path: point(), t: "0.5".into() }),
        ] {
            cases.push((format!("construct.point.{id}"), FeatureKind::ConstructionPoint { def }));
        }
        for (id, plane) in [
            ("midplane", PlaneRef::Midplane { a: reference(), b: reference() }),
            ("two_edges", PlaneRef::TwoEdges { a: reference(), b: reference() }),
            ("three_points", PlaneRef::ThreePoints { a: reference(), b: reference(), c: reference() }),
            ("tangent", PlaneRef::Tangent { face: reference(), at: Vec3::ZERO }),
            ("along_path", PlaneRef::AlongPath { path: reference(), t: "0.5".into() }),
            ("perpendicular", PlaneRef::Perpendicular { plane: reference(), line: reference() }),
        ] {
            cases.push((format!("construct.plane.{id}"), FeatureKind::ConstructionPlane { plane }));
        }
        for (id, feature) in cases {
            let command = solidcraft_engine::find_command(&id).unwrap();
            let icon = super::icon_of(&feature);
            assert_eq!(icon, command.icon, "{id}: history and toolbar disagree");
            assert!(!crate::icon_svg::resolve(icon).placeholder, "{id}");
        }
    }

    #[test]
    fn new_surface_and_web_history_icons_match_their_commands() {
        let cases = [
            ("surface.ruled", json!({"type":"surface_ruled", "body":"A", "edges":[], "distance":"5"})),
            ("surface.untrim", json!({"type":"surface_untrim", "face":{"body":"A", "point":[0,0,0]}})),
            ("surface.unstitch", json!({"type":"unstitch", "body":"A"})),
            ("surface.reverse_normal", json!({"type":"reverse_normal", "body":"A"})),
            ("surface.blend", json!({"type":"surface_blend", "from":{"body":"A", "edges":[]}, "to":{"body":"B", "edges":[]}})),
            ("surface.boundary", json!({"type":"surface_boundary", "sketch":0, "curves":[]})),
            ("surface.match", json!({"type":"surface_match", "body":"A", "target":"B", "edges":[]})),
            ("surface.fair", json!({"type":"surface_fair", "body":"A", "tolerance":"0.001"})),
            ("solid.rib", json!({"type":"rib", "sketch":0, "curves":[], "thickness":"2"})),
            ("solid.web", json!({"type":"rib", "sketch":0, "curves":[], "thickness":"2", "web":true})),
        ];
        for (id, definition) in cases {
            let feature: solidcraft_engine::doc::FeatureKind = serde_json::from_value(definition).unwrap();
            let command = solidcraft_engine::find_command(id).unwrap();
            assert_eq!(super::icon_of(&feature), command.icon, "{id}: history and toolbar disagree");
        }
    }

    #[test]
    fn a_path_pattern_keeps_its_path_icon_in_history() {
        use solidcraft_engine::doc::{FeatureKind, PatternKind};
        let feature = FeatureKind::Pattern {
            features: Vec::new(),
            bodies: Vec::new(),
            pattern: PatternKind::Path {
                path_sketch: solidcraft_engine::doc::SketchId(0),
                path: Vec::new(),
                count: "2".into(),
                spacing: "10".into(),
                extent: false,
                flip: false,
                orient: false,
            },
        };
        assert_eq!(super::icon_of(&feature), solidcraft_engine::find_command("solid.pattern.path").unwrap().icon);
    }

    fn two_boxes(left: bool) -> Harness {
        let mut h = Harness::new();
        h.app.preferences.timeline_left = left;
        h.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "First", "body_name": "One"})).unwrap();
        h.app
            .run(
                "solid.box",
                json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body", "name": "Second", "body_name": "Two"}),
            )
            .unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        h.frames(4);
        h
    }

    fn copied(h: &Harness) -> Vec<String> {
        h.app.session.clipboard.iter().map(|f| f.name.clone()).collect()
    }

    /// Ctrl+click picks two features; Copy in the menu copies both; Paste puts copies of both at
    /// the marker. In the bottom timeline and in the history beside the browser.
    #[test]
    fn the_feature_menu_copies_the_selected_features_and_pastes_them() {
        for left in [false, true] {
            let mut h = two_boxes(left);
            h.step(&json!({"click": {"handle": "timeline:First"}})).unwrap();
            h.step(&json!({"click": {"handle": "timeline:Second"}, "ctrl": true})).unwrap();
            let first = h.app.session.doc.find_feature("First").map(|f| f.id).unwrap();
            let second = h.app.session.doc.find_feature("Second").map(|f| f.id).unwrap();
            assert_eq!(super::selected_feature_ids(&h.app), [first, second], "left: {left}");

            h.step(&json!({"click": {"handle": "timeline:Second"}, "button": "right"})).unwrap();
            h.step(&json!({"widget": "Copy"})).unwrap();
            assert_eq!(copied(&h), ["First", "Second"], "left: {left}");

            h.step(&json!({"click": {"handle": "timeline:Second"}, "button": "right"})).unwrap();
            h.step(&json!({"widget": "Paste"})).unwrap();
            assert_eq!(h.app.session.doc.features.len(), 4, "left: {left}");
            let bodies: Vec<String> = h.app.session.model.state().bodies.iter().map(|b| b.name.clone()).collect();
            assert_eq!(bodies, ["One", "Two", "One (copy)", "Two (copy)"], "left: {left}");
        }
    }

    /// A feature with a warning mark offers Copy Warning Message, which copies what its tooltip
    /// says; a feature with nothing to report doesn't offer it. In both timelines.
    #[test]
    fn the_feature_menu_copies_a_warning_message() {
        for left in [false, true] {
            let mut h = two_boxes(left);
            // Too large for the box: reduced to what fits, which the feature's warning says.
            h.app.run("solid.fillet", json!({"edges": [[5, 0, 10]], "radius": 20, "fit": "reduce", "body": "One"})).unwrap();
            h.frames(4);
            let id = h.app.session.doc.features.last().map(|f| f.id).unwrap();
            let name = h.app.session.doc.features.last().map(|f| f.name.clone()).unwrap();
            let warning = h.app.session.model.result(id).and_then(|r| r.warning.clone()).unwrap();
            h.step(&json!({"click": {"handle": "timeline:First"}, "button": "right"})).unwrap();
            assert!(h.widget("Copy Warning Message").is_none(), "left: {left}");
            h.step(&json!({"key": "Escape"})).unwrap();
            h.step(&json!({"click": {"handle": format!("timeline:{name}")}, "button": "right"})).unwrap();
            let [x, y] = h.widget("Copy Warning Message").unwrap();
            let (request, _reply) = crate::control::ControlRequest::new("ui.click", json!({"x": x, "y": y}));
            crate::control::handle(&mut h.app, &h.ctx, &request);
            let mut copied = Vec::new();
            for _ in 0..8 {
                h.frame();
                copied.extend(h.output.commands.iter().filter_map(|c| match c {
                    egui::OutputCommand::CopyText(text) => Some(text.clone()),
                    _ => None,
                }));
            }
            assert!(copied.iter().any(|t| t.starts_with(&name) && t.contains(&format!("Warning: {warning}"))), "left: {left}: {copied:?}");
        }
    }

    /// Copy on a feature outside the selection copies that feature alone.
    #[test]
    fn copy_on_an_unselected_feature_copies_only_it() {
        let mut h = two_boxes(false);
        h.step(&json!({"click": {"handle": "timeline:First"}})).unwrap();
        h.step(&json!({"click": {"handle": "timeline:Second"}, "button": "right"})).unwrap();
        h.step(&json!({"widget": "Copy"})).unwrap();
        assert_eq!(copied(&h), ["Second"]);
        // Ctrl+click again takes a feature back out of the selection.
        h.step(&json!({"click": {"handle": "timeline:Second"}, "ctrl": true})).unwrap();
        h.step(&json!({"click": {"handle": "timeline:Second"}, "ctrl": true})).unwrap();
        let first = h.app.session.doc.find_feature("First").map(|f| f.id).unwrap();
        assert_eq!(super::selected_feature_ids(&h.app), [first]);
    }
}
