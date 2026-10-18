//! Typed remapping of assembly relationships for an independent component-tree copy.

use std::collections::{BTreeMap, BTreeSet};

use solidcraft_doc::Document;
use solidcraft_doc::joints::cam::CamProfile;
use solidcraft_doc::joints::{Assembly, Coupling, JointOrigin, Size};

use crate::{EngineError, Result};

const MAX_RECORDS: usize = 10_000;
const MAX_REFERENCES: usize = 100_000;
const MAX_TEXT: usize = 1_048_576;
const MAX_NAME: usize = 8192;

fn invalid(message: &str) -> EngineError {
    EngineError::Other(format!("Paste New assembly: {message}"))
}

fn add_references(count: &mut usize, more: usize) -> Result<()> {
    *count = count.saturating_add(more);
    if *count > MAX_REFERENCES {
        return Err(invalid("assembly references exceed the processing budget"));
    }
    Ok(())
}

fn finite_origin(origin: &JointOrigin) -> Result<()> {
    if !origin.snap.is_finite() {
        return Err(invalid("joint origin contains non-finite geometry"));
    }
    Ok(())
}

fn budget(assembly: &Assembly) -> Result<()> {
    let counts = [
        assembly.origins.len(),
        assembly.joints.len(),
        assembly.links.len(),
        assembly.contact_sets.len(),
        assembly.studies.len(),
        assembly.exploded.len(),
    ];
    if counts.iter().any(|n| *n > MAX_RECORDS) {
        return Err(invalid("too many assembly records"));
    }
    let mut references = counts.iter().fold(0usize, |n, c| n.saturating_add(*c));
    let mut bytes = 0usize;
    let mut text = |s: &str| -> Result<()> {
        bytes = bytes.saturating_add(s.len());
        if s.len() > MAX_NAME || bytes > MAX_TEXT {
            return Err(invalid("assembly names exceed the processing budget"));
        }
        Ok(())
    };
    for origin in &assembly.origins {
        finite_origin(&origin.origin)?;
        text(&origin.name)?;
        for face in origin.origin.snap.face_names() {
            text(face)?;
        }
    }
    for joint in &assembly.joints {
        text(&joint.name)?;
        add_references(&mut references, joint.values.len().saturating_add(joint.limits.len()))?;
        if !joint.values.iter().all(|v| v.is_finite())
            || !joint.limits.iter().flatten().all(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo <= hi)
            || !joint.offset.is_finite()
            || !joint.angle.is_finite()
        {
            return Err(invalid("joint contains invalid motion values"));
        }
        for origin in [&joint.a, &joint.b] {
            finite_origin(origin)?;
            for face in origin.snap.face_names() {
                text(face)?;
            }
        }
    }
    for contact in &assembly.contact_sets {
        text(&contact.name)?;
        add_references(&mut references, contact.occurrences.len())?;
    }
    for study in &assembly.studies {
        text(&study.name)?;
        add_references(&mut references, study.keys.len())?;
        for key in &study.keys {
            add_references(&mut references, key.points.len())?;
            if !key.points.iter().all(|(step, value)| step.is_finite() && value.is_finite()) {
                return Err(invalid("motion study contains non-finite keys"));
            }
        }
    }
    for view in &assembly.exploded {
        text(&view.name)?;
        add_references(&mut references, view.moves.len())?;
        if !view.moves.iter().all(|(_, p)| p.is_finite()) {
            return Err(invalid("exploded view contains non-finite moves"));
        }
    }
    if !assembly.links.iter().all(|l| l.ratio.is_finite() && l.offset.is_finite() && l.lift.iter().all(|v| v.is_finite())) {
        return Err(invalid("motion link contains non-finite values"));
    }
    for cam in assembly.links.iter().filter_map(|l| match &l.coupling {
        Some(Coupling::Cam(c)) => Some(c),
        _ => None,
    }) {
        add_references(&mut references, cam.tip.iter().count())?;
        if let Some(tip) = &cam.tip {
            finite_origin(tip)?;
        }
        match &cam.profile {
            CamProfile::Sketch { curves, .. } => {
                add_references(&mut references, curves.len())?;
                curves.iter().try_for_each(|c| text(c))?;
            }
            CamProfile::Edges { picks, names, .. } => {
                add_references(&mut references, picks.len() + names.len())?;
                if !picks.iter().all(|p| p.is_finite()) {
                    return Err(invalid("motion link contains non-finite values"));
                }
                names.iter().try_for_each(|n| text(n))?;
            }
        }
    }
    for size in assembly.links.iter().filter_map(|l| l.coupling.as_ref()).flat_map(Coupling::sizes) {
        match size {
            Size::Length(v) if !v.is_finite() => return Err(invalid("motion link contains non-finite values")),
            Size::Expr(e) => text(e)?,
            Size::Circle(o) => finite_origin(o)?,
            Size::Length(_) => {}
        }
    }
    if references > MAX_REFERENCES {
        return Err(invalid("assembly references exceed the processing budget"));
    }
    Ok(())
}

fn unique_name(base: &str, used: &mut BTreeSet<String>) -> Result<String> {
    if !used.contains(base) {
        used.insert(base.to_string());
        return Ok(base.to_string());
    }
    for index in 1..=MAX_RECORDS {
        let name = if index == 1 { format!("{base} (copy)") } else { format!("{base} (copy {index})") };
        if name.len() > MAX_NAME {
            return Err(invalid("copied assembly name is too long"));
        }
        if used.insert(name.clone()) {
            return Ok(name);
        }
    }
    Err(invalid("could not allocate a unique assembly name"))
}

fn next_id(id: &mut u64) -> Result<u64> {
    *id = id.checked_add(1).ok_or_else(|| invalid("assembly identifier space is exhausted"))?;
    Ok(*id)
}

fn unique_ids(ids: impl Iterator<Item = u64>) -> Result<u64> {
    let mut seen = BTreeSet::new();
    let mut largest = 0;
    for id in ids {
        if id == 0 || !seen.insert(id) {
            return Err(invalid("assembly identifiers must be nonzero and unique"));
        }
        largest = largest.max(id);
    }
    Ok(largest)
}

fn validate_map<K: Ord, V: Ord>(map: &BTreeMap<K, V>) -> Result<()> {
    if map.len() > MAX_RECORDS || map.values().collect::<BTreeSet<_>>().len() != map.len() {
        return Err(invalid("copy maps exceed their budget or contain duplicate destinations"));
    }
    Ok(())
}

/// Remap certified naming tokens, rather than replacing digit substrings (F1 must not
/// alter F10). Body-based legacy names are remapped only through an explicit identity map.
fn face_name(name: &str, features: &BTreeMap<u64, u64>, bodies: &BTreeMap<String, String>) -> Option<String> {
    let body = name.match_indices(':').filter_map(|(end, _)| bodies.get_key_value(name.get(..end)?)).next_back();
    let mut out = String::new();
    let mut last = 0usize;
    let mut mapped = false;
    if let Some((old, new)) = body {
        out.push_str(new);
        last = old.len();
        mapped = true;
    }
    let bytes = name.as_bytes();
    let mut i = last;
    while i < bytes.len() {
        let boundary = i == 0
            || bytes.get(i.wrapping_sub(1)).is_some_and(|c| matches!(*c, b'@' | b'|'))
            || name.get(..i).is_some_and(|prefix| prefix.ends_with(":blend:") || prefix.ends_with(":inner:"));
        if bytes.get(i) == Some(&b'F') && boundary {
            let mut end = i + 1;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if end > i + 1 && bytes.get(end).is_some_and(|c| matches!(*c, b':' | b'.')) {
                let id: u64 = name.get(i + 1..end)?.parse().ok()?;
                let replacement = features.get(&id)?;
                out.push_str(name.get(last..i)?);
                out.push_str(&format!("F{replacement}"));
                last = end;
                mapped |= i == 0;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    if !mapped {
        return None;
    }
    out.push_str(name.get(last..)?);
    Some(out)
}

fn origin(
    origin: &JointOrigin,
    occurrences: &BTreeMap<u64, u64>,
    features: &BTreeMap<u64, u64>,
    bodies: &BTreeMap<String, String>,
) -> Option<JointOrigin> {
    let mut out = origin.clone();
    out.occurrence = *occurrences.get(&origin.occurrence)?;
    for face in out.snap.face_names_mut() {
        *face = face_name(face, features, bodies)?;
    }
    Some(out)
}

fn report(messages: &mut Vec<String>, kind: &str, name: &str) {
    messages.push(format!("Skipped {kind} `{name}`: references outside the copied component tree or geometry without a copied identity."));
}

/// Called with a scratch document after geometry cloning. Only wholly internal records
/// are copied; cross-boundary relationships are reported and omitted without truncation.
/// The assembly is swapped in only after every map, allocation and aggregate check passes.
pub(super) fn clone_assembly(
    doc: &mut Document,
    source: &Document,
    occurrences: &BTreeMap<u64, u64>,
    features: &BTreeMap<u64, u64>,
    bodies: &BTreeMap<String, String>,
) -> Result<Vec<String>> {
    budget(&source.assembly)?;
    budget(&doc.assembly)?;
    validate_map(occurrences)?;
    validate_map(features)?;
    validate_map(bodies)?;
    if doc.occurrences.len() > MAX_REFERENCES {
        return Err(invalid("too many destination occurrences"));
    }
    let mut map_bytes = 0usize;
    for (old, new) in bodies {
        map_bytes = map_bytes.saturating_add(old.len()).saturating_add(new.len());
        if old.is_empty() || new.is_empty() || old.len() > MAX_NAME || new.len() > MAX_NAME || map_bytes > MAX_TEXT {
            return Err(invalid("invalid or excessive body identity map"));
        }
    }
    let available: BTreeSet<u64> = doc.occurrences.iter().map(|o| o.id).chain(std::iter::once(0)).collect();
    if occurrences.values().any(|id| !available.contains(id)) {
        return Err(invalid("copy occurrence map points to a missing occurrence"));
    }
    unique_ids(source.assembly.joints.iter().map(|j| j.id))?;
    let mut joint_id = unique_ids(doc.assembly.joints.iter().map(|j| j.id))?;
    let aux = |a: &Assembly| {
        a.contact_sets.iter().map(|x| x.id).chain(a.studies.iter().map(|x| x.id)).chain(a.exploded.iter().map(|x| x.id)).collect::<Vec<_>>()
    };
    unique_ids(aux(&source.assembly).into_iter())?;
    let mut auxiliary_id = unique_ids(aux(&doc.assembly).into_iter())?;
    let mut a = doc.assembly.clone();
    let mut messages = Vec::new();
    let mut origin_names = a.origins.iter().map(|o| o.name.clone()).collect();
    for old in &source.assembly.origins {
        if !occurrences.contains_key(&old.origin.occurrence) {
            continue;
        }
        if let Some(new) = origin(&old.origin, occurrences, features, bodies) {
            let mut item = old.clone();
            item.name = unique_name(&old.name, &mut origin_names)?;
            item.origin = new;
            a.origins.push(item);
        } else {
            report(&mut messages, "joint origin", &old.name);
        }
    }
    let mut joint_names = a.joints.iter().map(|j| j.name.clone()).collect();
    let mut joints = BTreeMap::new();
    let mut touched_joints = BTreeSet::new();
    for old in &source.assembly.joints {
        if !occurrences.contains_key(&old.a.occurrence) && !occurrences.contains_key(&old.b.occurrence) {
            continue;
        }
        touched_joints.insert(old.id);
        match (origin(&old.a, occurrences, features, bodies), origin(&old.b, occurrences, features, bodies)) {
            (Some(aa), Some(bb)) => {
                let mut item = old.clone();
                item.id = next_id(&mut joint_id)?;
                item.name = unique_name(&old.name, &mut joint_names)?;
                item.a = aa;
                item.b = bb;
                joints.insert(old.id, item.id);
                a.joints.push(item);
            }
            _ => report(&mut messages, "joint", &old.name),
        }
    }
    for old in &source.assembly.links {
        if !touched_joints.contains(&old.a) && !touched_joints.contains(&old.b) {
            continue;
        }
        let mut item = old.clone();
        // A gear's picked round faces are on occurrences of the copy too, and so are a cam's
        // profile and its follower's tip.
        let faces = item.coupling.as_mut().is_none_or(|c| {
            let sizes = c.sizes_mut().into_iter().all(|size| match size {
                Size::Circle(o) => origin(o, occurrences, features, bodies).map(|new| *o = new).is_some(),
                _ => true,
            });
            let Coupling::Cam(cam) = c else { return sizes };
            let tip = match &mut cam.tip {
                Some(t) => origin(t, occurrences, features, bodies).map(|new| *t = new).is_some(),
                None => true,
            };
            let profile = match &mut cam.profile {
                CamProfile::Sketch { sketch, .. } => features.get(sketch).map(|new| *sketch = *new).is_some(),
                CamProfile::Edges { occurrence, names, .. } => {
                    occurrences.get(occurrence).map(|new| *occurrence = *new).is_some()
                        && names.iter_mut().all(|n| n.is_empty() || face_name(n, features, bodies).map(|new| *n = new).is_some())
                }
            };
            sizes && tip && profile
        });
        match (joints.get(&old.a), joints.get(&old.b)) {
            (Some(aa), Some(bb)) if faces => {
                item.a = *aa;
                item.b = *bb;
                a.links.push(item);
            }
            _ => report(&mut messages, "motion link", &format!("{} → {}", old.a, old.b)),
        }
    }
    let mut contact_names = a.contact_sets.iter().map(|x| x.name.clone()).collect();
    for old in &source.assembly.contact_sets {
        if !old.occurrences.iter().any(|o| occurrences.contains_key(o)) {
            continue;
        }
        if let Some(ids) = old.occurrences.iter().map(|o| occurrences.get(o).copied()).collect::<Option<Vec<_>>>() {
            let mut item = old.clone();
            item.id = next_id(&mut auxiliary_id)?;
            item.name = unique_name(&old.name, &mut contact_names)?;
            item.occurrences = ids;
            a.contact_sets.push(item);
        } else {
            report(&mut messages, "contact set", &old.name);
        }
    }
    let mut study_names = a.studies.iter().map(|x| x.name.clone()).collect();
    for old in &source.assembly.studies {
        if !old.keys.iter().any(|k| touched_joints.contains(&k.joint)) {
            continue;
        }
        if old.keys.iter().all(|k| joints.contains_key(&k.joint)) {
            let mut item = old.clone();
            item.id = next_id(&mut auxiliary_id)?;
            item.name = unique_name(&old.name, &mut study_names)?;
            for key in &mut item.keys {
                key.joint = *joints.get(&key.joint).ok_or_else(|| invalid("missing copied study joint"))?;
            }
            a.studies.push(item);
        } else {
            report(&mut messages, "motion study", &old.name);
        }
    }
    let mut view_names = a.exploded.iter().map(|x| x.name.clone()).collect();
    for old in &source.assembly.exploded {
        if !old.moves.iter().any(|(o, _)| occurrences.contains_key(o)) {
            continue;
        }
        if old.moves.iter().all(|(o, _)| occurrences.contains_key(o)) {
            let mut item = old.clone();
            item.id = next_id(&mut auxiliary_id)?;
            item.name = unique_name(&old.name, &mut view_names)?;
            for (o, _) in &mut item.moves {
                *o = *occurrences.get(o).ok_or_else(|| invalid("missing copied exploded occurrence"))?;
            }
            a.exploded.push(item);
        } else {
            report(&mut messages, "exploded view", &old.name);
        }
    }
    budget(&a)?;
    doc.assembly = a;
    Ok(messages)
}

#[cfg(test)]
#[path = "component_clone_assembly_tests.rs"]
mod tests;
