//! Fastener families from library files: a family's sizes, head dimensions and the lengths
//! each size comes in with the grip (clamped thickness) each length holds.
//!
//! A part library is a JSON file (or a directory with `library.json` in it):
//!
//! ```json
//! {"format": "solidcraft-parts/1", "id": "example.rivets", "name": "Example rivets",
//!  "families": [{"kind": "fastener_family", "name": "rivet_dome", "shape": "blind_rivet",
//!    "sizes": [{"size": "4.0", "d": 4.0, "head": {"d": 8.0, "h": 1.4, "style": "dome"},
//!               "lengths": [{"length": 8, "grip": [3.0, 5.0]}]}]}]}
//! ```
//!
//! [`Library::decode`] refuses a file whose structure is wrong (the error names the JSON path
//! of the first bad value); [`FastenerFamily::check`] reports what makes a well-formed family
//! hard to use (grips that overlap, a size with no lengths).

use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "solidcraft-parts/1";
/// Largest library file read.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_FAMILIES: usize = 256;
const MAX_SIZES: usize = 256;
const MAX_LENGTHS: usize = 256;
/// Largest dimension accepted (mm).
const MAX_MM: f64 = 2000.0;

/// What a family's parts are, and so how they are built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastenerShape {
    /// A shank `d` under a head (`head.d` across, `head.h` high).
    BlindRivet,
    /// A shank `d` under a head (hex across flats `head.d`, or round), cosmetic thread.
    Bolt,
    Screw,
    /// A nut: hole `d`, `head.d` across flats, `head.h` high; no lengths.
    Nut,
    /// A ring: hole `d`, outside `head.d`, `head.h` thick; no lengths.
    Washer,
    /// A plain rod `d`; no head.
    Pin,
}

impl FastenerShape {
    pub fn has_lengths(self) -> bool {
        !matches!(self, Self::Nut | Self::Washer)
    }
    pub fn has_head(self) -> bool {
        !matches!(self, Self::Pin)
    }
}

/// How a head looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadStyle {
    /// A low rounded head.
    #[default]
    Dome,
    /// A flat cylinder.
    Flat,
    /// A cone that sinks into a countersink (its top is flush with the seating face).
    Countersunk,
    /// A hex prism (`d` across flats).
    Hex,
    /// A round head with a hex socket.
    Socket,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Head {
    pub d: f64,
    pub h: f64,
    #[serde(default)]
    pub style: HeadStyle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FastenerLength {
    pub length: f64,
    /// The clamped thickness this length holds: [min, max] (mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grip: Option<[f64; 2]>,
    /// The supplier's article number, passed through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FastenerSize {
    pub size: String,
    /// Nominal (shank) diameter.
    pub d: f64,
    /// Thread pitch (bolts, screws, nuts): a cosmetic thread is added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Head>,
    #[serde(default)]
    pub lengths: Vec<FastenerLength>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FastenerFamily {
    /// Always `fastener_family`.
    pub kind: String,
    /// The id commands take (`family`): lower case letters, digits and `_`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard: Option<String>,
    pub shape: FastenerShape,
    pub sizes: Vec<FastenerSize>,
}

/// A problem with a family, at a JSON path inside its library file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

fn mm(x: f64) -> bool {
    x.is_finite() && x > 0.0 && x <= MAX_MM
}

fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s == s.trim() && s.len() <= max && !s.chars().any(char::is_control)
}

/// A family id: 1–64 bytes, a lower-case letter first, then lower-case letters, digits and `_`.
pub fn valid_name(s: &str) -> bool {
    s.len() <= 64
        && s.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl FastenerFamily {
    /// Structural errors (bad names, numbers out of range, a head missing where the shape needs
    /// one), each at its JSON path under `at` (for example `families[0]`).
    pub fn validate(&self, at: &str) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut bad = |path: String, message: &str| out.push(Problem { path, message: message.into() });
        if self.kind != "fastener_family" {
            bad(format!("{at}.kind"), "must be \"fastener_family\"");
        }
        if !valid_name(&self.name) {
            bad(format!("{at}.name"), "must be 1–64 lower-case letters, digits or _ (a letter first)");
        }
        if self.label.as_deref().is_some_and(|l| !text(l, 128)) {
            bad(format!("{at}.label"), "must be 1–128 bytes of text");
        }
        if self.standard.as_deref().is_some_and(|l| !text(l, 128)) {
            bad(format!("{at}.standard"), "must be 1–128 bytes of text");
        }
        if self.sizes.len() > MAX_SIZES {
            bad(format!("{at}.sizes"), "at most 256 sizes");
        }
        for (i, s) in self.sizes.iter().enumerate().take(MAX_SIZES) {
            let at = format!("{at}.sizes[{i}]");
            if !text(&s.size, 32) {
                bad(format!("{at}.size"), "must be 1–32 bytes of text");
            }
            if !mm(s.d) {
                bad(format!("{at}.d"), "must be a diameter in mm above 0");
            }
            if s.pitch.is_some_and(|p| !(mm(p) && p < s.d)) {
                bad(format!("{at}.pitch"), "must be above 0 and below d");
            }
            match (s.head, self.shape.has_head()) {
                (None, true) => bad(format!("{at}.head"), "this shape needs a head: {d, h}"),
                (Some(h), _) => {
                    if !mm(h.d) || h.d <= s.d {
                        bad(format!("{at}.head.d"), "must be larger than d");
                    }
                    if !mm(h.h) {
                        bad(format!("{at}.head.h"), "must be a height in mm above 0");
                    }
                }
                (None, false) => {}
            }
            if s.lengths.len() > MAX_LENGTHS {
                bad(format!("{at}.lengths"), "at most 256 lengths");
            }
            if !self.shape.has_lengths() && !s.lengths.is_empty() {
                bad(format!("{at}.lengths"), "nuts and washers have no lengths");
            }
            for (j, l) in s.lengths.iter().enumerate().take(MAX_LENGTHS) {
                let at = format!("{at}.lengths[{j}]");
                if !mm(l.length) {
                    bad(format!("{at}.length"), "must be a length in mm above 0");
                }
                if let Some([lo, hi]) = l.grip
                    && !(lo.is_finite() && hi.is_finite() && lo >= 0.0 && lo <= hi && hi <= MAX_MM)
                {
                    bad(format!("{at}.grip"), "must be [min, max] in mm, 0 ≤ min ≤ max");
                }
                if l.article.as_deref().is_some_and(|a| !text(a, 128)) {
                    bad(format!("{at}.article"), "must be 1–128 bytes of text");
                }
            }
        }
        out
    }

    /// What makes a well-formed family hard to use: no sizes, a size listed twice, a size with
    /// no lengths, two lengths of a size whose grips overlap (touching ends are fine).
    pub fn check(&self, at: &str) -> Vec<Problem> {
        let mut out = self.validate(at);
        let mut bad = |path: String, message: String| out.push(Problem { path, message });
        if self.sizes.is_empty() {
            bad(format!("{at}.sizes"), "no sizes".into());
        }
        for (i, s) in self.sizes.iter().enumerate() {
            let at = format!("{at}.sizes[{i}]");
            if self.sizes.iter().take(i).any(|o| o.size == s.size) {
                bad(format!("{at}.size"), format!("size `{}` is listed twice", s.size));
            }
            if self.shape.has_lengths() && s.lengths.is_empty() {
                bad(format!("{at}.lengths"), format!("size `{}` has no lengths", s.size));
            }
            for (j, l) in s.lengths.iter().enumerate() {
                if s.lengths.iter().take(j).any(|o| (o.length - l.length).abs() < 1e-9) {
                    bad(format!("{at}.lengths[{j}].length"), format!("length {} is listed twice", l.length));
                }
                let Some([lo, hi]) = l.grip else { continue };
                for (k, o) in s.lengths.iter().enumerate().take(j) {
                    if let Some([a, b]) = o.grip
                        && lo < b - 1e-9
                        && a < hi - 1e-9
                    {
                        bad(format!("{at}.lengths[{j}].grip"), format!("grip [{lo}, {hi}] overlaps lengths[{k}]'s [{a}, {b}]"));
                    }
                }
            }
        }
        out
    }

    pub fn size(&self, size: &str) -> Option<&FastenerSize> {
        self.sizes.iter().find(|s| s.size.eq_ignore_ascii_case(size.trim()))
    }
}

impl FastenerSize {
    /// The shortest length whose grip range holds `grip` (mm).
    pub fn length_for_grip(&self, grip: f64) -> Option<&FastenerLength> {
        self.lengths
            .iter()
            .filter(|l| l.grip.is_some_and(|[lo, hi]| grip >= lo - 1e-9 && grip <= hi + 1e-9))
            .min_by(|a, b| a.length.total_cmp(&b.length))
    }
}

/// Where a library's families come from (shown, never fetched).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub format: String,
    /// Lower-case letters, digits, `.`, `_`, `-`; a letter first.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<LibraryProvenance>,
    pub families: Vec<FastenerFamily>,
}

impl Library {
    /// Read a library file's bytes: well-formed JSON of this format with valid families (every
    /// error names its JSON path), and no family listed twice.
    pub fn decode(bytes: &[u8]) -> Result<Self, Problem> {
        if bytes.len() > MAX_BYTES {
            return Err(Problem { path: "$".into(), message: "a part library is at most 4 MiB".into() });
        }
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| Problem { path: "$".into(), message: format!("not JSON: {e}") })?;
        // Name the first field serde refuses by walking the value ourselves.
        shape_check(&value)?;
        let lib: Self = serde_json::from_value(value).map_err(|e| Problem { path: "$".into(), message: e.to_string() })?;
        lib.validate()?;
        Ok(lib)
    }

    pub fn validate(&self) -> Result<(), Problem> {
        let first = |path: &str, message: &str| Err(Problem { path: path.into(), message: message.into() });
        if self.format != FORMAT {
            return first("format", "must be \"solidcraft-parts/1\"");
        }
        let id_ok = self.id.len() <= 64
            && self.id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && self.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if !id_ok {
            return first("id", "must be 1–64 lower-case letters, digits, ., _ or - (a letter first)");
        }
        if !text(&self.name, 128) {
            return first("name", "must be 1–128 bytes of text");
        }
        if self.families.len() > MAX_FAMILIES {
            return first("families", "at most 256 families");
        }
        for (i, f) in self.families.iter().enumerate() {
            if let Some(p) = f.validate(&format!("families[{i}]")).into_iter().next() {
                return Err(p);
            }
            if self.families.iter().take(i).any(|o| o.name == f.name) {
                return first(&format!("families[{i}].name"), "a family of that name is already in this library");
            }
        }
        Ok(())
    }
}

/// The JSON path of the first value of the wrong type or an unknown field, for an error that
/// says where (serde's own error gives only a line and column).
fn shape_check(v: &serde_json::Value) -> Result<(), Problem> {
    use serde_json::Value;
    let err = |path: String, message: &str| Err(Problem { path, message: message.into() });
    let obj = |v: &Value, path: &str, known: &[&str], required: &[&str]| -> Result<(), Problem> {
        let Some(m) = v.as_object() else { return err(path.into(), "must be an object") };
        for k in m.keys() {
            if !known.contains(&k.as_str()) {
                return err(format!("{path}.{k}"), "unknown field");
            }
        }
        for k in required {
            if !m.contains_key(*k) {
                return err(format!("{path}.{k}"), "is required");
            }
        }
        Ok(())
    };
    let number = |v: Option<&Value>, path: String| -> Result<(), Problem> {
        match v {
            None | Some(Value::Number(_)) => Ok(()),
            Some(_) => err(path, "must be a number"),
        }
    };
    let string = |v: Option<&Value>, path: String| -> Result<(), Problem> {
        match v {
            None | Some(Value::String(_)) => Ok(()),
            Some(_) => err(path, "must be text"),
        }
    };
    obj(v, "$", &["format", "id", "name", "provenance", "families"], &["format", "id", "name", "families"])?;
    for k in ["format", "id", "name"] {
        string(v.get(k), k.into())?;
    }
    let Some(families) = v.get("families").and_then(Value::as_array) else { return err("families".into(), "must be a list") };
    for (i, f) in families.iter().enumerate().take(MAX_FAMILIES) {
        let at = format!("families[{i}]");
        obj(f, &at, &["kind", "name", "label", "standard", "shape", "sizes"], &["kind", "name", "shape", "sizes"])?;
        for k in ["kind", "name", "label", "standard"] {
            string(f.get(k), format!("{at}.{k}"))?;
        }
        if let Some(shape) = f.get("shape")
            && serde_json::from_value::<FastenerShape>(shape.clone()).is_err()
        {
            return err(format!("{at}.shape"), "must be blind_rivet, bolt, screw, nut, washer or pin");
        }
        let Some(sizes) = f.get("sizes").and_then(Value::as_array) else { return err(format!("{at}.sizes"), "must be a list") };
        for (j, s) in sizes.iter().enumerate().take(MAX_SIZES) {
            let at = format!("{at}.sizes[{j}]");
            obj(s, &at, &["size", "d", "pitch", "head", "lengths"], &["size", "d"])?;
            string(s.get("size"), format!("{at}.size"))?;
            number(s.get("d"), format!("{at}.d"))?;
            number(s.get("pitch"), format!("{at}.pitch"))?;
            if let Some(h) = s.get("head") {
                obj(h, &format!("{at}.head"), &["d", "h", "style"], &["d", "h"])?;
                number(h.get("d"), format!("{at}.head.d"))?;
                number(h.get("h"), format!("{at}.head.h"))?;
                if let Some(style) = h.get("style")
                    && serde_json::from_value::<HeadStyle>(style.clone()).is_err()
                {
                    return err(format!("{at}.head.style"), "must be dome, flat, countersunk, hex or socket");
                }
            }
            let lengths = match s.get("lengths") {
                None => continue,
                Some(Value::Array(l)) => l,
                Some(_) => return err(format!("{at}.lengths"), "must be a list"),
            };
            for (k, l) in lengths.iter().enumerate().take(MAX_LENGTHS) {
                let at = format!("{at}.lengths[{k}]");
                obj(l, &at, &["length", "grip", "article"], &["length"])?;
                number(l.get("length"), format!("{at}.length"))?;
                string(l.get("article"), format!("{at}.article"))?;
                match l.get("grip") {
                    None => {}
                    Some(Value::Array(g)) if g.len() == 2 && g.iter().all(Value::is_number) => {}
                    Some(_) => return err(format!("{at}.grip"), "must be [min, max]"),
                }
            }
        }
    }
    Ok(())
}
