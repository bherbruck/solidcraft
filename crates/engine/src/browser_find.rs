//! Finding things in the browser by name (#1413): every placement of a component, and the
//! bodies, sketches, construction planes and joints, as the browser lists them. The browser's
//! search field and the `browser.find` command both use it, so they always agree.

use std::collections::HashMap;

use serde::Serialize;
use solidcraft_doc::FeatureKind;

use crate::occurrences::OccurrencePlacement;
use crate::{Sel, Session};

/// The longest pattern taken, in characters.
pub const MAX_PATTERN_CHARS: usize = 256;
/// The most matches listed; `total` still counts every one.
pub const MAX_LISTED: usize = 10_000;
/// The most placements looked through (the bound isolating placements has too).
pub const MAX_PLACEMENTS: usize = 100_000;
/// Only this much of a name is matched: a hostile design's huge names can't make a search slow.
const MAX_NAME_CHARS: usize = 4096;
/// Placement paths are at most this deep (as in [`Session::occurrence_placement`]).
const MAX_DEPTH: usize = 64;

/// What a match is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A placement of a component.
    Component,
    Body,
    Sketch,
    Construction,
    Joint,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Component, Kind::Body, Kind::Sketch, Kind::Construction, Kind::Joint];

    pub fn id(self) -> &'static str {
        match self {
            Kind::Component => "component",
            Kind::Body => "body",
            Kind::Sketch => "sketch",
            Kind::Construction => "construction",
            Kind::Joint => "joint",
        }
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// A name pattern: the text anywhere in the name, ignoring case. With `*` (any run of
/// characters) or `?` (any one character) in it, it is matched against the whole name instead,
/// so `*:1` is every name that ends in `:1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// Lower-cased, trimmed.
    text: String,
    /// The wildcard form, with runs of `*` made one; `None` for plain text.
    glob: Option<Vec<char>>,
}

impl Pattern {
    /// `None` for blank text or text longer than [`MAX_PATTERN_CHARS`]. Spaces around it don't
    /// count.
    pub fn new(text: &str) -> Option<Pattern> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_PATTERN_CHARS {
            return None;
        }
        let text = text.to_lowercase();
        let glob = text.contains(['*', '?']).then(|| {
            let mut chars: Vec<char> = Vec::with_capacity(text.len());
            for c in text.chars() {
                if !(c == '*' && chars.last() == Some(&'*')) {
                    chars.push(c);
                }
            }
            chars
        });
        Some(Pattern { text, glob })
    }

    pub fn matches(&self, name: &str) -> bool {
        let name: String = name.chars().take(MAX_NAME_CHARS).collect::<String>().to_lowercase();
        match &self.glob {
            None => name.contains(self.text.as_str()),
            Some(glob) => whole(glob, &name.chars().collect::<Vec<char>>()),
        }
    }
}

/// `*` and `?` against the whole of `name`. On a mismatch it goes back only to the last `*`, so
/// it takes at most `glob.len() * name.len()` steps whatever the pattern.
fn whole(glob: &[char], name: &[char]) -> bool {
    let (mut g, mut n) = (0, 0);
    // After the last `*` seen: where the pattern goes on, and where in the name that try began.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match glob.get(g) {
            Some('*') => {
                star = Some((g + 1, n));
                g += 1;
            }
            Some(&c) if c == '?' || name.get(n) == Some(&c) => {
                g += 1;
                n += 1;
            }
            _ => match star {
                Some((after, from)) => {
                    g = after;
                    n = from + 1;
                    star = Some((after, from + 1));
                }
                None => return false,
            },
        }
    }
    glob.get(g..).is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}

/// One thing a search found, as the browser lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Found {
    pub kind: Kind,
    /// The name it is listed under.
    pub name: String,
    /// The browser's key for it: a body's name in its component, a sketch's or construction
    /// plane's feature id, a placement's occurrence id, a joint's id.
    pub key: String,
    /// The placement it is listed in, or a component's own placement (empty: the design itself).
    pub path: Vec<u64>,
    /// What `select.set` takes for it (none for a joint).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<Sel>,
}

/// What a search found.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Search {
    /// The matches in browser order (folders, then joints, then placements, each placement's
    /// contents after it), at most [`MAX_LISTED`].
    pub found: Vec<Found>,
    /// Every match, listed or not.
    pub total: usize,
    /// The search stopped at [`MAX_PLACEMENTS`] placements: there may be more.
    pub incomplete: bool,
}

/// Everything in the browser whose name matches `pattern` (everything, without one), of the
/// `kinds` given (every kind when there are none).
pub fn find(s: &Session, pattern: Option<&Pattern>, kinds: &[Kind]) -> Search {
    find_within(s, pattern, kinds, MAX_LISTED, MAX_PLACEMENTS)
}

pub(crate) fn find_within(s: &Session, pattern: Option<&Pattern>, kinds: &[Kind], listed: usize, placements: usize) -> Search {
    let mut walk = Walk { s, pattern, kinds, max_listed: listed, placements_left: placements, out: Search::default() };
    walk.folders(0, &[], None);
    for j in &s.doc.assembly.joints {
        walk.hit(Kind::Joint, &j.name, || (j.id.to_string(), None), &[]);
    }
    walk.placements(&[], 0);
    walk.out
}

struct Walk<'a> {
    s: &'a Session,
    pattern: Option<&'a Pattern>,
    kinds: &'a [Kind],
    /// The most matches listed, and the placements still to look through.
    max_listed: usize,
    placements_left: usize,
    out: Search,
}

impl Walk<'_> {
    fn wants(&self, kind: Kind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    /// Counts and lists `name` if it matches; `rest` gives its key and item.
    fn hit(&mut self, kind: Kind, name: &str, rest: impl FnOnce() -> (String, Option<Sel>), path: &[u64]) {
        if !self.wants(kind) || !self.pattern.is_none_or(|p| p.matches(name)) {
            return;
        }
        self.out.total += 1;
        if self.out.found.len() < self.max_listed {
            let (key, item) = rest();
            self.out.found.push(Found { kind, name: name.to_string(), key, path: path.to_vec(), item });
        }
    }

    /// A component's Bodies, Sketches and Construction folders, in the design itself or in one
    /// placement of it (whose bodies have their own names in the world).
    fn folders(&mut self, component: u64, path: &[u64], placed: Option<&OccurrencePlacement>) {
        let s = self.s;
        if self.wants(Kind::Body) {
            let world: HashMap<&str, &str> =
                placed.map(|p| p.bodies.iter().map(|b| (b.source.as_str(), b.world.as_str())).collect()).unwrap_or_default();
            let state = s.model.state();
            for b in state.bodies.iter().filter(|b| s.doc.model_body_component(b) == component && !s.body_removed_later(&b.name)) {
                let item = || {
                    let name = world.get(b.name.as_str()).map_or_else(|| b.name.clone(), |w| (*w).to_string());
                    (b.name.clone(), Some(Sel::Body { name }))
                };
                self.hit(Kind::Body, &b.name, item, path);
            }
        }
        for f in s.doc.features.iter().filter(|f| f.component == component) {
            match f.kind {
                FeatureKind::Sketch { .. } => self.hit(Kind::Sketch, &f.name, || (f.id.to_string(), Some(Sel::Feature { id: f.id })), path),
                FeatureKind::ConstructionPlane { .. } => {
                    self.hit(Kind::Construction, &f.name, || (f.id.to_string(), Some(Sel::Plane { name: f.name.clone() })), path)
                }
                _ => {}
            }
        }
    }

    /// The placements in `component` (placed at `parent`), each followed by its contents.
    fn placements(&mut self, parent: &[u64], component: u64) {
        let s = self.s;
        for o in s.doc.occurrences.iter().filter(|o| o.parent == component) {
            if self.placements_left == 0 {
                self.out.incomplete = true;
                return;
            }
            self.placements_left -= 1;
            let mut path = parent.to_vec();
            path.push(o.id);
            // A missing component or a placement inside itself has no placement.
            let Ok(placed) = s.occurrence_placement(&path) else { continue };
            self.hit(Kind::Component, &placed.name, || (o.id.to_string(), Some(Sel::Occurrence { path: path.clone() })), &path);
            self.folders(placed.component, &path, Some(&placed));
            if path.len() < MAX_DEPTH {
                self.placements(&path, placed.component);
            }
        }
    }
}
