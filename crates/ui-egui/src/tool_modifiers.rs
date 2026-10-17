//! Centralised modifier-key binding types for sketch and selection behaviours.
//!
//! [`ModifierCombo`] is a serialisable set of modifier keys. Modifier state that
//! must cross the boundary between the viewport event-loop and rendering helpers
//! (gizmo, canvas) that do not carry `&SolveApp` is communicated through
//! per-frame thread-local flags — the same pattern as [`crate::drag_snap`] —
//! so that neither gizmo nor canvas needs access to application state.
//!
//! Strategy: bindings name a chord; call sites ask whether the chord matches the
//! held keys (`held` / `matches_exact`) instead of reading `Modifiers` fields
//! directly. Platform primary is Ctrl on Windows/Linux and Cmd on macOS; the
//! physical Ctrl key on macOS is never treated as primary.

use std::cell::Cell;

use egui::Modifiers;
use serde::{Deserialize, Serialize};

/// Runtime platform for labels and primary-key normalisation.
/// Prefer [`Platform::of`] over `cfg!(target_os)` so wasm on a Mac still says Cmd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Other,
}

impl Platform {
    pub fn of(ctx: &egui::Context) -> Self {
        match ctx.os() {
            egui::os::OperatingSystem::Mac | egui::os::OperatingSystem::IOS => Self::Mac,
            _ => Self::Other,
        }
    }

    /// Compile-time fallback for unit tests and call sites without a context.
    pub fn host() -> Self {
        if cfg!(target_os = "macos") { Self::Mac } else { Self::Other }
    }
}

/// Held keys, normalised for binding matches.
///
/// `primary` is Ctrl on Windows/Linux and Cmd on macOS (`Modifiers::command`).
/// On macOS, physical Ctrl (`Modifiers::ctrl`) is `other` and never counts as primary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Held {
    pub shift: bool,
    pub primary: bool,
    pub alt: bool,
    /// A key no chord names (physical Ctrl on macOS). Fails exact matches.
    pub other: bool,
}

impl Held {
    pub fn new(m: Modifiers, platform: Platform) -> Self {
        Self { shift: m.shift, primary: m.command, alt: m.alt, other: platform == Platform::Mac && m.ctrl }
    }

    /// Host-platform normalisation without an egui context (tests, prefs helpers).
    pub fn from_modifiers(m: Modifiers) -> Self {
        Self::new(m, Platform::host())
    }
}

/// How a chord matches held keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// Exactly these keys and no others.
    Exact,
    /// At least these keys; extras allowed.
    AtLeast,
}

/// A set of modifier keys for a binding.
///
/// `ctrl` is the persisted field name for **primary** (Ctrl on Windows/Linux, Cmd on macOS).
/// Matching uses [`Held::primary`] (`Modifiers::command`), not physical Ctrl on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModifierCombo {
    pub shift: bool,
    /// Primary: Ctrl (Windows/Linux) or Cmd (macOS). Serde key stays `ctrl` for prefs stability.
    pub ctrl: bool,
    pub alt: bool,
}

impl ModifierCombo {
    pub const NONE: Self = Self { shift: false, ctrl: false, alt: false };
    pub const SHIFT: Self = Self { shift: true, ctrl: false, alt: false };
    pub const CTRL: Self = Self { shift: false, ctrl: true, alt: false };
    pub const ALT: Self = Self { shift: false, ctrl: false, alt: true };
    pub const CTRL_ALT: Self = Self { shift: false, ctrl: true, alt: true };

    pub const fn key_count(self) -> u8 {
        self.shift as u8 + self.ctrl as u8 + self.alt as u8
    }

    pub fn matches(self, held: Held, how: Match) -> bool {
        match how {
            Match::Exact => !held.other && self.shift == held.shift && self.ctrl == held.primary && self.alt == held.alt,
            Match::AtLeast => (!self.shift || held.shift) && (!self.ctrl || held.primary) && (!self.alt || held.alt),
        }
    }

    /// True if **exactly** these modifiers (and no others) are held.
    pub fn matches_exact(self, m: Modifiers) -> bool {
        self.matches(Held::from_modifiers(m), Match::Exact)
    }

    /// True if **at least** these modifiers are held (subset / "held" match).
    pub fn held(self, m: Modifiers) -> bool {
        self.matches(Held::from_modifiers(m), Match::AtLeast)
    }

    /// Human-readable display label for the host platform.
    pub fn label(self) -> String {
        self.label_for(Platform::host())
    }

    /// Human-readable display label for a specific platform.
    pub fn label_for(self, platform: Platform) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.shift {
            parts.push("Shift");
        }
        if self.ctrl {
            parts.push(ctrl_name_for(platform));
        }
        if self.alt {
            parts.push(alt_name_for(platform));
        }
        if parts.is_empty() {
            return "None".into();
        }
        parts.join("+")
    }

    /// All single-key combos, in display order (for a picker widget).
    pub const SINGLE_KEYS: [Self; 3] = [Self::CTRL, Self::ALT, Self::SHIFT];
}

/// "Ctrl/Cmd" on Windows/Linux, "Cmd" on macOS.
pub fn ctrl_name() -> &'static str {
    ctrl_name_for(Platform::host())
}

pub fn ctrl_name_for(platform: Platform) -> &'static str {
    match platform {
        Platform::Mac => "Cmd",
        Platform::Other => "Ctrl/Cmd",
    }
}

/// "Alt" on Windows/Linux, "Option" on macOS.
pub fn alt_name() -> &'static str {
    alt_name_for(Platform::host())
}

pub fn alt_name_for(platform: Platform) -> &'static str {
    match platform {
        Platform::Mac => "Option",
        Platform::Other => "Alt",
    }
}

thread_local! {
    /// Set once per frame from the viewport event-loop; read by gizmo and canvas
    /// helpers that do not receive `&SolveApp`. Communicates whether the current
    /// modifier state enables free (unsnapped) manipulator drags.
    static MANIPULATOR_FREE: Cell<bool> = const { Cell::new(false) };
    /// Shift while sketching forces midpoint targeting (see [`ModAction::TargetMidpoint`]).
    static TARGET_MIDPOINT: Cell<bool> = const { Cell::new(false) };
    /// True while a canvas / gizmo manipulator drag is active this frame (or last frame until cleared).
    static MANIPULATOR_DRAGGING: Cell<bool> = const { Cell::new(false) };
}

/// Set whether manipulator drags are free (no snap) this frame.
///
/// Call from the viewport *before* canvas / gizmo rendering, passing the result of
/// [`crate::prefs::manipulator_free_held`].
pub fn set_manipulator_free(on: bool) {
    MANIPULATOR_FREE.with(|c| c.set(on));
}

/// Whether manipulator drags are free this frame (set by [`set_manipulator_free`]).
pub fn manipulator_free() -> bool {
    MANIPULATOR_FREE.with(Cell::get)
}

/// Set whether sketch refine-snap should target midpoints only this frame.
pub fn set_target_midpoint(on: bool) {
    TARGET_MIDPOINT.with(|c| c.set(on));
}

/// Whether sketch refine-snap is in midpoint-only mode.
pub fn target_midpoint() -> bool {
    TARGET_MIDPOINT.with(Cell::get)
}

/// Set whether a manipulator (arrow / gizmo) drag is active.
pub fn set_manipulator_dragging(on: bool) {
    MANIPULATOR_DRAGGING.with(|c| c.set(on));
}

/// Whether a manipulator drag is active (see [`set_manipulator_dragging`]).
pub fn manipulator_dragging() -> bool {
    MANIPULATOR_DRAGGING.with(Cell::get)
}

/// Semantic modifier actions. `id()` is persisted in prefs: never rename one, only add.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ModAction {
    SuppressSnap = 0,
    FreeManipulatorDrag = 1,
    SelectToggle = 2,
    SelectAdd = 3,
    TargetMidpoint = 4,
    Cycle3dSketchPlane = 5,
    PreviewReselect = 6,
    TextSingleLetter = 7,
    TextHandleConstrain = 8,
    IsoCurveAlternate = 9,
}

impl ModAction {
    pub const ALL: &'static [Self] = &[
        Self::SuppressSnap,
        Self::FreeManipulatorDrag,
        Self::SelectToggle,
        Self::SelectAdd,
        Self::TargetMidpoint,
        Self::Cycle3dSketchPlane,
        Self::PreviewReselect,
        Self::TextSingleLetter,
        Self::TextHandleConstrain,
        Self::IsoCurveAlternate,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::SuppressSnap => "suppress_snap",
            Self::FreeManipulatorDrag => "free_manipulator_drag",
            Self::SelectToggle => "select_toggle",
            Self::SelectAdd => "select_add",
            Self::TargetMidpoint => "target_midpoint",
            Self::Cycle3dSketchPlane => "cycle_3d_sketch_plane",
            Self::PreviewReselect => "preview_reselect",
            Self::TextSingleLetter => "text_single_letter",
            Self::TextHandleConstrain => "text_handle_constrain",
            Self::IsoCurveAlternate => "isocurve_alternate",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.id() == id)
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::SuppressSnap => "ui.mod.suppress_snap",
            Self::FreeManipulatorDrag => "ui.mod.free_manipulator_drag",
            Self::SelectToggle => "ui.mod.select_toggle",
            Self::SelectAdd => "ui.mod.select_add",
            Self::TargetMidpoint => "ui.mod.target_midpoint",
            Self::Cycle3dSketchPlane => "ui.mod.cycle_3d_sketch_plane",
            Self::PreviewReselect => "ui.mod.preview_reselect",
            Self::TextSingleLetter => "ui.mod.text_single_letter",
            Self::TextHandleConstrain => "ui.mod.text_handle_constrain",
            Self::IsoCurveAlternate => "ui.mod.isocurve_alternate",
        }
    }

    pub const fn group(self) -> Option<Group> {
        match self {
            Self::SelectToggle | Self::SelectAdd => Some(Group::SelectionClick),
            _ => None,
        }
    }
}

/// Actions that cannot both apply to one gesture: the most specific matching chord wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    SelectionClick,
}

/// When a binding is sampled relative to a pointer gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Re-evaluated every frame (snap suppression, free drag).
    Live,
    /// Sampled at primary press and latched until release (click selection).
    Press,
}

/// Whether a tool's modifier bindings have been reviewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audit {
    Unaudited,
    /// No tool-specific bindings beyond inherited scopes.
    VerifiedNoOp,
    Implemented,
    /// A behaviour was considered and deliberately left out.
    Declined,
}

/// One binding: an action, default chords, match mode, and rebind flag.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub action: ModAction,
    /// Any one of these activates the action; empty means off by default.
    pub defaults: &'static [ModifierCombo],
    pub matching: Match,
    pub trigger: Trigger,
    pub rebindable: bool,
}

/// Bindings shared by every tool active in that context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Selection,
    SketchDraw,
    SketchDrag,
    Manipulator,
    /// Feature dialogs with a live preview (Primary can roll the preview back for reselect).
    FeaturePreview,
}

impl Scope {
    pub const fn bindings(self) -> &'static [Binding] {
        match self {
            Self::Selection => SELECTION_BINDINGS,
            Self::SketchDraw | Self::SketchDrag => SKETCH_SNAP_BINDINGS,
            Self::Manipulator => MANIPULATOR_BINDINGS,
            Self::FeaturePreview => FEATURE_PREVIEW_BINDINGS,
        }
    }
}

const SELECTION_BINDINGS: &[Binding] = &[
    Binding {
        action: ModAction::SelectAdd,
        defaults: &[ModifierCombo::SHIFT],
        matching: Match::AtLeast,
        trigger: Trigger::Press,
        rebindable: true,
    },
    Binding {
        action: ModAction::SelectToggle,
        defaults: &[ModifierCombo::CTRL],
        matching: Match::AtLeast,
        trigger: Trigger::Press,
        rebindable: true,
    },
];

const SKETCH_SNAP_BINDINGS: &[Binding] = &[
    Binding {
        action: ModAction::SuppressSnap,
        defaults: &[ModifierCombo::ALT],
        matching: Match::AtLeast,
        trigger: Trigger::Live,
        rebindable: true,
    },
    Binding {
        // Shift while sketching / coincident targeting prefers a line midpoint.
        action: ModAction::TargetMidpoint,
        defaults: &[ModifierCombo::SHIFT],
        matching: Match::AtLeast,
        trigger: Trigger::Live,
        rebindable: true,
    },
];

const MANIPULATOR_BINDINGS: &[Binding] = &[Binding {
    action: ModAction::FreeManipulatorDrag,
    defaults: &[ModifierCombo::CTRL, ModifierCombo::ALT],
    matching: Match::AtLeast,
    trigger: Trigger::Live,
    rebindable: true,
}];

const FEATURE_PREVIEW_BINDINGS: &[Binding] = &[Binding {
    // Hold Primary to roll the command preview back so obscured geometry can be (re)selected.
    action: ModAction::PreviewReselect,
    defaults: &[ModifierCombo::CTRL],
    matching: Match::AtLeast,
    trigger: Trigger::Live,
    rebindable: true,
}];

/// One entry per interactive command id.
#[derive(Clone, Copy, Debug)]
pub struct ToolModifiers {
    pub tool: &'static str,
    pub scopes: &'static [Scope],
    /// Overrides scope bindings for the same action.
    pub bindings: &'static [Binding],
    pub audit: Audit,
}

/// Host slices filled by each tool module.
pub const HOSTS: &[&[ToolModifiers]] =
    &[crate::tools::MODIFIERS, crate::sketch_tools::MODIFIERS, crate::sketch_modify_ui::MODIFIERS, crate::dialogs::MODIFIERS];

pub fn descriptor(tool: &str) -> Option<&'static ToolModifiers> {
    HOSTS.iter().flat_map(|h| h.iter()).find(|t| t.tool == tool)
}

/// Bit set of active [`ModAction`]s for the current frame or press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionSet(u64);

impl ActionSet {
    pub fn insert(&mut self, a: ModAction) {
        self.0 |= 1 << (a as u8);
    }

    pub fn contains(self, a: ModAction) -> bool {
        self.0 & (1 << (a as u8)) != 0
    }
}

/// Selection click flags from held modifiers and the shift-add-only preference.
///
/// Returns `(add, allow_toggle)`. When `shift_add_only`, Shift is pure-add and Primary toggles.
pub fn selection_click(held: Held, shift_add_only: bool) -> (bool, bool) {
    let actions = resolve(&[Scope::Selection], None, held, Trigger::Press);
    let toggle = actions.contains(ModAction::SelectToggle);
    let add = toggle || actions.contains(ModAction::SelectAdd);
    let allow_toggle = if shift_add_only { toggle } else { add };
    (add, allow_toggle)
}

/// Whether preview reselect should apply: Primary chord matched and no manipulator drag under way.
pub fn preview_reselect(held: Held, manipulator_dragging: bool) -> bool {
    if manipulator_dragging {
        return false;
    }
    resolve(&[Scope::FeaturePreview], None, held, Trigger::Live).contains(ModAction::PreviewReselect)
}

/// Resolve which actions are active for `held` under the given scopes and optional tool.
pub fn resolve(context: &[Scope], tool: Option<&ToolModifiers>, held: Held, trigger: Trigger) -> ActionSet {
    let mut set = ActionSet::default();
    let mut bindings: Vec<&Binding> = context.iter().flat_map(|s| s.bindings()).collect();
    if let Some(t) = tool {
        for s in t.scopes {
            bindings.extend(s.bindings());
        }
        for b in t.bindings {
            // Tool-specific bindings replace earlier ones with the same action.
            bindings.retain(|x| x.action != b.action);
            bindings.push(b);
        }
    }
    // Most specific chord wins within a group.
    let mut best_group: [Option<(u8, ModAction)>; 1] = [None];
    for b in bindings {
        if b.trigger != trigger {
            continue;
        }
        if b.defaults.is_empty() {
            continue;
        }
        let matched = b.defaults.iter().any(|c| {
            if matches!(b.matching, Match::AtLeast) && c.key_count() == 0 {
                return false;
            }
            c.matches(held, b.matching)
        });
        if !matched {
            continue;
        }
        if let Some(g) = b.action.group() {
            let idx = match g {
                Group::SelectionClick => 0,
            };
            let specificity = b.defaults.iter().map(|c| c.key_count()).max().unwrap_or(0);
            match best_group[idx] {
                Some((s, _)) if s > specificity => {}
                Some((s, prev)) if s == specificity => {
                    // Tie: earlier ModAction::ALL order wins.
                    let prev_i = ModAction::ALL.iter().position(|a| *a == prev).unwrap_or(0);
                    let cur_i = ModAction::ALL.iter().position(|a| *a == b.action).unwrap_or(0);
                    if cur_i < prev_i {
                        best_group[idx] = Some((specificity, b.action));
                    }
                }
                _ => best_group[idx] = Some((specificity, b.action)),
            }
        } else {
            set.insert(b.action);
        }
    }
    for slot in best_group.into_iter().flatten() {
        set.insert(slot.1);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, ctrl: bool, alt: bool) -> Modifiers {
        Modifiers { shift, ctrl, alt, command: ctrl, ..Default::default() }
    }

    #[test]
    fn exact_match_requires_no_extra_keys() {
        let ctrl = ModifierCombo::CTRL;
        assert!(ctrl.matches_exact(mods(false, true, false)));
        assert!(!ctrl.matches_exact(mods(true, true, false)), "extra shift must fail");
        assert!(!ctrl.matches_exact(mods(false, false, false)));
    }

    #[test]
    fn held_match_allows_extra_keys() {
        let ctrl = ModifierCombo::CTRL;
        assert!(ctrl.held(mods(false, true, false)));
        assert!(ctrl.held(mods(true, true, false)), "shift+ctrl is fine for a ctrl binding");
        assert!(!ctrl.held(mods(false, false, false)));
    }

    #[test]
    fn cmd_normalises_to_ctrl() {
        let ctrl = ModifierCombo::CTRL;
        let cmd_mods = Modifiers { command: true, ..Default::default() };
        assert!(ctrl.held(cmd_mods), "Cmd on macOS should match a ctrl binding");
        assert!(ctrl.matches_exact(cmd_mods), "exact match also normalises command");
    }

    #[test]
    fn mac_physical_ctrl_is_not_primary() {
        let ctrl = ModifierCombo::CTRL;
        let physical = Modifiers { ctrl: true, command: false, ..Default::default() };
        let held = Held::new(physical, Platform::Mac);
        assert!(!held.primary && held.other);
        assert!(!ctrl.matches(held, Match::AtLeast));
        assert!(!ctrl.matches(held, Match::Exact));
        let win = Held::new(Modifiers { ctrl: true, command: true, ..Default::default() }, Platform::Other);
        assert!(ctrl.matches(win, Match::AtLeast));
    }

    #[test]
    fn label_formats_correctly() {
        assert_eq!(ModifierCombo::CTRL.label_for(Platform::Other), "Ctrl/Cmd");
        assert_eq!(ModifierCombo::CTRL.label_for(Platform::Mac), "Cmd");
        assert_eq!(ModifierCombo::NONE.label_for(Platform::Other), "None");
        assert_eq!(ModifierCombo::CTRL_ALT.label_for(Platform::Other), "Ctrl/Cmd+Alt");
        assert_eq!(ModifierCombo::CTRL_ALT.label_for(Platform::Mac), "Cmd+Option");
        assert_eq!(ModifierCombo { shift: true, ctrl: true, alt: false }.label_for(Platform::Other), "Shift+Ctrl/Cmd");
    }

    #[test]
    fn manipulator_free_thread_local_roundtrip() {
        set_manipulator_free(true);
        assert!(manipulator_free());
        set_manipulator_free(false);
        assert!(!manipulator_free());
    }

    #[test]
    fn target_midpoint_live_resolves_from_sketch_scopes() {
        set_target_midpoint(false);
        let shift = Held { shift: true, ..Default::default() };
        let set = resolve(&[Scope::SketchDraw], None, shift, Trigger::Live);
        assert!(set.contains(ModAction::TargetMidpoint));
        assert!(!set.contains(ModAction::SuppressSnap));
        set_target_midpoint(true);
        assert!(target_midpoint());
        set_target_midpoint(false);
        assert!(!target_midpoint());
    }

    #[test]
    fn preview_reselect_resolves_from_feature_preview_scope() {
        let primary = Held { primary: true, ..Default::default() };
        let set = resolve(&[Scope::FeaturePreview], None, primary, Trigger::Live);
        assert!(set.contains(ModAction::PreviewReselect));
        let press = resolve(&[Scope::FeaturePreview], None, primary, Trigger::Press);
        assert!(!press.contains(ModAction::PreviewReselect));
    }

    #[test]
    fn preview_reselect_skips_while_manipulator_dragging() {
        let primary = Held { primary: true, ..Default::default() };
        assert!(preview_reselect(primary, false));
        assert!(!preview_reselect(primary, true), "Ctrl free-drag must not hide the preview");
        set_manipulator_dragging(true);
        assert!(manipulator_dragging());
        set_manipulator_dragging(false);
        assert!(!manipulator_dragging());
    }

    #[test]
    fn selection_scope_resolves_shift_add_and_primary_toggle() {
        let shift = Held { shift: true, ..Default::default() };
        let primary = Held { primary: true, ..Default::default() };
        let both = Held { shift: true, primary: true, ..Default::default() };
        let add = resolve(&[Scope::Selection], None, shift, Trigger::Press);
        assert!(add.contains(ModAction::SelectAdd));
        assert!(!add.contains(ModAction::SelectToggle));
        let toggle = resolve(&[Scope::Selection], None, primary, Trigger::Press);
        assert!(toggle.contains(ModAction::SelectToggle));
        assert!(!toggle.contains(ModAction::SelectAdd));
        // Shift+Primary: single-key tie → SelectToggle wins (earlier in ModAction::ALL).
        let combo = resolve(&[Scope::Selection], None, both, Trigger::Press);
        assert!(combo.contains(ModAction::SelectToggle));
        assert!(!combo.contains(ModAction::SelectAdd));
    }

    #[test]
    fn selection_click_respects_shift_add_only_pref() {
        let shift = Held { shift: true, ..Default::default() };
        let primary = Held { primary: true, ..Default::default() };
        assert_eq!(selection_click(shift, true), (true, false));
        assert_eq!(selection_click(shift, false), (true, true));
        assert_eq!(selection_click(primary, true), (true, true));
        assert_eq!(selection_click(Held::default(), true), (false, false));
    }

    #[test]
    fn isocurve_shift_resolves_via_descriptor() {
        let held = Held { shift: true, ..Default::default() };
        let set = resolve(&[], descriptor("sketch.isoparametric_curve"), held, Trigger::Press);
        assert!(set.contains(ModAction::IsoCurveAlternate));
        let bare = resolve(&[], None, held, Trigger::Press);
        assert!(!bare.contains(ModAction::IsoCurveAlternate));
    }

    #[test]
    fn action_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for a in ModAction::ALL {
            assert!(seen.insert(a.id()), "duplicate id {}", a.id());
            assert_eq!(ModAction::from_id(a.id()), Some(*a));
        }
    }

    #[test]
    fn tools_host_covers_interactive_sketch_create_commands() {
        for entry in crate::tools::MODIFIERS {
            assert!(crate::tools::Tool::for_command(entry.tool).is_some(), "orphan descriptor {}", entry.tool);
            assert_eq!(descriptor(entry.tool).map(|d| d.tool), Some(entry.tool));
        }
        // Every command owned directly by tools.rs (not sketch_tools) has a descriptor.
        for id in ["sketch.line", "sketch.rectangle.two_point", "sketch.circle.center", "sketch.constraint.coincident", "sketch.dimension"] {
            assert!(descriptor(id).is_some(), "missing descriptor for {id}");
        }
    }

    #[test]
    fn hosts_have_unique_tool_ids_and_cover_seeded_commands() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in HOSTS.iter().flat_map(|h| h.iter()) {
            assert!(seen.insert(entry.tool), "duplicate tool id {}", entry.tool);
        }
        for id in ["sketch.isoparametric_curve", "sketch.pattern.rectangular", "solid.pattern.circular", "solid.extrude", "sketch.move"] {
            assert!(descriptor(id).is_some(), "missing descriptor for {id}");
        }
        assert_eq!(descriptor("sketch.pattern.rectangular").unwrap().audit, Audit::VerifiedNoOp);
        assert_eq!(descriptor("solid.pattern.circular").unwrap().audit, Audit::VerifiedNoOp);
        assert_eq!(descriptor("sketch.isoparametric_curve").unwrap().audit, Audit::Implemented);
        assert_eq!(descriptor("sketch.line").unwrap().audit, Audit::Implemented);
        // Seeded hosts: no Unaudited leftovers once scopes are assigned.
        for entry in HOSTS.iter().flat_map(|h| h.iter()) {
            assert_ne!(entry.audit, Audit::Unaudited, "{} still Unaudited", entry.tool);
        }
    }
}
