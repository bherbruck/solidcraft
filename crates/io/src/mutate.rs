//! Deterministic mutations of valid files, and the readers run over them (#469). Every reader of
//! bytes the user didn't write gets the same treatment: truncation, flipped and overwritten
//! bytes, dropped and repeated runs, appended garbage and, for text formats, dropped and
//! repeated lines and numbers that lie (`nan`, `inf`, `1e400`, …). Each mutated file must give an
//! error or a result, never a panic, within a time budget and the reader's own caps. A panic a
//! reader catches itself (`solidcraft_kernel::guard`) counts too: the browser build aborts on any
//! panic, so a caught one on the desktop is a crash there.
//!
//! The mutations come from a fixed seed (xorshift), so a failure names a case that reproduces.
//! No fuzzing dependency, nothing nightly, nothing native: it runs in the normal test suite and
//! builds for wasm. The readers' own hand-written cases stay; they cover particular lies (a
//! length in a container header) that random changes rarely hit. STEP, IGES and FreeCAD/Rhino
//! files are read by the kernel, which is moving to solidcore (#1597), and keep their own tests.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

/// A deterministic xorshift64 stream.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A value in `0..n` (0 when `n` is 0).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// Numbers a hostile file might hold in place of a real one.
const LIARS: [&str; 10] = ["nan", "inf", "-inf", "1e400", "-1e400", "-0", "4294967296", "-1", "99999999999999999999", "0x7fffffff"];

/// Most truncations of one file (every length up to this, evenly spaced beyond).
const MAX_TRUNCATIONS: usize = 1024;

/// Mutations of `good`, the same for the same `good` and `seed`. `text`: also line and number
/// mutations (the file is UTF-8 text).
pub(crate) fn mutations(good: &[u8], text: bool, seed: u64) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let n = good.len();
    let mut rng = Rng::new(seed);
    // Truncations: every length of a short file; of a long one, its first and last 64 lengths
    // (headers, final records) and evenly spaced ones between.
    if n <= MAX_TRUNCATIONS {
        out.extend((0..n).map(|k| good[..k].to_vec()));
    } else {
        let mut cuts: Vec<usize> = (0..64).chain(n - 64..n).collect();
        cuts.extend((0..MAX_TRUNCATIONS - 128).map(|i| 64 + i * (n - 128) / (MAX_TRUNCATIONS - 128)));
        cuts.sort_unstable();
        cuts.dedup();
        out.extend(cuts.into_iter().map(|k| good[..k].to_vec()));
    }
    if n == 0 {
        return out;
    }
    // Bytes flipped, zeroed and saturated at seeded places.
    for i in 0..384 {
        let at = rng.below(n);
        let mut b = good.to_vec();
        b[at] = match i % 3 {
            0 => b[at] ^ ((rng.next() >> 32) as u8 | 1),
            1 => 0x00,
            _ => 0xff,
        };
        out.push(b);
    }
    // Runs of bytes dropped, repeated and swapped with their neighbour.
    for _ in 0..96 {
        let at = rng.below(n);
        let len = 1 + rng.below(64.min(n - at));
        let end = at + len;
        let mut dropped = good[..at].to_vec();
        dropped.extend_from_slice(&good[end..]);
        out.push(dropped);
        let mut repeated = good[..end].to_vec();
        repeated.extend_from_slice(&good[at..]);
        out.push(repeated);
        if end + len <= n {
            let mut swapped = good[..at].to_vec();
            swapped.extend_from_slice(&good[end..end + len]);
            swapped.extend_from_slice(&good[at..end]);
            swapped.extend_from_slice(&good[end + len..]);
            out.push(swapped);
        }
    }
    // The whole file: with garbage after it, twice over, reversed.
    let mut tail = good.to_vec();
    tail.extend((0..4096).map(|_| (rng.next() >> 40) as u8));
    out.push(tail);
    out.push([good, good].concat());
    out.push(good.iter().rev().copied().collect());
    if text && let Ok(s) = std::str::from_utf8(good) {
        out.extend(text_mutations(s, &mut rng));
    }
    out
}

/// A text file's lines dropped, repeated and swapped, and its numbers replaced by lies.
fn text_mutations(s: &str, rng: &mut Rng) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let lines: Vec<&str> = s.split_inclusive('\n').collect();
    let join = |v: &[&str]| v.concat().into_bytes();
    let picked: Vec<usize> = if lines.len() <= 200 { (0..lines.len()).collect() } else { (0..200).map(|_| rng.below(lines.len())).collect() };
    for &i in &picked {
        let mut v = lines.clone();
        v.remove(i);
        out.push(join(&v));
        let mut v = lines.clone();
        v.insert(i, lines[i]);
        out.push(join(&v));
        if i + 1 < lines.len() {
            let mut v = lines.clone();
            v.swap(i, i + 1);
            out.push(join(&v));
        }
    }
    // A line in place of another (a block that inserts itself, a count where a coordinate was).
    for _ in 0..200.min(lines.len() * lines.len()) {
        let (i, j) = (rng.below(lines.len()), rng.below(lines.len()));
        let mut v = lines.clone();
        v[i] = lines[j];
        out.push(join(&v));
    }
    // A word in place of another of the file's own words: a renamed block, a swapped keyword, a
    // block that now inserts itself. Every pair in a small file, a sample in a large one.
    let words = word_spans(s);
    let mut vocab: Vec<&str> = words.iter().map(|&(a, b)| &s[a..b]).collect();
    vocab.sort_unstable();
    vocab.dedup();
    let pairs = words.len().saturating_mul(vocab.len());
    let swap = |(a, b): (usize, usize), w: &str| format!("{}{w}{}", &s[..a], &s[b..]).into_bytes();
    if pairs <= 3000 {
        for &span in &words {
            for &w in vocab.iter().filter(|w| **w != &s[span.0..span.1]) {
                out.push(swap(span, w));
            }
        }
    } else {
        for _ in 0..400 {
            out.push(swap(words[rng.below(words.len())], vocab[rng.below(vocab.len())]));
        }
    }
    // Numbers: each of up to 200 of them, in turn, becomes one of the liars.
    let numbers = number_spans(s);
    let chosen: Vec<(usize, usize)> = if numbers.len() <= 200 { numbers } else { (0..200).map(|_| numbers[rng.below(numbers.len())]).collect() };
    for (k, (a, b)) in chosen.into_iter().enumerate() {
        let liar = LIARS[k % LIARS.len()];
        out.push(format!("{}{liar}{}", &s[..a], &s[b..]).into_bytes());
    }
    // Every number lying at once (two counts that are only harmless one at a time).
    let all = number_spans(s);
    for liar in LIARS {
        let mut t = String::with_capacity(s.len());
        let mut at = 0;
        for &(a, b) in &all {
            t.push_str(&s[at..a]);
            t.push_str(liar);
            at = b;
        }
        t.push_str(&s[at..]);
        out.push(t.into_bytes());
    }
    out
}

/// Byte spans of the words in `s` (runs of ASCII letters, digits and `_` that start with a
/// letter).
fn word_spans(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphabetic() || (i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            i += 1;
            continue;
        }
        let a = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        out.push((a, i));
    }
    out
}

/// Byte spans of the decimal numbers in `s` (a sign, digits, a point, an exponent).
fn number_spans(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit() || ((b[i] == b'-' || b[i] == b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit));
        let after_word = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if !starts || after_word {
            i += 1;
            continue;
        }
        let a = i;
        i += 1;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || ((b[i] == b'e' || b[i] == b'E') && i + 1 < b.len())) {
            if (b[i] == b'e' || b[i] == b'E') && (b[i + 1] == b'-' || b[i + 1] == b'+') {
                i += 1;
            }
            i += 1;
        }
        out.push((a, i));
    }
    out
}

thread_local! {
    /// Panics on this thread so far, caught or not ([`count_panics`]).
    static PANICS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Count every panic per thread (tests run on several), then run the hook that was there.
fn count_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PANICS.with(|p| p.set(p.get() + 1));
            previous(info);
        }));
    });
}

/// The time any one reader call may take on a mutated file (a debug build on a loaded machine:
/// generous, but far below what unbounded work on a lying count would take).
pub(crate) const BUDGET: Duration = Duration::from_secs(2);

/// Run `read` over `inputs`: no panic, within [`BUDGET`], and `read`'s count (triangles,
/// entities, …) at most `cap`. `read` returns `None` for an error. Panics (the test fails) naming
/// the first case that breaks a rule.
pub(crate) fn survives(what: &str, inputs: &[Vec<u8>], cap: usize, mut read: impl FnMut(&[u8]) -> Option<usize>) {
    count_panics();
    let mut slowest = (Duration::ZERO, 0);
    let mut read_ok = 0;
    for (i, input) in inputs.iter().enumerate() {
        let before = PANICS.with(std::cell::Cell::get);
        let t = Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| read(input)));
        let took = t.elapsed();
        let head = String::from_utf8_lossy(&input[..input.len().min(120)]).into_owned();
        let Ok(r) = r else { panic!("{what}: case {i} ({} bytes) panicked; it starts {head:?}", input.len()) };
        let caught = PANICS.with(std::cell::Cell::get) - before;
        assert!(
            caught == 0,
            "{what}: case {i} ({} bytes) panicked inside the reader (caught there, but the browser aborts); it starts {head:?}",
            input.len()
        );
        assert!(took <= BUDGET, "{what}: case {i} ({} bytes) took {took:?} (budget {BUDGET:?}); it starts {head:?}", input.len());
        if let Some(count) = r {
            assert!(count <= cap, "{what}: case {i} gave {count} (cap {cap})");
            read_ok += 1;
        }
        slowest = slowest.max((took, i));
    }
    eprintln!("{what}: {} mutations, {read_ok} read, slowest case {} in {:?}", inputs.len(), slowest.1, slowest.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_geom::{Mesh, Vec2, Vec3};

    fn cube() -> Mesh {
        let positions = (0..8).map(|i| Vec3::new(f64::from(i & 1) * 10.0, f64::from((i >> 1) & 1) * 10.0, f64::from((i >> 2) & 1) * 10.0)).collect();
        let t = [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        Mesh { positions, triangles: t.to_vec(), ..Default::default() }
    }

    #[test]
    fn mutations_are_deterministic_and_cover_the_file() {
        let good = b"solid x\n facet normal 0 0 1\n  vertex 1.5 -2 3e2\n endfacet\nendsolid x\n";
        let a = mutations(good, true, 7);
        assert_eq!(a, mutations(good, true, 7), "the same seed gives the same cases");
        assert_ne!(a, mutations(good, true, 8));
        assert!(a.iter().any(Vec::is_empty) && a.iter().any(|m| m.len() == good.len() - 1), "truncations down to nothing");
        assert!(a.iter().any(|m| m.windows(3).any(|w| w == b"nan")), "a number replaced by a liar");
        assert_eq!(number_spans("v 1.5 -2 3e2 x1 .5e-3"), vec![(2, 5), (6, 8), (9, 12), (16, 21)]);
        // A long file: bounded truncations, kept to the first and last bytes and spaced between.
        let long = vec![b'a'; 100_000];
        let m = mutations(&long, false, 1);
        assert!(m.len() < 3000, "{} cases", m.len());
    }

    #[test]
    fn stl_survives_mutation() {
        let mesh = cube();
        let binary = crate::stl_binary(std::slice::from_ref(&mesh), "fuzz");
        let ascii = crate::stl_ascii(std::slice::from_ref(&mesh), "fuzz").into_bytes();
        assert_eq!(crate::read_stl(&binary).map(|t| t.len()).ok(), Some(12));
        assert_eq!(crate::read_stl(&ascii).map(|t| t.len()).ok(), Some(12));
        let read = |b: &[u8]| crate::read_stl(b).ok().map(|t| t.len());
        survives("binary STL", &mutations(&binary, false, 0x5717), crate::MAX_STL_TRIANGLES, read);
        survives("ASCII STL", &mutations(&ascii, true, 0x5718), crate::MAX_STL_TRIANGLES, read);
    }

    #[test]
    fn obj_survives_mutation() {
        let good = crate::obj(&[("Cube".into(), cube())]).into_bytes();
        assert!(crate::read_obj(&good, "fuzz").is_ok());
        survives("OBJ", &mutations(&good, true, 0x0b1), usize::MAX, |b| {
            crate::read_obj(b, "fuzz").ok().map(|(objects, _)| objects.iter().map(|o| o.triangles.len()).sum())
        });
    }

    #[test]
    fn svg_survives_mutation() {
        let good = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">
<g transform="translate(10 5) rotate(15) scale(1.5)">
<rect x="1" y="2" width="30" height="4" rx="1"/>
<circle cx="20" cy="20" r="5"/>
<ellipse cx="40" cy="20" rx="6" ry="3"/>
<polyline points="0,0 10,10 20,0"/>
<path d="M0 0 L5 5 H10 V0 C12 2 14 2 16 0 Q18 3 20 0 A5 5 0 0 1 30 0 Z"/>
</g>
<use href="#a"/><g id="a"><line x1="0" y1="0" x2="5" y2="5"/></g>
</svg>
"##;
        assert!(crate::read_svg(std::str::from_utf8(good).unwrap()).is_ok());
        survives("SVG", &mutations(good, true, 0x5f6), usize::MAX, |b| crate::read_svg(&String::from_utf8_lossy(b)).ok().map(|g| g.len()));
    }

    #[test]
    fn dxf_survives_mutation() {
        let geom = [
            crate::Geom2::Line(Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)),
            crate::Geom2::Circle(Vec2::new(5.0, 5.0), 2.5),
            crate::Geom2::Arc { c: Vec2::new(0.0, 0.0), a: Vec2::new(3.0, 0.0), b: Vec2::new(0.0, 3.0) },
            crate::Geom2::Point(Vec2::new(1.0, 1.0)),
        ];
        let written = crate::write_dxf(&geom).into_bytes();
        assert!(crate::read_dxf(std::str::from_utf8(&written).unwrap()).is_ok_and(|g| g.len() >= 3));
        survives("DXF (written)", &mutations(&written, true, 0xdf), usize::MAX, |b| {
            crate::read_dxf(&String::from_utf8_lossy(b)).ok().map(|g| g.len())
        });
        // Blocks inside blocks, an array insert and a bulged polyline: what the block recursion,
        // the array cells and the work budget guard.
        let pairs: &[(&str, &str)] = &[
            ("0", "SECTION"),
            ("2", "BLOCKS"),
            ("0", "BLOCK"),
            ("2", "inner"),
            ("10", "0"),
            ("20", "0"),
            ("0", "LINE"),
            ("10", "0"),
            ("20", "0"),
            ("11", "1"),
            ("21", "1"),
            ("0", "ENDBLK"),
            ("0", "BLOCK"),
            ("2", "outer"),
            ("10", "0"),
            ("20", "0"),
            ("0", "INSERT"),
            ("2", "inner"),
            ("10", "2"),
            ("20", "2"),
            ("0", "CIRCLE"),
            ("10", "0"),
            ("20", "0"),
            ("40", "1"),
            ("0", "ENDBLK"),
            ("0", "ENDSEC"),
            ("0", "SECTION"),
            ("2", "ENTITIES"),
            ("0", "INSERT"),
            ("2", "outer"),
            ("10", "5"),
            ("20", "5"),
            ("41", "2"),
            ("42", "2"),
            ("50", "30"),
            ("70", "3"),
            ("71", "2"),
            ("44", "10"),
            ("45", "10"),
            ("0", "LWPOLYLINE"),
            ("90", "3"),
            ("70", "1"),
            ("10", "0"),
            ("20", "0"),
            ("42", "0.5"),
            ("10", "5"),
            ("20", "0"),
            ("10", "5"),
            ("20", "5"),
            ("0", "ENDSEC"),
            ("0", "EOF"),
        ];
        let blocks: String = pairs
            .iter()
            .map(|(c, v)| {
                format!(
                    "{c}
{v}
"
                )
            })
            .collect();
        let n = crate::read_dxf(&blocks).map(|g| g.len()).unwrap_or(0);
        assert!(n >= 12, "the array insert expands its nested blocks: {n} entities");
        survives("DXF (blocks)", &mutations(blocks.as_bytes(), true, 0xdb), usize::MAX, |b| {
            crate::read_dxf(&String::from_utf8_lossy(b)).ok().map(|g| g.len())
        });
        // The two lies that need each other, written out: a block that inserts itself (the
        // depth limit), and an array of 10,000 by 10,000 copies (the cell and work budgets).
        let looped = blocks.replacen(
            "2
inner
10
2",
            "2
outer
10
2",
            1,
        );
        assert_ne!(looped, blocks, "the outer block now inserts itself");
        let huge = blocks.replacen(
            "70
3
71
2",
            "70
10000
71
10000",
            1,
        );
        assert_ne!(huge, blocks);
        survives("DXF (self-insert, huge array)", &[looped.into_bytes(), huge.into_bytes()], usize::MAX, |b| {
            crate::read_dxf(&String::from_utf8_lossy(b)).ok().map(|g| g.len())
        });
    }

    #[test]
    fn threemf_survives_mutation() {
        let mesh = cube();
        let object = crate::MeshObject {
            name: "Cube".into(),
            positions: mesh.positions,
            triangles: mesh.triangles,
            color: Some([0.8, 0.2, 0.1]),
            ..Default::default()
        };
        let good = crate::write_3mf(&[object]).unwrap();
        assert!(crate::read_3mf(&good).is_ok());
        survives("3MF", &mutations(&good, false, 0x3f), usize::MAX, |b| {
            crate::read_3mf(b).ok().map(|(objects, _)| objects.iter().map(|o| o.triangles.len()).sum())
        });
    }

    #[test]
    fn gltf_survives_mutation() {
        let mut f = crate::gltf_in::tests::Fixture::default();
        let m = f.cube_mesh("Cube", 0.01);
        let child = f.node(serde_json::json!({"name": "C", "mesh": m, "translation": [0.0, 0.1, 0.0]}));
        let n = f.node(serde_json::json!({"mesh": m, "children": [child], "rotation": [0.0, 0.0, std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2]}));
        f.roots = vec![n];
        let mut no_files = |name: &str| -> std::result::Result<Vec<u8>, String> { Err(format!("no file `{name}`")) };
        let glb = f.glb();
        assert!(crate::read_gltf(&glb, "fuzz.glb", &mut no_files).is_ok());
        survives("glTF (.glb)", &mutations(&glb, false, 0x91b), usize::MAX, |b| crate::read_gltf(b, "fuzz.glb", &mut no_files).ok().map(|_| 1));
        // The JSON form, its buffer embedded as a data URI.
        let uri = format!("data:application/octet-stream;base64,{}", base64(&f.bin));
        let gltf = f.json(Some(&uri)).to_string().into_bytes();
        assert!(crate::read_gltf(&gltf, "fuzz.gltf", &mut no_files).is_ok());
        survives("glTF (.gltf)", &mutations(&gltf, true, 0x917), usize::MAX, |b| crate::read_gltf(b, "fuzz.gltf", &mut no_files).ok().map(|_| 1));
    }

    #[test]
    fn designs_survive_mutation() {
        let mut doc = solidcraft_doc::Document::new("Fuzz");
        doc.set_param("width", "40", Some("mm"), None).unwrap();
        let mesh = solidcraft_doc::MeshData {
            name: "Preview".into(),
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            triangles: vec![0, 1, 2],
            color: None,
        };
        let preview = crate::DesignPreview { meshes: vec![mesh] };
        let container = crate::write_design_container_with_preview(&doc, crate::DesignCompression::Fast, Some(&preview)).unwrap();
        assert!(crate::read_design(&container).is_ok() && crate::read_design_preview(&container).is_some());
        survives("design container", &mutations(&container, false, 0xde5), usize::MAX, |b| {
            let _ = crate::read_design_preview(b);
            let _ = crate::read_design_thumbnail(b);
            crate::read_design(b).ok().map(|d| d.features.len())
        });
        let json = crate::write_design(&doc);
        assert!(crate::read_design(&json).is_ok());
        survives("design JSON", &mutations(&json, true, 0xde6), usize::MAX, |b| crate::read_design(b).ok().map(|d| d.features.len()));
    }

    /// Standard base64 (RFC 4648) of `bytes`, for a data URI.
    fn base64(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in bytes.chunks(3) {
            let v = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            for k in 0..4 {
                if k <= c.len() {
                    s.push(char::from(A[(v >> (18 - 6 * k)) as usize & 63]));
                } else {
                    s.push('=');
                }
            }
        }
        s
    }
}
