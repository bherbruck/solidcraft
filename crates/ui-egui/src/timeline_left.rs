//! The left feature-history panel: named rows, folders, rollback and drag-and-drop.

use egui::{Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::timeline::{
    TimelineKind, clipboard_buttons, delete_feature, delete_features, icon_of, message_button, selected_feature_ids, transport_glyph,
};
use crate::{SolveApp, icons};

const FOLDER_INDENT: f32 = 14.0;
const DROP_LEVEL_ANCHOR: f32 = 12.0;
const DROP_BAR_INSET: f32 = 4.0;

/// What a drag in the timeline is moving.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    Marker,
    Items(DragItems),
}

#[derive(Clone, Debug, PartialEq)]
struct DragItems {
    /// Features selected directly, whose folder membership changes on an indented drop.
    features: Vec<u64>,
    /// Folders selected directly, whose parent changes on an indented drop.
    groups: Vec<u64>,
    /// Pointer position and folder level at drag start. Indentation follows horizontal movement
    /// from this level, independent of whether the row was grabbed by its icon or its text.
    origin_x: f32,
    source_group: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct VerticalSlot {
    y: f32,
    position: usize,
    /// Folder containing an item inserted at this boundary without extra indentation.
    group: Option<u64>,
    /// Folder being exited at this boundary; dragging right keeps the item inside it.
    indent_group: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelineItem {
    Group(u64),
    Feature(u64),
}

impl TimelineItem {
    fn key(self) -> String {
        match self {
            TimelineItem::Group(id) => format!("g:{id}"),
            TimelineItem::Feature(id) => format!("f:{id}"),
        }
    }
}

/// A standalone left feature-history panel, used when the parts browser is hidden.
pub fn left_panel(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    egui::Panel::left("sc_timeline_left")
        .exact_size(250.0)
        .resizable(false)
        .frame(egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin::symmetric(6, 6)))
        .show(ui, |ui| left_timeline(app, ui));
}

/// Vertical feature history with named rows, folders and a horizontal rollback bar. The caller
/// gives this a bounded height so the parts and scene browser can use the space below it.
pub fn left_timeline(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let groups = app.session.doc.timeline_groups.clone();
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.label(RichText::new("FEATURE HISTORY").size(11.0).strong().color(t.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let count = selected_feature_ids(app).len() + app.tree.picked_timeline_groups.len();
            if count > 0 {
                ui.menu_button(format!("{count} selected"), |ui| selection_actions(app, ui, &groups));
            }
        });
    });
    timeline_transport(app, ui);
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("sc_left_timeline_scroll")
        .auto_shrink([false, false])
        .scroll_source(egui::scroll_area::ScrollSource { scroll_bar: true, mouse_wheel: true, ..egui::scroll_area::ScrollSource::NONE })
        .show(ui, |ui| vertical_features(app, ui));
}

fn timeline_transport(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let n = app.session.doc.features.len();
    let marker = app.session.doc.marker.unwrap_or(n);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let controls = [("first", Some(0usize)), ("back", Some(marker.saturating_sub(1))), ("forward", Some((marker + 1).min(n))), ("last", None)];
        for (what, target) in controls {
            let (rect, response) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
            if response.hovered() {
                ui.painter().rect_filled(rect, 3.0, t.hover);
            }
            transport_glyph(ui.painter(), rect.center(), what, t.icon);
            if response
                .on_hover_text(match what {
                    "first" => "Roll back to the start",
                    "back" => "Step back one feature",
                    "forward" => "Step forward one feature",
                    _ => "Play to the end",
                })
                .clicked()
            {
                let params = match target {
                    Some(position) if position < n => json!({"position": position}),
                    _ => json!({}),
                };
                let _ = app.run_heavy("timeline.roll_to", params);
            }
        }
    });
}

fn nearest_vertical_slot(slots: &[VerticalSlot], y: f32) -> Option<VerticalSlot> {
    slots.iter().min_by(|a, b| (a.y - y).abs().total_cmp(&(b.y - y).abs())).copied()
}

fn valid_folder_target(items: &DragItems, groups: &[solidcraft_engine::doc::TimelineGroup], target: u64) -> bool {
    !items.groups.iter().any(|source| *source == target || group_is_descendant(groups, target, *source))
}

/// Resolve the folder implied by a vertical drop. A folder row is always a target. At a
/// folder boundary, horizontal indentation selects every available ancestor rather than only
/// the timeline root or the deepest folder.
fn vertical_drop_group(
    pointer: egui::Pos2,
    slot: VerticalSlot,
    folder_drop_zones: &[(Rect, u64)],
    groups: &[solidcraft_engine::doc::TimelineGroup],
    panel_left: f32,
    drag_x: f32,
    source_group: Option<u64>,
) -> Option<u64> {
    if let Some((_, group)) = folder_drop_zones.iter().find(|(rect, _)| rect.shrink2(vec2(0.0, 5.0)).contains(pointer)) {
        return Some(*group);
    }
    let (deepest, base, start) = match slot.indent_group {
        Some(deepest) => (deepest, slot.group, drop_level_anchor(panel_left, slot.group, groups)),
        None if slot.group == source_group => {
            let source = source_group?;
            (source, None, drop_level_anchor(panel_left, Some(source), groups))
        }
        None => return slot.group,
    };
    let mut path = Vec::new();
    let mut current = Some(deepest);
    let mut steps = 0usize;
    while current != base {
        let Some(id) = current else { return base };
        path.push(id);
        current = groups.iter().find(|group| group.id == id).and_then(|group| group.parent);
        steps = steps.saturating_add(1);
        if steps > groups.len() {
            return base;
        }
    }
    path.reverse();
    let mut target = base;
    let relative_indent_x = start + drag_x;
    // A deeply nested row can be grabbed close enough to the panel edge that there is not
    // enough screen space for one full drag step per ancestor. Moving left therefore also
    // honours the pointer's absolute indentation, so the panel edge always reaches root.
    let indent_x = if drag_x < 0.0 { relative_indent_x.min(pointer.x) } else { relative_indent_x };
    for id in path {
        let threshold = drop_level_anchor(panel_left, Some(id), groups);
        if indent_x >= threshold {
            target = Some(id);
        }
    }
    target
}

fn drop_level_anchor(panel_left: f32, group: Option<u64>, groups: &[solidcraft_engine::doc::TimelineGroup]) -> f32 {
    panel_left + DROP_LEVEL_ANCHOR + group.map(|id| (group_depth(groups, id) + 1) as f32 * FOLDER_INDENT).unwrap_or(0.0)
}

fn vertical_drop_bar_left(panel_left: f32, target: Option<u64>, groups: &[solidcraft_engine::doc::TimelineGroup]) -> f32 {
    target.map(|group| panel_left + DROP_BAR_INSET + (group_depth(groups, group) + 1) as f32 * FOLDER_INDENT).unwrap_or(panel_left + DROP_BAR_INSET)
}

fn vertical_marker(app: &mut SolveApp, ui: &mut egui::Ui, drag: &mut Option<Drag>) {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 7.0), Sense::drag());
    let line = Rect::from_min_max(pos2(rect.left() + 8.0, rect.center().y - 1.5), pos2(rect.right() - 5.0, rect.center().y + 1.5));
    let colour = t.accent;
    ui.painter().rect_filled(line, 1.5, colour);
    ui.painter().circle_filled(pos2(line.right(), line.center().y), 4.0, colour);
    crate::scenario::publish_handle("timeline:rollback", rect.center());
    if response.drag_started() {
        *drag = Some(Drag::Marker);
    }
    response.on_hover_text("Rollback bar: drag to roll the model back; new features are inserted here");
    let _ = app;
}

fn group_for(groups: &[solidcraft_engine::doc::TimelineGroup], feature: u64) -> Option<u64> {
    groups.iter().find(|group| group.features.contains(&feature)).map(|group| group.id)
}

fn selected_drag_items(app: &SolveApp, origin_x: f32, source_group: Option<u64>) -> DragItems {
    DragItems { features: selected_feature_ids(app), groups: app.tree.picked_timeline_groups.clone(), origin_x, source_group }
}

fn drag_press_x(response: &egui::Response, fallback: f32) -> f32 {
    response.ctx.input(|input| input.pointer.press_origin()).map_or(fallback, |origin| origin.x)
}

fn drag_x(items: &DragItems, pointer_x: f32) -> f32 {
    pointer_x - items.origin_x
}

fn select_only(app: &mut SolveApp, item: TimelineItem) {
    match item {
        TimelineItem::Group(id) => {
            app.tree.picked_timeline_groups.clear();
            app.tree.picked_timeline_groups.push(id);
            let _ = app.run("select.clear", json!({}));
        }
        TimelineItem::Feature(id) => {
            app.tree.picked_timeline_groups.clear();
            let _ = app.run("select.set", json!({"items": [{"type": "feature", "id": id}]}));
        }
    }
    app.tree.timeline_anchor = Some(item.key());
}

fn select_item(app: &mut SolveApp, ui: &egui::Ui, item: TimelineItem, order: &[TimelineItem]) {
    let modifiers = ui.input(|input| input.modifiers);
    if modifiers.shift
        && let Some(anchor) = app.tree.timeline_anchor.as_deref()
        && let (Some(from), Some(to)) = (order.iter().position(|entry| entry.key() == anchor), order.iter().position(|entry| *entry == item))
    {
        let (lo, hi) = (from.min(to), from.max(to));
        let range = &order[lo..=hi];
        let mut group_ids: Vec<u64> = range
            .iter()
            .filter_map(|entry| match entry {
                TimelineItem::Group(id) => Some(*id),
                TimelineItem::Feature(_) => None,
            })
            .collect();
        if modifiers.command || modifiers.ctrl {
            group_ids.extend(app.tree.picked_timeline_groups.iter().copied());
            group_ids.sort_unstable();
            group_ids.dedup();
        }
        app.tree.picked_timeline_groups = group_ids;
        let items: Vec<_> = range
            .iter()
            .filter_map(|entry| match entry {
                TimelineItem::Feature(id) => Some(json!({"type": "feature", "id": id})),
                TimelineItem::Group(_) => None,
            })
            .collect();
        if items.is_empty() && !(modifiers.command || modifiers.ctrl) {
            let _ = app.run("select.clear", json!({}));
        } else if !items.is_empty() {
            let _ = app.run("select.set", json!({"items": items, "add": modifiers.command || modifiers.ctrl}));
        }
        return;
    }

    app.tree.timeline_anchor = Some(item.key());
    let additive = modifiers.command || modifiers.ctrl;
    match item {
        TimelineItem::Group(id) => {
            if !additive {
                app.tree.picked_timeline_groups.clear();
                let _ = app.run("select.clear", json!({}));
            }
            if app.tree.picked_timeline_groups.contains(&id) {
                app.tree.picked_timeline_groups.retain(|group| *group != id);
            } else {
                app.tree.picked_timeline_groups.push(id);
            }
        }
        TimelineItem::Feature(id) => {
            if !additive {
                app.tree.picked_timeline_groups.clear();
                let _ = app.run("select.set", json!({"items": [{"type": "feature", "id": id}]}));
            } else {
                let selected = selected_feature_ids(app);
                if selected.contains(&id) {
                    let items: Vec<_> =
                        selected.into_iter().filter(|selected| *selected != id).map(|selected| json!({"type": "feature", "id": selected})).collect();
                    let _ = app.run("select.set", json!({"items": items}));
                } else {
                    let _ = app.run("select.set", json!({"items": [{"type": "feature", "id": id}], "add": true}));
                }
            }
        }
    }
}

fn selection_actions(app: &mut SolveApp, ui: &mut egui::Ui, groups: &[solidcraft_engine::doc::TimelineGroup]) {
    let features = selected_feature_ids(app);
    let selected_groups = app.tree.picked_timeline_groups.clone();
    let has_selection = !features.is_empty() || !selected_groups.is_empty();
    if has_selection {
        if ui.button("Add to New Folder").clicked() {
            let result = app.run("timeline.group", json!({"features": features, "groups": selected_groups}));
            if let Ok(result) = result
                && let Some(group) = result.get("group").and_then(serde_json::Value::as_u64)
            {
                app.tree.collapsed.remove(&format!("timeline_group:{group}"));
                crate::context_menu::start_rename(app, crate::context_menu::RenameWhat::TimelineGroup(group), pos2(0.0, 0.0));
            }
            ui.close();
        }
        let grouped = features.iter().any(|feature| group_for(groups, *feature).is_some()) || !selected_groups.is_empty();
        if grouped && ui.button("Remove from Folder").clicked() {
            let mut params = json!({"group": null});
            if !features.is_empty() {
                params["features"] = json!(features);
            }
            if !selected_groups.is_empty() {
                params["groups"] = json!(selected_groups);
            }
            let _ = app.run("timeline.move_to_group", params);
            ui.close();
        }
    }
    if !app.tree.picked_timeline_groups.is_empty() {
        if !features.is_empty() || !selected_groups.is_empty() {
            ui.separator();
        }
        if ui.button("Delete Selected Folders").on_hover_text("Keep their features and nested folders in the history").clicked() {
            let ids = app.tree.picked_timeline_groups.clone();
            let _ = app.run("timeline.ungroup", json!({"groups": ids}));
            app.tree.picked_timeline_groups.clear();
            ui.close();
        }
    }
    if !features.is_empty() {
        ui.separator();
        if ui.button("Delete Selected Features…").clicked() {
            delete_features(app, features);
            ui.close();
        }
    }
}

/// Delete selected timeline folders while preserving their features and nested folders.
pub fn delete_selected_folders(app: &mut SolveApp) -> bool {
    if app.tree.picked_timeline_groups.is_empty() {
        return false;
    }
    let groups = app.tree.picked_timeline_groups.clone();
    if app.run("timeline.ungroup", json!({"groups": groups})).is_ok() {
        app.tree.picked_timeline_groups.clear();
    }
    true
}

fn group_is_descendant(groups: &[solidcraft_engine::doc::TimelineGroup], group: u64, ancestor: u64) -> bool {
    let mut current = groups.iter().find(|candidate| candidate.id == group).and_then(|candidate| candidate.parent);
    let mut steps = 0usize;
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        steps += 1;
        if steps > groups.len() {
            return true;
        }
        current = groups.iter().find(|candidate| candidate.id == id).and_then(|candidate| candidate.parent);
    }
    false
}

fn group_depth(groups: &[solidcraft_engine::doc::TimelineGroup], id: u64) -> usize {
    let mut depth = 0;
    let mut current = groups.iter().find(|group| group.id == id).and_then(|group| group.parent);
    while let Some(parent) = current {
        depth += 1;
        if depth > groups.len() {
            break;
        }
        current = groups.iter().find(|group| group.id == parent).and_then(|group| group.parent);
    }
    depth
}

fn group_is_open(app: &SolveApp, groups: &[solidcraft_engine::doc::TimelineGroup], id: u64) -> bool {
    let mut current = Some(id);
    let mut steps = 0usize;
    while let Some(group_id) = current {
        if app.tree.collapsed.contains(&format!("timeline_group:{group_id}")) {
            return false;
        }
        steps += 1;
        if steps > groups.len() {
            return false;
        }
        current = groups.iter().find(|group| group.id == group_id).and_then(|group| group.parent);
    }
    true
}

fn group_ancestors_are_open(app: &SolveApp, groups: &[solidcraft_engine::doc::TimelineGroup], id: u64) -> bool {
    let mut current = groups.iter().find(|group| group.id == id).and_then(|group| group.parent);
    let mut steps = 0usize;
    while let Some(group_id) = current {
        if app.tree.collapsed.contains(&format!("timeline_group:{group_id}")) {
            return false;
        }
        steps += 1;
        if steps > groups.len() {
            return false;
        }
        current = groups.iter().find(|group| group.id == group_id).and_then(|group| group.parent);
    }
    true
}

fn group_first_feature(
    groups: &[solidcraft_engine::doc::TimelineGroup],
    features: &[(u64, String, &'static str, bool, &'static str, TimelineKind)],
    id: u64,
) -> Option<usize> {
    let group = groups.iter().find(|group| group.id == id)?;
    let direct = group.features.iter().filter_map(|feature| features.iter().position(|item| item.0 == *feature)).min();
    let children = groups.iter().filter(|child| child.parent == Some(id)).filter_map(|child| group_first_feature(groups, features, child.id)).min();
    direct.into_iter().chain(children).min()
}

fn group_last_feature(
    groups: &[solidcraft_engine::doc::TimelineGroup],
    features: &[(u64, String, &'static str, bool, &'static str, TimelineKind)],
    id: u64,
) -> Option<usize> {
    let group = groups.iter().find(|group| group.id == id)?;
    let direct = group.features.iter().filter_map(|feature| features.iter().position(|item| item.0 == *feature)).max();
    let children = groups.iter().filter(|child| child.parent == Some(id)).filter_map(|child| group_last_feature(groups, features, child.id)).max();
    direct.into_iter().chain(children).max()
}

fn visible_order(
    app: &SolveApp,
    features: &[(u64, String, &'static str, bool, &'static str, TimelineKind)],
    groups: &[solidcraft_engine::doc::TimelineGroup],
) -> Vec<TimelineItem> {
    let mut order = Vec::new();
    let mut shown = std::collections::BTreeSet::new();
    for (index, (id, ..)) in features.iter().enumerate() {
        let mut at_index: Vec<_> = groups.iter().filter(|group| group_first_feature(groups, features, group.id) == Some(index)).collect();
        at_index.sort_by_key(|group| (group_depth(groups, group.id), group.id));
        for group in at_index {
            if group_ancestors_are_open(app, groups, group.id) && shown.insert(group.id) {
                order.push(TimelineItem::Group(group.id));
            }
        }
        let group_id = group_for(groups, *id);
        let open = group_id.is_none_or(|group_id| group_is_open(app, groups, group_id));
        if open {
            order.push(TimelineItem::Feature(*id));
        }
    }
    let mut trailing: Vec<_> = groups.iter().filter(|group| !shown.contains(&group.id)).collect();
    trailing.sort_by_key(|group| (group_first_feature(groups, features, group.id).unwrap_or(usize::MAX), group_depth(groups, group.id), group.id));
    for group in trailing {
        if group_ancestors_are_open(app, groups, group.id) && shown.insert(group.id) {
            order.push(TimelineItem::Group(group.id));
        }
    }
    order
}

fn timeline_group_name(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    group: &solidcraft_engine::doc::TimelineGroup,
    rect: Rect,
    offset: f32,
    t: &Tokens,
) -> bool {
    let editing =
        matches!(app.menu.rename.as_ref().map(|rename| &rename.what), Some(crate::context_menu::RenameWhat::TimelineGroup(id)) if *id == group.id);
    if !editing {
        ui.painter().text(
            pos2(rect.left() + 40.0 + offset, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &group.name,
            egui::FontId::proportional(12.0),
            t.text,
        );
        return false;
    }

    let Some(mut rename) = app.menu.rename.take() else { return false };
    let left = rect.left() + 38.0 + offset;
    let edit_rect = Rect::from_min_max(pos2(left, rect.top() + 1.0), pos2(rect.right() - 3.0, rect.bottom() - 1.0));
    let edit_id = ui.id().with(("timeline_group_rename", group.id));
    let response = ui.put(
        edit_rect,
        egui::TextEdit::singleline(&mut rename.text).id(edit_id).frame(egui::Frame::NONE).text_color(t.text).vertical_align(egui::Align::Center),
    );
    crate::scenario::publish_handle(&format!("timeline-folder-rename:{}", group.name), edit_rect.center());
    if !rename.focused {
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit_id) {
            let end = rename.text.chars().count();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(end))));
            state.store(ui.ctx(), edit_id);
        }
        response.request_focus();
        rename.focused = true;
    }
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let commit = ui.input(|input| input.key_pressed(egui::Key::Enter)) || response.lost_focus();
    if escape {
        // Dropping the shared state cancels the edit and leaves the old name intact.
    } else if commit {
        crate::context_menu::commit_rename(app, &rename);
    } else {
        app.menu.rename = Some(rename);
    }
    true
}

fn vertical_group_row(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    group: &solidcraft_engine::doc::TimelineGroup,
    groups: &[solidcraft_engine::doc::TimelineGroup],
    order: &[TimelineItem],
    drag: &mut Option<Drag>,
    depth: usize,
    position: usize,
    end_position: usize,
    previous_visible_group: Option<u64>,
    slots: &mut Vec<VerticalSlot>,
    drop_zones: &mut Vec<(Rect, u64)>,
) -> bool {
    let t = Tokens::get();
    let key = format!("timeline_group:{}", group.id);
    let open = !app.tree.collapsed.contains(&key);
    let selected = app.tree.picked_timeline_groups.contains(&group.id);
    slots.push(VerticalSlot {
        y: ui.cursor().top(),
        position,
        group: group.parent,
        indent_group: previous_visible_group.filter(|previous| Some(*previous) != group.parent),
    });
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 23.0), Sense::click_and_drag());
    drop_zones.push((rect, group.id));
    slots.push(VerticalSlot { y: rect.bottom(), position: if open { position } else { end_position }, group: Some(group.id), indent_group: None });
    if selected || response.hovered() {
        ui.painter().rect_filled(rect, 3.0, if selected { t.accent_soft } else { t.hover });
    }
    let offset = depth as f32 * 14.0;
    let centre = pos2(rect.left() + 9.0 + offset, rect.center().y);
    let arrow = if open {
        vec![centre + vec2(-4.0, -2.0), centre + vec2(4.0, -2.0), centre + vec2(0.0, 3.0)]
    } else {
        vec![centre + vec2(-2.0, -4.0), centre + vec2(-2.0, 4.0), centre + vec2(3.0, 0.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(arrow, t.text_dim, Stroke::NONE));
    let icon = Rect::from_center_size(pos2(rect.left() + 27.5 + offset, rect.center().y), vec2(17.0, 17.0));
    icons::paint(ui.painter(), icon, "folder", t.icon, t.icon_fill, t.accent);
    let renaming = timeline_group_name(app, ui, group, rect, offset, &t);
    crate::scenario::publish_handle(&format!("timeline-folder:{}", group.name), rect.center());
    let fold_rect = Rect::from_min_max(pos2(rect.left() + offset, rect.top()), pos2(rect.left() + offset + 18.0, rect.bottom()));
    crate::scenario::publish_handle(&format!("timeline-folder-fold:{}", group.name), fold_rect.center());
    let fold = ui.interact(fold_rect, ui.id().with(("timeline_group_fold", group.id)), Sense::click());
    if fold.clicked() {
        if open {
            app.tree.collapsed.insert(key.clone());
        } else {
            app.tree.collapsed.remove(&key);
        }
    } else if !renaming && response.clicked() {
        select_item(app, ui, TimelineItem::Group(group.id), order);
    }
    if !renaming && response.drag_started() {
        if !app.tree.picked_timeline_groups.contains(&group.id) {
            select_only(app, TimelineItem::Group(group.id));
        }
        let origin_x = drag_press_x(&response, rect.center().x);
        *drag = Some(Drag::Items(selected_drag_items(app, origin_x, group.parent)));
    }
    if !renaming && response.double_clicked() {
        crate::context_menu::start_rename(app, crate::context_menu::RenameWhat::TimelineGroup(group.id), rect.center());
    }
    if response.secondary_clicked() && !selected {
        select_only(app, TimelineItem::Group(group.id));
    }
    response.context_menu(|ui| {
        if ui.button("Rename Folder").clicked() {
            crate::context_menu::start_rename(app, crate::context_menu::RenameWhat::TimelineGroup(group.id), rect.center());
            ui.close();
        }
        ui.separator();
        selection_actions(app, ui, groups);
    });
    open
}

fn vertical_features(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    ui.spacing_mut().item_spacing.y = 0.0;
    let features: Vec<_> = app
        .session
        .doc
        .features
        .iter()
        .map(|feature| {
            (feature.id, feature.name.clone(), icon_of(&feature.kind), feature.suppressed, feature.kind.type_name(), TimelineKind::of(&feature.kind))
        })
        .collect();
    let groups = app.session.doc.timeline_groups.clone();
    let order = visible_order(app, &features, &groups);
    let first_indices: std::collections::BTreeMap<u64, Option<usize>> =
        groups.iter().map(|group| (group.id, group_first_feature(&groups, &features, group.id))).collect();
    let end_indices: std::collections::BTreeMap<u64, usize> = groups
        .iter()
        .map(|group| (group.id, group_last_feature(&groups, &features, group.id).map_or(features.len(), |index| index.saturating_add(1))))
        .collect();
    let n = features.len();
    let marker = app.session.doc.marker.unwrap_or(n);
    let from_view = match &app.viewport.hover {
        Some(crate::viewport::Hit::Face { body, .. } | crate::viewport::Hit::Edge { body, .. } | crate::viewport::Hit::Vertex { body, .. }) => {
            app.session.world_state().body(body).map(|body| body.feature)
        }
        _ => None,
    };
    let drag_id = ui.id().with("tl_vertical_drag");
    let mut drag: Option<Drag> = ui.data(|data| data.get_temp(drag_id));
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let mut slots = Vec::new();
    let mut folder_drop_zones = Vec::new();
    let mut shown_groups = std::collections::BTreeSet::new();
    let mut previous_visible_group = None;
    let mut hovered = None;
    let mut edit = None;

    for (index, (id, name, icon, suppressed, ty, kind)) in features.iter().enumerate() {
        let group_id = group_for(&groups, *id);
        let mut at_index: Vec<_> = groups.iter().filter(|group| first_indices.get(&group.id).copied().flatten() == Some(index)).collect();
        at_index.sort_by_key(|group| (group_depth(&groups, group.id), group.id));
        let marker_before_group = index == marker && !at_index.is_empty();
        if marker_before_group {
            let group = at_index.first().and_then(|group| group.parent);
            let indent_group = previous_visible_group.filter(|previous| Some(*previous) != group);
            slots.push(VerticalSlot { y: ui.cursor().top(), position: index, group, indent_group });
            vertical_marker(app, ui, &mut drag);
        }
        let mut rendered_group_header = false;
        for group in at_index {
            if group_ancestors_are_open(app, &groups, group.id) && shown_groups.insert(group.id) {
                vertical_group_row(
                    app,
                    ui,
                    group,
                    &groups,
                    &order,
                    &mut drag,
                    group_depth(&groups, group.id),
                    index,
                    end_indices.get(&group.id).copied().unwrap_or(index),
                    previous_visible_group,
                    &mut slots,
                    &mut folder_drop_zones,
                );
                rendered_group_header = true;
            }
        }
        let group_open = group_id.is_none_or(|id| group_is_open(app, &groups, id));
        let indent_group = (!rendered_group_header).then_some(previous_visible_group).flatten().filter(|previous| Some(*previous) != group_id);
        if index == marker && !marker_before_group {
            slots.push(VerticalSlot { y: ui.cursor().top(), position: index, group: group_open.then_some(group_id).flatten(), indent_group });
            vertical_marker(app, ui, &mut drag);
        }
        if !group_open {
            continue;
        }
        slots.push(VerticalSlot { y: ui.cursor().top(), position: index, group: group_id, indent_group });
        let indent = group_id.map(|id| (group_depth(&groups, id) + 1) as f32 * 14.0).unwrap_or(0.0);
        let has_patterns = matches!(kind, TimelineKind::Sketch) && crate::sketch_pattern_tree::has_patterns(app, *id);
        let pattern_key = format!("timeline/sketch/{id}/patterns");
        let patterns_open = crate::browser::is_open(app, &pattern_key, true);
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click_and_drag());
        let fold_rect = Rect::from_min_size(rect.left_top() + vec2(indent, 0.0), vec2(16.0, rect.height()));
        let on_fold = has_patterns && response.interact_pointer_pos().is_some_and(|p| fold_rect.contains(p));
        crate::scenario::publish_handle(&format!("timeline:{name}"), rect.center());
        let result = app.session.model.result(*id);
        let error = result.and_then(|result| result.error.clone());
        let warning = result.and_then(|result| result.warning.clone());
        let rolled = index >= marker;
        let selected =
            app.session.selection.iter().any(|selection| matches!(selection, solidcraft_engine::Sel::Feature { id: selected } if selected == id));
        let background = if error.is_some() {
            t.timeline_item_error
        } else if selected {
            t.accent_soft
        } else if response.hovered() || from_view == Some(*id) {
            t.hover
        } else {
            Color32::TRANSPARENT
        };
        if background != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, 3.0, background);
        }
        if has_patterns {
            crate::icons::glyphs::disclosure(ui.painter(), fold_rect.shrink(3.0), patterns_open, t.text_dim);
            crate::scenario::publish_handle(&format!("timeline-sketch-fold:{name}"), fold_rect.center());
        }
        let icon_rect =
            Rect::from_center_size(pos2(rect.left() + 13.0 + indent + if has_patterns { 16.0 } else { 0.0 }, rect.center().y), vec2(18.0, 18.0));
        let ink = if rolled || *suppressed { t.border } else { t.icon };
        icons::paint(ui.painter(), icon_rect, icon, ink, if rolled { t.panel_header } else { t.icon_fill }, if rolled { t.border } else { t.accent });
        if *suppressed {
            ui.painter().line_segment([icon_rect.left_bottom(), icon_rect.right_top()], Stroke::new(1.5, t.text_dim));
        }
        let text_colour = if rolled || *suppressed { t.text_dim } else { t.text };
        ui.painter().text(
            pos2(icon_rect.right() + 7.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(12.0),
            text_colour,
        );
        // A failed feature has a mark of its own, not only its colour (#972).
        if error.is_some() {
            let at = Rect::from_min_size(pos2(rect.right() - 15.0, rect.center().y - 6.0), vec2(12.0, 12.0));
            icons::paint(ui.painter(), at, "error", t.panel, t.error, t.error);
        }
        if warning.is_some() && error.is_none() {
            icons::paint(
                ui.painter(),
                Rect::from_min_size(pos2(rect.right() - 15.0, rect.center().y - 6.0), vec2(12.0, 12.0)),
                "warning",
                t.icon,
                t.warning,
                t.warning,
            );
        }
        if response.hovered() {
            hovered = Some(*id);
        }
        if response.drag_started() && !on_fold {
            if !selected {
                select_only(app, TimelineItem::Feature(*id));
            }
            let origin_x = drag_press_x(&response, rect.center().x);
            drag = Some(Drag::Items(selected_drag_items(app, origin_x, group_id)));
        }
        let mut tip = format!("{name}  ({ty})");
        if let Some(error) = &error {
            tip.push_str(&format!("\nError: {error}"));
        }
        if let Some(warning) = &warning {
            tip.push_str(&format!("\nWarning: {warning}"));
        }
        tip.push_str("\nDouble-click to edit · drag to reorder · right-click for more");
        let response = response.on_hover_text(tip);
        if response.clicked() && on_fold {
            crate::browser::toggle(app, &pattern_key, true);
        } else if response.clicked() {
            select_item(app, ui, TimelineItem::Feature(*id), &order);
        }
        if response.double_clicked() && !on_fold {
            edit = Some(*id);
        }
        let sketch = kind.profile_sketch();
        if response.secondary_clicked() && !selected {
            select_only(app, TimelineItem::Feature(*id));
        }
        response.context_menu(|ui| {
            if ui.button("Edit Feature").clicked() {
                edit = Some(*id);
                ui.close();
            }
            if let Some(sketch) = sketch
                && ui.button("Edit Profile Sketch").clicked()
            {
                app.edit_sketch(sketch.0);
                ui.close();
            }
            if ui.button("Rename Feature").clicked() {
                crate::context_menu::start_rename(app, crate::context_menu::RenameWhat::Feature(*id), rect.center());
                ui.close();
            }
            if ui.button(if *suppressed { "Unsuppress Feature" } else { "Suppress Feature" }).clicked() {
                let _ = app.run_heavy("timeline.suppress", json!({"feature": id}));
                ui.close();
            }
            if ui.button("Roll Back Here").clicked() {
                let _ = app.run_heavy("timeline.roll_to", json!({"feature": id}));
                ui.close();
            }
            ui.separator();
            clipboard_buttons(app, ui, *id);
            message_button(app, ui, *id);
            ui.separator();
            selection_actions(app, ui, &groups);
            ui.separator();
            if ui.button("Delete").clicked() {
                delete_feature(app, *id);
                ui.close();
            }
        });
        if has_patterns && patterns_open {
            let depth = group_id.map_or(1, |group| group_depth(&groups, group) + 2);
            crate::sketch_pattern_tree::rows(app, ui, *id, depth, "timeline", None);
        }
        previous_visible_group = group_id;
    }
    let mut trailing: Vec<_> = groups.iter().filter(|group| !shown_groups.contains(&group.id)).collect();
    trailing.sort_by_key(|group| (first_indices.get(&group.id).copied().flatten().unwrap_or(usize::MAX), group_depth(&groups, group.id), group.id));
    for group in trailing {
        if group_ancestors_are_open(app, &groups, group.id) && shown_groups.insert(group.id) {
            vertical_group_row(
                app,
                ui,
                group,
                &groups,
                &order,
                &mut drag,
                group_depth(&groups, group.id),
                n,
                n,
                previous_visible_group,
                &mut slots,
                &mut folder_drop_zones,
            );
        }
    }
    slots.push(VerticalSlot { y: ui.cursor().top(), position: n, group: None, indent_group: previous_visible_group });
    if marker >= n {
        vertical_marker(app, ui, &mut drag);
    }
    crate::dialogs_assembly::timeline_joints_vertical(app, ui);

    if let (Some(active), Some(pointer)) = (drag.as_ref(), pointer)
        && let Some(slot) = nearest_vertical_slot(&slots, pointer.y)
    {
        let folder_target = match active {
            Drag::Items(items) => {
                let panel_left = ui.min_rect().left();
                vertical_drop_group(pointer, slot, &folder_drop_zones, &groups, panel_left, drag_x(items, pointer.x), items.source_group)
                    .filter(|target| valid_folder_target(items, &groups, *target))
            }
            Drag::Marker => None,
        };
        let hovered_folder = folder_target.and_then(|target| {
            folder_drop_zones.iter().find(|(rect, group)| *group == target && rect.shrink2(vec2(0.0, 5.0)).contains(pointer)).map(|(rect, _)| *rect)
        });
        if let Some(rect) = hovered_folder {
            ui.painter().rect_stroke(rect.shrink(1.0), 3.0, Stroke::new(2.0, t.sketch_accent), egui::StrokeKind::Inside);
        } else {
            let left = vertical_drop_bar_left(ui.min_rect().left(), folder_target, &groups);
            ui.painter().line_segment(
                [pos2(left, slot.y), pos2(ui.max_rect().right() - 5.0, slot.y)],
                Stroke::new(2.0, if matches!(active, Drag::Marker) { t.accent } else { t.sketch_accent }),
            );
        }
    }
    if ui.input(|input| input.pointer.any_released())
        && let Some(active) = drag.take()
        && let Some(pointer) = pointer
        && let Some(slot) = nearest_vertical_slot(&slots, pointer.y)
    {
        match active {
            Drag::Marker => {
                let _ = app.run_heavy("timeline.roll_to", if slot.position >= n { json!({}) } else { json!({"position": slot.position}) });
            }
            Drag::Items(items) => {
                let panel_left = ui.min_rect().left();
                let folder_target =
                    vertical_drop_group(pointer, slot, &folder_drop_zones, &groups, panel_left, drag_x(&items, pointer.x), items.source_group);
                let valid_target = folder_target.is_none_or(|target| valid_folder_target(&items, &groups, target));
                if valid_target {
                    let mut params = json!({"group": folder_target, "position": slot.position});
                    if !items.features.is_empty() {
                        params["features"] = json!(items.features);
                    }
                    if !items.groups.is_empty() {
                        params["groups"] = json!(items.groups);
                    }
                    let _ = app.run("timeline.move_many", params);
                    if let Some(group) = folder_target {
                        app.tree.collapsed.remove(&format!("timeline_group:{group}"));
                    }
                }
            }
        }
    }
    ui.data_mut(|data| match drag {
        Some(active) => {
            data.insert_temp(drag_id, active.clone());
        }
        None => data.remove::<Drag>(drag_id),
    });
    app.viewport.hover_feature = hovered;
    if let Some(feature) = edit {
        app.edit_feature(feature);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{Harness, handle_at};

    #[test]
    fn left_history_shows_named_features_and_folders_above_the_browser() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 20, "width": 15, "height": 10, "name": "Base"})).unwrap();
        harness.app.run("solid.fillet", json!({"edges": [[20, 7.5, 10]], "radius": 1, "name": "Round"})).unwrap();
        harness.app.run("timeline.group", json!({"name": "Housing", "features": ["Base", "Round"]})).unwrap();
        harness.frames(4);

        let folder = handle_at("timeline-folder:Housing").unwrap();
        let base = handle_at("timeline:Base").unwrap();
        let round = handle_at("timeline:Round").unwrap();
        let browser = handle_at("row:Untitled").unwrap();
        assert!(folder.x < 250.0 && base.x < 250.0 && round.x < 250.0);
        assert!(folder.y < base.y && base.y < round.y);
        assert!(round.y < browser.y, "the parts/scene browser belongs below the feature history");
    }

    #[test]
    fn history_browser_divider_resizes_and_browser_collapses() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.preferences.timeline_left_height = 180.0;
        harness.frames(4);

        let divider = handle_at("left-history-divider").unwrap();
        let before = harness.app.preferences.timeline_left_height;
        harness.step(&json!({"drag": [[divider.x, divider.y], [divider.x, divider.y + 40.0]]})).unwrap();
        assert!(
            harness.app.preferences.timeline_left_height > before + 20.0,
            "dragging the divider should remember a taller history section: {before} -> {}",
            harness.app.preferences.timeline_left_height
        );

        harness.step(&json!({"click": {"handle": "left-browser-toggle"}})).unwrap();
        assert!(harness.app.preferences.timeline_left_browser_collapsed);
        assert!(handle_at("row:Untitled").is_none(), "the collapsed browser should hide its rows");
        harness.step(&json!({"click": {"handle": "left-browser-toggle"}})).unwrap();
        assert!(!harness.app.preferences.timeline_left_browser_collapsed);
        assert!(handle_at("row:Untitled").is_some(), "expanding the browser should restore its rows");
    }

    #[test]
    fn rollback_bar_above_last_feature_can_be_dragged_to_the_end() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "First"})).unwrap();
        harness
            .app
            .run("solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body", "name": "Last"}))
            .unwrap();
        harness.app.run("timeline.roll_to", json!({"position": 1})).unwrap();
        harness.frames(4);

        let last = handle_at("timeline:Last").unwrap();
        harness.step(&json!({"drag": [{"handle": "timeline:rollback"}, [last.x, last.y + 15.0]]})).unwrap();
        assert_eq!(harness.app.session.doc.marker, None, "dropping below the final row must restore the fully evaluated history");
    }

    #[test]
    fn rollback_bar_before_a_folders_first_feature_is_above_its_header() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "First"})).unwrap();
        harness
            .app
            .run("solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body", "name": "Second"}))
            .unwrap();
        harness.app.run("timeline.group", json!({"name": "Folder", "features": ["First", "Second"]})).unwrap();
        harness.app.run("timeline.roll_to", json!({"position": 0})).unwrap();
        harness.frames(4);

        let marker = handle_at("timeline:rollback").unwrap();
        let folder = handle_at("timeline-folder:Folder").unwrap();
        let first = handle_at("timeline:First").unwrap();
        assert!(marker.y < folder.y && folder.y < first.y, "rollback marker must precede the folder containing the first rolled-back feature");
    }

    #[test]
    fn feature_folder_selection_can_move_as_a_block() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        for (name, body, x) in [("First", "FirstBody", 0), ("Second", "SecondBody", 20), ("Third", "ThirdBody", 40)] {
            harness
                .app
                .run(
                    "solid.box",
                    json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
                )
                .unwrap();
        }
        harness.app.run("timeline.group", json!({"name": "Pair", "features": ["First", "Second"]})).unwrap();
        harness.frames(4);

        let third = handle_at("timeline:Third").unwrap();
        harness.step(&json!({"drag": [{"handle": "timeline-folder:Pair"}, [third.x, third.y + 15.0]]})).unwrap();
        let names: Vec<_> = harness.app.session.doc.features.iter().map(|feature| feature.name.as_str()).collect();
        assert_eq!(names, vec!["Third", "First", "Second"]);
    }

    #[test]
    fn selected_folders_and_features_can_be_dragged_into_a_folder() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "Grouped"})).unwrap();
        harness
            .app
            .run("solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body", "name": "Loose"}))
            .unwrap();
        let group = harness.app.run("timeline.group", json!({"name": "Folder", "features": ["Grouped"]})).unwrap()["group"].as_u64().unwrap();
        let destination = harness.app.run("timeline.group", json!({"name": "Destination"})).unwrap()["group"].as_u64().unwrap();
        harness.frames(4);

        harness.step(&json!({"click": {"handle": "timeline-folder:Folder"}})).unwrap();
        harness.step(&json!({"click": {"handle": "timeline:Loose"}, "ctrl": true})).unwrap();
        assert_eq!(harness.app.tree.picked_timeline_groups, vec![group]);
        assert!(harness
            .app
            .session
            .selection
            .iter()
            .any(|selection| matches!(selection, solidcraft_engine::Sel::Feature { id } if harness.app.session.doc.feature(*id).is_some_and(|feature| feature.name == "Loose"))));
        harness.step(&json!({"drag": [{"handle": "timeline:Loose"}, {"handle": "timeline-folder:Destination"}]})).unwrap();
        let source = harness.app.session.doc.timeline_groups.iter().find(|entry| entry.id == group).unwrap();
        let target = harness.app.session.doc.timeline_groups.iter().find(|entry| entry.id == destination).unwrap();
        let loose = harness.app.session.doc.find_feature("Loose").unwrap().id;
        assert_eq!(source.parent, Some(destination), "the selected folder should nest in the drop target");
        assert!(target.features.contains(&loose), "the selected feature should move into the drop target");
    }

    #[test]
    fn vertical_folder_drop_targets_follow_each_available_indent_level() {
        let inside = VerticalSlot { y: 40.0, position: 1, group: Some(7), indent_group: None };
        let at_end = VerticalSlot { y: 60.0, position: 2, group: None, indent_group: Some(3) };
        let folder_rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 20.0));
        let groups = vec![
            solidcraft_engine::doc::TimelineGroup { id: 1, name: "Outer".into(), parent: None, features: vec![] },
            solidcraft_engine::doc::TimelineGroup { id: 2, name: "Middle".into(), parent: Some(1), features: vec![] },
            solidcraft_engine::doc::TimelineGroup { id: 3, name: "Inner".into(), parent: Some(2), features: vec![] },
            solidcraft_engine::doc::TimelineGroup { id: 7, name: "Target".into(), parent: None, features: vec![] },
        ];

        assert_eq!(
            vertical_drop_group(pos2(80.0, 10.0), at_end, &[(folder_rect, 7)], &groups, 0.0, 80.0, None),
            Some(7),
            "dropping over a folder row"
        );
        assert_eq!(vertical_drop_group(pos2(80.0, 40.0), inside, &[], &groups, 0.0, 80.0, None), Some(7), "dropping between expanded folder items");
        assert_eq!(vertical_drop_group(pos2(20.0, 60.0), at_end, &[], &groups, 0.0, 0.0, None), None);
        assert_eq!(vertical_drop_group(pos2(30.0, 60.0), at_end, &[], &groups, 0.0, 14.0, None), Some(1));
        assert_eq!(vertical_drop_group(pos2(44.0, 60.0), at_end, &[], &groups, 0.0, 28.0, None), Some(2));
        assert_eq!(vertical_drop_group(pos2(58.0, 60.0), at_end, &[], &groups, 0.0, 42.0, None), Some(3));

        let same_inner_row = VerticalSlot { y: 80.0, position: 2, group: Some(3), indent_group: None };
        assert_eq!(
            vertical_drop_group(pos2(44.0, 80.0), same_inner_row, &[], &groups, 0.0, -14.0, Some(3)),
            Some(2),
            "dragging left on the same row should outdent without vertical movement"
        );
        assert_eq!(vertical_drop_bar_left(0.0, Some(2), &groups), 32.0, "the bar should begin before the target level's feature icon");
        let root_drag = DragItems { features: vec![9], groups: vec![], origin_x: 100.0, source_group: None };
        assert_eq!(drag_x(&root_drag, 114.0), 14.0, "every folder level should require the same 14-point drag");
    }

    #[test]
    fn dragging_out_of_a_folder_updates_membership() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "Inside"})).unwrap();
        harness
            .app
            .run("solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body", "name": "Loose"}))
            .unwrap();
        let group = harness.app.run("timeline.group", json!({"name": "Folder", "features": ["Inside"]})).unwrap()["group"].as_u64().unwrap();
        harness.frames(4);

        let loose = handle_at("timeline:Loose").unwrap();
        harness.step(&json!({"drag": [{"handle": "timeline:Inside"}, [loose.x, loose.y + 15.0]]})).unwrap();

        let inside = harness.app.session.doc.find_feature("Inside").unwrap().id;
        let group = harness.app.session.doc.timeline_groups.iter().find(|candidate| candidate.id == group).unwrap();
        assert!(!group.features.contains(&inside), "reordering outside a folder must also remove folder membership");
    }

    #[test]
    fn folder_header_top_edge_is_an_insertion_slot() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "Grouped"})).unwrap();
        harness
            .app
            .run("solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body", "name": "Loose"}))
            .unwrap();
        let group = harness.app.run("timeline.group", json!({"name": "Folder", "features": ["Grouped"]})).unwrap()["group"].as_u64().unwrap();
        harness.frames(4);

        let folder = handle_at("timeline-folder:Folder").unwrap();
        harness.step(&json!({"drag": [{"handle": "timeline:Loose"}, [folder.x, folder.y - 10.0]]})).unwrap();

        let loose = harness.app.session.doc.find_feature("Loose").unwrap().id;
        let group = harness.app.session.doc.timeline_groups.iter().find(|candidate| candidate.id == group).unwrap();
        assert!(!group.features.contains(&loose), "the folder header's top edge must not capture the drop");
        assert_eq!(harness.app.session.doc.features.first().map(|feature| feature.name.as_str()), Some("Loose"));
    }

    #[test]
    fn selected_count_does_not_move_the_history_rows() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "Fixed"})).unwrap();
        harness.frames(4);
        let before = handle_at("timeline:Fixed").unwrap();

        harness.step(&json!({"click": {"handle": "timeline:Fixed"}})).unwrap();
        let after = handle_at("timeline:Fixed").unwrap();
        assert!((after.y - before.y).abs() < 0.5, "selection count shifted the row from {} to {}", before.y, after.y);
    }

    #[test]
    fn new_folder_starts_inline_rename_with_its_name_selected() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "Feature"})).unwrap();
        harness.frames(4);

        harness.step(&json!({"click": {"handle": "timeline:Feature"}})).unwrap();
        harness.step(&json!({"widget": "1 selected"})).unwrap();
        harness.step(&json!({"widget": "Add to New Folder"})).unwrap();
        harness.frames(2);

        let original = harness.app.session.doc.timeline_groups.first().unwrap().name.clone();
        assert!(handle_at(&format!("timeline-folder-rename:{original}")).is_some(), "new folder did not show its inline name editor");
        harness.step(&json!({"text": "Renamed"})).unwrap();
        harness.step(&json!({"key": "Enter"})).unwrap();
        assert_eq!(harness.app.session.doc.timeline_groups.first().map(|group| group.name.as_str()), Some("Renamed"));

        let group = harness.app.session.doc.timeline_groups.first().unwrap().id;
        crate::context_menu::start_rename(&mut harness.app, crate::context_menu::RenameWhat::TimelineGroup(group), pos2(0.0, 0.0));
        harness.frames(2);
        assert!(handle_at("timeline-folder-rename:Renamed").is_some(), "folder rename did not stay in the folder row");
        harness.step(&json!({"text": "Again"})).unwrap();
        harness.step(&json!({"key": "Enter"})).unwrap();
        assert_eq!(harness.app.session.doc.timeline_groups.first().map(|group| group.name.as_str()), Some("Again"));
    }

    #[test]
    fn nested_folder_delete_keeps_parent_and_features() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "Feature"})).unwrap();
        let root = harness.app.run("timeline.group", json!({"name": "Root"})).unwrap()["group"].as_u64().unwrap();
        let child =
            harness.app.run("timeline.group", json!({"name": "Child", "parent": root, "features": ["Feature"]})).unwrap()["group"].as_u64().unwrap();
        harness.frames(4);
        harness.step(&json!({"click": {"handle": "timeline-folder:Child"}})).unwrap();
        harness.step(&json!({"key": "Delete"})).unwrap();
        assert!(harness.app.session.doc.timeline_groups.iter().all(|group| group.id != child));
        assert!(harness.app.session.doc.timeline_groups.iter().any(|group| group.id == root));
        assert!(harness.app.session.doc.find_feature("Feature").is_some());
    }

    #[test]
    fn collapsed_folder_stays_visible_while_its_contents_hide() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        harness.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "name": "Inside"})).unwrap();
        harness.app.run("timeline.group", json!({"name": "Folder", "features": ["Inside"]})).unwrap();
        harness.frames(4);
        assert!(handle_at("timeline:Inside").is_some());

        harness.step(&json!({"click": {"handle": "timeline-folder-fold:Folder"}})).unwrap();
        assert!(handle_at("timeline-folder:Folder").is_some(), "collapsed folder row disappeared");
        assert!(handle_at("timeline:Inside").is_none(), "collapsed folder still showed its contents");
    }

    #[test]
    fn drop_below_a_collapsed_folder_appends_to_that_folder() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        for (name, body, x) in [("First", "FirstBody", 0), ("Second", "SecondBody", 20), ("Loose", "LooseBody", 40)] {
            harness
                .app
                .run(
                    "solid.box",
                    json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
                )
                .unwrap();
        }
        let group = harness.app.run("timeline.group", json!({"name": "Folder", "features": ["First", "Second"]})).unwrap()["group"].as_u64().unwrap();
        harness.frames(4);
        harness.step(&json!({"click": {"handle": "timeline-folder-fold:Folder"}})).unwrap();
        let folder = handle_at("timeline-folder:Folder").unwrap();

        harness.step(&json!({"drag": [{"handle": "timeline:Loose"}, [folder.x, folder.y + 14.0]]})).unwrap();

        let names: Vec<_> = harness.app.session.doc.features.iter().map(|feature| feature.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second", "Loose"], "the dropped feature belongs at the collapsed folder's end");
        let folder = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == group).unwrap();
        let loose = harness.app.session.doc.find_feature("Loose").unwrap().id;
        assert_eq!(folder.features.last(), Some(&loose));
    }

    #[test]
    fn drop_at_nested_folder_bottom_can_choose_the_outer_folder() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        for (name, body, x) in [("Inside", "InsideBody", 0), ("Loose", "LooseBody", 20)] {
            harness
                .app
                .run(
                    "solid.box",
                    json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
                )
                .unwrap();
        }
        let inner = harness.app.run("timeline.group", json!({"name": "Inner", "features": ["Inside"]})).unwrap()["group"].as_u64().unwrap();
        let outer = harness.app.run("timeline.group", json!({"name": "Outer", "groups": [inner]})).unwrap()["group"].as_u64().unwrap();
        harness.frames(4);
        let loose_row = handle_at("timeline:Loose").unwrap();

        harness.step(&json!({"drag": [{"handle": "timeline:Loose"}, [loose_row.x + FOLDER_INDENT, loose_row.y - 12.0]]})).unwrap();

        let loose = harness.app.session.doc.find_feature("Loose").unwrap().id;
        let outer = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == outer).unwrap();
        let inner = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == inner).unwrap();
        assert!(outer.features.contains(&loose), "the intermediate indentation should put the feature directly in the outer folder");
        assert!(!inner.features.contains(&loose));
    }

    #[test]
    fn last_features_in_inner_folder_can_move_out_to_their_parent_without_reordering() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        for (name, body, x) in [("First", "FirstBody", 0), ("Penultimate", "PenultimateBody", 20), ("Last", "LastBody", 40)] {
            harness
                .app
                .run(
                    "solid.box",
                    json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
                )
                .unwrap();
        }
        let inner = harness.app.run("timeline.group", json!({"name": "Inner", "features": ["First", "Penultimate", "Last"]})).unwrap()["group"]
            .as_u64()
            .unwrap();
        let outer = harness.app.run("timeline.group", json!({"name": "Outer", "groups": [inner]})).unwrap()["group"].as_u64().unwrap();
        harness.frames(4);
        harness.step(&json!({"click": {"handle": "timeline:Penultimate"}})).unwrap();
        harness.step(&json!({"click": {"handle": "timeline:Last"}, "ctrl": true})).unwrap();
        let last_row = handle_at("timeline:Last").unwrap();

        let start = last_row;
        let outdented = pos2(start.x - 14.0, start.y);
        let destination = outdented;
        harness.app.synthetic.push(egui::Event::PointerMoved(start));
        harness.frame();
        harness.app.synthetic.push(egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.frame();
        // Cross the drag threshold horizontally and release without moving vertically. The
        // initial movement must select the parent folder while keeping the timeline position.
        harness.app.synthetic.push(egui::Event::PointerMoved(outdented));
        harness.frame();
        harness.app.synthetic.push(egui::Event::PointerButton {
            pos: destination,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.frames(4);

        let moved: Vec<_> = ["Penultimate", "Last"].into_iter().map(|name| harness.app.session.doc.find_feature(name).unwrap().id).collect();
        let outer = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == outer).unwrap();
        let inner = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == inner).unwrap();
        assert!(
            moved.iter().all(|feature| outer.features.contains(feature)),
            "dragging the inner folder's last features left should move them to their parent without moving them down"
        );
        assert!(moved.iter().all(|feature| !inner.features.contains(feature)));
        assert_eq!(
            harness.app.session.doc.features.iter().map(|feature| feature.name.as_str()).collect::<Vec<_>>(),
            vec!["First", "Penultimate", "Last"],
            "outdenting should preserve timeline order"
        );
    }

    #[test]
    fn last_features_in_middle_folder_can_move_to_outer_without_reordering() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        for (name, body, x) in [
            ("InnerFeature", "InnerBody", 0),
            ("MiddleFirst", "MiddleFirstBody", 20),
            ("MiddleLast", "MiddleLastBody", 40),
            ("OuterFeature", "OuterBody", 60),
        ] {
            harness
                .app
                .run(
                    "solid.box",
                    json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
                )
                .unwrap();
        }
        let inner = harness.app.run("timeline.group", json!({"name": "Inner", "features": ["InnerFeature"]})).unwrap()["group"].as_u64().unwrap();
        let middle = harness
            .app
            .run("timeline.group", json!({"name": "Middle", "features": ["MiddleFirst", "MiddleLast"], "groups": [inner]}))
            .unwrap()["group"]
            .as_u64()
            .unwrap();
        let outer = harness.app.run("timeline.group", json!({"name": "Outer", "features": ["OuterFeature"], "groups": [middle]})).unwrap()["group"]
            .as_u64()
            .unwrap();
        harness.frames(4);
        harness.step(&json!({"click": {"handle": "timeline:MiddleFirst"}})).unwrap();
        harness.step(&json!({"click": {"handle": "timeline:MiddleLast"}, "ctrl": true})).unwrap();
        let start = handle_at("timeline:MiddleLast").unwrap();
        let destination = pos2(start.x - FOLDER_INDENT, start.y);

        harness.step(&json!({"drag": [[start.x, start.y], [destination.x, destination.y]]})).unwrap();

        let moved: Vec<_> = ["MiddleFirst", "MiddleLast"].into_iter().map(|name| harness.app.session.doc.find_feature(name).unwrap().id).collect();
        let outer = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == outer).unwrap();
        let middle = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == middle).unwrap();
        assert!(moved.iter().all(|feature| outer.features.contains(feature)), "middle-folder items should move directly into the outer folder");
        assert!(moved.iter().all(|feature| !middle.features.contains(feature)));
        assert_eq!(
            harness.app.session.doc.features.iter().map(|feature| feature.name.as_str()).collect::<Vec<_>>(),
            vec!["InnerFeature", "MiddleFirst", "MiddleLast", "OuterFeature"]
        );
    }

    #[test]
    fn last_feature_in_middle_sibling_folder_can_exit_with_a_vertical_drag() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        for (name, body, x) in [("TopFeature", "TopBody", 0), ("MiddleFeature", "MiddleBody", 20), ("BottomFeature", "BottomBody", 40)] {
            harness
                .app
                .run(
                    "solid.box",
                    json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
                )
                .unwrap();
        }
        harness.app.run("timeline.group", json!({"name": "Top", "features": ["TopFeature"]})).unwrap();
        let middle = harness.app.run("timeline.group", json!({"name": "Middle", "features": ["MiddleFeature"]})).unwrap()["group"].as_u64().unwrap();
        harness.app.run("timeline.group", json!({"name": "Bottom", "features": ["BottomFeature"]})).unwrap();
        harness.frames(4);
        let start = handle_at("timeline:MiddleFeature").unwrap();
        let bottom = handle_at("timeline-folder:Bottom").unwrap();

        harness.step(&json!({"drag": [[start.x, start.y], [start.x, bottom.y - 10.0]]})).unwrap();

        let feature = harness.app.session.doc.find_feature("MiddleFeature").unwrap().id;
        let middle = harness.app.session.doc.timeline_groups.iter().find(|folder| folder.id == middle).unwrap();
        assert!(!middle.features.contains(&feature), "a straight vertical drag to the outside boundary should remove the item from its folder");
        assert!(harness.app.session.doc.timeline_groups.iter().all(|folder| !folder.features.contains(&feature)));
    }

    #[test]
    fn last_nested_feature_can_outdent_two_levels_at_the_same_position() {
        let mut harness = Harness::new();
        harness.app.preferences.timeline_left = true;
        for (name, body, x) in [
            ("Shell", "ShellBody", 0),
            ("Lip", "LipBody", 20),
            ("SnapFit", "SnapFitBody", 40),
            ("Boss1", "Boss1Body", 60),
            ("Boss2", "Boss2Body", 80),
            ("Rest", "RestBody", 100),
        ] {
            harness
                .app
                .run(
                    "solid.box",
                    json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0], "operation": "new_body", "name": name, "body_name": body}),
                )
                .unwrap();
        }
        let inner = harness.app.run("timeline.group", json!({"name": "Folder1", "features": ["Lip", "SnapFit", "Boss1", "Boss2"]})).unwrap()["group"]
            .as_u64()
            .unwrap();
        harness.app.run("timeline.group", json!({"name": "Folder2", "features": ["Shell"], "groups": [inner]})).unwrap();
        harness.frames(4);
        let row = handle_at("timeline:Boss2").unwrap();
        let start = pos2(20.0, row.y);

        harness.step(&json!({"drag": [[start.x, start.y], [6.0, start.y]]})).unwrap();

        let feature = harness.app.session.doc.find_feature("Boss2").unwrap().id;
        assert!(
            harness.app.session.doc.timeline_groups.iter().all(|folder| !folder.features.contains(&feature)),
            "the last nested item should reach root without moving vertically"
        );
        assert_eq!(
            harness.app.session.doc.features.iter().map(|feature| feature.name.as_str()).collect::<Vec<_>>(),
            vec!["Shell", "Lip", "SnapFit", "Boss1", "Boss2", "Rest"]
        );
    }
}
