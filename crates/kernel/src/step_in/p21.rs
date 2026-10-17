//! ISO 10303-21 ("Part 21") exchange structure parser: the HEADER and the DATA section's entity
//! instances, simple (`#1=NAME(...)`) and complex (`#2=(A(...) B(...))`). Written from the
//! public standard. Input is hostile: sizes, entity counts and nesting depth are capped and every
//! error is a message, never a panic.

use std::collections::HashMap;

/// Largest STEP text we parse.
pub const MAX_BYTES: usize = 512 << 20;
/// Most entity instances in one file.
pub const MAX_ENTITIES: usize = 20_000_000;
/// Deepest parameter list nesting.
const MAX_DEPTH: usize = 64;

/// A parameter value.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    Int(i64),
    Real(f64),
    Str(String),
    /// `.NAME.` (`.T.`/`.F.` included).
    Enum(String),
    Ref(u64),
    List(Vec<Param>),
    /// `NAME(params)` (a typed value such as `LENGTH_MEASURE(1.)`).
    Typed(String, Vec<Param>),
    Binary(String),
    /// `$`
    Null,
    /// `*`
    Derived,
}

impl Param {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Param::Real(x) => Some(*x).filter(|x| x.is_finite()),
            Param::Int(i) => Some(*i as f64),
            Param::Typed(_, v) if v.len() == 1 => v.first().and_then(Param::as_f64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Param::Int(i) => Some(*i),
            Param::Real(x) if x.fract() == 0.0 && x.abs() < 9e15 => Some(*x as i64),
            _ => None,
        }
    }
    pub fn as_ref_id(&self) -> Option<u64> {
        match self {
            Param::Ref(r) => Some(*r),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Param::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_enum(&self) -> Option<&str> {
        match self {
            Param::Enum(s) => Some(s),
            _ => None,
        }
    }
    /// `.T.` → true, `.F.` → false (`.U.` and anything else → None).
    pub fn as_bool(&self) -> Option<bool> {
        match self.as_enum()? {
            "T" => Some(true),
            "F" => Some(false),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Param]> {
        match self {
            Param::List(v) => Some(v),
            _ => None,
        }
    }
}

/// One record of an entity instance: a simple instance has one, a complex one several.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub name: String,
    pub params: Vec<Param>,
}

/// An entity instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub records: Vec<Record>,
}

impl Entity {
    pub fn is_complex(&self) -> bool {
        self.records.len() > 1
    }
    /// Name of a simple instance (the first record of a complex one).
    pub fn name(&self) -> &str {
        self.records.first().map(|r| r.name.as_str()).unwrap_or("")
    }
    /// Parameters of the record called `name`.
    pub fn record(&self, name: &str) -> Option<&[Param]> {
        self.records.iter().find(|r| r.name == name).map(|r| r.params.as_slice())
    }
    pub fn has(&self, name: &str) -> bool {
        self.records.iter().any(|r| r.name == name)
    }
    /// Parameters of a simple instance.
    pub fn params(&self) -> &[Param] {
        self.records.first().map(|r| r.params.as_slice()).unwrap_or(&[])
    }
}

/// A parsed exchange file.
#[derive(Clone, Debug, Default)]
pub struct Exchange {
    /// FILE_SCHEMA names.
    pub schemas: Vec<String>,
    /// FILE_NAME originating system.
    pub originating_system: String,
    pub entities: HashMap<u64, Entity>,
    /// Ids defined more than once (the first definition is kept).
    pub duplicates: Vec<u64>,
}

impl Exchange {
    pub fn get(&self, id: u64) -> Option<&Entity> {
        self.entities.get(&id)
    }
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

fn err<T>(msg: impl Into<String>) -> Result<T, String> {
    Err(msg.into())
}

impl<'a> Lexer<'a> {
    fn line(&self) -> usize {
        self.s.get(..self.i).map(|b| b.iter().filter(|c| **c == b'\n').count() + 1).unwrap_or(0)
    }
    fn fail<T>(&self, msg: &str) -> Result<T, String> {
        err(format!("STEP syntax error at line {}: {msg}", self.line()))
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    /// Skip whitespace and comments.
    fn ws(&mut self) -> Result<(), String> {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.i += 1,
                Some(b'/') if self.s.get(self.i + 1) == Some(&b'*') => {
                    let rest = self.s.get(self.i + 2..).unwrap_or(&[]);
                    match rest.windows(2).position(|w| w == b"*/") {
                        Some(p) => self.i += p + 4,
                        None => return self.fail("unterminated comment"),
                    }
                }
                _ => return Ok(()),
            }
        }
    }
    fn eat(&mut self, c: u8) -> Result<bool, String> {
        self.ws()?;
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.eat(c)? { Ok(()) } else { self.fail(&format!("expected `{}`", c as char)) }
    }
    fn keyword(&mut self) -> Result<String, String> {
        self.ws()?;
        let st = self.i;
        if self.peek() == Some(b'!') {
            self.i += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == st {
            return self.fail("expected a keyword");
        }
        Ok(String::from_utf8_lossy(self.s.get(st..self.i).unwrap_or(&[])).to_ascii_uppercase())
    }
    fn number(&mut self) -> Result<Param, String> {
        let st = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.' | b'E' | b'e') {
                self.i += 1;
            } else {
                break;
            }
        }
        let t = std::str::from_utf8(self.s.get(st..self.i).unwrap_or(&[])).unwrap_or("");
        if !t.contains(['.', 'E', 'e'])
            && let Ok(i) = t.parse::<i64>()
        {
            return Ok(Param::Int(i));
        }
        // STEP allows `1.` and `1.E-3`.
        let fixed = t.replace(".E", ".0E").replace(".e", ".0e");
        let fixed = if fixed.ends_with('.') { format!("{fixed}0") } else { fixed };
        match fixed.parse::<f64>() {
            Ok(x) => Ok(Param::Real(x)),
            Err(_) => self.fail(&format!("bad number `{t}`")),
        }
    }
    fn string(&mut self) -> Result<String, String> {
        // Opening quote consumed by the caller.
        let mut raw = Vec::new();
        loop {
            match self.peek() {
                None => return self.fail("unterminated string"),
                Some(b'\'') => {
                    if self.s.get(self.i + 1) == Some(&b'\'') {
                        raw.push(b'\'');
                        self.i += 2;
                    } else {
                        self.i += 1;
                        break;
                    }
                }
                Some(b'\n') | Some(b'\r') => self.i += 1,
                Some(c) => {
                    raw.push(c);
                    self.i += 1;
                }
            }
        }
        Ok(decode_string(&raw))
    }
    fn param(&mut self, depth: usize) -> Result<Param, String> {
        if depth > MAX_DEPTH {
            return self.fail("parameters nested too deeply");
        }
        self.ws()?;
        let Some(c) = self.peek() else { return self.fail("unexpected end of file") };
        match c {
            b'$' => {
                self.i += 1;
                Ok(Param::Null)
            }
            b'*' => {
                self.i += 1;
                Ok(Param::Derived)
            }
            b'\'' => {
                self.i += 1;
                Ok(Param::Str(self.string()?))
            }
            b'"' => {
                self.i += 1;
                let st = self.i;
                while self.peek().is_some_and(|c| c != b'"') {
                    self.i += 1;
                }
                let v = String::from_utf8_lossy(self.s.get(st..self.i).unwrap_or(&[])).to_string();
                self.expect(b'"')?;
                Ok(Param::Binary(v))
            }
            b'#' => {
                self.i += 1;
                let st = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(self.s.get(st..self.i).unwrap_or(&[])).unwrap_or("");
                t.parse::<u64>().map(Param::Ref).or_else(|_| self.fail("bad entity reference"))
            }
            b'.' if self.s.get(self.i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') => {
                self.i += 1;
                let st = self.i;
                while self.peek().is_some_and(|c| c != b'.') {
                    self.i += 1;
                }
                let v = String::from_utf8_lossy(self.s.get(st..self.i).unwrap_or(&[])).trim().to_ascii_uppercase();
                self.expect(b'.')?;
                Ok(Param::Enum(v))
            }
            b'(' => {
                self.i += 1;
                Ok(Param::List(self.list_rest(depth + 1)?))
            }
            c if c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.') => self.number(),
            c if c.is_ascii_alphabetic() || c == b'!' => {
                let name = self.keyword()?;
                self.expect(b'(')?;
                Ok(Param::Typed(name, self.list_rest(depth + 1)?))
            }
            _ => self.fail(&format!("unexpected `{}`", c as char)),
        }
    }
    /// Parameters after an opening `(` up to and including the `)`.
    fn list_rest(&mut self, depth: usize) -> Result<Vec<Param>, String> {
        let mut v = Vec::new();
        if self.eat(b')')? {
            return Ok(v);
        }
        loop {
            v.push(self.param(depth)?);
            if self.eat(b',')? {
                continue;
            }
            self.expect(b')')?;
            return Ok(v);
        }
    }
    fn record(&mut self) -> Result<Record, String> {
        let name = self.keyword()?;
        self.expect(b'(')?;
        Ok(Record { name, params: self.list_rest(1)? })
    }
}

/// Decode the Part 21 string escapes (`\X\hh`, `\X2\…\X0\`, `\X4\…\X0\`, `\S\c`, `\\`).
fn decode_string(raw: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    let hex = |b: &[u8]| std::str::from_utf8(b).ok().and_then(|t| u32::from_str_radix(t, 16).ok());
    while let Some(&c) = raw.get(i) {
        if c == b'\\' {
            let rest = raw.get(i..).unwrap_or(&[]);
            if rest.starts_with(b"\\\\") {
                out.push('\\');
                i += 2;
                continue;
            }
            if rest.starts_with(b"\\X\\")
                && let Some(v) = rest.get(3..5).and_then(hex)
            {
                out.push(char::from_u32(v).unwrap_or('?'));
                i += 5;
                continue;
            }
            for (tag, width) in [(&b"\\X2\\"[..], 4usize), (&b"\\X4\\"[..], 8)] {
                if rest.starts_with(tag)
                    && let Some(end) = rest.windows(4).position(|w| w == b"\\X0\\")
                {
                    let body = rest.get(4..end).unwrap_or(&[]);
                    let units: Vec<u32> = body.chunks(width).filter_map(hex).collect();
                    if width == 4 {
                        let u16s: Vec<u16> = units.iter().map(|u| *u as u16).collect();
                        out.push_str(&String::from_utf16_lossy(&u16s));
                    } else {
                        out.extend(units.iter().map(|u| char::from_u32(*u).unwrap_or('?')));
                    }
                    i += end + 4;
                    break;
                }
            }
            if i < raw.len() && raw.get(i) == Some(&b'\\') {
                if rest.starts_with(b"\\S\\")
                    && let Some(&ch) = rest.get(3)
                {
                    out.push(char::from_u32(ch as u32 + 128).unwrap_or('?'));
                    i += 4;
                    continue;
                }
                out.push('\\');
                i += 1;
            }
            continue;
        }
        // Plain bytes: keep UTF-8 runs intact.
        let st = i;
        while raw.get(i).is_some_and(|c| *c != b'\\') {
            i += 1;
        }
        out.push_str(&String::from_utf8_lossy(raw.get(st..i).unwrap_or(&[])));
    }
    out
}

/// Parse a Part 21 file.
pub fn parse(text: &str) -> Result<Exchange, String> {
    if text.len() > MAX_BYTES {
        return err(format!("STEP file too large ({} MB, limit {} MB)", text.len() >> 20, MAX_BYTES >> 20));
    }
    let mut lx = Lexer { s: text.as_bytes(), i: 0 };
    let magic = lx.keyword()?;
    if magic != "ISO-10303-21" {
        return err("not a STEP file (no ISO-10303-21 header)");
    }
    lx.expect(b';')?;
    let mut ex = Exchange::default();
    let watch = crate::watch::current();
    if let Some(w) = &watch {
        if w.cancelled() {
            return err("STEP import cancelled");
        }
        w.progress("parsing", 0, text.len());
    }
    loop {
        lx.ws()?;
        if lx.peek().is_none() {
            return err("STEP file ends without END-ISO-10303-21");
        }
        let kw = lx.keyword()?;
        match kw.as_str() {
            "HEADER" => {
                lx.expect(b';')?;
                loop {
                    let k = lx.keyword()?;
                    if k == "ENDSEC" {
                        lx.expect(b';')?;
                        break;
                    }
                    lx.expect(b'(')?;
                    let params = lx.list_rest(1)?;
                    lx.expect(b';')?;
                    match k.as_str() {
                        "FILE_SCHEMA" => {
                            if let Some(Param::List(v)) = params.first() {
                                ex.schemas = v.iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
                            }
                        }
                        "FILE_NAME" => {
                            ex.originating_system = params.get(5).and_then(Param::as_str).unwrap_or("").to_string();
                        }
                        _ => {}
                    }
                }
            }
            "DATA" => {
                // Optional `DATA('name', ('schema'));`
                if lx.eat(b'(')? {
                    lx.list_rest(1)?;
                }
                lx.expect(b';')?;
                loop {
                    lx.ws()?;
                    if lx.peek() != Some(b'#') {
                        let k = lx.keyword()?;
                        if k == "ENDSEC" {
                            lx.expect(b';')?;
                            break;
                        }
                        return lx.fail(&format!("unexpected `{k}` in DATA"));
                    }
                    lx.i += 1;
                    let st = lx.i;
                    while lx.peek().is_some_and(|c| c.is_ascii_digit()) {
                        lx.i += 1;
                    }
                    let id: u64 = match std::str::from_utf8(lx.s.get(st..lx.i).unwrap_or(&[])).ok().and_then(|t| t.parse().ok()) {
                        Some(id) => id,
                        None => return lx.fail("bad entity id"),
                    };
                    lx.expect(b'=')?;
                    let records = if lx.eat(b'(')? {
                        let mut rs = Vec::new();
                        while !lx.eat(b')')? {
                            if rs.len() > 64 {
                                return lx.fail("complex entity with too many parts");
                            }
                            rs.push(lx.record()?);
                        }
                        rs
                    } else {
                        vec![lx.record()?]
                    };
                    lx.expect(b';')?;
                    if ex.entities.len() % 4096 == 0
                        && let Some(w) = &watch
                    {
                        if w.cancelled() {
                            return err("STEP import cancelled");
                        }
                        w.progress("parsing", lx.i, lx.s.len());
                    }
                    if ex.entities.len() >= MAX_ENTITIES {
                        return err(format!("STEP file has more than {MAX_ENTITIES} entities"));
                    }
                    // A duplicate id is invalid; some writers emit them for geometry nobody
                    // reads. Keep the first definition and report it.
                    match ex.entities.entry(id) {
                        std::collections::hash_map::Entry::Occupied(_) => ex.duplicates.push(id),
                        std::collections::hash_map::Entry::Vacant(v) => {
                            v.insert(Entity { records });
                        }
                    }
                }
            }
            "END-ISO-10303-21" => {
                lx.expect(b';')?;
                if let Some(w) = &watch {
                    if w.cancelled() {
                        return err("STEP import cancelled");
                    }
                    w.progress("parsing", text.len(), text.len());
                }
                return Ok(ex);
            }
            // ANCHOR / REFERENCE / SIGNATURE sections (Part 21 edition 3): skip to ENDSEC.
            "ANCHOR" | "REFERENCE" | "SIGNATURE" => {
                let rest = lx.s.get(lx.i..).unwrap_or(&[]);
                match rest.windows(7).position(|w| w == b"ENDSEC;") {
                    Some(p) => lx.i += p + 7,
                    None => return lx.fail("unterminated section"),
                }
            }
            _ => return lx.fail(&format!("unexpected `{kw}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_and_complex() {
        let t = "ISO-10303-21;\nHEADER;FILE_DESCRIPTION((''),'2;1');\nFILE_NAME('a','',(''),(''),'','sys','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;\nDATA;\n#1=CARTESIAN_POINT('p',(0.,1.5E1,-2));\n/* c */ #2=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n#3=X('it''s','\\X2\\00E9\\X0\\',$,#1,LENGTH_MEASURE(1.E-3),\"0F\");\nENDSEC;\nEND-ISO-10303-21;\n";
        let ex = parse(t).unwrap();
        assert_eq!(ex.schemas, vec!["AUTOMOTIVE_DESIGN"]);
        assert_eq!(ex.originating_system, "sys");
        let p = ex.get(1).unwrap();
        assert_eq!(p.name(), "CARTESIAN_POINT");
        let xyz: Vec<f64> = p.params()[1].as_list().unwrap().iter().filter_map(Param::as_f64).collect();
        assert_eq!(xyz, vec![0.0, 15.0, -2.0]);
        let u = ex.get(2).unwrap();
        assert!(u.is_complex() && u.has("SI_UNIT"));
        assert_eq!(u.record("SI_UNIT").unwrap()[0].as_enum(), Some("MILLI"));
        let x = ex.get(3).unwrap().params();
        assert_eq!(x[0].as_str(), Some("it's"));
        assert_eq!(x[1].as_str(), Some("\u{e9}"));
        assert_eq!(x[2], Param::Null);
        assert_eq!(x[3].as_ref_id(), Some(1));
        assert_eq!(x[4].as_f64(), Some(1e-3));
    }

    #[test]
    fn hostile_text_is_an_error() {
        for t in [
            "",
            "garbage",
            "ISO-10303-21;",
            "ISO-10303-21;DATA;#1=A(",
            "ISO-10303-21;DATA;#1=A('x",
            "ISO-10303-21;DATA;#1=A(/*",
            "ISO-10303-21;DATA;#=A();ENDSEC;END-ISO-10303-21;",
            "ISO-10303-21;DATA;#99999999999999999999999=A();ENDSEC;END-ISO-10303-21;",
            "ISO-10303-21;DATA;#1=A(1e999999.);ENDSEC;END-ISO-10303-21;",
        ] {
            let _ = parse(t);
        }
        let deep = format!("ISO-10303-21;DATA;#1=A({}{});ENDSEC;END-ISO-10303-21;", "(".repeat(10_000), ")".repeat(10_000));
        assert!(parse(&deep).is_err());
        let dup = parse("ISO-10303-21;DATA;#1=A(1);#1=B();ENDSEC;END-ISO-10303-21;").unwrap();
        assert_eq!(dup.duplicates, vec![1]);
        assert_eq!(dup.get(1).unwrap().name(), "A");
        assert_eq!(decode_string(b"\\X\\E9"), "\u{e9}");
        assert_eq!(decode_string(b"a\\X2\\00E900E8\\X0\\b"), "a\u{e9}\u{e8}b");
        assert_eq!(decode_string(b"\\"), "\\");
    }
}
