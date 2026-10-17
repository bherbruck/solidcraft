//! Explicit toolbar layouts: Sketch CREATE, the SOLID tab's ASSEMBLE panel, SHEET METAL and
//! PLASTIC. A command is registered on one tab and panel;
//! these panels list the commands they show in order (the first few get big buttons), so Extrude
//! or Joint appear on several tabs. Commands registered on such a panel but not listed follow at
//! the end.

use solidcraft_engine::CommandSpec;
use solidcraft_engine::doc::FeatureKind;

const ASSEMBLE: &[&str] = &[
    "component.create",
    "joint.create",
    "joint.as_built",
    "component.insert_design",
    "joint.origin",
    "joint.rigid_group",
    "joint.drive",
    "joint.motion_link",
    "joint.gear",
    "joint.rack_pinion",
    "joint.screw",
    "joint.cam",
    "motion.study",
    "motion.trace",
    "explode.create",
    "contact.enable_sets",
    "contact.enable_all",
    "contact.disable_all",
    "contact.create",
    "inspect.interference",
];

const SHEET_CREATE: &[&str] = &[
    "sketch.create",
    "sheet.flange",
    "sheet.flat_pattern",
    "sheet.hem",
    "sheet.fold",
    "sheet.convert",
    "component.create",
    "solid.extrude",
    "solid.hole",
    "solid.thread",
    "solid.pattern.rectangular",
    "solid.pattern.circular",
    "solid.pattern.path",
    "solid.mirror",
    "joint.origin",
];

const SHEET_MODIFY: &[&str] = &[
    "sheet.manage_rules",
    "sheet.unfold",
    "sheet.refold",
    "solid.fillet",
    "solid.chamfer",
    "solid.move",
    "solid.align",
    "timeline.delete",
    "solid.remove",
    "material.assign",
    "parameters.change",
];

const PLASTIC_SETUP: &[&str] = &["plastic.manage_rules", "plastic.assign_rule"];

const PLASTIC_CREATE: &[&str] = &[
    "sketch.create",
    "plastic.boss",
    "plastic.snap_fit",
    "plastic.lip",
    "plastic.rest",
    "component.create",
    "solid.extrude",
    "solid.revolve",
    "solid.sweep",
    "solid.loft",
    "solid.rib",
    "solid.web",
    "solid.emboss",
    "solid.hole",
    "solid.thread",
    "solid.box",
    "solid.cylinder",
    "solid.sphere",
    "solid.torus",
    "solid.coil",
    "solid.pipe",
    "solid.pattern.rectangular",
    "solid.pattern.circular",
    "solid.pattern.path",
    "solid.mirror",
    "joint.origin",
];

/// Panels of a tab, when this module lays the tab out.
pub fn panels(tab: &str) -> Option<&'static [&'static str]> {
    match tab {
        "SOLID" => Some(&["CREATE", "MODIFY", "ASSEMBLE", "CONFIGURE", "CONSTRUCT", "INSPECT", "INSERT", "SELECT"]),
        "PLASTIC" => Some(&["SETUP", "CREATE", "MODIFY", "ASSEMBLE", "CONSTRUCT", "INSPECT", "INSERT", "SELECT"]),
        _ => None,
    }
}

/// The commands a panel shows, in order, and how many get a big button; `None` leaves the panel
/// to the registry.
pub fn layout<'a>(tab: &str, panel: &str, specs: &[&'a CommandSpec]) -> Option<(Vec<&'a CommandSpec>, usize)> {
    let (ids, promoted, own_tab): (&[&str], usize, &str) = match (tab, panel) {
        ("SKETCH", "CREATE") => (
            &[
                "sketch.line",
                "sketch.rectangle.two_point",
                "sketch.circle.center",
                "sketch.arc.three_point",
                "sketch.slot.center_to_center",
                "sketch.polygon.inscribed",
                "sketch.spline.fit_point",
                "sketch.point",
            ],
            8,
            tab,
        ),
        ("SOLID", "CONFIGURE") => (&["parameters.change", "config.table"], 2, tab),
        ("SKETCH", "PATTERN") => (&["sketch.mirror", "sketch.pattern.rectangular", "sketch.pattern.circular"], 3, tab),
        ("SOLID" | "SHEET METAL" | "PLASTIC", "ASSEMBLE") => (ASSEMBLE, 3, tab),
        ("SHEET METAL", "CREATE") => (SHEET_CREATE, 3, tab),
        ("SHEET METAL", "MODIFY") => (SHEET_MODIFY, 2, tab),
        ("PLASTIC", "SETUP") => (PLASTIC_SETUP, 2, tab),
        ("PLASTIC", "CREATE") => (PLASTIC_CREATE, 5, tab),
        // The solid tab's panels.
        ("PLASTIC", "MODIFY") => (&[], 4, "SOLID"),
        ("SHEET METAL" | "PLASTIC", "CONSTRUCT" | "INSPECT" | "INSERT") => (&[], 2, "SOLID"),
        _ => return None,
    };
    let mut out: Vec<&CommandSpec> = ids.iter().filter_map(|id| specs.iter().find(|c| c.id == *id).copied()).collect();
    for c in specs.iter().filter(|c| (c.tab == tab || c.tab == own_tab) && c.panel == panel) {
        if !out.iter().any(|o| o.id == c.id) {
            out.push(*c);
        }
    }
    Some((out, promoted))
}

/// Timeline and browser icon of the sheet metal and plastic features (`None`: not one of them).
pub fn feature_icon(k: &FeatureKind) -> Option<&'static str> {
    Some(match k {
        FeatureKind::SheetBase { .. } | FeatureKind::SheetContour { .. } | FeatureKind::SheetFlange { .. } => "flange",
        FeatureKind::SheetHem { .. } => "hem",
        FeatureKind::SheetFold { .. } => "fold",
        FeatureKind::SheetUnfold { refold: false, .. } => "unfold",
        FeatureKind::SheetUnfold { .. } => "refold",
        FeatureKind::SheetConvert { .. } => "convert_sheet",
        FeatureKind::Boss { .. } => "boss",
        FeatureKind::Lip { .. } => "lip",
        FeatureKind::SnapFit { .. } => "snap_fit",
        FeatureKind::Rest { .. } => "rest",
        FeatureKind::Rib { web: true, .. } => "web",
        FeatureKind::Rib { .. } => "rib",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(tab: &str, panel: &str) -> Vec<&'static str> {
        let specs = solidcraft_engine::command_specs();
        layout(tab, panel, &specs).map(|(v, _)| v.iter().map(|c| c.id).collect()).unwrap_or_default()
    }

    #[test]
    fn configure_header_clicks_open_parameters_and_configurations_separately() {
        use serde_json::json;
        let mut h = crate::scenario::Harness::new();
        let commands = ids("SOLID", "CONFIGURE");
        assert_eq!(&commands[..2], &["parameters.change", "config.table"]);
        assert!(commands.contains(&"config.configure"), "Configure remains available in the dropdown");
        h.step(&json!({"widget":"Parameters"})).unwrap();
        assert!(h.app.params.is_some(), "the ordinary Parameters button opens the editor");
        assert!(h.app.dialog.is_none());
        assert!(h.widget("User Parameters").is_some());
        assert!(h.widget("Model Parameters").is_some());
        // Exercise the editor's Add button, not a script-only parameter command.
        let parameters = h.app.params.as_mut().unwrap();
        parameters.new_name = "width".into();
        parameters.new_expr = "25 mm".into();
        h.frames(3);
        h.step(&json!({"widget":"Add"})).unwrap();
        let values = h.app.session.execute("parameters.list", &json!({})).unwrap();
        assert!(values["parameters"].as_array().unwrap().iter().any(|p| p["name"] == "width" && p["value"] == 25.0));
        h.step(&json!({"click":{"handle":"params:close"}})).unwrap();
        assert!(h.app.params.is_none());
        h.step(&json!({"widget":"Configurations"})).unwrap();
        assert!(h.app.params.is_none());
        assert!(matches!(
            h.app.dialog.as_ref().map(|d| &d.kind),
            Some(crate::dialogs::Kind::Motion(crate::dialogs_motion::Mo::Configurations { .. }))
        ));
        assert!(h.widget("Add Column").is_some());
        assert!(h.widget("Add Row").is_some());
    }

    /// Fusion's layout: joints on every tab's ASSEMBLE panel, sheet metal and plastic tabs
    /// reach the shared solid commands, the plastic rules sit on SETUP.
    #[test]
    fn tabs_share_commands_in_fusion_order() {
        for tab in ["SOLID", "SHEET METAL", "PLASTIC"] {
            let a = ids(tab, "ASSEMBLE");
            assert_eq!(a.get(1).copied(), Some("joint.create"), "{tab}: {a:?}");
            assert!(a.contains(&"joint.drive") && a.contains(&"component.create"));
        }
        let c = ids("SHEET METAL", "CREATE");
        assert_eq!(c.get(1).copied(), Some("sheet.flange"));
        assert!(c.contains(&"solid.extrude") && c.contains(&"sheet.flat_pattern"));
        assert!(ids("SHEET METAL", "MODIFY").contains(&"sheet.unfold"));
        assert!(ids("PLASTIC", "SETUP").contains(&"plastic.manage_rules"));
        let p = ids("PLASTIC", "CREATE");
        assert!(p.contains(&"plastic.boss") && p.contains(&"solid.rib") && p.contains(&"plastic.lip"));
        assert!(ids("PLASTIC", "MODIFY").contains(&"solid.shell"));
        assert!(panels("PLASTIC").is_some_and(|p| p.first() == Some(&"SETUP")));
    }

    /// #1240: a saved part reaches an assembly from the ASSEMBLE panel (and the File menu) as a
    /// linked component, and a STEP file picked there is inserted as Insert STEP would.
    #[test]
    fn insert_design_is_on_the_assemble_panel_and_inserts_a_saved_part() {
        use serde_json::json;
        for tab in ["SOLID", "SHEET METAL", "PLASTIC"] {
            assert!(ids(tab, "ASSEMBLE").contains(&"component.insert_design"), "{tab}");
        }
        assert!(!ids("SOLID", "ASSEMBLE")[..3].contains(&"component.insert_design"), "Component, Joint and As-built Joint keep the big buttons");
        let mut h = crate::scenario::Harness::new();
        let part = h.dir.join("part1.solidcraft");
        let mut s = solidcraft_engine::Session::default();
        s.execute("solid.box", &json!({"length": 10, "width": 20, "height": 30})).unwrap();
        s.execute("file.save", &json!({"path": part})).unwrap();
        s.execute("file.export", &json!({"path": h.dir.join("part2.step")})).unwrap();
        h.step(&json!({"choose_file": "part1.solidcraft"})).unwrap();
        h.step(&json!({"start": "component.insert_design"})).unwrap();
        h.frames(3);
        let links = h.app.session.execute("component.links", &json!({})).unwrap();
        assert_eq!(links["links"][0]["name"], "part1", "{links}");
        assert_eq!(h.app.session.world_state().bodies.len(), 1);
        h.step(&json!({"choose_file": "part2.step"})).unwrap();
        h.step(&json!({"start": "component.insert_design"})).unwrap();
        for _ in 0..600 {
            if h.app.session.world_state().bodies.len() == 2 {
                break;
            }
            h.frames(1);
        }
        assert_eq!(h.app.session.world_state().bodies.len(), 2, "the STEP file's body is inserted");
    }
}
