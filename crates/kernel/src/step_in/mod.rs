//! STEP import (ISO 10303-21 files, AP203/AP214/AP242 geometry).
//!
//! Our own reader, written from the public standards: [`p21`] parses the exchange structure,
//! [`geom`] and [`brep`] turn geometry and topology into kernel solids, and this module walks
//! the product structure (products, shape representations, assemblies with their placements,
//! units, colours). STEP files are untrusted: every count and nesting depth is capped, every
//! truck call runs under [`guard`], and what cannot be read becomes a warning rather than a
//! failure wherever the rest of the file is still usable.

mod brep;
pub(crate) mod geom;
pub(crate) mod p21;
mod parallel;
pub(crate) mod split;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use mt::EuclideanSpace as _;
use serde::{Deserialize, Serialize};
use truck_modeling as mt;

use crate::body::Body;
use crate::{KernelError, Result, guard};
use geom::R;
use p21::{Entity, Exchange, Param};

/// Deepest assembly nesting followed.
const MAX_ASSEMBLY_DEPTH: usize = 32;
/// Most body instances in one import.
const MAX_BODIES: usize = 10_000;
/// Most product occurrences and mapped representations visited (a shared sub-assembly used
/// twice at every level would otherwise grow exponentially).
const MAX_VISITS: usize = 100_000;
/// Most representation entries walked while deciding which items are converted exactly once. A
/// larger file keeps every conversion instead of paying for the walk.
const MAX_CLOSURE_WORK: usize = 1 << 16;

/// A body read from a STEP file.
#[derive(Clone, Debug)]
pub struct ImportedBody {
    /// Immutable STEP source item, scoped to its embedded payload.
    pub source_item: u64,
    /// Original STEP face entity of each kernel face, including seam pieces.
    pub face_origin: Vec<u64>,
    /// Solid name (the brep's label), else the product name.
    pub name: String,
    pub body: Body,
    /// Surface colour (linear-ish RGB 0..1) from the file's presentation styles.
    pub color: Option<[f32; 3]>,
    /// Product names from the root to the part that holds the body (the last is the part's own
    /// name; empty for a solid outside any product).
    pub path: Vec<String>,
    /// The name of the assembly occurrence (instance) that holds the body: the usage's name in
    /// the file, else its id. `None` for a body of a root product or outside any product.
    pub instance: Option<String>,
    /// The body is a closed solid (false: faces were left out, or the file had a surface model).
    pub closed: bool,
    /// Faces read from the file. The body can have more: a face that wraps all the way around
    /// a closed surface is split in two for the kernel.
    pub file_faces: usize,
}

/// One product occurrence in the file's assembly tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepNode {
    pub name: String,
    /// Placement in the parent (column-major 4×4, millimetres).
    pub transform: [[f64; 4]; 4],
    /// Indices into [`StepImport::bodies`] of this product's own bodies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bodies: Vec<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<StepNode>,
}

/// Result of [`step_import`].
#[derive(Clone, Debug)]
pub struct StepImport {
    pub bodies: Vec<ImportedBody>,
    /// Root products (an assembly tree; a plain part is a single root).
    pub tree: Vec<StepNode>,
    /// What could not be read (unsupported entities, faces left out…).
    pub warnings: Vec<String>,
    /// FILE_SCHEMA (e.g. `AUTOMOTIVE_DESIGN`, `CONFIG_CONTROL_DESIGN`, `AP242_…`).
    pub schema: String,
    /// FILE_NAME originating system.
    pub system: String,
}

/// A solid the walk found, converted later.
struct Planned {
    item: u64,
    units: Units,
    batch: usize,
}

/// Where the solids of a batch go: their placement and names.
struct Batch {
    m: mt::Matrix4,
    path: Vec<String>,
    product: String,
    instance: Option<String>,
}

/// Reading context: the file and the units of the representation being read.
pub(crate) struct Ctx<'a> {
    pub ex: &'a Exchange,
    /// Millimetres per file length unit.
    pub len: f64,
    /// Radians per file angle unit.
    pub ang: f64,
    /// Vertex-on-curve tolerance (mm).
    pub tol: f64,
}

#[derive(Clone, Copy)]
struct Units {
    len: f64,
    ang: f64,
    tol: f64,
}

const DEFAULT_UNITS: Units = Units { len: 1.0, ang: 1.0, tol: 0.01 };

fn si_prefix(p: Option<&Param>) -> f64 {
    match p.and_then(Param::as_enum) {
        Some("EXA") => 1e18,
        Some("PETA") => 1e15,
        Some("TERA") => 1e12,
        Some("GIGA") => 1e9,
        Some("MEGA") => 1e6,
        Some("KILO") => 1e3,
        Some("HECTO") => 1e2,
        Some("DECA") => 1e1,
        Some("DECI") => 1e-1,
        Some("CENTI") => 1e-2,
        Some("MILLI") => 1e-3,
        Some("MICRO") => 1e-6,
        Some("NANO") => 1e-9,
        Some("PICO") => 1e-12,
        Some("FEMTO") => 1e-15,
        Some("ATTO") => 1e-18,
        _ => 1.0,
    }
}

/// A unit's size: millimetres for lengths, radians for plane angles.
fn unit_factor(ex: &Exchange, id: u64, depth: usize) -> Option<(&'static str, f64)> {
    if depth > 4 {
        return None;
    }
    let e = ex.get(id)?;
    let kind = if e.has("LENGTH_UNIT") {
        "length"
    } else if e.has("PLANE_ANGLE_UNIT") {
        "angle"
    } else {
        "other"
    };
    if let Some(si) = e.record("SI_UNIT") {
        let base = match si.get(1).and_then(Param::as_enum) {
            Some("METRE") => 1000.0,
            Some("RADIAN") => 1.0,
            _ => return Some((kind, 1.0)),
        };
        return Some((kind, si_prefix(si.first()) * base));
    }
    if let Some(cb) = e.record("CONVERSION_BASED_UNIT") {
        let m = ex.get(cb.get(1)?.as_ref_id()?)?;
        let mp = m.params();
        let v = mp.first()?.as_f64()?;
        let (_, f) = unit_factor(ex, mp.get(1)?.as_ref_id()?, depth + 1)?;
        let x = v * f;
        return (x.is_finite() && x > 0.0).then_some((kind, x));
    }
    Some((kind, 1.0))
}

/// Units of a representation context (GLOBAL_UNIT_ASSIGNED_CONTEXT, uncertainty).
fn context_units(ex: &Exchange, ctx: Option<u64>, cache: &mut HashMap<u64, Units>) -> Units {
    let Some(id) = ctx else { return DEFAULT_UNITS };
    if let Some(u) = cache.get(&id) {
        return *u;
    }
    let mut u = DEFAULT_UNITS;
    let mut tol_file = None;
    if let Some(e) = ex.get(id) {
        if let Some(units) = e.record("GLOBAL_UNIT_ASSIGNED_CONTEXT").and_then(|r| r.first()).and_then(Param::as_list) {
            for uid in units.iter().filter_map(Param::as_ref_id) {
                match unit_factor(ex, uid, 0) {
                    Some(("length", f)) if f.is_finite() && f > 0.0 => u.len = f,
                    Some(("angle", f)) if f.is_finite() && f > 0.0 => u.ang = f,
                    _ => {}
                }
            }
        }
        if let Some(unc) = e.record("GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT").and_then(|r| r.first()).and_then(Param::as_list) {
            for uid in unc.iter().filter_map(Param::as_ref_id) {
                if let Some(m) = ex.get(uid)
                    && let Some(v) = m.params().first().and_then(Param::as_f64)
                {
                    let f = m.params().get(1).and_then(Param::as_ref_id).and_then(|x| unit_factor(ex, x, 0)).map(|x| x.1);
                    tol_file = Some(v * f.unwrap_or(u.len));
                }
            }
        }
    }
    u.tol = tol_file.filter(|t| t.is_finite() && *t > 0.0).map(|t| (t * 10.0).max(1e-3)).unwrap_or(u.len * 1e-2).clamp(1e-4, 1.0);
    cache.insert(id, u);
    u
}

/// Items and context of a representation entity.
fn representation(e: &Entity) -> Option<(Vec<u64>, Option<u64>)> {
    let p = if e.is_complex() {
        e.record("REPRESENTATION")
            .or_else(|| e.records.iter().find(|r| r.name.ends_with("REPRESENTATION") && r.params.len() >= 3).map(|r| r.params.as_slice()))?
    } else if e.name().ends_with("REPRESENTATION") {
        e.params()
    } else {
        return None;
    };
    let items = p.get(1)?.as_list()?.iter().filter_map(Param::as_ref_id).collect();
    Some((items, p.get(2).and_then(Param::as_ref_id)))
}

fn mat_mul(a: &mt::Matrix4, b: &mt::Matrix4) -> mt::Matrix4 {
    a * b
}

fn to_array(m: &mt::Matrix4) -> [[f64; 4]; 4] {
    [[m.x.x, m.x.y, m.x.z, m.x.w], [m.y.x, m.y.y, m.y.z, m.y.w], [m.z.x, m.z.y, m.z.z, m.z.w], [m.w.x, m.w.y, m.w.z, m.w.w]]
}

/// Inverse of a rigid placement matrix (orthonormal rotation + translation).
fn rigid_inverse(m: &mt::Matrix4) -> mt::Matrix4 {
    use mt::SquareMatrix;
    m.invert().unwrap_or_else(mt::Matrix4::identity)
}

/// One converted representation item: the truck solid, whether its shells are closed, the
/// faces the file had, and the file face of each kernel face (band faces split into two).
type SolidEntry = (mt::Solid, bool, usize, Vec<u64>);

struct Reader {
    ex: Arc<Exchange>,
    units: HashMap<u64, Units>,
    warnings: BTreeMap<String, usize>,
    /// Representation → its items and context.
    reps: HashMap<u64, (Vec<u64>, Option<u64>)>,
    /// Plain representation relationships (same product, no placement).
    rep_links: HashMap<u64, Vec<u64>>,
    /// Product definition → its shape representations.
    pd_reps: HashMap<u64, Vec<u64>>,
    /// Product definition → child occurrences (NAUO ids).
    children: HashMap<u64, Vec<u64>>,
    /// NAUO → placement relationship entity.
    placements: HashMap<u64, u64>,
    /// Styled item → colour.
    colors: HashMap<u64, [f32; 3]>,
    /// Opacity of styled items with a transparency (1 = opaque).
    opacity: HashMap<u64, f32>,
    /// Converted items (brep id → entry, or the error). Held behind `Arc` so the conversion is
    /// handed on without copying wherever the file cannot ask for the item a second time.
    solids: HashMap<u64, std::result::Result<Arc<SolidEntry>, String>>,
    /// Items the file converts exactly once (see `index`): keeping their solids would hold a
    /// second copy of the whole import for nothing.
    once: HashSet<u64>,
    /// An item placed more than once: its first instance's display, shared by the later ones
    /// (with the placement that first instance had), so the part is meshed once.
    shown: HashMap<u64, (Arc<crate::body::SharedDisplay>, mt::Matrix4)>,
    used_items: HashSet<u64>,
    /// Items already instanced for the product being read.
    node_items: HashSet<u64>,
    /// Occurrences and mapped representations visited so far.
    visits: usize,
    /// Items whose meshing was checked.
    unmeshed_checked: HashSet<u64>,
    bodies: Vec<ImportedBody>,
    /// Solids the walk found, in file order, to be converted into bodies afterwards (so the
    /// conversion, the long part, can be done a piece at a time: [`StepReading`]).
    plan: Vec<Planned>,
    /// What the solids of one batch (consecutive solids of one representation) share.
    batches: Vec<Batch>,
    /// Planned solids converted so far.
    converted: usize,
    /// The body each converted planned solid became (`None`: it failed).
    made: Vec<Option<usize>>,
    workers: usize,
    /// Whoever waits for the import (progress, Cancel).
    watch: Option<Arc<dyn crate::watch::Watch>>,
    /// Solid items in the file, the measure of the conversion's progress.
    solid_items: usize,
    /// The watch cancelled the import: the walk stops and the import fails.
    cancelled: bool,
}

impl Reader {
    fn new(ex: Arc<Exchange>, workers: usize) -> Self {
        Self {
            ex,
            units: HashMap::new(),
            warnings: BTreeMap::new(),
            reps: HashMap::new(),
            rep_links: HashMap::new(),
            pd_reps: HashMap::new(),
            children: HashMap::new(),
            placements: HashMap::new(),
            colors: HashMap::new(),
            opacity: HashMap::new(),
            solids: HashMap::new(),
            once: HashSet::new(),
            shown: HashMap::new(),
            used_items: HashSet::new(),
            node_items: HashSet::new(),
            visits: 0,
            unmeshed_checked: HashSet::new(),
            bodies: Vec::new(),
            plan: Vec::new(),
            batches: Vec::new(),
            converted: 0,
            made: Vec::new(),
            workers,
            watch: None,
            solid_items: 0,
            cancelled: false,
        }
    }

    /// Report progress; true once the import is cancelled (the walk then unwinds).
    fn stop(&mut self) -> bool {
        if self.cancelled {
            return true;
        }
        if let Some(w) = &self.watch {
            if w.cancelled() {
                self.cancelled = true;
                return true;
            }
            w.progress("converting", self.converted.min(self.solid_items), self.solid_items);
        }
        false
    }

    fn warn(&mut self, msg: impl Into<String>) {
        *self.warnings.entry(msg.into()).or_default() += 1;
    }

    fn index(&mut self) {
        let ex = self.ex.clone();
        let ex = &*ex;
        let mut pds: HashMap<u64, u64> = HashMap::new(); // PDS → definition
        let mut sdrs: Vec<(u64, u64)> = Vec::new(); // (PDS, rep)
        let mut cdsrs: Vec<(u64, u64)> = Vec::new(); // (relationship, PDS)
        let mut nauos: Vec<(u64, u64, u64)> = Vec::new(); // (id, parent, child)
        // Item → how many shape representations list it, and the one representation the item
        // comes from, to tell items the file converts exactly once (see `once`).
        let mut item_reps: HashMap<u64, usize> = HashMap::new();
        let mut item_rep: HashMap<u64, u64> = HashMap::new();
        // True as soon as the file can walk a product definition twice: every occurrence of a
        // component makes the reader convert the component's items again.
        let mut repeated = false;
        let mut ids: Vec<&u64> = ex.entities.keys().collect();
        ids.sort();
        for &id in ids {
            let Some(e) = ex.get(id) else { continue };
            if let Some(r) = representation(e) {
                for item in &r.0 {
                    *item_reps.entry(*item).or_default() += 1;
                    item_rep.entry(*item).or_insert(id);
                }
                self.reps.insert(id, r);
            }
            let p = e.params();
            let rf = |i: usize| p.get(i).and_then(Param::as_ref_id);
            match e.name() {
                "PRODUCT_DEFINITION_SHAPE" | "PROPERTY_DEFINITION" => {
                    if let Some(d) = rf(2) {
                        pds.insert(id, d);
                    }
                }
                "SHAPE_DEFINITION_REPRESENTATION" => {
                    if let (Some(d), Some(r)) = (rf(0), rf(1)) {
                        sdrs.push((d, r));
                    }
                }
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" => {
                    repeated = true;
                    if let (Some(r), Some(d)) = (rf(0), rf(1)) {
                        cdsrs.push((r, d));
                    }
                }
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE" | "ASSEMBLY_COMPONENT_USAGE" | "SPECIFIED_HIGHER_USAGE_OCCURRENCE" => {
                    repeated = true;
                    if let (Some(a), Some(b)) = (rf(3), rf(4)) {
                        nauos.push((id, a, b));
                    }
                }
                "SHAPE_REPRESENTATION_RELATIONSHIP" | "REPRESENTATION_RELATIONSHIP" if !e.is_complex() => {
                    if let (Some(a), Some(b)) = (rf(2), rf(3)) {
                        self.rep_links.entry(a).or_default().push(b);
                        self.rep_links.entry(b).or_default().push(a);
                    }
                }
                "STYLED_ITEM" | "OVER_RIDING_STYLED_ITEM" => {
                    if let Some(item) = rf(2)
                        && let Some(c) = self.find_colour(p.get(1))
                    {
                        self.colors.entry(item).or_insert(c);
                        if let Some(t) = self.find_transparency(p.get(1)) {
                            self.opacity.entry(item).or_insert((1.0 - t).clamp(0.0, 1.0));
                        }
                    }
                }
                _ => {}
            }
        }
        for (d, r) in sdrs {
            if let Some(&pd) = pds.get(&d) {
                self.pd_reps.entry(pd).or_default().push(r);
            }
        }
        for (r, d) in cdsrs {
            if let Some(&nauo) = pds.get(&d) {
                self.placements.insert(nauo, r);
            }
        }
        for (id, parent, child) in nauos {
            if parent != child {
                self.children.entry(parent).or_default().push(id);
            }
        }
        // A file without component occurrences walks each product definition once, so a
        // representation reached from exactly one of them is walked once too. The walk follows
        // MAPPED_ITEM targets as the reader does: a mapping in one product can walk a
        // representation a second time from another product's node. An item that only one such
        // walk lists is converted exactly once and needs no cache entry; anything else keeps
        // every conversion, as before. A file too large to walk cheaply keeps every conversion
        // as well.
        if !repeated {
            let mut entries: HashMap<u64, usize> = HashMap::new();
            let mut seen: HashSet<u64> = HashSet::new();
            let mut queue: Vec<u64> = Vec::new();
            let mut work = 0usize;
            let mut within_budget = true;
            'pds: for reps in self.pd_reps.values() {
                seen.clear();
                queue.clear();
                queue.extend(self.rep_closure(reps));
                while let Some(r) = queue.pop() {
                    if !seen.insert(r) {
                        continue;
                    }
                    work += 1;
                    if work > MAX_CLOSURE_WORK {
                        within_budget = false;
                        break 'pds;
                    }
                    *entries.entry(r).or_default() += 1;
                    for item in self.reps.get(&r).map(|x| &x.0).into_iter().flatten() {
                        queue.extend(self.mapped_targets(*item));
                    }
                }
            }
            if within_budget {
                let mut once = HashSet::new();
                for (&item, &n) in &item_reps {
                    if n != 1 {
                        continue;
                    }
                    let Some(rep) = item_rep.get(&item) else { continue };
                    if entries.get(rep).copied() == Some(1) {
                        once.insert(item);
                    }
                }
                self.once = once;
            }
        }
    }

    /// First colour reachable from a style list (bounded search).
    /// SURFACE_STYLE_TRANSPARENT under a style list (0 = opaque, 1 = invisible).
    fn find_transparency(&self, styles: Option<&Param>) -> Option<f32> {
        let mut queue: Vec<u64> = styles?.as_list()?.iter().filter_map(Param::as_ref_id).collect();
        let mut seen = HashSet::new();
        while let Some(id) = queue.pop() {
            if seen.len() > 64 || !seen.insert(id) {
                continue;
            }
            let e = self.ex.get(id)?;
            if e.name() == "SURFACE_STYLE_TRANSPARENT" {
                return e.params().first().and_then(Param::as_f64).filter(|x| x.is_finite()).map(|x| x.clamp(0.0, 1.0) as f32);
            }
            if e.name() == "COLOUR_RGB" {
                continue;
            }
            for r in &e.records {
                for p in &r.params {
                    match p {
                        Param::Ref(x) => queue.push(*x),
                        Param::List(v) => queue.extend(v.iter().filter_map(Param::as_ref_id)),
                        _ => {}
                    }
                }
            }
        }
        None
    }

    fn find_colour(&self, styles: Option<&Param>) -> Option<[f32; 3]> {
        let mut queue: Vec<u64> = styles?.as_list()?.iter().filter_map(Param::as_ref_id).collect();
        let mut seen = HashSet::new();
        while let Some(id) = queue.pop() {
            if seen.len() > 64 || !seen.insert(id) {
                continue;
            }
            let e = self.ex.get(id)?;
            if e.name() == "COLOUR_RGB" {
                let p = e.params();
                let c = |i: usize| p.get(i).and_then(Param::as_f64).map(|x| x.clamp(0.0, 1.0) as f32);
                return Some([c(1)?, c(2)?, c(3)?]);
            }
            if e.name() == "DRAUGHTING_PRE_DEFINED_COLOUR" {
                return Some(match e.params().first().and_then(Param::as_str).unwrap_or("") {
                    "red" => [1.0, 0.0, 0.0],
                    "green" => [0.0, 1.0, 0.0],
                    "blue" => [0.0, 0.0, 1.0],
                    "yellow" => [1.0, 1.0, 0.0],
                    "magenta" => [1.0, 0.0, 1.0],
                    "cyan" => [0.0, 1.0, 1.0],
                    "black" => [0.0, 0.0, 0.0],
                    _ => [1.0, 1.0, 1.0],
                });
            }
            fn walk(p: &Param, out: &mut Vec<u64>, depth: usize) {
                match p {
                    Param::Ref(r) => out.push(*r),
                    Param::List(v) | Param::Typed(_, v) if depth < 4 => v.iter().for_each(|x| walk(x, out, depth + 1)),
                    _ => {}
                }
            }
            for r in &e.records {
                for p in &r.params {
                    walk(p, &mut queue, 0);
                }
            }
        }
        None
    }

    fn units_of(&mut self, rep: u64) -> Units {
        let ctx = self.reps.get(&rep).and_then(|r| r.1);
        context_units(&self.ex, ctx, &mut self.units)
    }

    fn placement_matrix(&mut self, item: u64, u: Units) -> R<mt::Matrix4> {
        let cx = Ctx { ex: &self.ex, len: u.len, ang: u.ang, tol: u.tol };
        let e = geom::entity(&cx, item)?;
        if e.name() == "CARTESIAN_TRANSFORMATION_OPERATOR_3D" {
            let p = e.params();
            let o = geom::point(&cx, p.get(4).and_then(Param::as_ref_id).ok_or("transformation without origin")?)?;
            return Ok(mt::Matrix4::from_translation(o.to_homogeneous().truncate()));
        }
        Ok(geom::frame(&cx, item)?.matrix())
    }

    /// Representations of a product (its own shape representations and those linked to them).
    fn rep_closure(&self, roots: &[u64]) -> Vec<u64> {
        let mut seen: Vec<u64> = Vec::new();
        let mut stack: Vec<u64> = roots.to_vec();
        while let Some(r) = stack.pop() {
            if seen.contains(&r) || seen.len() > 1024 {
                continue;
            }
            seen.push(r);
            if let Some(l) = self.rep_links.get(&r) {
                stack.extend(l.iter().copied());
            }
        }
        seen
    }

    /// Representations a MAPPED_ITEM walks (empty for any other item).
    fn mapped_targets(&self, item: u64) -> Vec<u64> {
        let Some(e) = self.ex.get(item).filter(|e| e.name() == "MAPPED_ITEM") else { return Vec::new() };
        let mapped = e
            .params()
            .get(1)
            .and_then(Param::as_ref_id)
            .and_then(|map| self.ex.get(map))
            .and_then(|map| map.params().get(1))
            .and_then(Param::as_ref_id);
        mapped.map(|rep| self.rep_closure(&[rep])).unwrap_or_default()
    }

    fn item_solid(&mut self, item: u64, u: Units) -> Option<Arc<SolidEntry>> {
        if let Some(r) = self.solids.get(&item) {
            return r.as_ref().ok().cloned();
        }
        let cx = Ctx { ex: &self.ex, len: u.len, ang: u.ang, tol: u.tol };
        let r = match guard("step brep", || brep::read_item(&cx, item).map_err(KernelError::Failed)) {
            Ok(out) => {
                let label = self.label(item);
                for f in &out.failed {
                    self.warn(format!("{label}: {f}"));
                }
                if !out.closed {
                    self.warn(format!("{label}: imported as an open body (the shell is not closed)"));
                }
                Ok((out.solid, out.closed, out.file_faces, out.face_origin))
            }
            Err(e) => {
                let label = self.label(item);
                let msg = e.to_string().trim_start_matches("the operation failed: ").to_string();
                self.warn(format!("{label}: not imported: {msg}"));
                Err(msg)
            }
        };
        let entry = r.map(Arc::new);
        // An item the file converts exactly once hands its only reference on and keeps no copy;
        // anything it may ask for again keeps its conversion.
        if !self.once.contains(&item) {
            self.solids.insert(item, entry.clone());
        }
        entry.ok()
    }

    fn label(&self, item: u64) -> String {
        let name = self.ex.get(item).and_then(|e| e.params().first()).and_then(Param::as_str).unwrap_or("").trim().to_string();
        if name.is_empty() { format!("solid #{item}") } else { format!("solid '{name}'") }
    }

    /// Bodies of a representation's items (following mapped items), placed by `m`.
    fn rep_bodies(&mut self, rep: u64, m: &mt::Matrix4, path: &[String], product: &str, instance: Option<&str>, out: &mut Vec<usize>, depth: usize) {
        if depth > MAX_ASSEMBLY_DEPTH {
            self.warn("mapped items nested too deeply");
            return;
        }
        self.visits += 1;
        if self.visits > MAX_VISITS {
            self.warn(format!("more than {MAX_VISITS} component instances: the rest are left out"));
            return;
        }
        let u = self.units_of(rep);
        let mut items = self.reps.get(&rep).map(|r| r.0.clone()).unwrap_or_default().into_iter().peekable();
        while let Some(item) = items.next() {
            if self.stop() {
                return;
            }
            let Some(e) = self.ex.get(item) else { continue };
            match e.name() {
                "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS" | "FACETED_BREP" | "SHELL_BASED_SURFACE_MODEL" => {
                    // Only batch consecutive solids: mapped items must retain traversal and
                    // first-use unit/cache order. Bound temporary geometry independently of file size.
                    let mut batch = vec![item];
                    let capacity = MAX_BODIES.saturating_sub(self.bodies_so_far()).min(64);
                    while batch.len() < capacity
                        && items.peek().is_some_and(|id| {
                            self.ex.get(*id).is_some_and(|e| {
                                matches!(e.name(), "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS" | "FACETED_BREP" | "SHELL_BASED_SURFACE_MODEL")
                            })
                        })
                    {
                        if let Some(next) = items.next() {
                            batch.push(next);
                        }
                    }
                    let id = self.batches.len();
                    self.batches.push(Batch { m: *m, path: path.to_vec(), product: product.to_string(), instance: instance.map(str::to_string) });
                    for item in batch {
                        self.used_items.insert(item);
                        if !self.node_items.insert(item) {
                            continue;
                        }
                        if self.bodies_so_far() >= MAX_BODIES {
                            self.warn(format!("more than {MAX_BODIES} bodies: the rest are left out"));
                            return;
                        }
                        // The node lists the planned solid; it becomes a body index once converted.
                        out.push(self.plan.len());
                        self.plan.push(Planned { item, units: u, batch: id });
                    }
                }
                "MAPPED_ITEM" => {
                    let p = e.params();
                    let (Some(map), Some(target)) = (p.get(1).and_then(Param::as_ref_id), p.get(2).and_then(Param::as_ref_id)) else { continue };
                    let Some(me) = self.ex.get(map) else { continue };
                    let (Some(origin), Some(mapped)) =
                        (me.params().first().and_then(Param::as_ref_id), me.params().get(1).and_then(Param::as_ref_id))
                    else {
                        continue;
                    };
                    let mu = self.units_of(mapped);
                    match (self.placement_matrix(target, u), self.placement_matrix(origin, mu)) {
                        (Ok(t), Ok(o)) => {
                            let mm = mat_mul(m, &mat_mul(&t, &rigid_inverse(&o)));
                            for r in self.rep_closure(&[mapped]) {
                                self.rep_bodies(r, &mm, path, product, instance, out, depth + 1);
                            }
                        }
                        (Err(e), _) | (_, Err(e)) => self.warn(format!("mapped item #{item}: {e}")),
                    }
                }
                _ => {}
            }
        }
    }

    /// Bodies made, and planned ones still to convert (the most a failure can leave out).
    fn bodies_so_far(&self) -> usize {
        self.bodies.len() + self.plan.len() - self.converted
    }

    /// Convert the next batch of planned solids into bodies; false once every one is done.
    /// `single`: one solid at a time (the caller wants short steps and has no worker threads).
    fn convert_next(&mut self, single: bool) -> bool {
        if self.converted >= self.plan.len() {
            return false;
        }
        if self.stop() {
            return false;
        }
        let first = self.converted;
        let id = self.plan[first].batch;
        let mut last = first + 1;
        while !single && last < self.plan.len() && self.plan[last].batch == id {
            last += 1;
        }
        self.converted = last;
        let ex = self.ex.clone();
        let mut sources = Vec::new();
        let mut solids = Vec::new();
        for i in first..last {
            let (item, u) = (self.plan[i].item, self.plan[i].units);
            self.made.push(None);
            let Some(entry) = self.item_solid(item, u) else { continue };
            // A one-use item is the only reference to its entry, so the solid moves
            // out; an item whose conversion is cached for a later instance needs its
            // own copy.
            let (solid, closed, file_faces, face_origin) = match Arc::try_unwrap(entry) {
                Ok(entry) => entry,
                Err(shared) => (*shared).clone(),
            };
            sources.push((i, item, closed, file_faces, face_origin));
            solids.push(parallel::Job { solid, check_mesh: !self.unmeshed_checked.contains(&item) });
        }
        let Some(batch) = self.batches.get(id) else { return true };
        let (m, path, product, instance) = (batch.m, batch.path.clone(), batch.product.clone(), batch.instance.clone());
        let prepared = parallel::bodies(&solids, &m, self.workers);
        for ((i, item, closed, file_faces, face_origin), body) in sources.into_iter().zip(prepared) {
            let Some(e) = ex.get(item) else { continue };
            match body {
                Ok((body, missing)) => {
                    let own = e.params().first().and_then(Param::as_str).unwrap_or("").trim().to_string();
                    let name = if own.is_empty() { product.clone() } else { own };
                    // Faces the mesher gives up on (or that the budget cut short) are reported.
                    if self.unmeshed_checked.insert(item) && missing > 0 {
                        self.warn(format!("solid '{name}': {missing} face(s) could not be meshed and are not shown"));
                    }
                    if let Some(slot) = self.made.get_mut(i) {
                        *slot = Some(self.bodies.len());
                    }
                    let color = self.colors.get(&item).copied();
                    // See-through bodies and faces with colours of their own.
                    let faces = face_origin
                        .iter()
                        .enumerate()
                        .filter_map(|(i, f)| {
                            let c = self.colors.get(f)?;
                            Some(crate::FacePaint { face: i, color: *c, opacity: self.opacity.get(f).copied().unwrap_or(1.0) })
                        })
                        .collect();
                    let paint = crate::Paint { opacity: self.opacity.get(&item).copied().unwrap_or(1.0), faces };
                    let body = body.with_color(color).with_paint(Some(paint));
                    let identities = face_origin
                        .iter()
                        .map(|face| Some(crate::FaceSource::Imported { item, face: *face, payload_sha256: None, producer: None }))
                        .collect();
                    let body = body.with_face_sources(Some(std::sync::Arc::new(identities)));
                    // A later instance of an item shows its first instance's mesh, placed.
                    let body = match self.shown.get(&item) {
                        Some((shared, first)) => body.with_display_of(shared.clone(), to_array(&mat_mul(&m, &rigid_inverse(first)))),
                        None => {
                            if !self.once.contains(&item) {
                                self.shown.insert(item, (Arc::new(crate::body::SharedDisplay::new(body.clone())), m));
                            }
                            body
                        }
                    };
                    self.bodies.push(ImportedBody {
                        name,
                        body,
                        color,
                        path: path.clone(),
                        instance: instance.clone(),
                        closed,
                        file_faces,
                        source_item: item,
                        face_origin,
                    });
                }
                Err(err) => {
                    let label = self.label(item);
                    self.warn(format!("{label}: not imported: {err}"));
                }
            }
        }
        true
    }

    /// Planned-solid indices (a node's `bodies` after the walk) as body indices, leaving out
    /// the ones that failed.
    fn remap(&self, planned: &[usize]) -> Vec<usize> {
        planned.iter().filter_map(|i| self.made.get(*i).copied().flatten()).collect()
    }

    fn remap_node(&self, n: &mut StepNode) {
        n.bodies = self.remap(&n.bodies);
        for c in &mut n.children {
            self.remap_node(c);
        }
    }

    /// The name of an assembly occurrence: the usage's name, else its id.
    fn occurrence_name(&self, nauo: u64) -> Option<String> {
        let p = self.ex.get(nauo)?.params();
        let name = p.get(1).and_then(Param::as_str).filter(|s| !s.trim().is_empty()).or_else(|| p.first().and_then(Param::as_str))?;
        Some(name.trim().to_string()).filter(|n| !n.is_empty())
    }

    fn product_name(&self, pd: u64) -> String {
        let name = (|| {
            let pdf = self.ex.get(self.ex.get(pd)?.params().get(2)?.as_ref_id()?)?;
            let prod = self.ex.get(pdf.params().get(2)?.as_ref_id()?)?;
            let p = prod.params();
            let n = p.get(1).and_then(Param::as_str).filter(|s| !s.trim().is_empty()).or_else(|| p.first().and_then(Param::as_str))?;
            Some(n.trim().to_string())
        })();
        name.filter(|n| !n.is_empty()).unwrap_or_else(|| "Import".into())
    }

    /// Placement of a child occurrence in its parent.
    fn occurrence_matrix(&mut self, nauo: u64, child_reps: &[u64]) -> mt::Matrix4 {
        let Some(&rel) = self.placements.get(&nauo) else { return mt::Matrix4::from_scale(1.0) };
        let ex = self.ex.clone();
        let r = (|| -> R<mt::Matrix4> {
            let e = ex.get(rel).ok_or("missing placement")?;
            let rr = e.record("REPRESENTATION_RELATIONSHIP").ok_or("placement without representations")?;
            let (r1, r2) =
                (rr.get(2).and_then(Param::as_ref_id).ok_or("bad placement")?, rr.get(3).and_then(Param::as_ref_id).ok_or("bad placement")?);
            let op = e
                .record("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
                .and_then(|x| x.first())
                .and_then(Param::as_ref_id)
                .ok_or("placement without transformation")?;
            let ope = ex.get(op).ok_or("missing transformation")?;
            let (u1, u2) = (self.units_of(r1), self.units_of(r2));
            if ope.name() == "ITEM_DEFINED_TRANSFORMATION" {
                let p = ope.params();
                let (i1, i2) = (
                    p.get(2).and_then(Param::as_ref_id).ok_or("bad transformation")?,
                    p.get(3).and_then(Param::as_ref_id).ok_or("bad transformation")?,
                );
                let m1 = self.placement_matrix(i1, u1)?;
                let m2 = self.placement_matrix(i2, u2)?;
                // rep_1 is normally the component: map its item onto the assembly's item.
                let child_is_1 = child_reps.contains(&r1) || !child_reps.contains(&r2);
                Ok(if child_is_1 { mat_mul(&m2, &rigid_inverse(&m1)) } else { mat_mul(&m1, &rigid_inverse(&m2)) })
            } else {
                self.placement_matrix(op, u2)
            }
        })();
        match r {
            Ok(m) => m,
            Err(e) => {
                self.warn(format!("component placement #{rel}: {e} (placed at the origin)"));
                mt::Matrix4::from_scale(1.0)
            }
        }
    }

    fn node(
        &mut self,
        pd: u64,
        local: mt::Matrix4,
        world: mt::Matrix4,
        path: &[String],
        instance: Option<&str>,
        depth: usize,
        stack: &mut Vec<u64>,
    ) -> StepNode {
        let name = self.product_name(pd);
        let mut node = StepNode { name: name.clone(), transform: to_array(&local), bodies: Vec::new(), children: Vec::new() };
        if self.stop() {
            return node;
        }
        if depth > MAX_ASSEMBLY_DEPTH || stack.contains(&pd) {
            self.warn("assembly nested too deeply or recursive: some components were left out");
            return node;
        }
        self.visits += 1;
        if self.visits > MAX_VISITS {
            self.warn(format!("more than {MAX_VISITS} component instances: the rest are left out"));
            return node;
        }
        stack.push(pd);
        let mut path = path.to_vec();
        path.push(name.clone());
        let own = self.pd_reps.get(&pd).cloned().unwrap_or_default();
        let reps = self.rep_closure(&own);
        let mut bodies = Vec::new();
        self.node_items.clear();
        for r in &reps {
            self.rep_bodies(*r, &world, &path, &name, instance, &mut bodies, 0);
        }
        node.bodies = bodies;
        for nauo in self.children.get(&pd).cloned().unwrap_or_default() {
            let Some(child) = self.ex.get(nauo).and_then(|e| e.params().get(4)).and_then(Param::as_ref_id) else { continue };
            let child_own = self.pd_reps.get(&child).cloned().unwrap_or_default();
            let child_reps = self.rep_closure(&child_own);
            let m = self.occurrence_matrix(nauo, &child_reps);
            let w = mat_mul(&world, &m);
            let instance = self.occurrence_name(nauo);
            let c = self.node(child, m, w, &path, instance.as_deref(), depth + 1, stack);
            node.children.push(c);
        }
        stack.pop();
        node
    }
}

/// Check a STEP file's structure: it parses, no entity id is defined twice, every reference
/// names a defined entity, nothing but root entities (relationships, definitions,
/// presentation) is left unreferenced, and ids are dense (`#1`…`#n`). Returns the entity count.
/// Faces whose boundary runs the wrong way round their normal, as a strict reader (Fusion)
/// sees them: each face's normal is its surface's own parametric normal (∂S/∂u × ∂S/∂v, ISO
/// 10303-42), reversed when `same_sense` is false, and the outer loop must run counter-clockwise
/// seen from that side. Checked on faces whose loop spans a clear direction (patches, not bands
/// around a closed surface). Returns one message per bad face.
pub fn step_orientation_errors(text: &str) -> std::result::Result<Vec<String>, String> {
    use mt::{BoundedCurve, InnerSpace, ParametricCurve, ParametricSurface3D, SearchNearestParameter};
    let imp = step_import(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for b in &imp.bodies {
        let r = guard("orientation check", || {
            let mut bad = Vec::new();
            for (fi, f) in b.body.solid.face_iter().enumerate() {
                let bounds = f.boundaries();
                // Patches only: a face with several loops (a band between two rings, a face with
                // holes) has no single loop direction to compare.
                let [w] = bounds.as_slice() else { continue };
                // Only revolutions (cylinders, cones, spheres, tori: closed surfaces, where a loop
                // read the wrong way round silently gives the rest of the surface). On free-form
                // strips the loop's normals vary too much for this test; the round trips' volume
                // checks cover those.
                if !matches!(f.surface(), mt::Surface::RevolutedCurve(_)) {
                    continue;
                }
                // A loop through a vertex twice (a figure eight round a pole) has no single direction.
                let fronts: std::collections::HashSet<_> = w.edge_iter().map(|e| e.front().id()).collect();
                if fronts.len() != w.len() {
                    continue;
                }
                let pts: Vec<mt::Point3> = w
                    .edge_iter()
                    .flat_map(|e| {
                        let c = e.oriented_curve();
                        let (t0, t1) = c.range_tuple();
                        (0..16).map(move |i| c.subs(t0 + (t1 - t0) * i as f64 / 16.0)).collect::<Vec<_>>()
                    })
                    .collect();
                let n = pts.len();
                if n < 3 {
                    continue;
                }
                let mut newell = mt::Vector3::new(0.0, 0.0, 0.0);
                let mut c = mt::Vector3::new(0.0, 0.0, 0.0);
                let mut span: f64 = 0.0;
                for i in 0..n {
                    let (Some(a), Some(q)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
                    newell += (a.to_vec()).cross(q.to_vec());
                    c += a.to_vec();
                    span = span.max((a - pts[0]).magnitude());
                }
                let c = mt::Point3::from_vec(c / n as f64);
                // Bands round a closed surface have no clear loop direction.
                if !(newell.magnitude() > 0.05 * span * span) {
                    continue;
                }
                // The face normal averaged along the loop (each loop point is on the surface;
                // the loop's centroid can be nearer the wrong side of a curved face).
                let s = f.oriented_surface();
                let mut normal = mt::Vector3::new(0.0, 0.0, 0.0);
                let mut count = 0usize;
                for p in &pts {
                    if let Some(uv) = s.search_nearest_parameter(*p, None, 100) {
                        let n = s.normal(uv.0, uv.1);
                        if n.x.is_finite() && n.y.is_finite() && n.z.is_finite() {
                            normal += n;
                            count += 1;
                        }
                    }
                }
                let _ = c;
                // Only patches clearly to one side of their loop are judged: a loop on a closed
                // surface (a sphere's cube-face square) bounds either part.
                if count == 0 || normal.magnitude() < 0.7 * count as f64 {
                    continue;
                }
                if normal.dot(newell) < 0.0 {
                    bad.push(format!("{}: face {fi} runs clockwise around its normal", b.name));
                }
            }
            Ok(bad)
        })
        .map_err(|e| e.to_string())?;
        out.extend(r);
    }
    Ok(out)
}

pub fn step_validate(text: &str) -> std::result::Result<usize, String> {
    let ex = p21::parse(text)?;
    if let Some(d) = ex.duplicates.first() {
        return Err(format!("{} duplicate entity ids (first #{d})", ex.duplicates.len()));
    }
    fn refs(p: &Param, out: &mut Vec<u64>) {
        match p {
            Param::Ref(r) => out.push(*r),
            Param::List(v) | Param::Typed(_, v) => v.iter().for_each(|x| refs(x, out)),
            _ => {}
        }
    }
    let mut ids: Vec<&u64> = ex.entities.keys().collect();
    ids.sort();
    for id in &ids {
        let mut out = Vec::new();
        if let Some(e) = ex.get(**id) {
            e.records.iter().flat_map(|r| &r.params).for_each(|p| refs(p, &mut out));
        }
        if let Some(r) = out.iter().find(|r| ex.get(**r).is_none()) {
            return Err(format!("#{id} refers to undefined #{r}"));
        }
    }
    // Unreferenced entities must be roots of the exchange (relationships, definitions,
    // presentation); anything else is dead data.
    let mut used = std::collections::HashSet::new();
    for e in ex.entities.values() {
        let mut out = Vec::new();
        e.records.iter().flat_map(|r| &r.params).for_each(|p| refs(p, &mut out));
        used.extend(out);
    }
    const ROOTS: &[&str] = &[
        "SHAPE_DEFINITION_REPRESENTATION",
        "APPLICATION_PROTOCOL_DEFINITION",
        "PRODUCT_RELATED_PRODUCT_CATEGORY",
        "PRODUCT_CATEGORY_RELATIONSHIP",
        "SHAPE_REPRESENTATION_RELATIONSHIP",
        "REPRESENTATION_RELATIONSHIP",
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
        "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION",
        "DRAUGHTING_MODEL",
        "PROPERTY_DEFINITION_REPRESENTATION",
        "PRESENTATION_LAYER_ASSIGNMENT",
    ];
    for id in &ids {
        if used.contains(*id) {
            continue;
        }
        let Some(e) = ex.get(**id) else { continue };
        if !e.records.iter().any(|r| ROOTS.contains(&r.name.as_str()) || r.name.starts_with("APPLIED_")) {
            return Err(format!("#{id} ({}) is not referenced by anything", e.name()));
        }
    }
    let n = ex.entities.len();
    if ids.last().is_some_and(|m| **m != n as u64) || ids.first().is_some_and(|m| **m != 1) {
        return Err(format!("entity ids are not dense: {n} entities, #{}…#{}", ids.first().map_or(0, |x| **x), ids.last().map_or(0, |x| **x)));
    }
    Ok(n)
}

/// A recent import: held weakly, so it lives only as long as someone uses it (the command that
/// read the file, until the timeline has evaluated it), and strongly only when it is small.
struct Recent {
    key: (u64, usize),
    import: std::sync::Weak<StepImport>,
    /// Keeps a small import alive (never read).
    _small: Option<Arc<StepImport>>,
}

/// Recent imports, so a file read for a command and then evaluated in the timeline is only
/// read once.
static RECENT: std::sync::Mutex<Vec<Recent>> = std::sync::Mutex::new(Vec::new());

/// Files up to this size stay cached after their last user is gone (a few cheap re-reads
/// saved); a larger import is freed with the design that holds it.
const KEEP_SMALL_BYTES: usize = 1 << 20;

/// [`step_import`] through a small cache of recent results (keyed by the text). Keep the result
/// while the same text is still to be read elsewhere (in the timeline): that read reuses it.
pub fn step_import_shared(text: &str) -> Result<std::sync::Arc<StepImport>> {
    let key = text_key(text);
    if let Ok(cache) = RECENT.lock()
        && let Some(imp) = cache.iter().filter(|r| r.key == key).find_map(|r| r.import.upgrade())
    {
        return Ok(imp);
    }
    let imp = std::sync::Arc::new(step_import(text)?);
    remember(key, &imp, text.len() <= KEEP_SMALL_BYTES);
    Ok(imp)
}

/// The cache key of a STEP text: its hash and length.
fn text_key(text: &str) -> (u64, usize) {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    (h.finish(), text.len())
}

/// Keep `imp` in the recent imports (strongly only when `small`).
fn remember(key: (u64, usize), imp: &Arc<StepImport>, small: bool) {
    if let Ok(mut cache) = RECENT.lock() {
        cache.retain(|r| r.key != key && r.import.strong_count() > 0);
        cache.push(Recent { key, import: Arc::downgrade(imp), _small: small.then(|| imp.clone()) });
        while cache.len() > 4 {
            cache.remove(0);
        }
    }
}

/// Imports the cache still holds alive, and how many STEP bytes they came from (tests and
/// diagnostics: a closed design must not leave its import behind).
pub fn step_cache_alive() -> (usize, usize) {
    RECENT.lock().map(|c| c.iter().filter(|r| r.import.strong_count() > 0).fold((0, 0), |(n, b), r| (n + 1, b + r.key.1))).unwrap_or((0, 0))
}

/// Read a STEP file: its solids (as bodies, placed by the assembly structure) and the tree.
pub fn step_import(text: &str) -> Result<StepImport> {
    step_import_with_workers(text, parallel::workers())
}

fn step_import_with_workers(text: &str, workers: usize) -> Result<StepImport> {
    let mut reading = StepReading::with_workers(text, workers)?;
    while reading.step() {}
    reading.finish()
}

/// A STEP file being read a piece at a time, for a caller that can't wait for the whole read
/// in one go (the web app, which has one thread and must keep drawing): [`StepReading::new`]
/// parses the file and walks its structure, each [`StepReading::step`] converts the next few
/// solids into bodies, and [`StepReading::finish_shared`] gives the import, cached as
/// [`step_import_shared`] would, so a command reading the same text next finds it read.
pub struct StepReading {
    rd: Reader,
    tree: Vec<StepNode>,
    key: (u64, usize),
    small: bool,
    /// One solid per step (no worker threads to share a batch between).
    single: bool,
}

impl StepReading {
    /// Parse `text` and walk it; the solids are converted by [`Self::step`].
    pub fn new(text: &str) -> Result<StepReading> {
        Self::with_workers(text, parallel::workers())
    }

    fn with_workers(text: &str, workers: usize) -> Result<StepReading> {
        let key = text_key(text);
        let watch = crate::watch::current();
        let cancelled = || watch.as_ref().is_some_and(|w| w.cancelled());
        let ex = match p21::parse(text) {
            Ok(ex) => Arc::new(ex),
            Err(_) if cancelled() => return Err(crate::watch::cancelled_error("STEP import")),
            Err(e) => return Err(KernelError::Invalid(e)),
        };
        let mut rd = Reader::new(ex.clone(), workers);
        rd.solid_items = ex
            .entities
            .values()
            .filter(|e| matches!(e.name(), "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS" | "FACETED_BREP" | "SHELL_BASED_SURFACE_MODEL"))
            .count();
        rd.watch = watch.clone();
        rd.index();
        // Roots: product definitions that are nobody's component.
        let child_pds: HashSet<u64> =
            rd.children.values().flatten().filter_map(|n| ex.get(*n).and_then(|e| e.params().get(4)).and_then(Param::as_ref_id)).collect();
        let mut roots: Vec<u64> =
            ex.entities.iter().filter(|(id, e)| e.name() == "PRODUCT_DEFINITION" && !child_pds.contains(id)).map(|(id, _)| *id).collect();
        roots.sort();
        let identity = mt::Matrix4::from_scale(1.0);
        let mut tree = Vec::new();
        for pd in roots {
            tree.push(rd.node(pd, identity, identity, &[], None, 0, &mut Vec::new()));
        }
        // Solids outside any product structure (bare geometry files).
        let mut loose: Vec<u64> = rd
            .reps
            .iter()
            .flat_map(|(r, (items, _))| items.iter().map(move |i| (*r, *i)))
            .filter(|(_, i)| !rd.used_items.contains(i))
            .filter(|(_, i)| {
                ex.get(*i)
                    .is_some_and(|e| matches!(e.name(), "MANIFOLD_SOLID_BREP" | "BREP_WITH_VOIDS" | "FACETED_BREP" | "SHELL_BASED_SURFACE_MODEL"))
            })
            .map(|(r, _)| r)
            .collect();
        loose.sort();
        loose.dedup();
        if !loose.is_empty() {
            let mut bodies = Vec::new();
            rd.node_items = rd.used_items.clone();
            rd.visits = 0;
            for r in loose {
                rd.rep_bodies(r, &identity, &[], "Import", None, &mut bodies, 0);
            }
            tree.push(StepNode { name: "Import".into(), transform: to_array(&identity), bodies, children: Vec::new() });
        }
        let single = workers <= 1;
        Ok(StepReading { rd, tree, key, small: text.len() <= KEEP_SMALL_BYTES, single })
    }

    /// Convert the next solid (or batch of solids); false once all are converted (or the
    /// watch cancelled the import).
    pub fn step(&mut self) -> bool {
        self.rd.convert_next(self.single)
    }

    /// Solids converted, of the solids to convert.
    pub fn progress(&self) -> (usize, usize) {
        (self.rd.converted, self.rd.plan.len())
    }

    /// The import, once [`Self::step`] has converted every solid.
    pub fn finish(mut self) -> Result<StepImport> {
        while self.step() {}
        let rd = &mut self.rd;
        // Cancelled: what was read so far is dropped here, not cached.
        if rd.cancelled || rd.watch.as_ref().is_some_and(|w| w.cancelled()) {
            return Err(crate::watch::cancelled_error("STEP import"));
        }
        let mut tree = std::mem::take(&mut self.tree);
        for n in &mut tree {
            rd.remap_node(n);
        }
        tree.retain(|n| !n.bodies.is_empty() || !n.children.is_empty());
        let ex = rd.ex.clone();
        if let Some(first) = ex.duplicates.first() {
            rd.warn(format!("{} entity ids are defined twice (first #{first}); the first definitions were used", ex.duplicates.len()));
        }
        let warnings: Vec<String> = rd.warnings.iter().map(|(m, n)| if *n > 1 { format!("{m} (×{n})") } else { m.clone() }).collect();
        if rd.bodies.is_empty() {
            let why = warnings.first().cloned().unwrap_or_else(|| "the file has no solids".into());
            return Err(KernelError::Failed(format!("STEP import: no bodies ({why})")));
        }
        let bodies = std::mem::take(&mut rd.bodies);
        Ok(StepImport { bodies, tree, warnings, schema: ex.schemas.join(", "), system: ex.originating_system.clone() })
    }

    /// [`Self::finish`], and the import is cached for the next read of the same text.
    pub fn finish_shared(self) -> Result<Arc<StepImport>> {
        let (key, small) = (self.key, self.small);
        let imp = Arc::new(self.finish()?);
        remember(key, &imp, small);
        Ok(imp)
    }
}

#[cfg(test)]
mod conic_tests;
#[cfg(test)]
mod tests;
