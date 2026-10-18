//! The command registry. Ids are SolidCraft's own dotted ids (`solid.extrude`,
//! `sketch.rectangle.two_point`); the toolbar placement (`tab`, `panel`) follows the familiar
//! Design workspace layout.

/// The params-doc note of every command that adds sketch geometry (see `sketch::created`).
macro_rules! created_doc {
    () => {
        ". Returns created: {points, curves, constraints, dimensions}: the ids of what it added (stable and never reused: usable right away in sketch.dimension, sketch.constraint.*, sketch.delete…)"
    };
}

/// A command that is designed and registered (id, toolbar place, icon, params doc) but not built
/// yet. It is disabled with the reason, so the toolbar greys it and says why, and its `run`
/// refuses too, however it is reached. `$issue` is the GitHub issue that tracks building it.
macro_rules! planned {
    ($id:literal, $label:literal, $issue:literal) => {
        CommandSpec::new($id, $label, |_, _| Err(crate::EngineError::Other(concat!("not supported yet: ", $label, " (issue #", $issue, ")").into())))
            .enabled(|_| Err(concat!("not supported yet (issue #", $issue, ")").into()))
            .planned($issue)
    };
}

mod browser;
mod clearance;
mod clipboard;
mod compare;
mod component;
mod component_clone;
mod component_clone_assembly;
mod construct;
mod delete;
mod diagnostics;
mod edit;
mod face;
mod features;
mod features_more;
pub use features::{auto_operation, extrude_reach};
mod component_link;
mod config;
mod file;
mod inspect;
pub(crate) mod joints;
mod libraries;
mod material_properties;
mod materials;
mod measure_sel;
mod measure_shape;
mod mesh;
mod metadata;
pub(crate) mod motion;
mod parts;
mod script;
mod section;
pub(crate) use measure_sel::measure_items;
#[cfg(test)]
mod kernel_heal_tests;
mod parameters;
mod plastic;
mod render;
#[cfg(test)]
mod script_tests;
mod sheet;
mod silhouette;
mod sketch;
pub(crate) use sketch::author_path_for_rebind;
pub(crate) use sketch::created as sketch_created;
mod sketch_3d;
mod sketch_canvas;
pub mod sketch_clipboard;
mod sketch_constraints;
mod sketch_create;
mod sketch_ends;
mod sketch_freeform;
mod sketch_import;
mod sketch_inspect;
mod sketch_modify;
mod sketch_project;
mod surface;
mod surface_analysis;
mod surface_boundary;
mod surface_match;
mod view_dxf;
mod view_navigation;

use serde::Serialize;
use serde_json::Value;

use crate::{Result, Session};

pub type Run = fn(&mut Session, &Value) -> Result<Value>;
/// A command's form for one frame of an interactive gesture: its result and the sketch it
/// changed, for [`Session::execute`] to show without evaluating the rest of the model (`None`:
/// evaluate as usual).
pub type LiveRun = fn(&mut Session, &Value) -> Result<(Value, Option<crate::LiveEdit>)>;
pub type Enabled = fn(&Session) -> std::result::Result<(), String>;

pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Toolbar tab (`SOLID`, `SKETCH`…); empty = not on the toolbar.
    pub tab: &'static str,
    /// Toolbar panel (`CREATE`, `MODIFY`…).
    pub panel: &'static str,
    /// Icon name for the UI (drawn in code).
    pub icon: &'static str,
    pub shortcut: Option<&'static str>,
    /// One-line parameter documentation.
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    pub undoable: bool,
    /// What runs instead of `run` when the command is given `live: true` (a frame of an
    /// interactive gesture, see [`Session::execute`]).
    pub live: Option<LiveRun>,
    /// Reads the evaluated model (the default). A command that reads only the document and the
    /// session's history runs on a model left stale by a live gesture without evaluating it first.
    pub reads_model: bool,
    /// Registered but not built yet (see `planned!`): the GitHub issue number that tracks it.
    pub planned: Option<&'static str>,
}

impl CommandSpec {
    pub const fn new(id: &'static str, label: &'static str, run: Run) -> Self {
        CommandSpec {
            id,
            label,
            tab: "",
            panel: "",
            icon: "",
            shortcut: None,
            params: "",
            enabled: always,
            run,
            undoable: true,
            live: None,
            reads_model: true,
            planned: None,
        }
    }
    /// Not built yet; `issue` tracks building it.
    pub const fn planned(mut self, issue: &'static str) -> Self {
        self.planned = Some(issue);
        self
    }
    /// Takes `live: true`, run by `run`.
    pub const fn live(mut self, run: LiveRun) -> Self {
        self.live = Some(run);
        self
    }
    /// Reads only the document (and history), never the evaluated model.
    pub const fn doc_only(mut self) -> Self {
        self.reads_model = false;
        self
    }
    pub const fn at(mut self, tab: &'static str, panel: &'static str) -> Self {
        self.tab = tab;
        self.panel = panel;
        self
    }
    pub const fn icon(mut self, i: &'static str) -> Self {
        self.icon = i;
        self
    }
    pub const fn key(mut self, k: &'static str) -> Self {
        self.shortcut = Some(k);
        self
    }
    pub const fn params(mut self, p: &'static str) -> Self {
        self.params = p;
        self
    }
    pub const fn enabled(mut self, e: Enabled) -> Self {
        self.enabled = e;
        self
    }
    pub const fn noundo(mut self) -> Self {
        self.undoable = false;
        self
    }
    /// Run inside another command: when it is enabled, its `run` (the caller's command owns the
    /// undo step and the evaluation).
    pub(crate) fn run_nested(&self, s: &mut Session, p: &Value) -> Result<Value> {
        (self.enabled)(s).map_err(|why| crate::EngineError::Disabled(self.id.to_string(), why))?;
        (self.run)(s, p)
    }
    pub fn info(&self, s: &Session) -> CommandInfo {
        let e = (self.enabled)(s);
        CommandInfo {
            id: self.id,
            label: self.label,
            tab: self.tab,
            panel: self.panel,
            icon: self.icon,
            shortcut: self.shortcut,
            params: self.params,
            enabled: e.is_ok(),
            disabled_reason: e.err(),
            planned: self.planned,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub tab: &'static str,
    pub panel: &'static str,
    pub icon: &'static str,
    pub shortcut: Option<&'static str>,
    pub params: &'static str,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned: Option<&'static str>,
}

pub fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

pub fn in_sketch(s: &Session) -> std::result::Result<(), String> {
    if s.active_sketch.is_some() { Ok(()) } else { Err("no sketch is being edited (Create Sketch or Edit Sketch first)".into()) }
}

/// The mirror of [`in_sketch`], for a command that acts on bodies and has nothing to offer the
/// geometry under the cursor while a sketch is open.
pub fn not_in_sketch(s: &Session) -> std::result::Result<(), String> {
    if s.active_sketch.is_none() { Ok(()) } else { Err("a sketch is being edited (this works on bodies: Finish Sketch first)".into()) }
}

/// Every command, in toolbar order.
pub fn command_specs() -> Vec<&'static CommandSpec> {
    let mut v: Vec<&'static CommandSpec> = Vec::new();
    v.extend(sketch::COMMANDS.iter());
    v.extend(sketch_create::COMMANDS.iter());
    v.extend(sketch_constraints::COMMANDS.iter());
    v.extend(sketch_import::COMMANDS.iter());
    v.extend(sketch_inspect::COMMANDS.iter());
    v.extend(sketch_ends::COMMANDS.iter());
    v.extend(sketch_canvas::COMMANDS.iter());
    v.extend(sketch_3d::COMMANDS.iter());
    v.extend(surface_analysis::COMMANDS.iter());
    v.extend(surface_boundary::COMMANDS.iter());
    v.extend(surface::COMMANDS.iter());
    v.extend(surface_match::COMMANDS.iter());
    v.extend(sketch_modify::COMMANDS.iter());
    v.extend(sketch_project::COMMANDS.iter());
    v.extend(features::COMMANDS.iter());
    v.extend(features_more::COMMANDS.iter());
    v.extend(mesh::COMMANDS.iter());
    v.extend(construct::COMMANDS.iter());
    v.extend(sheet::COMMANDS.iter());
    v.extend(plastic::COMMANDS.iter());
    v.extend(component::COMMANDS.iter());
    v.extend(component_link::COMMANDS.iter());
    v.extend(joints::COMMANDS.iter());
    v.extend(clearance::COMMANDS.iter());
    v.extend(motion::COMMANDS.iter());
    v.extend(config::COMMANDS.iter());
    v.extend(parts::COMMANDS.iter());
    v.extend(browser::COMMANDS.iter());
    v.extend(delete::COMMANDS.iter());
    v.extend(edit::COMMANDS.iter());
    v.extend(clipboard::COMMANDS.iter());
    v.extend(sketch_clipboard::COMMANDS.iter());
    v.extend(script::COMMANDS.iter());
    v.extend(parameters::COMMANDS.iter());
    v.extend(file::COMMANDS.iter());
    v.extend(metadata::COMMANDS.iter());
    v.extend(inspect::COMMANDS.iter());
    v.extend(compare::COMMANDS.iter());
    v.extend(silhouette::COMMANDS.iter());
    v.extend(materials::COMMANDS.iter());
    v.extend(material_properties::COMMANDS.iter());
    v.extend(libraries::COMMANDS.iter());
    v.extend(section::COMMANDS.iter());
    v.extend(view_navigation::COMMANDS.iter());
    v.extend(view_dxf::COMMANDS.iter());
    v.extend(diagnostics::COMMANDS.iter());
    v.extend(render::COMMANDS.iter());
    v
}

pub fn find_command(id: &str) -> Option<&'static CommandSpec> {
    let all = command_specs();
    all.iter().find(|c| c.id == id).or_else(|| all.iter().find(|c| c.id.eq_ignore_ascii_case(id))).copied()
}

/// The id for a new item of a list: one past the largest id in it. Ids come from the design file,
/// so the largest may be the largest a `u64` holds, and one past that is an error, not an overflow
/// (a panic where overflow is checked, an id that is already taken where it is not).
pub(crate) fn next_free_id(cmd: &str, ids: impl Iterator<Item = u64>) -> Result<u64> {
    ids.max().unwrap_or(0).checked_add(1).ok_or_else(|| crate::params::bad(cmd, "this design has no ids left to give"))
}

/// The end of an unknown command's error: up to three ids like `id`, nearest first. Ids a few
/// letters away (`solid.extrud` → `solid.extrude`) come before ids that end in the same word
/// (`file.open` → `doc.open`). Empty when nothing is close.
pub(crate) fn did_you_mean(id: &str) -> String {
    let id = id.to_ascii_lowercase();
    if id.len() > 64 {
        return String::new();
    }
    let last = |s: &str| s.rsplit('.').next().unwrap_or(s).to_string();
    let near = (id.chars().count() / 4).max(2);
    let mut found: Vec<(bool, usize, &str)> = command_specs()
        .iter()
        .filter_map(|c| {
            let d = edit_distance(&id, c.id);
            (d <= near || last(&id) == last(c.id)).then_some((d > near, d, c.id))
        })
        .collect();
    found.sort();
    let ids: Vec<String> = found.iter().take(3).map(|(_, _, c)| format!("`{c}`")).collect();
    match ids.as_slice() {
        [] => String::new(),
        [one] => format!(" (did you mean {one}?)"),
        [rest @ .., end] => format!(" (did you mean {} or {end}?)", rest.join(", ")),
    }
}

/// Levenshtein distance: the single-character edits that turn `a` into `b`.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(row[j + 1] + 1);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

pub(super) fn alignment_param(p: &Value, key: &str, cmd: &str) -> Result<solidcraft_sketch::PatternAlignment> {
    p.get(key).map_or(Ok(solidcraft_sketch::PatternAlignment::default()), |value| {
        serde_json::from_value(value.clone()).map_err(|_| crate::params::bad(cmd, format!("`{key}` must be left, center or right")))
    })
}

pub(super) fn reversed_param(p: &Value, key: &str, cmd: &str) -> Result<bool> {
    p.get(key).map_or(Ok(false), |value| {
        serde_json::from_value(value.clone()).map_err(|_| crate::params::bad(cmd, format!("`{key}` must be true or false")))
    })
}
