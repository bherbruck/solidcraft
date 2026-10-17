//! Parameter expressions: `2 * width + 5 mm`, `angle / 2`, `sqrt(a^2 + b^2)`, `atan2(h; w)`.
//!
//! Values carry a simple dimension (powers of length and angle). Lengths are millimetres and
//! angles radians internally. A unit-less number added to a dimensioned value takes that value's
//! unit: the parameter's own unit when it has one (`width + 5` adds 5 in for an inch
//! parameter), else millimetres and degrees. A unit-less final result takes the expected unit
//! (`2` for an inch parameter is 2 in). Trigonometric functions take an angle; a unit-less
//! argument is read as degrees.

use std::collections::BTreeMap;

use crate::DocError;

const MAX_LEN: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_ARGS: usize = 64;

/// An expression a feature holds, as the user wrote it (`2 * width + 5 mm`): kept as text and
/// evaluated with the design's parameters. It is saved as a plain string, and reads as one
/// (`&expr` is a `&str`); it changes only as a whole (`*expr = new.into()`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Expr(String);

impl Expr {
    pub fn new(s: impl Into<String>) -> Self {
        Expr(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::ops::Deref for Expr {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Expr {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for Expr {
    fn from(s: String) -> Self {
        Expr(s)
    }
}

impl From<&str> for Expr {
    fn from(s: &str) -> Self {
        Expr(s.to_string())
    }
}

impl From<&String> for Expr {
    fn from(s: &String) -> Self {
        Expr(s.clone())
    }
}

impl From<Expr> for String {
    fn from(e: Expr) -> Self {
        e.0
    }
}

impl std::fmt::Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<str> for Expr {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for Expr {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<Expr> for String {
    fn eq(&self, other: &Expr) -> bool {
        *self == other.0
    }
}

impl PartialEq<String> for Expr {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

/// What an expression must evaluate to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Length,
    Angle,
    Unitless,
}

impl Kind {
    pub fn from_unit(u: &str) -> Kind {
        unit_info(u).map(|(k, _)| k).unwrap_or(Kind::Length)
    }
    /// The unit a new parameter of this kind gets (`mm`, `deg` or none).
    pub fn default_unit(self) -> &'static str {
        match self {
            Kind::Length => "mm",
            Kind::Angle => "deg",
            Kind::Unitless => "",
        }
    }
}

/// Units a parameter or an expression may use: name, kind and size (mm or radians).
pub const UNITS: [(&str, Kind, f64); 14] = [
    ("mm", Kind::Length, 1.0),
    ("cm", Kind::Length, 10.0),
    ("m", Kind::Length, 1000.0),
    ("um", Kind::Length, 0.001),
    ("µm", Kind::Length, 0.001),
    ("in", Kind::Length, 25.4),
    ("inch", Kind::Length, 25.4),
    ("ft", Kind::Length, 304.8),
    ("yd", Kind::Length, 914.4),
    ("km", Kind::Length, 1_000_000.0),
    ("mil", Kind::Length, 0.0254),
    ("nm", Kind::Length, 1e-6),
    ("deg", Kind::Angle, std::f64::consts::PI / 180.0),
    ("rad", Kind::Angle, 1.0),
];

/// Functions an expression may call (name, signature) — for autocomplete and help.
pub const FUNCTIONS: [(&str, &str); 23] = [
    ("sin", "sin(angle)"),
    ("cos", "cos(angle)"),
    ("tan", "tan(angle)"),
    ("asin", "asin(x) → angle"),
    ("acos", "acos(x) → angle"),
    ("atan", "atan(x) → angle"),
    ("atan2", "atan2(y, x) → angle"),
    ("sqrt", "sqrt(x)"),
    ("abs", "abs(x)"),
    ("min", "min(a, b, …)"),
    ("max", "max(a, b, …)"),
    ("floor", "floor(x)"),
    ("ceil", "ceil(x)"),
    ("round", "round(x)"),
    ("exp", "exp(x)"),
    ("ln", "ln(x)"),
    ("log", "log(x) (base 10)"),
    ("log10", "log10(x)"),
    ("sign", "sign(x)"),
    ("hypot", "hypot(a, b)"),
    ("pow", "pow(x, n)"),
    ("trunc", "trunc(x)"),
    ("mod", "mod(a, b)"),
];

/// Constants an expression may use.
pub const CONSTANTS: [&str; 2] = ["pi", "PI"];

/// Kind and size (mm or radians per unit) of a parameter's unit; empty means unit-less.
pub fn unit_info(u: &str) -> Option<(Kind, f64)> {
    match u.trim() {
        "" | "none" | "unitless" => Some((Kind::Unitless, 1.0)),
        "°" | "degree" | "degrees" => Some((Kind::Angle, std::f64::consts::PI / 180.0)),
        t => UNITS.iter().find(|(n, _, _)| *n == t).map(|(_, k, s)| (*k, *s)),
    }
}

/// How unit-less numbers are read: millimetres / degrees by default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Defaults {
    /// Millimetres per unit-less length number.
    pub len: f64,
    /// Radians per unit-less angle number.
    pub ang: f64,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults { len: 1.0, ang: std::f64::consts::PI / 180.0 }
    }
}

impl Defaults {
    /// Defaults for a parameter with this unit (`in` reads bare numbers as inches); the
    /// document's length unit covers the other kind.
    pub fn for_unit(unit: &str, doc_len: f64) -> Defaults {
        let mut d = Defaults { len: doc_len, ..Defaults::default() };
        match unit_info(unit) {
            Some((Kind::Length, s)) => d.len = s,
            Some((Kind::Angle, s)) => d.ang = s,
            _ => {}
        }
        d
    }
}

/// A dimensioned value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Value {
    pub v: f64,
    pub len: i8,
    pub ang: i8,
}

impl Value {
    pub fn num(v: f64) -> Value {
        Value { v, len: 0, ang: 0 }
    }
    pub fn length(mm: f64) -> Value {
        Value { v: mm, len: 1, ang: 0 }
    }
    pub fn angle(rad: f64) -> Value {
        Value { v: rad, len: 0, ang: 1 }
    }
    pub fn unitless(&self) -> bool {
        self.len == 0 && self.ang == 0
    }
    /// The value's kind, if it is a plain length, angle or number.
    pub fn kind(&self) -> Option<Kind> {
        match (self.len, self.ang) {
            (0, 0) => Some(Kind::Unitless),
            (1, 0) => Some(Kind::Length),
            (0, 1) => Some(Kind::Angle),
            _ => None,
        }
    }
    /// Convert to the expected kind (mm or radians); unit-less numbers are mm / degrees.
    pub fn to_kind(self, k: Kind) -> Result<f64, DocError> {
        self.to_kind_in(k, Defaults::default())
    }
    /// Convert to the expected kind, reading a unit-less result in the given defaults.
    pub fn to_kind_in(self, k: Kind, d: Defaults) -> Result<f64, DocError> {
        let out = match k {
            Kind::Length if self.len == 1 && self.ang == 0 => self.v,
            Kind::Length if self.unitless() => self.v * d.len,
            Kind::Angle if self.ang == 1 && self.len == 0 => self.v,
            Kind::Angle if self.unitless() => self.v * d.ang,
            Kind::Unitless if self.unitless() => self.v,
            _ => {
                return Err(DocError::Expr(format!("expected {}, got {}", kind_word(k), self.describe())));
            }
        };
        if out.is_finite() { Ok(out) } else { Err(DocError::Expr("result is not a finite number".into())) }
    }
    /// "a length", "an angle", "length²"…
    pub fn describe(&self) -> String {
        match self.kind() {
            Some(k) => kind_word(k).to_string(),
            None => {
                let pow = |name: &str, n: i8| match n {
                    0 => String::new(),
                    1 => name.to_string(),
                    _ => format!("{name}^{n}"),
                };
                let parts: Vec<String> = [pow("length", self.len), pow("angle", self.ang)].into_iter().filter(|s| !s.is_empty()).collect();
                format!("a value in {}", parts.join("·"))
            }
        }
    }
}

fn kind_word(k: Kind) -> &'static str {
    match k {
        Kind::Length => "a length",
        Kind::Angle => "an angle",
        Kind::Unitless => "a unit-less number",
    }
}

/// A number in the given unit (e.g. 50.8 mm in `in` is 2); unknown units leave it as is.
pub fn value_in_unit(v: Value, unit: &str) -> f64 {
    match (unit_info(unit), v.kind()) {
        (Some((Kind::Length, s)), Some(Kind::Length)) | (Some((Kind::Angle, s)), Some(Kind::Angle)) if s != 0.0 => v.v / s,
        _ => v.v,
    }
}

fn unit(name: &str) -> Option<Value> {
    UNITS.iter().find(|(n, _, _)| *n == name).map(|(_, k, s)| match k {
        Kind::Angle => Value::angle(*s),
        _ => Value::length(*s),
    })
}

fn is_constant(id: &str) -> bool {
    CONSTANTS.contains(&id)
}

/// Is this a name the expression language reserves (unit, constant or function)?
pub fn is_reserved(name: &str) -> bool {
    unit(name).is_some() || is_constant(name) || FUNCTIONS.iter().any(|(f, _)| *f == name)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(char),
}

/// Tokens with their byte ranges in the source.
fn lex_spans(s: &str) -> Result<Vec<(Tok, usize, usize)>, DocError> {
    if s.len() > MAX_LEN {
        return Err(DocError::Expr("expression too long".into()));
    }
    let cs: Vec<(usize, char)> = s.char_indices().collect();
    let at = |i: usize| cs.get(i).map(|x| x.0).unwrap_or(s.len());
    let mut i = 0;
    let mut out = Vec::new();
    // Parentheses open at this point: a comma outside them can only be a decimal comma.
    let mut depth = 0usize;
    while let Some(&(_, c)) = cs.get(i) {
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let st = i;
            while cs.get(i).is_some_and(|(_, c)| c.is_ascii_digit() || *c == '.') {
                i += 1;
            }
            // A decimal comma (`2,5 mm`, as typed with a numeric keypad in many locales, #1347):
            // a comma between digits, in a number with no point, outside any parentheses (inside
            // them a comma separates a function's arguments: `max(1,2)`).
            let digit = |k: usize| cs.get(k).is_some_and(|(_, c)| c.is_ascii_digit());
            let has_point = cs.get(st..i).is_some_and(|ds| ds.iter().any(|(_, c)| *c == '.'));
            if depth == 0 && !has_point && digit(i.wrapping_sub(1)) && cs.get(i).is_some_and(|(_, c)| *c == ',') && digit(i + 1) {
                i += 1;
                while digit(i) {
                    i += 1;
                }
            }
            // Exponent: 1e-3 (but not `2em`).
            if cs.get(i).is_some_and(|(_, c)| *c == 'e' || *c == 'E') {
                let sign = cs.get(i + 1).is_some_and(|(_, c)| *c == '-' || *c == '+');
                let digit_at = if sign { i + 2 } else { i + 1 };
                if cs.get(digit_at).is_some_and(|(_, c)| c.is_ascii_digit()) {
                    i = digit_at;
                    while cs.get(i).is_some_and(|(_, c)| c.is_ascii_digit()) {
                        i += 1;
                    }
                }
            }
            let t = s.get(at(st)..at(i)).unwrap_or_default();
            let v: f64 = t.replace(',', ".").parse().map_err(|_| DocError::Expr(format!("bad number `{t}`")))?;
            // A mixed number, as imperial sizes are written (`2 1/8"` is 2.125 in, #1802): a whole
            // number, then a proper fraction of whole numbers, is one value, not `2 × 1/8`.
            if t.chars().all(|c| c.is_ascii_digit())
                && let Some((fraction, end)) = mixed_fraction(&cs, i, depth)
            {
                out.push((Tok::Num(v + fraction), at(st), at(end)));
                i = end;
                continue;
            }
            out.push((Tok::Num(v), at(st), at(i)));
        } else if c.is_alphabetic() || c == '_' || c == 'µ' {
            let st = i;
            while cs.get(i).is_some_and(|(_, c)| c.is_alphanumeric() || *c == '_') {
                i += 1;
            }
            out.push((Tok::Id(s.get(at(st)..at(i)).unwrap_or_default().to_string()), at(st), at(i)));
        } else if c == '°' {
            out.push((Tok::Id("deg".into()), at(i), at(i + 1)));
            i += 1;
        } else if c == '"' || c == '″' {
            // Inch and foot marks (`1"`, `2'`, #1502), straight or as primes.
            out.push((Tok::Id("in".into()), at(i), at(i + 1)));
            i += 1;
        } else if c == '\'' || c == '′' {
            out.push((Tok::Id("ft".into()), at(i), at(i + 1)));
            i += 1;
        } else if c == '×' || c == '·' {
            out.push((Tok::Op('*'), at(i), at(i + 1)));
            i += 1;
        } else if c == '÷' {
            out.push((Tok::Op('/'), at(i), at(i + 1)));
            i += 1;
        } else if c == '−' {
            out.push((Tok::Op('-'), at(i), at(i + 1)));
            i += 1;
        } else if c == ';' {
            // Fusion separates function arguments with `;` in some locales.
            out.push((Tok::Op(','), at(i), at(i + 1)));
            i += 1;
        } else if "+-*/^(),%".contains(c) {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            out.push((Tok::Op(c), at(i), at(i + 1)));
            i += 1;
        } else {
            return Err(DocError::Expr(format!("unexpected `{c}`")));
        }
    }
    Ok(out)
}

/// The proper fraction of a mixed number whose whole part ends at `i`: spaces, digits, `/`,
/// digits, with nothing after them that would continue the number (`2 1/8.5`, `2 1/8e3`, a
/// decimal comma outside parentheses: `2 1/8,5`). Its value and where it ends. `depth`: the
/// parentheses open there (inside them a comma separates arguments: `max(2 1/8,1)`).
fn mixed_fraction(cs: &[(usize, char)], mut i: usize, depth: usize) -> Option<(f64, usize)> {
    let digits = |from: usize| cs.get(from..).map_or(0, |rest| rest.iter().take_while(|(_, c)| c.is_ascii_digit()).count());
    let space = i;
    while cs.get(i).is_some_and(|(_, c)| *c == ' ' || *c == '\t') {
        i += 1;
    }
    let numerator = digits(i);
    if i == space || numerator == 0 || !cs.get(i + numerator).is_some_and(|(_, c)| *c == '/') {
        return None;
    }
    let denominator = digits(i + numerator + 1);
    let end = i + numerator + 1 + denominator;
    let next = |k: usize| cs.get(end + k).map(|(_, c)| *c);
    let continues = matches!(next(0), Some('.'))
        || (depth == 0 && matches!(next(0), Some(',')) && next(1).is_some_and(|c| c.is_ascii_digit()))
        || (matches!(next(0), Some('e' | 'E')) && next(1).is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+'));
    if denominator == 0 || continues {
        return None;
    }
    // Compared as digits, not as floats: long ones round to the same float (#1802 review).
    let digits_of = |from: usize, len: usize| -> Option<String> {
        let s: String = cs.get(from..from + len)?.iter().map(|(_, c)| *c).collect();
        Some(s.trim_start_matches('0').to_string())
    };
    let (n, d) = (digits_of(i, numerator)?, digits_of(i + numerator + 1, denominator)?);
    let proper = !d.is_empty() && (n.len(), n.as_str()) < (d.len(), d.as_str());
    let value = |t: &str| if t.is_empty() { Some(0.0) } else { t.parse::<f64>().ok() };
    if !proper {
        return None;
    }
    Some((value(&n)? / value(&d)?, end))
}

fn lex(s: &str) -> Result<Vec<Tok>, DocError> {
    Ok(lex_spans(s)?.into_iter().map(|(t, _, _)| t).collect())
}

/// Is the identifier token at `i` a reference to a parameter (not a call, unit or constant)?
fn is_reference(toks: &[Tok], i: usize, id: &str) -> bool {
    let is_call = matches!(toks.get(i + 1), Some(Tok::Op('(')));
    !is_call && unit(id).is_none() && !is_constant(id)
}

/// Identifiers an expression refers to (excluding units, constants and functions).
pub fn references(s: &str) -> Vec<String> {
    let Ok(toks) = lex(s) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if let Tok::Id(id) = t
            && is_reference(&toks, i, id)
            && !out.contains(id)
        {
            out.push(id.clone());
        }
    }
    out
}

/// The expression with every reference to `old` renamed to `new` (text otherwise kept).
pub fn rename_reference(s: &str, old: &str, new: &str) -> String {
    let Ok(spans) = lex_spans(s) else { return s.to_string() };
    let toks: Vec<Tok> = spans.iter().map(|(t, _, _)| t.clone()).collect();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (i, (t, a, b)) in spans.iter().enumerate() {
        if let Tok::Id(id) = t
            && id == old
            && is_reference(&toks, i, id)
        {
            out.push_str(s.get(last..*a).unwrap_or_default());
            out.push_str(new);
            last = *b;
        }
    }
    out.push_str(s.get(last..).unwrap_or_default());
    out
}

/// Is the expression just a number (with an optional unit), like `20`, `20 mm` or `-1.5 in`?
pub fn is_literal(s: &str) -> bool {
    let Ok(toks) = lex(s) else { return false };
    let toks: &[Tok] = match toks.first() {
        Some(Tok::Op('-' | '+')) => toks.get(1..).unwrap_or_default(),
        _ => &toks,
    };
    match toks {
        [Tok::Num(_)] => true,
        [Tok::Num(_), Tok::Id(u)] => unit(u).is_some(),
        _ => false,
    }
}

/// Is the expression a bare number without a unit, like `20` or `-1.5`?
pub fn is_bare_number(s: &str) -> bool {
    is_literal(s) && s.trim().trim_start_matches(['-', '+']).trim().parse::<f64>().is_ok()
}

/// The length unit of a design with these units: `mm` unless they name another length unit.
pub fn length_unit_or_mm(units: &str) -> &str {
    let u = units.trim();
    if unit_info(u).is_some_and(|(k, _)| k == Kind::Length) { u } else { "mm" }
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    lookup: &'a dyn Fn(&str) -> Result<Value, DocError>,
    depth: usize,
    defaults: Defaults,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn enter(&mut self) -> Result<(), DocError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(DocError::Expr("expression nested too deeply".into())) } else { Ok(()) }
    }
    fn expr(&mut self) -> Result<Value, DocError> {
        self.enter()?;
        let mut a = self.term()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let b = self.term()?;
            a = add(a, b, c == '-', self.defaults)?;
        }
        self.depth -= 1;
        Ok(a)
    }
    fn term(&mut self) -> Result<Value, DocError> {
        let mut a = self.unary()?;
        loop {
            match self.peek().cloned() {
                Some(Tok::Op('*')) => {
                    self.pos += 1;
                    let b = self.unary()?;
                    a = mul(a, b, false)?;
                }
                Some(Tok::Op('/')) => {
                    self.pos += 1;
                    let b = self.unary()?;
                    a = mul(a, b, true)?;
                }
                Some(Tok::Op('%')) => {
                    self.pos += 1;
                    let b = self.unary()?;
                    a = modulo(a, b, self.defaults)?;
                }
                // Juxtaposition: `10 mm`, `2 width`.
                Some(Tok::Id(_) | Tok::Num(_)) | Some(Tok::Op('(')) => {
                    let b = self.unary()?;
                    a = mul(a, b, false)?;
                }
                _ => return Ok(a),
            }
        }
    }
    fn unary(&mut self) -> Result<Value, DocError> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                self.enter()?;
                let v = self.unary()?;
                self.depth -= 1;
                Ok(Value { v: -v.v, ..v })
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.enter()?;
                let v = self.unary()?;
                self.depth -= 1;
                Ok(v)
            }
            _ => self.power(),
        }
    }
    fn power(&mut self) -> Result<Value, DocError> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            self.enter()?;
            let e = self.unary()?;
            self.depth -= 1;
            return pow(base, e);
        }
        Ok(base)
    }
    fn atom(&mut self) -> Result<Value, DocError> {
        match self.next() {
            Some(Tok::Num(v)) => Ok(Value::num(v)),
            Some(Tok::Op('(')) => {
                let v = self.expr()?;
                match self.next() {
                    Some(Tok::Op(')')) => Ok(v),
                    _ => Err(DocError::Expr("missing `)`".into())),
                }
            }
            Some(Tok::Id(id)) => {
                if let Some(Tok::Op('(')) = self.peek() {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::Op(')'))) {
                        loop {
                            if args.len() >= MAX_ARGS {
                                return Err(DocError::Expr(format!("too many arguments to `{id}`")));
                            }
                            args.push(self.expr()?);
                            match self.next() {
                                Some(Tok::Op(',')) => continue,
                                Some(Tok::Op(')')) => break,
                                _ => return Err(DocError::Expr(format!("missing `)` after the arguments of `{id}`"))),
                            }
                        }
                    } else {
                        self.pos += 1;
                    }
                    return call(&id, &args, self.defaults);
                }
                match id.as_str() {
                    "pi" | "PI" => return Ok(Value::num(std::f64::consts::PI)),
                    _ => {}
                }
                if let Some(u) = unit(&id) {
                    return Ok(u);
                }
                (self.lookup)(&id)
            }
            Some(Tok::Op(c)) => Err(DocError::Expr(format!("unexpected `{c}`"))),
            None => Err(DocError::Expr("unexpected end of expression".into())),
        }
    }
}

/// A unit-less number next to a dimensioned value, in that value's default unit.
fn scale_bare(x: Value, len: i8, ang: i8, d: Defaults) -> f64 {
    if !x.unitless() {
        x.v
    } else if len == 1 && ang == 0 {
        x.v * d.len
    } else if ang == 1 && len == 0 {
        x.v * d.ang
    } else {
        x.v
    }
}

fn add(a: Value, b: Value, sub: bool, d: Defaults) -> Result<Value, DocError> {
    let (len, ang) = if a.unitless() {
        (b.len, b.ang)
    } else if b.unitless() || (a.len == b.len && a.ang == b.ang) {
        (a.len, a.ang)
    } else {
        return Err(DocError::Expr(format!("cannot {} {} and {}", if sub { "subtract" } else { "add" }, a.describe(), b.describe())));
    };
    let (x, y) = (scale_bare(a, len, ang, d), scale_bare(b, len, ang, d));
    Ok(Value { v: if sub { x - y } else { x + y }, len, ang })
}

fn dims(a: i8, b: i8, sub: bool) -> Result<i8, DocError> {
    let r = if sub { a.checked_sub(b) } else { a.checked_add(b) };
    r.filter(|x| x.abs() <= 16).ok_or_else(|| DocError::Expr("unit powers too large".into()))
}

fn mul(a: Value, b: Value, div: bool) -> Result<Value, DocError> {
    if div {
        if b.v == 0.0 {
            return Err(DocError::Expr("division by zero".into()));
        }
        Ok(Value { v: a.v / b.v, len: dims(a.len, b.len, true)?, ang: dims(a.ang, b.ang, true)? })
    } else {
        Ok(Value { v: a.v * b.v, len: dims(a.len, b.len, false)?, ang: dims(a.ang, b.ang, false)? })
    }
}

fn modulo(a: Value, b: Value, d: Defaults) -> Result<Value, DocError> {
    let (len, ang) = if a.unitless() { (b.len, b.ang) } else { (a.len, a.ang) };
    if !(a.unitless() || b.unitless() || (a.len == b.len && a.ang == b.ang)) {
        return Err(DocError::Expr(format!("cannot take {} modulo {}", a.describe(), b.describe())));
    }
    let (x, y) = (scale_bare(a, len, ang, d), scale_bare(b, len, ang, d));
    if y == 0.0 {
        return Err(DocError::Expr("modulo by zero".into()));
    }
    Ok(Value { v: x.rem_euclid(y), len, ang })
}

fn pow(base: Value, e: Value) -> Result<Value, DocError> {
    if !e.unitless() {
        return Err(DocError::Expr(format!("an exponent must be a unit-less number, not {}", e.describe())));
    }
    let n = e.v;
    let k = if (n - n.round()).abs() < 1e-12 && n.abs() < 16.0 { n.round() as i8 } else { 0 };
    if !base.unitless() && k == 0 {
        return Err(DocError::Expr("a value with units needs a small whole-number exponent".into()));
    }
    Ok(Value { v: base.v.powf(n), len: dims(0, base.len.saturating_mul(k), false)?, ang: dims(0, base.ang.saturating_mul(k), false)? })
}

fn call(f: &str, args: &[Value], d: Defaults) -> Result<Value, DocError> {
    let one = || match args {
        [a] => Ok(*a),
        _ => Err(DocError::Expr(format!("`{f}` takes one argument"))),
    };
    let two = || match args {
        [a, b] => Ok((*a, *b)),
        _ => Err(DocError::Expr(format!("`{f}` takes two arguments"))),
    };
    let rad = |a: Value| {
        if a.ang == 1 && a.len == 0 {
            Ok(a.v)
        } else if a.unitless() {
            Ok(a.v * d.ang)
        } else {
            Err(DocError::Expr(format!("`{f}` needs an angle, not {}", a.describe())))
        }
    };
    let num = |a: Value| if a.unitless() { Ok(a.v) } else { Err(DocError::Expr(format!("`{f}` needs a unit-less number, not {}", a.describe()))) };
    let same = |a: Value, b: Value| -> Result<(Value, Value), DocError> {
        // Bring a bare number to the other operand's unit (as in `a + b`).
        let z = add(a, Value { v: 0.0, ..b }, false, d)?;
        let w = add(b, Value { v: 0.0, ..a }, false, d)?;
        Ok((z, w))
    };
    let unit_fn = |x: f64, a: Value| Value { v: x, ..a };
    Ok(match f {
        "sin" => Value::num(rad(one()?)?.sin()),
        "cos" => Value::num(rad(one()?)?.cos()),
        "tan" => Value::num(rad(one()?)?.tan()),
        "asin" | "acos" => {
            let x = num(one()?)?;
            if !(-1.0..=1.0).contains(&x) {
                return Err(DocError::Expr(format!("`{f}` needs a number between -1 and 1")));
            }
            Value::angle(if f == "asin" { x.asin() } else { x.acos() })
        }
        "atan" => Value::angle(num(one()?)?.atan()),
        "atan2" => {
            let (y, x) = two()?;
            let (y, x) = same(y, x)?;
            if y.len != x.len || y.ang != x.ang {
                return Err(DocError::Expr("`atan2` needs two values with the same units".into()));
            }
            Value::angle(y.v.atan2(x.v))
        }
        "sqrt" => {
            let a = one()?;
            if a.len % 2 != 0 || a.ang % 2 != 0 {
                return Err(DocError::Expr("sqrt of an odd power of a unit".into()));
            }
            if a.v < 0.0 {
                return Err(DocError::Expr("sqrt of a negative number".into()));
            }
            Value { v: a.v.sqrt(), len: a.len / 2, ang: a.ang / 2 }
        }
        "abs" => {
            let a = one()?;
            unit_fn(a.v.abs(), a)
        }
        "sign" => Value::num(one()?.v.signum()),
        "floor" | "ceil" | "round" | "trunc" => {
            let a = one()?;
            // Round in the value's own default unit (round(2.4 in) = 2 in for an inch parameter).
            let scale = scale_bare(Value::num(1.0), a.len, a.ang, d);
            let x = if scale != 0.0 { a.v / scale } else { a.v };
            let r = match f {
                "floor" => x.floor(),
                "ceil" => x.ceil(),
                "trunc" => x.trunc(),
                _ => x.round(),
            };
            unit_fn(r * if scale != 0.0 { scale } else { 1.0 }, a)
        }
        "exp" => Value::num(num(one()?)?.exp()),
        "ln" | "log" | "log10" => {
            let x = num(one()?)?;
            if x <= 0.0 {
                return Err(DocError::Expr(format!("`{f}` needs a positive number")));
            }
            Value::num(if f == "ln" { x.ln() } else { x.log10() })
        }
        "hypot" => {
            let (a, b) = two()?;
            let (a, b) = same(a, b)?;
            if a.len != b.len || a.ang != b.ang {
                return Err(DocError::Expr("`hypot` needs two values with the same units".into()));
            }
            unit_fn(a.v.hypot(b.v), a)
        }
        "pow" => {
            let (a, b) = two()?;
            pow(a, b)?
        }
        "mod" => {
            let (a, b) = two()?;
            modulo(a, b, d)?
        }
        "min" | "max" => {
            let Some(first) = args.first().copied() else { return Err(DocError::Expr(format!("`{f}` needs arguments"))) };
            let mut acc = first;
            for a in args.iter().skip(1) {
                let (x, y) = same(*a, acc)?;
                if x.len != y.len || x.ang != y.ang {
                    return Err(DocError::Expr(format!("`{f}` needs values with the same units")));
                }
                acc = if (f == "min" && x.v < y.v) || (f == "max" && x.v > y.v) { x } else { y };
            }
            acc
        }
        _ => return Err(DocError::Expr(format!("unknown function `{f}`"))),
    })
}

/// Evaluate an expression; `lookup` resolves parameter names. Bare numbers are mm / degrees.
pub fn eval_with(s: &str, lookup: &dyn Fn(&str) -> Result<Value, DocError>) -> Result<Value, DocError> {
    eval_with_defaults(s, lookup, Defaults::default())
}

/// Evaluate an expression reading bare numbers next to lengths / angles in `defaults`.
pub fn eval_with_defaults(s: &str, lookup: &dyn Fn(&str) -> Result<Value, DocError>, defaults: Defaults) -> Result<Value, DocError> {
    let toks = lex(s)?;
    if toks.is_empty() {
        return Err(DocError::Expr("empty expression".into()));
    }
    let mut p = Parser { toks, pos: 0, lookup, depth: 0, defaults };
    let v = p.expr()?;
    if p.pos < p.toks.len() {
        return Err(DocError::Expr(format!("unexpected input in `{s}`")));
    }
    if !v.v.is_finite() {
        return Err(DocError::Expr(format!("`{s}` is not a finite number")));
    }
    Ok(v)
}

/// One parameter to evaluate: name, expression and unit (`mm`, `in`, `deg`, empty…).
#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: String,
    pub expr: String,
    pub unit: String,
}

/// Evaluate all parameters in dependency order: values (mm / rad / unit-less) and errors by
/// name. Circular references are reported with their path (`a → b → a`). `doc_len` is the
/// document's length unit in mm (bare numbers of unit-less-unit parameters next to lengths).
pub fn eval_params(params: &[ParamDef], doc_len: f64) -> (BTreeMap<String, Value>, BTreeMap<String, String>) {
    let mut done: BTreeMap<String, Value> = BTreeMap::new();
    let mut errors: BTreeMap<String, String> = BTreeMap::new();
    let index: BTreeMap<&str, &ParamDef> = params.iter().map(|p| (p.name.as_str(), p)).collect();
    struct Ctx<'a> {
        index: BTreeMap<&'a str, &'a ParamDef>,
        done: BTreeMap<String, Value>,
        errors: BTreeMap<String, String>,
        stack: Vec<String>,
        doc_len: f64,
    }
    fn visit(name: &str, cx: &mut Ctx) -> Result<Value, DocError> {
        if let Some(v) = cx.done.get(name) {
            return Ok(*v);
        }
        if let Some(e) = cx.errors.get(name) {
            return Err(DocError::Expr(format!("`{name}` has an error: {e}")));
        }
        let Some(p) = cx.index.get(name).copied() else {
            return Err(DocError::Expr(format!("unknown parameter `{name}`")));
        };
        if let Some(at) = cx.stack.iter().position(|s| s == name) {
            let mut path: Vec<&str> = cx.stack.iter().skip(at).map(String::as_str).collect();
            path.push(name);
            return Err(DocError::Expr(format!("circular reference: {}", path.join(" → "))));
        }
        if cx.stack.len() > 512 {
            return Err(DocError::Expr("parameters nested too deeply".into()));
        }
        let Some((kind, _)) = unit_info(&p.unit).or(Some((Kind::Length, 1.0))) else {
            return Err(DocError::Expr(format!("unknown unit `{}`", p.unit)));
        };
        let defaults = Defaults::for_unit(&p.unit, cx.doc_len);
        cx.stack.push(name.to_string());
        let mut vals: BTreeMap<String, Value> = BTreeMap::new();
        let mut err = None;
        for d in references(&p.expr) {
            match visit(&d, cx) {
                Ok(v) => {
                    vals.insert(d, v);
                }
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        cx.stack.pop();
        let r = match err {
            Some(e) => Err(e),
            None => {
                eval_with_defaults(&p.expr, &|n| vals.get(n).copied().ok_or_else(|| DocError::Expr(format!("unknown parameter `{n}`"))), defaults)
                    .and_then(|v| {
                        let x = v.to_kind_in(kind, defaults)?;
                        Ok(match kind {
                            Kind::Length => Value::length(x),
                            Kind::Angle => Value::angle(x),
                            Kind::Unitless => Value::num(x),
                        })
                    })
            }
        };
        match &r {
            Ok(v) => {
                cx.done.insert(name.to_string(), *v);
            }
            Err(e) => {
                let msg = e.to_string();
                cx.errors.insert(name.to_string(), msg.strip_prefix("expression: ").unwrap_or(&msg).to_string());
            }
        }
        r
    }
    let mut cx = Ctx { index, done: std::mem::take(&mut done), errors: std::mem::take(&mut errors), stack: Vec::new(), doc_len };
    for p in params {
        cx.stack.clear();
        let _ = visit(&p.name, &mut cx);
    }
    (cx.done, cx.errors)
}

/// Dependency edges between parameters: (parameter, parameter it references).
pub fn param_edges(params: &[ParamDef]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for p in params {
        for r in references(&p.expr) {
            out.push((p.name.clone(), r));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str, k: Kind) -> Result<f64, DocError> {
        let look = |n: &str| match n {
            "width" => Ok(Value::length(40.0)),
            "ang" => Ok(Value::angle(std::f64::consts::FRAC_PI_2)),
            "count" => Ok(Value::num(3.0)),
            _ => Err(DocError::Expr(format!("unknown {n}"))),
        };
        eval_with(s, &look)?.to_kind(k)
    }

    fn def(n: &str, e: &str, u: &str) -> ParamDef {
        ParamDef { name: n.into(), expr: e.into(), unit: u.into() }
    }

    #[test]
    fn a_decimal_comma_outside_parentheses() {
        // #1347: a comma between digits is a decimal comma where it cannot separate arguments.
        assert_eq!(ev("2,5", Kind::Unitless).unwrap(), 2.5);
        assert_eq!(ev("2,5 mm", Kind::Length).unwrap(), 2.5);
        assert_eq!(ev("12,75mm + 0,25", Kind::Length).unwrap(), 13.0);
        assert_eq!(ev("width / 2,5", Kind::Length).unwrap(), 16.0);
        assert!((ev("1,5e3 mm", Kind::Length).unwrap() - 1500.0).abs() < 1e-9);
        assert!((ev("22,5 deg", Kind::Angle).unwrap() - 22.5f64.to_radians()).abs() < 1e-12);
        // Inside parentheses a comma still separates a function's arguments.
        assert_eq!(ev("max(1,2)", Kind::Unitless).unwrap(), 2.0);
        assert_eq!(ev("max(1 mm, 3, 2)", Kind::Length).unwrap(), 3.0);
        assert_eq!(ev("(2,5)", Kind::Unitless).ok(), None, "a comma inside parentheses is not a decimal comma");
        // Not between two digits, or a second comma: still an error.
        for bad in ["2,", ",5", "2 ,5", "2, 5", "1,5,3", "1.5,3"] {
            assert!(ev(bad, Kind::Unitless).is_err(), "{bad}");
        }
    }

    #[test]
    fn inch_and_foot_marks() {
        // #1502: `1"` in a millimetre design is an inch, `2'` two feet, straight or as primes.
        assert_eq!(ev("1\"", Kind::Length).unwrap(), 25.4);
        assert_eq!(ev("1″", Kind::Length).unwrap(), 25.4);
        assert_eq!(ev("2'", Kind::Length).unwrap(), 609.6);
        assert_eq!(ev("2′", Kind::Length).unwrap(), 609.6);
        assert_eq!(ev("1/2\"", Kind::Length).unwrap(), 12.7);
        assert_eq!(ev("width + 1\"", Kind::Length).unwrap(), 65.4);
        assert_eq!(ev("2' + 3\"", Kind::Length).unwrap(), 685.8);
        // A mark alone is its unit alone, as `in` is.
        assert_eq!(ev("\"", Kind::Length).ok(), ev("in", Kind::Length).ok());
    }

    #[test]
    fn mixed_numbers() {
        // #1802: `2 1/8"` is 2.125 in, as imperial sizes are written, not 2 × 1/8 in.
        assert_eq!(ev("2 1/8\"", Kind::Length).unwrap(), 2.125 * 25.4);
        assert_eq!(ev("2 1/8 in", Kind::Length).unwrap(), 2.125 * 25.4);
        assert_eq!(ev("3 7/16\"", Kind::Length).unwrap(), 3.4375 * 25.4);
        assert_eq!(ev("10 1/2mm", Kind::Length).unwrap(), 10.5);
        assert_eq!(ev("width + 1 1/2", Kind::Length).unwrap(), 41.5);
        assert_eq!(ev("max(2 1/8, 3)", Kind::Unitless).unwrap(), 3.0);
        // Inside parentheses a comma after the fraction separates arguments; outside, a comma
        // between digits is a decimal comma, which continues the number (as before).
        assert_eq!(ev("max(2 1/8,1)", Kind::Unitless).unwrap(), 2.125);
        assert_eq!(ev("2 1/8,5", Kind::Unitless).unwrap(), 2.0 / 8.5);
        // Long numerators and denominators are compared as digits, not as rounded floats.
        assert!((ev("2 9007199254740992/9007199254740993", Kind::Unitless).unwrap() - 3.0).abs() < 1e-12);
        assert_eq!(ev("2 007/8", Kind::Unitless).unwrap(), 2.875);
        assert_eq!(ev("-2 1/4", Kind::Unitless).unwrap(), -2.25);
        // Not a proper fraction of whole numbers after a whole number: as before.
        for (s, v) in [("2 3/2", 3.0), ("2.5 1/2", 1.25), ("2 1/8.5", 2.0 / 8.5), ("2 1 / 8", 0.25), ("2 1/0", f64::INFINITY), ("1/8", 0.125)] {
            assert_eq!(ev(s, Kind::Unitless).ok(), Some(v).filter(|v| v.is_finite()), "{s}");
        }
    }

    #[test]
    fn arithmetic_and_units() {
        assert_eq!(ev("10", Kind::Length).unwrap(), 10.0);
        assert!((ev("90 deg", Kind::Angle).unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(ev("10 mm", Kind::Length).unwrap(), 10.0);
        assert_eq!(ev("1 in", Kind::Length).unwrap(), 25.4);
        assert_eq!(ev("2 cm + 5", Kind::Length).unwrap(), 25.0);
        assert_eq!(ev("width / 2 + 1", Kind::Length).unwrap(), 21.0);
        assert_eq!(ev("width * count", Kind::Length).unwrap(), 120.0);
        assert_eq!(ev("-(2+3)*2^2", Kind::Unitless).unwrap(), -20.0);
        assert!((ev("45", Kind::Angle).unwrap() - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        assert!((ev("ang / 2", Kind::Angle).unwrap() - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        assert!((ev("ang + 90", Kind::Angle).unwrap() - std::f64::consts::PI).abs() < 1e-12);
        assert!((ev("sqrt((3 mm) ^ 2 + (4 mm) ^ 2)", Kind::Length).unwrap() - 5.0).abs() < 1e-12);
        assert!((ev("sin(30)", Kind::Unitless).unwrap() - 0.5).abs() < 1e-12);
        assert!((ev("2*pi", Kind::Unitless).unwrap() - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(ev("max(1 mm, 3, 2)", Kind::Length).unwrap(), 3.0);
        assert_eq!(ev("min(1 in, 30 mm)", Kind::Length).unwrap(), 25.4);
        assert_eq!(ev("1e-3 m", Kind::Length).unwrap(), 1.0);
        assert_eq!(ev("1 ft - 1 in", Kind::Length).unwrap(), 304.8 - 25.4);
        assert!((ev("atan2(1 mm, 1 mm)", Kind::Angle).unwrap() - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        assert!((ev("atan2(1; 1)", Kind::Angle).unwrap() - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        assert!((ev("exp(ln(5))", Kind::Unitless).unwrap() - 5.0).abs() < 1e-12);
        assert!((ev("log(1000)", Kind::Unitless).unwrap() - 3.0).abs() < 1e-12);
        assert_eq!(ev("floor(2.7) + ceil(0.2) + round(1.5) + abs(-1)", Kind::Unitless).unwrap(), 6.0);
        assert!((ev("1 rad", Kind::Angle).unwrap() - 1.0).abs() < 1e-12);
        assert!((ev("acos(0)", Kind::Angle).unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(ev("2 × 3 ÷ 4 − 1", Kind::Unitless).unwrap(), 0.5);
        assert_eq!(ev("hypot(3 mm, 4 mm)", Kind::Length).unwrap(), 5.0);
        assert_eq!(ev("7 % 4", Kind::Unitless).unwrap(), 3.0);
        assert_eq!(ev("width^2 / width", Kind::Length).unwrap(), 40.0);
    }

    #[test]
    fn errors() {
        assert!(ev("width + ang", Kind::Length).is_err());
        let e = ev("10 mm + 5 deg", Kind::Length).unwrap_err().to_string();
        assert!(e.contains("cannot add a length and an angle"), "{e}");
        assert!(ev("width * width", Kind::Length).unwrap_err().to_string().contains("expected a length"));
        assert!(ev("nope", Kind::Length).is_err());
        assert!(ev("(1", Kind::Length).is_err());
        assert!(ev("1 / 0", Kind::Length).is_err());
        assert!(ev("", Kind::Length).is_err());
        assert!(ev("1 $ 2", Kind::Length).is_err());
        assert!(ev("sqrt(-1)", Kind::Unitless).is_err());
        assert!(ev("asin(2)", Kind::Angle).is_err());
        assert!(ev("ln(0)", Kind::Unitless).is_err());
        assert!(ev("sin(1 mm)", Kind::Unitless).is_err());
        assert!(ev("atan2(1 mm, 1 deg)", Kind::Angle).is_err());
        assert!(ev("10 ^ 400", Kind::Unitless).is_err());
        assert!(ev("width ^ 1.5", Kind::Unitless).is_err());
        assert!(ev("max()", Kind::Unitless).is_err());
        assert!(ev("frob(1)", Kind::Unitless).is_err());
        let deep = "(".repeat(500) + "1" + &")".repeat(500);
        assert!(ev(&deep, Kind::Length).is_err());
        assert!(ev(&"-".repeat(500), Kind::Length).is_err());
        assert!(ev(&"+".repeat(500), Kind::Length).is_err());
        assert!(ev(&"width*".repeat(100), Kind::Length).is_err());
        assert!(ev(&("2^".repeat(200) + "2"), Kind::Unitless).is_err());
    }

    /// Hostile input never panics: random strings over the expression alphabet.
    #[test]
    fn fuzz_never_panics() {
        let alphabet: Vec<char> = "0123456789.eE+-*/^(),;% abcdmnpitrsgxw_°×÷−µ\u{0}\u{7f}é\"'".chars().collect();
        let words = ["width", "sin(", "atan2(", "mm", "deg", "pi", "1e308", "1e-400", "(", ")", "^", "max(", "in", "count", ","];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let n = (rnd() % 40) as usize;
            let mut s = String::new();
            for _ in 0..n {
                if rnd() % 4 == 0 {
                    s.push_str(words[(rnd() % words.len() as u64) as usize]);
                } else {
                    s.push(alphabet[(rnd() % alphabet.len() as u64) as usize]);
                }
            }
            for k in [Kind::Length, Kind::Angle, Kind::Unitless] {
                let _ = ev(&s, k);
            }
            let _ = references(&s);
            let _ = rename_reference(&s, "width", "w2");
            let _ = is_literal(&s);
        }
    }

    #[test]
    fn params_in_order_with_cycles() {
        let p = vec![def("b", "a * 2", "mm"), def("a", "10 mm", "mm"), def("x", "y", "mm"), def("y", "z + 1", "mm"), def("z", "x", "mm")];
        let (v, e) = eval_params(&p, 1.0);
        assert_eq!(v.get("b").unwrap().v, 20.0);
        assert!(e.contains_key("x") && e.contains_key("y") && e.contains_key("z"));
        assert!(e["x"].contains("circular reference: x → y → z → x"), "{}", e["x"]);
        assert_eq!(references("2*width + sin(ang) + 3 mm + pi"), vec!["width".to_string(), "ang".to_string()]);
        let (_, e) = eval_params(&[def("s", "s", "mm")], 1.0);
        assert!(e["s"].contains("s → s"));
    }

    #[test]
    fn parameter_units() {
        let p = vec![
            def("a", "2", "in"),
            def("b", "a + 1", "in"),
            def("c", "b", "mm"),
            def("t", "0.5", "rad"),
            def("u", "t + 1", "rad"),
            def("k", "3", ""),
            def("bad", "a + t", "mm"),
            def("mixed", "1 in + 2", "mm"),
        ];
        let (v, e) = eval_params(&p, 1.0);
        assert!((v["a"].v - 50.8).abs() < 1e-9);
        assert!((v["b"].v - 76.2).abs() < 1e-9);
        assert!((v["c"].v - 76.2).abs() < 1e-9);
        assert!((v["u"].v - 1.5).abs() < 1e-12);
        assert_eq!(v["k"], Value::num(3.0));
        assert!(e["bad"].contains("cannot add"));
        assert!((v["mixed"].v - 27.4).abs() < 1e-9);
        assert!((value_in_unit(v["b"], "in") - 3.0).abs() < 1e-12);
        assert!((value_in_unit(Value::angle(std::f64::consts::PI), "deg") - 180.0).abs() < 1e-9);
    }

    #[test]
    fn rename_and_literals() {
        assert_eq!(rename_reference("width*2 + widths + width(1) + sin(width)", "width", "w"), "w*2 + widths + width(1) + sin(w)");
        assert_eq!(rename_reference("10 mm", "mm", "x"), "10 mm");
        assert!(is_literal("20") && is_literal("-1.5 in") && is_literal("3 deg"));
        assert!(!is_literal("d1") && !is_literal("2 * 3") && !is_literal("2 width"));
        assert!(is_bare_number(" -1.5 ") && is_bare_number("+2") && !is_bare_number("20 mm") && !is_bare_number("(2)"));
        assert_eq!((length_unit_or_mm(" in "), length_unit_or_mm("deg"), length_unit_or_mm("")), ("in", "mm", "mm"));
    }
}
