//! Toolbar customisation: which commands each panel shows as buttons, per workspace and tab.
//! A command's menu (right-click on a toolbar button, a panel drop-down row or an S box row)
//! pins it to the toolbar, removes it, or pins it to the S box's favourites; buttons are dragged
//! to reorder them within and between panels; a drop-down row dragged onto the toolbar pins it
//! there, a button dragged off the toolbar (or back into the drop-down) leaves it; the pin icon
//! on a hovered drop-down row pins or unpins; the toolbar's own menu (right-click its empty
//! space) has Customize Toolbar… (a window listing every command of the tab with a check box)
//! and Reset Toolbar; Reset Toolbar restores the tab. Kept with the
//! preferences and exported or imported with the keyboard shortcuts.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solidcraft_engine::{CommandSpec, command_specs};

use crate::SolveApp;
use crate::context_menu::Item;

/// The only workspace so far.
pub const WORKSPACE: &str = "DESIGN";

#[derive(Default, Clone, Debug, PartialEq)]
pub struct ToolbarCustom {
    /// `"<workspace>/<tab>/<panel>"` → the panel's buttons, in order (only panels the user
    /// changed).
    pub panels: BTreeMap<String, Vec<String>>,
    /// The Customize Toolbar window is open (not kept).
    pub customizing: bool,
}

impl ToolbarCustom {
    pub fn prefs(&self) -> Value {
        json!({"version": 2, "panels": self.panels})
    }
    pub fn load(&mut self, v: &Value) {
        let current = v.get("version").and_then(Value::as_u64) == Some(2);
        let panels = if current { v.get("panels").unwrap_or(&Value::Null) } else { v };
        self.panels = panels
            .as_object()
            .map(|o| {
                o.iter()
                    .take(500)
                    .filter_map(|(k, v)| {
                        let ids: Vec<String> = v.as_array()?.iter().take(100).filter_map(|x| x.as_str().map(str::to_string)).collect();
                        Some((k.clone(), ids))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !current {
            self.migrate();
        }
    }
    fn migrate(&mut self) {
        use std::collections::BTreeSet;
        let specs = command_specs();
        let mut pins: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut removals: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let old = std::mem::take(&mut self.panels);
        for (k, ids) in old {
            let parts: Vec<_> = k.split('/').collect();
            let ["DESIGN", tab, panel] = parts.as_slice() else {
                self.panels.insert(k, ids);
                continue;
            };
            if crate::workspace::legacy_panel(tab, panel).is_none() {
                self.panels.insert(k, ids);
                continue;
            }
            if let Some((t, p)) = crate::workspace::legacy_panel(tab, panel) {
                pins.entry(key(t, p)).or_default();
            }
            for id in crate::workspace::legacy_buttons(tab, panel) {
                if !ids.iter().any(|x| x == id)
                    && let Some((t, p)) = crate::workspace::legacy_destination(tab, panel, id)
                {
                    removals.entry(key(t, p)).or_default().insert(id.into());
                }
            }
            for id in ids {
                if let Some((t, p)) = crate::workspace::legacy_destination(tab, panel, &id) {
                    let list = pins.entry(key(t, p)).or_default();
                    if !list.contains(&id) {
                        list.push(id);
                    }
                }
            }
        }
        let touched: BTreeSet<_> = pins.keys().chain(removals.keys()).cloned().collect();
        for k in touched {
            if self.panels.contains_key(&k) {
                continue;
            } // An existing new destination wins.
            let mut ids = pins.remove(&k).unwrap_or_default();
            let parts: Vec<_> = k.split('/').collect();
            if let [_, tab, panel] = parts.as_slice()
                && let Some((cmds, n)) = crate::workspace::layout(tab, panel, &specs)
            {
                for c in cmds.iter().take(n) {
                    if !ids.iter().any(|id| id == c.id) && !removals.get(&k).is_some_and(|r| r.contains(c.id)) {
                        ids.push(c.id.into());
                    }
                }
            }
            self.panels.insert(k, ids);
        }
    }
}

fn key(tab: &str, panel: &str) -> String {
    format!("{WORKSPACE}/{tab}/{panel}")
}

/// The buttons a panel shows: the user's list, or the first `promoted` commands of the panel.
pub fn buttons(app: &SolveApp, tab: &str, panel: &str, cmds: &[&'static CommandSpec], promoted: usize) -> Vec<&'static CommandSpec> {
    match app.toolbar_custom.panels.get(&key(tab, panel)) {
        Some(ids) => ids.iter().filter_map(|id| solidcraft_engine::find_command(id)).collect(),
        None => cmds.iter().take(promoted).copied().collect(),
    }
}

/// The panels of a tab with their commands and default button counts (what the toolbar draws).
pub fn panel_lists(tab: &str) -> Vec<(&'static str, Vec<&'static CommandSpec>, usize)> {
    let specs = command_specs();
    crate::toolbar::panels(tab)
        .iter()
        .filter_map(|panel| {
            let (cmds, n) = crate::workspace::layout(tab, panel, &specs)?;
            (!cmds.is_empty() || *panel == "SELECT").then_some((*panel, cmds, n))
        })
        .collect()
}

/// The buttons of a panel as ids, made the user's own list (so it can change).
fn own(app: &mut SolveApp, tab: &str, panel: &str) -> Vec<String> {
    let k = key(tab, panel);
    if let Some(v) = app.toolbar_custom.panels.get(&k) {
        return v.clone();
    }
    let lists = panel_lists(tab);
    let ids: Vec<String> =
        lists.iter().find(|(p, ..)| *p == panel).map(|(_, cmds, n)| cmds.iter().take(*n).map(|c| c.id.to_string()).collect()).unwrap_or_default();
    ids
}

/// Where a command is a button on a tab now: its panel.
pub fn pinned_on(app: &SolveApp, tab: &str, id: &str) -> Option<&'static str> {
    panel_lists(tab).into_iter().find(|(panel, cmds, n)| buttons(app, tab, panel, cmds, *n).iter().any(|c| c.id == id)).map(|(p, ..)| p)
}

/// The panel a command belongs to on a tab (its drop-down), or the tab's first panel.
fn home_panel(tab: &str, id: &str) -> Option<&'static str> {
    let lists = panel_lists(tab);
    lists.iter().find(|(_, cmds, _)| cmds.iter().any(|c| c.id == id)).or(lists.first()).map(|(p, ..)| *p)
}

/// Pin a command to the toolbar of a tab (as a button of its panel, at the end).
pub fn pin(app: &mut SolveApp, tab: &str, id: &str) -> Result<(), String> {
    if solidcraft_engine::find_command(id).is_none() {
        return Err(format!("no command `{id}`"));
    }
    if pinned_on(app, tab, id).is_some() {
        return Ok(());
    }
    let panel = home_panel(tab, id).ok_or_else(|| format!("the {tab} tab has no panels"))?;
    let mut ids = own(app, tab, panel);
    ids.push(id.to_string());
    app.toolbar_custom.panels.insert(key(tab, panel), ids);
    Ok(())
}

/// Remove a command's button from a tab (it stays in its panel's drop-down).
pub fn remove(app: &mut SolveApp, tab: &str, id: &str) {
    let Some(panel) = pinned_on(app, tab, id) else { return };
    let mut ids = own(app, tab, panel);
    ids.retain(|x| x != id);
    app.toolbar_custom.panels.insert(key(tab, panel), ids);
}

/// Move a button before another one (`before`: None puts it at the end of `panel`).
pub fn reorder(app: &mut SolveApp, tab: &str, id: &str, panel: &str, before: Option<&str>) {
    if let Some(from) = pinned_on(app, tab, id) {
        let mut ids = own(app, tab, from);
        ids.retain(|x| x != id);
        app.toolbar_custom.panels.insert(key(tab, from), ids);
    }
    let mut ids = own(app, tab, panel);
    ids.retain(|x| x != id);
    let at = before.and_then(|b| ids.iter().position(|x| x == b)).unwrap_or(ids.len());
    ids.insert(at, id.to_string());
    app.toolbar_custom.panels.insert(key(tab, panel), ids);
}

/// The pin icon of a drop-down row: pin the command, or remove it when it is pinned.
pub fn toggle(app: &mut SolveApp, tab: &str, id: &str) {
    if pinned_on(app, tab, id).is_some() {
        remove(app, tab, id);
    } else if let Err(e) = pin(app, tab, id) {
        app.set_status(e, true);
    }
}

/// Reset Toolbar: a tab's panels back to their defaults.
pub fn reset(app: &mut SolveApp, tab: &str) {
    let prefix = format!("{WORKSPACE}/{tab}/");
    app.toolbar_custom.panels.retain(|k, _| !k.starts_with(&prefix));
}

/// The menu of a command (toolbar button, drop-down row, S box row).
pub fn command_items(app: &SolveApp, id: &str) -> Vec<Item> {
    let tab = app.ui.tab.clone();
    let on = pinned_on(app, &tab, id).is_some();
    let fav = app.sbox.pinned.iter().any(|p| p == id);
    let mut v = Vec::new();
    v.push(if on {
        Item::action("ui.toolbarRemove", "Remove from Toolbar", "").with(json!({ "command": id }))
    } else {
        Item::action("ui.toolbarPin", "Pin to Toolbar", "").with(json!({ "command": id }))
    });
    v.push(Item::action("ui.shortcutPin", if fav { "Unpin from Shortcuts" } else { "Pin to Shortcuts" }, "").with(json!({ "command": id })));
    v.push(Item::sep());
    v.push(Item::action("ui.toolbarCustomize", "Customize Toolbar…", ""));
    v.push(Item::action("ui.toolbarReset", "Reset Toolbar", "").with(json!({ "tab": tab })));
    v
}

/// The toolbar's own menu (right-click on its empty space).
pub fn toolbar_items(app: &SolveApp) -> Vec<Item> {
    vec![
        Item::action("ui.toolbarCustomize", "Customize Toolbar…", ""),
        Item::action("ui.toolbarReset", "Reset Toolbar", "").with(json!({ "tab": app.ui.tab })),
    ]
}

/// Run a toolbar item from a menu.
pub fn run(app: &mut SolveApp, action: &str, p: &Value) {
    let tab = p.get("tab").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| app.ui.tab.clone());
    let id = p.get("command").and_then(Value::as_str).unwrap_or("");
    match action {
        "ui.toolbarPin" => {
            if let Err(e) = pin(app, &tab, id) {
                app.set_status(e, true);
            }
        }
        "ui.toolbarRemove" => remove(app, &tab, id),
        "ui.toolbarReset" => reset(app, &tab),
        "ui.toolbarCustomize" => app.toolbar_custom.customizing = true,
        "ui.shortcutPin" => app.sbox.toggle_pin(id),
        _ => {}
    }
}

/// Shortcut keys, S box favourites and the toolbar as one file (Keyboard Shortcuts › Export).
pub fn export(app: &SolveApp) -> Value {
    json!({ "format": "solidcraft-shortcuts/1", "shortcuts": app.keymap.prefs(), "favourites": app.sbox.pinned, "toolbar": app.toolbar_custom.prefs() })
}

pub fn import(app: &mut SolveApp, v: &Value) -> Result<(), String> {
    // "solvecraft-shortcuts/1": files exported before the rename (compatibility shim).
    if !matches!(v.get("format").and_then(Value::as_str), Some("solidcraft-shortcuts/1" | "solvecraft-shortcuts/1")) {
        return Err("not a SolidCraft shortcuts file".into());
    }
    if let Some(s) = v.get("shortcuts") {
        app.keymap.load(s);
    }
    if let Some(f) = v.get("favourites").and_then(Value::as_array) {
        app.sbox.pinned = f.iter().take(64).filter_map(|x| x.as_str().map(str::to_string)).collect();
    }
    if let Some(t) = v.get("toolbar") {
        app.toolbar_custom.load(t);
    }
    Ok(())
}

/// Customize Toolbar: every command of the tab's panels with a check box (on the toolbar or
/// not), how to drag them, and Reset Toolbar.
pub fn window(app: &mut SolveApp, ctx: &egui::Context) {
    if !app.toolbar_custom.customizing {
        return;
    }
    let t = crate::theme::Tokens::get();
    let tab = app.ui.tab.clone();
    let mut open = true;
    let mut changes: Vec<(&'static str, bool)> = Vec::new();
    let mut reset_tab = false;
    crate::frame::window(ctx, crate::i18n::tr_label("Customize Toolbar"), crate::frame::Width::Normal)
        .id(egui::Id::new("sc_toolbar_customize"))
        .open(&mut open)
        .default_height(520.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} tab. Tick a command to give it a toolbar button. You can also drag a row of a panel's drop-down onto the toolbar, drag a button off the toolbar to remove it, or use the pin on a drop-down row.",
                    app.tr_label(&tab)
                ))
                .color(t.text_dim),
            );
            ui.separator();
            egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                for (panel, cmds, n) in panel_lists(&tab) {
                    if cmds.is_empty() {
                        continue;
                    }
                    ui.label(egui::RichText::new(app.tr_label(panel)).strong().size(11.0));
                    let shown = buttons(app, &tab, panel, &cmds, n);
                    for c in &cmds {
                        let mut on = shown.iter().any(|b| b.id == c.id) || pinned_on(app, &tab, c.id).is_some();
                        let r = ui.checkbox(&mut on, app.tr_label(c.label));
                        crate::scenario::publish_handle(&format!("customize:{}", c.id), r.rect.left_center() + egui::vec2(8.0, 0.0));
                        if r.changed() {
                            changes.push((c.id, on));
                        }
                    }
                    ui.add_space(4.0);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::tr_label("Reset Toolbar")).clicked() {
                    reset_tab = true;
                }
            });
        });
    for (id, on) in changes {
        if on {
            if let Err(e) = pin(app, &tab, id) {
                app.set_status(e, true);
            }
        } else {
            remove(app, &tab, id);
        }
    }
    if reset_tab {
        reset(app, &tab);
    }
    app.toolbar_custom.customizing = open;
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn legacy_pins_split_merge_and_preserve_removals_and_unknown_keys() {
        let mut custom = ToolbarCustom::default();
        custom.load(&json!({
            "DESIGN/SOLID/CREATE":["solid.hole","solid.revolve","solid.move"],
            "DESIGN/PLASTIC/CREATE":["plastic.boss","plastic.rest","sketch.create"],
            "DESIGN/SOLID/MODIFY":["solid.chamfer"],
            "DESIGN/UTILITIES/SELECT":[],
            "DESIGN/future/panel":["future.command"],
            "OTHER/SOLID/CREATE":["solid.extrude"]
        }));
        let create = &custom.panels["DESIGN/MODEL/CREATE"];
        assert_eq!(create, &["plastic.boss", "plastic.rest", "solid.hole", "solid.revolve"]);
        assert!(!create.iter().any(|id| id == "solid.extrude"));
        assert!(custom.panels["DESIGN/MODEL/ARRANGE"].contains(&"solid.move".into()));
        assert!(!custom.panels["DESIGN/MODEL/MODIFY"].contains(&"solid.fillet".into()));
        assert!(custom.panels["DESIGN/MODEL/MODIFY"].contains(&"solid.chamfer".into()));
        assert_eq!(custom.panels["DESIGN/future/panel"], vec!["future.command"]);
        assert!(custom.panels.contains_key("OTHER/SOLID/CREATE"));
        assert!(!custom.panels.contains_key("DESIGN/SOLID/CREATE"));
        let saved = custom.prefs();
        let mut reloaded = ToolbarCustom::default();
        reloaded.load(&saved);
        assert_eq!(custom, reloaded, "migration runs once, even for unchanged SKETCH/SURFACE keys");
    }

    #[test]
    fn empty_old_panels_keep_default_button_removals() {
        let mut custom = ToolbarCustom::default();
        custom.load(&json!({"DESIGN/SOLID/CREATE":[],"DESIGN/SKETCH/CONSTRAINTS":[],"DESIGN/PLASTIC/SETUP":[]}));
        assert!(!custom.panels["DESIGN/MODEL/SKETCH"].contains(&"sketch.create".into()));
        assert!(!custom.panels["DESIGN/MODEL/CREATE"].contains(&"solid.extrude".into()));
        assert!(!custom.panels["DESIGN/MANAGE/RULES"].contains(&"plastic.manage_rules".into()));
        assert_eq!(custom.panels["DESIGN/SKETCH/CONSTRAIN"], vec!["sketch.dimension"]);
    }
}
