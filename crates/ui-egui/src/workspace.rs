//! Ribbon structure is UI data; command registrations retain their original placements.
//! Explicit ids override the old placement table and can also share a command across sections.

use solidcraft_engine::CommandSpec;
use solidcraft_engine::doc::FeatureKind;

pub const TABS: &[&str] = &["MODEL", "SURFACE", "ASSEMBLE", "INSERT", "INSPECT", "MANAGE", "VIEW"];

/// Reading order of the compact, three-column constraint gallery. Icons come from each spec.
pub const CONSTRAINT_GALLERY: &[&str] = &[
    "sketch.constraint.horizontal_vertical",
    "sketch.constraint.coincident",
    "sketch.constraint.tangent",
    "sketch.constraint.equal",
    "sketch.constraint.parallel",
    "sketch.constraint.perpendicular",
];

pub struct CommandGroup {
    pub label: &'static str,
    pub ids: &'static [&'static str],
}

/// Optional headings for section lists. Unlisted commands retain their order at the end.
pub fn groups(tab: &str, panel: &str) -> &'static [CommandGroup] {
    match (tab, panel) {
        ("MODEL", "MODIFY") => &[
            CommandGroup { label: "Edges", ids: &["solid.fillet", "solid.chamfer"] },
            CommandGroup {
                label: "Faces",
                ids: &["solid.press_pull", "solid.draft", "solid.offset_face", "solid.replace_face", "solid.split_face", "face.delete"],
            },
            CommandGroup { label: "Bodies", ids: &["solid.shell", "solid.combine", "solid.split_body", "solid.scale", "solid.remove"] },
            CommandGroup { label: "Specialise", ids: &["sheet.convert"] },
        ],
        _ => &[],
    }
}

/// Menu order is independent of the ribbon's promoted-button order.
pub fn grouped_commands<'a>(tab: &str, panel: &str, commands: &[&'a CommandSpec]) -> Vec<(Option<&'static str>, &'a CommandSpec)> {
    let groups = groups(tab, panel);
    let mut out = Vec::new();
    for group in groups {
        let mut heading = Some(group.label);
        for id in group.ids {
            if let Some(command) = commands.iter().find(|c| c.id == *id) {
                out.push((heading.take(), *command));
            }
        }
    }
    out.extend(commands.iter().filter(|c| !groups.iter().any(|g| g.ids.contains(&c.id))).map(|c| (None, *c)));
    out
}

pub struct Section {
    pub tab: &'static str,
    pub panel: &'static str,
    pub promoted: usize,
    pub ids: &'static [&'static str],
}

pub const SECTIONS: &[Section] = &[
    Section {
        tab: "MODEL",
        panel: "SKETCH",
        promoted: 1,
        ids: &["sketch.create", "sketch.edit", "sketch.line3d", "sketch.spline3d", "sketch.point3d", "sketch.move3d"],
    },
    Section { tab: "MODEL", panel: "CREATE", promoted: 3, ids: &["solid.extrude", "solid.revolve", "solid.hole"] },
    Section {
        tab: "MODEL",
        panel: "MODIFY",
        promoted: 4,
        ids: &["solid.fillet", "solid.chamfer", "solid.shell", "solid.press_pull", "sheet.convert"],
    },
    Section {
        tab: "MODEL",
        panel: "ARRANGE",
        promoted: 3,
        ids: &["solid.move", "solid.mirror", "solid.pattern.rectangular", "solid.align", "solid.pattern.circular", "solid.pattern.path"],
    },
    Section { tab: "MODEL", panel: "CONSTRUCT", promoted: 1, ids: &["construct.plane.offset"] },
    Section { tab: "SURFACE", panel: "CREATE", promoted: 4, ids: &["surface.extrude", "surface.revolve", "surface.sweep", "surface.loft"] },
    Section { tab: "SURFACE", panel: "EDIT", promoted: 4, ids: &["surface.trim", "surface.untrim", "surface.extend", "surface.stitch"] },
    Section { tab: "SURFACE", panel: "CONVERT", promoted: 2, ids: &["surface.thicken", "mesh.convert"] },
    Section {
        tab: "ASSEMBLE",
        panel: "COMPONENTS",
        promoted: 4,
        ids: &["component.create", "component.from_bodies", "component.insert_design", "parts.library"],
    },
    Section {
        tab: "ASSEMBLE",
        panel: "JOINTS",
        promoted: 4,
        ids: &[
            "joint.create",
            "joint.as_built",
            "joint.origin",
            "joint.rigid_group",
            "joint.limits",
            "joint.gear",
            "joint.rack_pinion",
            "joint.screw",
            "joint.cam",
            "joint.reach",
        ],
    },
    Section {
        tab: "ASSEMBLE",
        panel: "POSITION",
        promoted: 3,
        ids: &["occurrence.ground", "component.capture_position", "component.revert_position"],
    },
    Section {
        tab: "ASSEMBLE",
        panel: "MOTION",
        promoted: 4,
        ids: &["joint.drive", "joint.motion_link", "motion.study", "motion.trace", "explode.create"],
    },
    Section {
        tab: "ASSEMBLE",
        panel: "CONTACT",
        promoted: 2,
        ids: &["contact.enable_sets", "contact.check", "contact.enable_all", "contact.disable_all", "contact.create", "contact.edit", "contact.list"],
    },
    Section {
        tab: "INSERT",
        panel: "FILES",
        promoted: 3,
        ids: &["file.insert_step", "file.insert_mesh", "component.insert_design", "file.save_mesh"],
    },
    Section { tab: "INSERT", panel: "IMAGES", promoted: 3, ids: &["canvas.insert", "canvas.decal", "canvas.edit"] },
    Section { tab: "INSERT", panel: "2D", promoted: 2, ids: &["sketch.insert_dxf", "sketch.insert_svg"] },
    Section { tab: "INSERT", panel: "LIBRARY", promoted: 3, ids: &["parts.fastener", "parts.library", "parts.resize", "parts.insert"] },
    Section { tab: "INSERT", panel: "LINKS", promoted: 2, ids: &["component.update_links", "component.break_link"] },
    Section { tab: "INSPECT", panel: "MEASURE", promoted: 3, ids: &["inspect.measure", "inspect.center_of_mass", "component.properties"] },
    Section { tab: "INSPECT", panel: "SECTION", promoted: 1, ids: &["inspect.section"] },
    Section {
        tab: "INSPECT",
        panel: "CHECK",
        promoted: 4,
        ids: &["inspect.interference", "inspect.draft", "inspect.accessibility", "contact.check", "inspect.compare"],
    },
    Section {
        tab: "INSPECT",
        panel: "SURFACE QUALITY",
        promoted: 4,
        ids: &[
            "inspect.zebra",
            "inspect.curvature_map",
            "inspect.curvature_comb",
            "inspect.isocurve",
            "inspect.minimum_radius",
            "inspect.environment_map",
            "inspect.continuity",
        ],
    },
    Section { tab: "INSPECT", panel: "HISTORY", promoted: 2, ids: &["timeline.dependents", "document.inspect"] },
    Section {
        tab: "MANAGE",
        panel: "PARAMETERS",
        promoted: 2,
        ids: &["parameters.change", "parameters.graph", "parameters.move", "parameters.import", "parameters.export"],
    },
    Section { tab: "MANAGE", panel: "CONFIGURATIONS", promoted: 2, ids: &["config.table", "config.configure", "config.rules", "config.preview"] },
    Section {
        tab: "MANAGE",
        panel: "MATERIALS",
        promoted: 2,
        ids: &["material.assign", "appearance.assign", "material.properties.validate", "library.inspect", "library.export", "library.apply"],
    },
    Section { tab: "MANAGE", panel: "RULES", promoted: 3, ids: &["sheet.manage_rules", "plastic.manage_rules", "plastic.assign_rule"] },
    Section {
        tab: "MANAGE",
        panel: "DESIGN",
        promoted: 2,
        ids: &["document.units", "timeline.compute_all", "edit.checkpoint", "edit.restore_checkpoint"],
    },
    Section {
        tab: "VIEW",
        panel: "VIEWS",
        promoted: 2,
        ids: &["view.apply", "view.save", "view.rename", "view.delete", "view.list", "view.navigate"],
    },
    Section { tab: "VIEW", panel: "SELECT", promoted: 0, ids: &[] },
    Section {
        tab: "SKETCH",
        panel: "CREATE",
        promoted: 4,
        ids: &["sketch.line", "sketch.rectangle.two_point", "sketch.circle.center", "sketch.arc.three_point", "sketch.infinite"],
    },
    Section {
        tab: "SKETCH",
        panel: "MODIFY",
        promoted: 3,
        ids: &["sketch.trim", "sketch.offset", "sketch.fillet", "sketch.copy", "sketch.cut", "sketch.paste"],
    },
    Section { tab: "SKETCH", panel: "PATTERN", promoted: 3, ids: &["sketch.mirror", "sketch.pattern.circular", "sketch.pattern.rectangular"] },
    Section {
        tab: "SKETCH",
        panel: "CONSTRAIN",
        promoted: 7,
        ids: &[
            "sketch.dimension",
            "sketch.constraint.horizontal_vertical",
            "sketch.constraint.coincident",
            "sketch.constraint.tangent",
            "sketch.constraint.equal",
            "sketch.constraint.parallel",
            "sketch.constraint.perpendicular",
            "sketch.construction",
            "sketch.constrainer",
        ],
    },
    Section {
        tab: "SKETCH",
        panel: "REFERENCE",
        promoted: 1,
        ids: &[
            "sketch.project",
            "sketch.intersect",
            "sketch.include_3d",
            "sketch.project_to_surface",
            "sketch.intersection_curve",
            "sketch.spun_profile",
            "sketch.isoparametric_curve",
            "sketch.fit_curves_to_section",
            "sketch.insert_dxf",
            "sketch.insert_svg",
            "canvas.insert",
        ],
    },
    Section { tab: "SKETCH", panel: "FINISH SKETCH", promoted: 1, ids: &["sketch.finish"] },
    Section {
        tab: "SHEET METAL",
        panel: "CREATE",
        promoted: 4,
        ids: &["sheet.flange", "sheet.hem", "sheet.fold", "sheet.perforate", "sheet.convert"],
    },
    Section { tab: "SHEET METAL", panel: "FLAT", promoted: 3, ids: &["sheet.unfold", "sheet.refold", "sheet.flat_pattern"] },
    Section { tab: "SHEET METAL", panel: "OUTPUT", promoted: 2, ids: &["sheet.export_dxf", "sheet.export_bend_table"] },
    Section { tab: "SHEET METAL", panel: "RULES", promoted: 1, ids: &["sheet.manage_rules"] },
    Section { tab: "RENDER", panel: "SCENE", promoted: 2, ids: &["render.scene"] },
    Section { tab: "RENDER", panel: "IMAGE", promoted: 1, ids: &["render.start"] },
];

/// Compact source labels for painted big buttons only. Menus, search and tooltips
/// retain the engine label. Route these through `tr_label`, like other UI labels.
pub const RIBBON_LABELS: &[(&str, &str)] = &[
    ("appearance.assign", "Appearance"),
    ("component.capture_position", "Capture Pos."),
    ("component.create", "New Comp."),
    ("component.from_bodies", "From Bodies"),
    ("component.insert_design", "Insert"),
    ("component.properties", "Properties"),
    ("component.revert_position", "Revert Pos."),
    ("config.table", "Config Table"),
    ("contact.check", "Contact"),
    ("contact.enable_sets", "Contact Sets"),
    ("document.inspect", "Inspect"),
    ("document.units", "Units"),
    ("inspect.accessibility", "Access"),
    ("inspect.center_of_mass", "Mass Center"),
    ("inspect.curvature_comb", "Curv. Comb"),
    ("inspect.curvature_map", "Curvature"),
    ("inspect.draft", "Draft"),
    ("inspect.interference", "Interference"),
    ("inspect.isocurve", "Isocurve"),
    ("inspect.section", "Section"),
    ("inspect.zebra", "Zebra"),
    ("joint.as_built", "As-Built"),
    ("material.assign", "Material"),
    ("parameters.change", "Parameters"),
    ("parameters.graph", "Param. Graph"),
    ("parts.fastener", "Fastener"),
    ("parts.library", "Parts"),
    ("parts.resize", "Resize Part"),
    ("plastic.assign_rule", "Assign Rule"),
    ("plastic.manage_rules", "Plastic Rules"),
    ("render.scene", "Scene"),
    ("sheet.export_bend_table", "Bend Table"),
    ("sheet.export_dxf", "Export DXF"),
    ("sheet.flat_pattern", "Flat Pattern"),
    ("sheet.manage_rules", "Sheet Rules"),
    ("sketch.circle.center", "Circle"),
    ("sketch.constraint.coincident", "Coincident"),
    ("sketch.constraint.horizontal_vertical", "Horiz./Vert."),
    ("sketch.create", "New Sketch"),
    ("sketch.dimension", "Dimension"),
    ("sketch.finish", "Finish Sketch"),
    ("sketch.pattern.circular", "Circular"),
    ("sketch.pattern.rectangular", "Rectangular"),
    ("sketch.rectangle.two_point", "Rectangle"),
    ("solid.move", "Move / Copy"),
    ("solid.pattern.rectangular", "Pattern"),
    ("surface.extrude", "Extrude"),
    ("surface.revolve", "Revolve"),
    ("surface.sweep", "Sweep"),
    ("timeline.dependents", "Dependents"),
    ("view.apply", "Restore View"),
    ("view.save", "Save View"),
];

pub fn ribbon_button_label(command: &CommandSpec) -> &str {
    RIBBON_LABELS.iter().find(|(id, _)| *id == command.id).map_or(command.label, |(_, label)| *label)
}

/// Registry placement → default ribbon destination, after explicit id overrides.
pub const PLACEMENTS: &[(&str, &str, &str, &str)] = &[
    ("SOLID", "CREATE", "MODEL", "CREATE"),
    ("SOLID", "MODIFY", "MODEL", "MODIFY"),
    ("SOLID", "CONSTRUCT", "MODEL", "CONSTRUCT"),
    ("SOLID", "ASSEMBLE", "ASSEMBLE", "JOINTS"),
    ("SOLID", "POSITION", "ASSEMBLE", "POSITION"),
    ("SOLID", "CONFIGURE", "MANAGE", "CONFIGURATIONS"),
    ("SOLID", "INSPECT", "INSPECT", "CHECK"),
    ("SOLID", "INSERT", "INSERT", "FILES"),
    ("SURFACE", "CREATE", "SURFACE", "CREATE"),
    ("SURFACE", "MODIFY", "SURFACE", "EDIT"),
    ("SURFACE", "INSPECT", "INSPECT", "SURFACE QUALITY"),
    ("SKETCH", "CREATE", "SKETCH", "CREATE"),
    ("SKETCH", "MODIFY", "SKETCH", "MODIFY"),
    ("SKETCH", "PATTERN", "SKETCH", "PATTERN"),
    ("SKETCH", "CONSTRAINTS", "SKETCH", "CONSTRAIN"),
    ("SKETCH", "INSERT", "INSERT", "IMAGES"),
    ("SKETCH", "INSPECT", "INSPECT", "SURFACE QUALITY"),
    ("SKETCH", "FINISH SKETCH", "SKETCH", "FINISH SKETCH"),
    ("SHEET METAL", "CREATE", "SHEET METAL", "CREATE"),
    ("SHEET METAL", "MODIFY", "SHEET METAL", "FLAT"),
    ("PLASTIC", "CREATE", "MODEL", "CREATE"),
    ("PLASTIC", "SETUP", "MANAGE", "RULES"),
    ("MESH", "MODIFY", "INSERT", "FILES"),
    ("RENDER", "SCENE", "RENDER", "SCENE"),
    ("RENDER", "IMAGE", "RENDER", "IMAGE"),
];

pub fn panels(tab: &str) -> Vec<&'static str> {
    SECTIONS.iter().filter(|s| s.tab == tab).map(|s| s.panel).collect()
}

pub fn destination(c: &CommandSpec) -> Option<(&'static str, &'static str)> {
    // A few shared commands have a canonical home that differs from their first occurrence.
    let primary = match c.id {
        "sheet.manage_rules" => Some(("MANAGE", "RULES")),
        "sheet.convert" => Some(("MODEL", "MODIFY")),
        _ => None,
    };
    primary
        .or_else(|| SECTIONS.iter().find(|s| s.ids.contains(&c.id)).map(|s| (s.tab, s.panel)))
        .or_else(|| PLACEMENTS.iter().find(|(t, p, ..)| *t == c.tab && *p == c.panel).map(|(_, _, t, p)| (*t, *p)))
}

/// Ordered menu commands, with the promoted commands first. Empty sections are omitted by
/// panel_lists, except the selection filter, which has its own existing controls.
pub fn layout<'a>(tab: &str, panel: &str, specs: &[&'a CommandSpec]) -> Option<(Vec<&'a CommandSpec>, usize)> {
    let section = SECTIONS.iter().find(|s| s.tab == tab && s.panel == panel)?;
    let mut out: Vec<_> = section.ids.iter().filter_map(|id| specs.iter().find(|c| c.id == *id).copied()).collect();
    for c in specs.iter().filter(|c| destination(c) == Some((section.tab, section.panel))) {
        if !out.iter().any(|o| o.id == c.id) {
            out.push(*c);
        }
    }
    Some((out, section.promoted))
}

/// Old keys may be split across sections. Resolve each saved button by the same command map;
/// unknown command ids follow the old panel's default destination.
pub fn legacy_destination(tab: &str, panel: &str, id: &str) -> Option<(&'static str, &'static str)> {
    legacy_panel(tab, panel)?;
    solidcraft_engine::find_command(id).and_then(destination).or_else(|| legacy_panel(tab, panel))
}

pub fn legacy_panel(tab: &str, panel: &str) -> Option<(&'static str, &'static str)> {
    if panel == "SELECT" && matches!(tab, "SOLID" | "SURFACE" | "MESH" | "SHEET METAL" | "PLASTIC" | "UTILITIES" | "SKETCH") {
        return Some(("VIEW", "SELECT"));
    }
    PLACEMENTS.iter().find(|(t, p, ..)| *t == tab && *p == panel).map(|(_, _, t, p)| (*t, *p)).or(match (tab, panel) {
        ("PLASTIC" | "SHEET METAL", "MODIFY") => Some(("MODEL", "MODIFY")),
        ("PLASTIC" | "SHEET METAL" | "SURFACE" | "MESH", "ASSEMBLE") => Some(("ASSEMBLE", "JOINTS")),
        ("PLASTIC" | "SHEET METAL" | "SURFACE" | "MESH", "CONSTRUCT") => Some(("MODEL", "CONSTRUCT")),
        ("PLASTIC" | "SHEET METAL" | "SURFACE" | "MESH", "INSPECT") => Some(("INSPECT", "CHECK")),
        ("PLASTIC" | "SHEET METAL" | "SURFACE" | "MESH", "INSERT") => Some(("INSERT", "FILES")),
        ("MESH", "CREATE" | "PREPARE") => Some(("SURFACE", "CONVERT")),
        ("UTILITIES", "MAKE" | "ADD-INS" | "UTILITY") => Some(("MANAGE", "DESIGN")),
        ("UTILITIES", "INSPECT") => Some(("INSPECT", "CHECK")),
        _ => None,
    })
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

/// Selected geometry identifies the active body; without a body selection use the last body
/// of the active component, matching feature dialogs. Rules on unrelated bodies do not count.
pub fn sheet_body(app: &crate::SolveApp) -> Option<String> {
    use solidcraft_engine::Sel;
    let state = app.session.model.state();
    let selected = app.session.selection.iter().find_map(|s| match s {
        Sel::Body { name } => Some(name.as_str()),
        Sel::Face { body, .. } | Sel::Edge { body, .. } => Some(body.as_str()),
        _ => None,
    });
    let body = selected.or_else(|| {
        state.bodies.iter().rev().find(|b| app.session.doc.model_body_component(b) == app.session.active_component).map(|b| b.name.as_str())
    })?;
    state.sheets.iter().find(|s| s.body == body).map(|s| s.body.clone())
}

pub fn visible_tabs(app: &crate::SolveApp) -> Vec<&'static str> {
    if crate::render_workspace::active(app) {
        return vec!["RENDER"];
    }
    let mut tabs = TABS.to_vec();
    if app.session.active_sketch.is_some() {
        tabs.push("SKETCH");
    }
    if sheet_body(app).is_some() {
        tabs.push("SHEET METAL");
    }
    tabs
}

/// Select contextual tabs on entry (or a different sheet body), allowing the other tabs to
/// remain usable for the rest of that context. Sketch takes precedence over sheet metal.
pub fn sync_context(app: &mut crate::SolveApp) {
    if crate::render_workspace::active(app) {
        return;
    }
    let context = app.session.active_sketch.map(|id| format!("sketch:{id}")).or_else(|| sheet_body(app).map(|body| format!("sheet:{body}")));
    if app.ui.ribbon_context != context {
        app.ui.tab = if app.session.active_sketch.is_some() {
            "SKETCH"
        } else if context.is_some() {
            "SHEET METAL"
        } else {
            "MODEL"
        }
        .into();
        app.ui.ribbon_context = context;
    }
    if !visible_tabs(app).contains(&app.ui.tab.as_str()) {
        app.ui.tab = "MODEL".into();
    }
}

pub fn select_command_tab(app: &mut crate::SolveApp, id: &str) {
    sync_context(app);
    let Some(c) = solidcraft_engine::find_command(id) else { return };
    // Keep the tab a toolbar click came from when it already lists this shared command.
    if crate::toolbar_custom::panel_lists(&app.ui.tab).iter().any(|(_, cmds, _)| cmds.iter().any(|c| c.id == id)) {
        return;
    }
    if let Some((tab, _)) = destination(c) {
        if tab == "RENDER" {
            return;
        } // The Render workspace owns its entry/exit lifecycle.
        if visible_tabs(app).contains(&tab) {
            app.ui.tab = tab.into();
        }
    }
}

/// Defaults from the previous ribbon, used only to carry removals into the new layout.
pub fn legacy_buttons(tab: &str, panel: &str) -> Vec<&'static str> {
    let (ids, n, source): (&[&str], usize, &str) = match (tab, panel) {
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
        ("SKETCH", "PATTERN") => (&["sketch.mirror", "sketch.pattern.rectangular", "sketch.pattern.circular"], 3, tab),
        ("SKETCH", "CONSTRAINTS") => (&[], 6, tab),
        ("SOLID", "CONFIGURE") => (&["parameters.change", "config.table"], 2, tab),
        ("SOLID" | "SHEET METAL" | "PLASTIC", "ASSEMBLE") => (&["component.create", "joint.create", "joint.as_built"], 3, tab),
        ("SHEET METAL", "CREATE") => (&["sketch.create", "sheet.flange", "sheet.flat_pattern"], 3, tab),
        ("SHEET METAL", "MODIFY") => (&["sheet.manage_rules", "sheet.unfold"], 2, tab),
        ("PLASTIC", "SETUP") => (&["plastic.manage_rules", "plastic.assign_rule"], 2, tab),
        ("PLASTIC", "CREATE") => (&["sketch.create", "plastic.boss", "plastic.snap_fit", "plastic.lip", "plastic.rest"], 5, tab),
        ("SOLID" | "PLASTIC", "MODIFY") => (&[], 4, "SOLID"),
        ("SOLID", "CREATE") => (&[], 4, tab),
        ("SHEET METAL" | "PLASTIC", "CONSTRUCT" | "INSPECT" | "INSERT") => (&[], 2, "SOLID"),
        (_, "FINISH SKETCH") => (&[], 1, tab),
        _ => (&[], 2, tab),
    };
    let mut out = ids.to_vec();
    for c in solidcraft_engine::command_specs().into_iter().filter(|c| c.tab == source && c.panel == panel) {
        if !out.contains(&c.id) {
            out.push(c.id);
        }
    }
    out.truncate(n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modify_groups_order_commands_and_keep_unlisted_commands_at_the_end() {
        let specs = solidcraft_engine::command_specs();
        let (commands, _) = layout("MODEL", "MODIFY", &specs).unwrap();
        let grouped = grouped_commands("MODEL", "MODIFY", &commands);
        assert_eq!(grouped.iter().filter_map(|(h, _)| *h).collect::<Vec<_>>(), ["Edges", "Faces", "Bodies", "Specialise"]);
        assert_eq!(grouped.len(), commands.len());
        assert_eq!(
            grouped.iter().map(|(_, c)| c.id).collect::<Vec<_>>(),
            [
                "solid.fillet",
                "solid.chamfer",
                "solid.press_pull",
                "solid.draft",
                "solid.offset_face",
                "solid.replace_face",
                "solid.split_face",
                "face.delete",
                "solid.shell",
                "solid.combine",
                "solid.split_body",
                "solid.scale",
                "solid.remove",
                "sheet.convert",
                "timeline.delete",
            ]
        );
        let plain = grouped_commands("SURFACE", "CREATE", &commands);
        assert!(plain.iter().all(|(h, _)| h.is_none()));
        assert_eq!(plain.iter().map(|(_, c)| c.id).collect::<Vec<_>>(), commands.iter().map(|c| c.id).collect::<Vec<_>>());
    }

    #[test]
    fn promoted_long_labels_have_compact_ribbon_names() {
        let specs = solidcraft_engine::command_specs();
        let english: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("../../../assets/locales/en.json")).unwrap();
        for section in SECTIONS {
            let (commands, count) = layout(section.tab, section.panel, &specs).unwrap();
            for command in commands.iter().take(count) {
                // Gallery cells are icon-only; they retain full names in their tooltips.
                if section.tab == "SKETCH" && section.panel == "CONSTRAIN" && CONSTRAINT_GALLERY.contains(&command.id) {
                    continue;
                }
                if command.label.chars().count() > 12 || command.label.split_whitespace().any(|word| word.chars().count() > 9) {
                    assert!(RIBBON_LABELS.iter().any(|(id, _)| *id == command.id), "missing compact label for {}", command.id);
                }
            }
        }
        for (id, label) in RIBBON_LABELS {
            assert!(solidcraft_engine::find_command(id).is_some(), "unknown command {id}");
            assert!(label.chars().count() <= 13, "{id}: {label}");
            assert!(english.values().any(|value| value == label), "untranslated ribbon label {label}");
        }
        assert_eq!(ribbon_button_label(solidcraft_engine::find_command("sketch.create").unwrap()), "New Sketch");
        assert_eq!(solidcraft_engine::find_command("sketch.create").unwrap().label, "Create Sketch");
    }

    #[test]
    fn every_placed_command_is_reachable_and_every_layout_id_exists() {
        let specs = solidcraft_engine::command_specs();
        let lists: Vec<_> = SECTIONS.iter().map(|s| (s, layout(s.tab, s.panel, &specs).unwrap())).collect();
        for c in specs.iter().filter(|c| !c.tab.is_empty()) {
            assert!(lists.iter().any(|(_, (cmds, _))| cmds.iter().any(|x| x.id == c.id)), "lost toolbar command {} ({}/{})", c.id, c.tab, c.panel);
        }
        for (s, (cmds, n)) in lists {
            assert!(n <= 4 || (s.tab == "SKETCH" && s.panel == "CONSTRAIN" && n == 7), "{}/{} promotes {n} commands", s.tab, s.panel);
            for id in s.ids {
                assert!(specs.iter().any(|c| c.id == *id), "unknown layout id {id}");
            }
            let unique: std::collections::BTreeSet<_> = cmds.iter().map(|c| c.id).collect();
            assert_eq!(unique.len(), cmds.len());
        }
    }

    #[test]
    fn model_defaults_and_shared_commands_follow_the_map() {
        let specs = solidcraft_engine::command_specs();
        for (panel, expected) in [
            ("SKETCH", vec!["sketch.create"]),
            ("CREATE", vec!["solid.extrude", "solid.revolve", "solid.hole"]),
            ("MODIFY", vec!["solid.fillet", "solid.chamfer", "solid.shell", "solid.press_pull"]),
            ("ARRANGE", vec!["solid.move", "solid.mirror", "solid.pattern.rectangular"]),
            ("CONSTRUCT", vec!["construct.plane.offset"]),
        ] {
            let (cmds, n) = layout("MODEL", panel, &specs).unwrap();
            assert_eq!(cmds.iter().take(n).map(|c| c.id).collect::<Vec<_>>(), expected);
        }
        for (tab, panel, id) in [
            ("MODEL", "CREATE", "plastic.boss"),
            ("INSERT", "FILES", "component.insert_design"),
            ("ASSEMBLE", "COMPONENTS", "component.insert_design"),
            ("SHEET METAL", "CREATE", "sheet.convert"),
            ("SKETCH", "REFERENCE", "canvas.insert"),
        ] {
            assert!(layout(tab, panel, &specs).unwrap().0.iter().any(|c| c.id == id));
        }
    }

    #[test]
    fn contextual_tabs_follow_the_active_body_and_allow_other_tabs() {
        let mut h = crate::scenario::Harness::new();
        assert_eq!(visible_tabs(&h.app), TABS);
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        assert_eq!(h.app.ui.tab, "SKETCH");
        h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[40,30]})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
        h.app.run("sheet.flange", json!({"name":"Sheet", "body_name":"Sheet"})).unwrap();
        assert_eq!(h.app.ui.tab, "SHEET METAL");
        assert!(visible_tabs(&h.app).contains(&"SHEET METAL"));
        let path = h.dir.join("sheet.solidcraft");
        h.app.run("file.save", json!({"path":path})).unwrap();
        h.app.ui.tab = "INSPECT".into();
        h.app.run("doc.open", json!({"path":path})).unwrap();
        assert_eq!(h.app.ui.tab, "SHEET METAL", "opening a document re-enters its sheet context");

        h.app.ui.tab = "INSPECT".into();
        sync_context(&mut h.app);
        assert_eq!(h.app.ui.tab, "INSPECT");
        h.app.run("solid.box", json!({"body_name":"Solid", "operation":"new", "length":10, "width":10, "height":10})).unwrap();
        h.app.run("select.set", json!({"items":[{"type":"body","name":"Solid"}]})).unwrap();
        sync_context(&mut h.app);
        assert!(!visible_tabs(&h.app).contains(&"SHEET METAL"));
        h.app.run("select.set", json!({"items":[{"type":"body","name":"Sheet"}]})).unwrap();
        sync_context(&mut h.app);
        assert_eq!(h.app.ui.tab, "SHEET METAL");
    }

    #[test]
    fn starts_palette_and_control_use_the_new_destinations() {
        let mut h = crate::scenario::Harness::new();
        for (id, tab) in [("parameters.change", "MANAGE"), ("solid.box", "MODEL"), ("surface.patch", "SURFACE")] {
            h.step(&json!({"start":id})).unwrap();
            assert_eq!(h.app.ui.tab, tab, "{id}");
            h.step(&json!({"key":"Escape"})).unwrap();
        }
        h.step(&json!({"key":"S"})).unwrap();
        h.step(&json!({"text":"Change Parameters"})).unwrap();
        h.step(&json!({"key":"Enter"})).unwrap();
        assert_eq!(h.app.ui.tab, "MANAGE");
        h.step(&json!({"key":"Escape"})).unwrap();
        h.app.start("parameters.change");
        assert_eq!(h.app.ui.tab, "MANAGE");
        h.frames(2);
        for tab in TABS {
            assert!(crate::scenario::handle_at(&format!("tab:{tab}")).is_some(), "{tab}");
        }
    }

    #[test]
    fn parameters_and_configurations_open_separately() {
        use serde_json::json;
        let mut h = crate::scenario::Harness::new();
        let specs = solidcraft_engine::command_specs();
        let (commands, _) = layout("MANAGE", "CONFIGURATIONS", &specs).unwrap();
        assert!(commands.iter().any(|c| c.id == "config.configure"));
        h.step(&json!({"click":{"handle":"tab:MANAGE"}})).unwrap();
        h.step(&json!({"click":{"handle":"button:parameters.change"}})).unwrap();
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
        h.step(&json!({"click":{"handle":"button:config.table"}})).unwrap();
        assert!(h.app.params.is_none());
        assert!(matches!(
            h.app.dialog.as_ref().map(|d| &d.kind),
            Some(crate::dialogs::Kind::Motion(crate::dialogs_motion::Mo::Configurations { .. }))
        ));
        assert!(h.widget("Add Column").is_some());
        assert!(h.widget("Add Row").is_some());
    }

    /// #1240: a saved part reaches an assembly from the ASSEMBLE panel (and the File menu) as a
    /// linked component, and a STEP file picked there is inserted as Insert STEP would.
    #[test]
    fn insert_design_is_on_the_assemble_panel_and_inserts_a_saved_part() {
        use serde_json::json;
        let specs = solidcraft_engine::command_specs();
        assert!(layout("ASSEMBLE", "COMPONENTS", &specs).unwrap().0.iter().any(|c| c.id == "component.insert_design"));
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
