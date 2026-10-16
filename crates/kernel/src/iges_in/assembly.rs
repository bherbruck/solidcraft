//! IGES groups (402) and subfigures (308/408), from the published IGES specification.
//! Definitions become STEP products; instances refer to them without copying geometry.
//! Reference: https://nvlpubs.nist.gov/nistpubs/Legacy/IR/nbsir88-3813.pdf
use super::*;
use std::collections::HashSet;

const MAX_GROUPS: usize = 10_000;
const MAX_INSTANCES: usize = 100_000;
const MAX_REFERENCES: usize = 4 * MAX_ENTITIES;

struct Group {
    seq: usize,
    members: Vec<usize>,
}

struct Instance {
    definition: usize,
    transform: Xf,
}

pub(super) struct Assembly {
    groups: Vec<Group>,
    indices: HashMap<usize, usize>,
    instances: HashMap<usize, Instance>,
    referenced: HashSet<usize>,
    surface_references: HashSet<usize>,
}

impl Assembly {
    pub(super) fn read(t: &Tr, seqs: &[usize]) -> R<Self> {
        let mut out = Self {
            groups: Vec::new(),
            indices: HashMap::new(),
            instances: HashMap::new(),
            referenced: HashSet::new(),
            surface_references: HashSet::new(),
        };
        let mut references = 0usize;
        let watch = crate::watch::current();
        for &seq in seqs {
            if watch.as_ref().is_some_and(|w| w.cancelled()) {
                return Err("IGES import cancelled".into());
            }
            let de = t.de(seq as i64)?;
            if de.typ == 308 || (de.typ == 402 && matches!(de.form, 1 | 7 | 14 | 15)) {
                if out.groups.len() >= MAX_GROUPS {
                    return Err("too many IGES assembly definitions".into());
                }
                let field = if de.typ == 308 { 3 } else { 1 };
                let count = usize::try_from(t.i(seq as i64, field)?).map_err(|_| "negative IGES group size")?;
                references = references.saturating_add(count);
                if count > MAX_LIST || references > MAX_REFERENCES {
                    return Err("IGES assembly references exceed the work limit".into());
                }
                let p = t.p(seq as i64)?;
                let entries = p.get(field + 1..field + 1 + count).ok_or("truncated IGES group")?;
                let mut members = Vec::with_capacity(count);
                for entry in entries {
                    let child = int(entry).and_then(|x| usize::try_from(x).ok()).filter(|x| *x > 0).ok_or("invalid IGES group reference")?;
                    t.de(child as i64)?;
                    members.push(child);
                    out.referenced.insert(child);
                }
                out.indices.insert(seq, out.groups.len());
                out.groups.push(Group { seq, members });
            } else if de.typ == 408 {
                if out.instances.len() >= MAX_INSTANCES {
                    return Err("too many IGES assembly instances".into());
                }
                let definition = usize::try_from(t.i(seq as i64, 1)?).map_err(|_| "invalid IGES subfigure reference")?;
                if t.de(definition as i64)?.typ != 308 {
                    return Err("IGES instance does not refer to a subfigure definition".into());
                }
                let p = t.p(seq as i64)?;
                let scale = match p.get(5).map(|s| s.trim()) {
                    None | Some("") => 1.0,
                    Some(s) => num(s).filter(|x| *x != 0.0).ok_or("invalid IGES instance scale")?,
                };
                let [x, y, z] = t.pt(seq as i64, 2)?;
                let local = Xf([[scale, 0.0, 0.0, x], [0.0, scale, 0.0, y], [0.0, 0.0, scale, z]]);
                let transform = t.own_xf(seq as i64, 0)?.then(&local).then(&t.own_xf(definition as i64, 0)?);
                if !transform.0.iter().flatten().all(|v| v.is_finite()) {
                    return Err("non-finite IGES instance placement".into());
                }
                out.instances.insert(seq, Instance { definition, transform });
                out.referenced.insert(definition);
            }
        }
        // Index geometry references once instead of rescanning all parameters for each patch.
        for (&seq, params) in &t.f.params {
            if watch.as_ref().is_some_and(|w| w.cancelled()) {
                return Err("IGES import cancelled".into());
            }
            if t.f.des.get(&seq).is_some_and(|d| matches!(d.typ, 308 | 402 | 406 | 408)) {
                continue;
            }
            for p in params.iter().skip(1) {
                let p = p.trim();
                if p.starts_with('0') {
                    continue;
                }
                if let Ok(id) = p.parse::<usize>()
                    && id != seq
                    && t.f.des.get(&id).is_some_and(|d| d.typ == 128)
                {
                    out.surface_references.insert(id);
                }
            }
        }
        let mut depths = HashMap::with_capacity(out.groups.len());
        let mut placements = out.groups.iter().filter(|g| !out.referenced.contains(&g.seq)).count()
            + out.instances.keys().filter(|s| !out.referenced.contains(*s)).count();
        for group in &out.groups {
            placements += group.members.iter().filter(|s| out.indices.contains_key(*s) || out.instances.contains_key(*s)).count();
            if placements > MAX_INSTANCES {
                return Err("too many IGES assembly placements".into());
            }
            out.depth(group.seq, 0, &mut depths)?;
        }
        Ok(out)
    }

    fn depth(&self, seq: usize, depth: usize, seen: &mut HashMap<usize, Option<usize>>) -> R<usize> {
        if depth >= MAX_DEPTH {
            return Err("IGES assembly nesting is too deep".into());
        }
        if let Some(height) = seen.get(&seq) {
            return height.ok_or_else(|| "cyclic IGES assembly".into());
        }
        seen.insert(seq, None);
        let group = self.indices.get(&seq).and_then(|i| self.groups.get(*i)).ok_or("missing IGES assembly definition")?;
        let mut height = 1;
        for child in &group.members {
            let definition = self.instances.get(child).map(|i| i.definition).unwrap_or(*child);
            if self.indices.contains_key(&definition) {
                height = height.max(1 + self.depth(definition, depth + 1, seen)?);
            }
        }
        if height > MAX_DEPTH {
            return Err("IGES assembly nesting is too deep".into());
        }
        seen.insert(seq, Some(height));
        Ok(height)
    }

    pub(super) fn write(&self, t: &mut Tr, seqs: &[usize], solids: &HashMap<usize, u64>, styles: &mut Vec<(u64, [f32; 3])>, ctx: u64) -> R<()> {
        let app = t.o.add("APPLICATION_CONTEXT('mechanical design')");
        let pc = t.o.add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
        let dc = t.o.add(format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
        let origin = t.o.placement([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let mut products = HashMap::with_capacity(self.groups.len() + 1);
        let mut items = Vec::new();
        let mut list = String::new();
        let mut fallback = String::new();
        let mut made = 0usize;
        let f = t.f;
        let mut faces = Vec::new();
        let mut sewer = Sewer { tol: t.tol, verts: Vec::new(), grid: HashMap::new(), edges: Vec::new(), by_ends: HashMap::new() };
        let watch = crate::watch::current();
        // A synthetic root holds loose geometry and top-level groups/instances.
        for group in std::iter::once(None).chain(self.groups.iter().map(Some)) {
            if watch.as_ref().is_some_and(|w| w.cancelled()) {
                return Err("IGES import cancelled".into());
            }
            let seq = group.map(|g| g.seq).unwrap_or(0);
            let name = if seq == 0 {
                "Import"
            } else if let Some(name) = entity_name(f, seq) {
                name
            } else {
                fallback.clear();
                let _ = write!(fallback, "Group {seq}");
                fallback.as_str()
            };
            items.clear();
            let members = group.map(|g| g.members.as_slice()).unwrap_or(seqs);
            items.extend(members.iter().filter(|s| seq != 0 || !self.referenced.contains(*s)).filter_map(|s| solids.get(s).copied()));
            let surfaces = members
                .iter()
                .copied()
                .filter(|s| seq != 0 || !self.referenced.contains(s))
                .filter(|s| f.des.get(s).is_some_and(|d| matches!(d.typ, 143 | 144) || (d.typ == 128 && !self.surface_references.contains(s))));
            if let Some(item) = sew_surfaces(t, surfaces, if seq == 0 { "Body1" } else { name }, styles, &mut faces, &mut sewer)? {
                items.push(item);
            }
            made += items.len();
            list.clear();
            let _ = write!(list, "#{origin}");
            for item in &items {
                let _ = write!(list, ",#{item}");
            }
            let rep = t.o.add(format!("ADVANCED_BREP_SHAPE_REPRESENTATION('',({list}),#{ctx})"));
            let name = name.replace('\'', "''");
            let product = t.o.add(format!("PRODUCT('{name}','{name}','',(#{pc}))"));
            let formation = t.o.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
            let pd = t.o.add(format!("PRODUCT_DEFINITION('design','',#{formation},#{dc})"));
            let shape = t.o.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
            t.o.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{rep})"));
            products.insert(seq, (pd, rep));
        }
        if made == 0 {
            let why = t.warnings.first().map(String::as_str).unwrap_or("no solids or trimmed surfaces");
            return Err(format!("IGES import: nothing to read ({why})"));
        }
        for group in std::iter::once(None).chain(self.groups.iter().map(Some)) {
            let parent = group.map(|g| g.seq).unwrap_or(0);
            let members = group.map(|g| g.members.as_slice()).unwrap_or(seqs);
            for &child in members {
                if watch.as_ref().is_some_and(|w| w.cancelled()) {
                    return Err("IGES import cancelled".into());
                }
                if parent == 0 && self.referenced.contains(&child) {
                    continue;
                }
                let (definition, transform) = match self.instances.get(&child) {
                    Some(i) => (i.definition, i.transform),
                    None if self.indices.contains_key(&child) => (child, t.own_xf(child as i64, 0)?),
                    _ => continue,
                };
                let (parent_pd, parent_rep) = products.get(&parent).copied().ok_or("missing IGES assembly parent")?;
                let (child_pd, child_rep) = products.get(&definition).copied().ok_or("missing IGES assembly child")?;
                let label = entity_name(f, child).unwrap_or("").replace('\'', "''");
                let nauo = t.o.add(format!("NEXT_ASSEMBLY_USAGE_OCCURRENCE('{child}','{label}','',#{parent_pd},#{child_pd},$)"));
                let shape = t.o.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{nauo})"));
                let op = transformation(&mut t.o, transform)?;
                let rr = t.o.add(format!("(REPRESENTATION_RELATIONSHIP('','',#{child_rep},#{parent_rep})REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{op})SHAPE_REPRESENTATION_RELATIONSHIP())"));
                t.o.add(format!("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr},#{shape})"));
            }
        }
        Ok(())
    }
}

fn transformation(out: &mut Out, xf: Xf) -> R<u64> {
    if !xf.0.iter().flatten().all(|v| v.is_finite()) {
        return Err("non-finite IGES assembly placement".into());
    }
    let [x, y, z] = [xf.dir([1.0, 0.0, 0.0]), xf.dir([0.0, 1.0, 0.0]), xf.dir([0.0, 0.0, 1.0])];
    let [sx, sy, sz] = [norm(x), norm(y), norm(z)];
    let scale = xf.scale();
    if [sx, sy, sz, scale].iter().any(|s| !s.is_finite() || *s <= 1e-300) {
        return Err("singular IGES assembly placement".into());
    }
    let (x, y, z, o) = (out.dir(mul(x, 1.0 / sx)), out.dir(mul(y, 1.0 / sy)), out.dir(mul(z, 1.0 / sz)), out.point(xf.point([0.0; 3])));
    Ok(out.add(format!("CARTESIAN_TRANSFORMATION_OPERATOR_3D_NON_UNIFORM('','',#{x},#{y},#{o},{},#{z},{},{})", r(sx), r(sy), r(sz))))
}

/// Name properties follow the associativity pointers; member pointers are not properties.
pub(super) fn entity_name(f: &Iges, seq: usize) -> Option<&str> {
    let de = f.des.get(&seq)?;
    let p = f.params.get(&seq)?;
    let count = |i: usize| p.get(i).and_then(|s| int(s)).and_then(|x| usize::try_from(x).ok()).filter(|n| *n <= MAX_LIST);
    let end = match de.typ {
        186 => count(3)?.checked_mul(2)?.checked_add(4)?,
        308 => count(3)?.checked_add(4)?,
        402 => count(1)?.checked_add(2)?,
        408 => 6,
        _ => p.len(),
    };
    let property = (|| {
        let after = end.checked_add(count(end)?)?.checked_add(1)?;
        let n = count(after)?;
        p.get(after + 1..after + 1 + n)?.iter().find_map(|s| {
            let id = int(s).and_then(|x| usize::try_from(x).ok())?;
            let d = f.des.get(&id)?;
            if d.typ != 406 || d.form != 15 {
                return None;
            }
            f.params.get(&id)?.get(2).and_then(|s| name_text(s))
        })
    })();
    property.or_else(|| if de.typ == 308 { p.get(2).and_then(|s| name_text(s)) } else { None }).or_else(|| name_text(&de.label))
}

/// Match the component-name limit before copying or repeating a shared name property.
fn name_text(s: &str) -> Option<&str> {
    let end = s.char_indices().nth(128).map(|(i, _)| i).unwrap_or(s.len());
    let name = s.get(..end)?.trim();
    (!name.is_empty()).then_some(name)
}
