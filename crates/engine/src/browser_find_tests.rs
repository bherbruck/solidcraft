//! Finding things in the browser by name: `browser_find` and `browser.find` (#1413).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::browser_find::{Kind, MAX_PATTERN_CHARS, Pattern, find, find_within};
use crate::{Sel, Session};

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn pattern(text: &str) -> Pattern {
    Pattern::new(text).unwrap_or_else(|| panic!("`{text}` is a pattern"))
}

/// A base plate with a sketch and a construction plane; two columns, each with two M20 bolts;
/// two more bolts and a washer on the plate, the washer held by a joint.
fn assembly() -> Session {
    let mut s = Session::default();
    let script = json!({"commands": [
        {"command": "solid.box", "params": {"length": 420, "width": 120, "height": 10, "corner": [-60, -40, -10], "body_name": "Base plate"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Bolt holes"}},
        {"command": "sketch.circle.center", "params": {"center": [40, 20], "diameter": 22}},
        {"command": "sketch.finish", "params": {}},
        {"command": "construct.plane.offset", "params": {"base": "XY", "offset": 150, "name": "Bolt line"}},
        {"command": "component.create", "params": {"name": "Column"}},
        {"command": "solid.box", "params": {"length": 40, "width": 40, "height": 300, "corner": [0, 0, 0], "body_name": "Column web"}},
        {"command": "component.create", "params": {"name": "Bolt M20", "parent": "Column"}},
        {"command": "solid.cylinder", "params": {"radius": 10, "height": 80, "base": [20, -20, 80], "axis": "Y", "body_name": "Bolt shank"}},
        {"command": "component.activate", "params": {"component": "Column"}},
        {"command": "occurrence.copy", "params": {"component": "Bolt M20", "translate": [0, 0, 120]}},
        {"command": "component.activate", "params": {"component": "root"}},
        {"command": "occurrence.copy", "params": {"component": "Column", "translate": [300, 0, 0]}},
        {"command": "occurrence.copy", "params": {"component": "Bolt M20", "parent": "root", "translate": [150, 0, -60]}},
        {"command": "occurrence.copy", "params": {"component": "Bolt M20", "parent": "root", "translate": [190, 0, -60]}},
        {"command": "component.create", "params": {"name": "Washer M20"}},
        {"command": "solid.cylinder", "params": {"radius": 18, "height": 3, "base": [20, -23, 80], "axis": "Y", "body_name": "Washer"}},
        {"command": "component.activate", "params": {"component": "root"}}
    ]});
    if let Err(e) = s.run_script(&script) {
        panic!("assembly: {e}");
    }
    let washer = occurrence(&s, 0, "Washer M20:1");
    let bolt = occurrence(&s, 0, "Bolt M20:3");
    run(&mut s, "joint.as_built", json!({"type": "rigid", "a": bolt, "b": washer, "name": "Washer seat"}));
    s
}

/// The occurrence called `name` placed in `parent`.
fn occurrence(s: &Session, parent: u64, name: &str) -> u64 {
    s.doc.occurrences.iter().find(|o| o.parent == parent && o.name == name).map(|o| o.id).unwrap_or_else(|| panic!("no {name} in {parent}"))
}

fn names(s: &Session, text: &str, kinds: &[Kind]) -> Vec<(Kind, String, Vec<u64>)> {
    find(s, Pattern::new(text).as_ref(), kinds).found.into_iter().map(|f| (f.kind, f.name, f.path)).collect()
}

#[test]
fn plain_text_is_found_anywhere_in_a_name_ignoring_case() {
    let p = pattern("  bOlT m2 ");
    assert!(p.matches("Bolt M20:3"));
    assert!(p.matches("Long BOLT M2 thread"));
    assert!(!p.matches("Bolt  M20"));
    assert!(!p.matches("Bol"));
    // Non-ASCII letters fold too.
    assert!(pattern("ÉCROU").matches("petit écrou"));
    assert!(pattern("болт").matches("БОЛТ М20"));
}

#[test]
fn wildcards_match_the_whole_name() {
    let p = pattern("*:1");
    assert!(p.matches("Column:1"));
    assert!(!p.matches("Column:12"));
    assert!(!p.matches("Column:1 copy"));
    let p = pattern("?olumn*");
    assert!(p.matches("Column:2"));
    assert!(p.matches("column"));
    assert!(!p.matches("A column"));
    assert!(pattern("b?lt m20:?").matches("Bolt M20:4"));
    assert!(!pattern("b?lt m20:?").matches("Bolt M20:40"));
    assert!(pattern("*").matches(""));
    assert!(pattern("**a***").matches("banana"));
    assert!(!pattern("?").matches(""));
    assert!(pattern("?").matches("é"));
}

#[test]
fn blank_and_overlong_patterns_are_no_search() {
    assert_eq!(Pattern::new(""), None);
    assert_eq!(Pattern::new(" \t\n "), None);
    assert!(Pattern::new(&"a".repeat(MAX_PATTERN_CHARS)).is_some());
    assert_eq!(Pattern::new(&"a".repeat(MAX_PATTERN_CHARS + 1)), None);
    // Characters, not bytes.
    assert!(Pattern::new(&"é".repeat(MAX_PATTERN_CHARS)).is_some());
}

/// `*` and `?` by a table of every prefix pair: slow, and plainly right.
fn by_table(glob: &[char], name: &[char]) -> bool {
    let mut m = vec![vec![false; name.len() + 1]; glob.len() + 1];
    m[0][0] = true;
    for i in 1..=glob.len() {
        if glob[i - 1] == '*' {
            m[i][0] = m[i - 1][0];
        }
        for j in 1..=name.len() {
            m[i][j] = match glob[i - 1] {
                '*' => m[i - 1][j] || m[i][j - 1],
                '?' => m[i - 1][j - 1],
                c => m[i - 1][j - 1] && c == name[j - 1],
            };
        }
    }
    m[glob.len()][name.len()]
}

#[test]
fn the_matcher_agrees_with_a_table_on_random_patterns() {
    const ALPHABET: [char; 9] = ['a', 'b', 'A', 'B', '*', '?', 'é', 'É', ':'];
    const NAME: [char; 7] = ['a', 'b', 'A', 'B', 'é', 'É', ':'];
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let mut wild = 0;
    for _ in 0..20_000 {
        let text: String = (0..1 + next(8)).map(|_| ALPHABET[next(ALPHABET.len())]).collect();
        let name: String = (0..next(12)).map(|_| NAME[next(NAME.len())]).collect();
        let Some(p) = Pattern::new(&text) else { continue };
        let (text, lower) = (text.to_lowercase(), name.to_lowercase());
        let expected = if text.contains(['*', '?']) {
            wild += 1;
            by_table(&text.chars().collect::<Vec<_>>(), &lower.chars().collect::<Vec<_>>())
        } else {
            lower.contains(&text)
        };
        assert_eq!(p.matches(&name), expected, "`{text}` against `{name}`");
    }
    assert!(wild > 5_000, "the wildcards were tried ({wild})");
}

#[test]
fn hostile_patterns_and_names_finish_quickly() {
    let started = Instant::now();
    let long = "a".repeat(1_000_000);
    for text in ["*a".repeat(100) + "b", "?".repeat(MAX_PATTERN_CHARS), "*".repeat(MAX_PATTERN_CHARS), "a*".repeat(128), "\0*\u{FFFF}?".into()] {
        let p = pattern(&text);
        let _ = p.matches(&long);
        let _ = p.matches("");
        let _ = p.matches("\0\u{10FFFF}");
    }
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn every_placement_is_its_own_match_with_its_path() {
    let s = assembly();
    let (c1, c2) = (occurrence(&s, 0, "Column:1"), occurrence(&s, 0, "Column:2"));
    let column = s.doc.find_component("Column").unwrap();
    let (b1, b2) = (occurrence(&s, column, "Bolt M20:1"), occurrence(&s, column, "Bolt M20:2"));
    let (b3, b4, w) = (occurrence(&s, 0, "Bolt M20:3"), occurrence(&s, 0, "Bolt M20:4"), occurrence(&s, 0, "Washer M20:1"));
    let c = Kind::Component;
    assert_eq!(
        names(&s, "m20", &[]),
        [
            (c, "Bolt M20:1".into(), vec![c1, b1]),
            (c, "Bolt M20:2".into(), vec![c1, b2]),
            (c, "Bolt M20:1".into(), vec![c2, b1]),
            (c, "Bolt M20:2".into(), vec![c2, b2]),
            (c, "Bolt M20:3".into(), vec![b3]),
            (c, "Bolt M20:4".into(), vec![b4]),
            (c, "Washer M20:1".into(), vec![w]),
        ]
    );
    // Each bolt's shank is a body of its own in the world: six of them, each with its path.
    let shanks = find(&s, Some(&pattern("shank")), &[]).found;
    assert_eq!(shanks.len(), 6);
    let mut worlds: Vec<String> =
        shanks.iter().filter_map(|f| if let Some(Sel::Body { name }) = &f.item { Some(name.clone()) } else { None }).collect();
    worlds.sort();
    worlds.dedup();
    assert_eq!(worlds.len(), 6, "{worlds:?}");
    assert!(shanks.iter().all(|f| f.key == "Bolt shank" && f.name == "Bolt shank"));
    assert_eq!(
        shanks.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
        [vec![c1, b1], vec![c1, b2], vec![c2, b1], vec![c2, b2], vec![b3], vec![b4]]
    );
    // The design's own things, in folder order, then the joint.
    let bolt = names(&s, "bolt", &[Kind::Sketch, Kind::Construction, Kind::Joint]);
    assert_eq!(bolt, [(Kind::Sketch, "Bolt holes".into(), vec![]), (Kind::Construction, "Bolt line".into(), vec![])]);
    let seat = find(&s, Some(&pattern("seat")), &[]);
    assert_eq!(seat.total, 1);
    let joint = &seat.found[0];
    assert_eq!((joint.kind, joint.name.as_str(), joint.item.as_ref()), (Kind::Joint, "Washer seat", None));
    assert_eq!(joint.key, s.doc.assembly.joints[0].id.to_string());
    // Wildcards over the whole name.
    assert_eq!(names(&s, "*:1", &[]).iter().map(|f| f.1.as_str()).collect::<Vec<_>>(), ["Column:1", "Bolt M20:1", "Bolt M20:1", "Washer M20:1"]);
    assert_eq!(find(&s, Some(&pattern("nothing like it")), &[]), Default::default());
}

#[test]
fn kinds_narrow_a_search_and_alone_list_everything_of_a_kind() {
    let s = assembly();
    assert_eq!(find(&s, Some(&pattern("bolt")), &[]).total, 6 + 6 + 2);
    assert_eq!(find(&s, Some(&pattern("bolt")), &[Kind::Component]).total, 6);
    assert_eq!(find(&s, Some(&pattern("bolt")), &[Kind::Body, Kind::Sketch]).total, 7);
    let bodies = find(&s, None, &[Kind::Body]);
    // The plate, a web in each column, a shank in each bolt, the washer.
    assert_eq!(bodies.total, 1 + 2 + 6 + 1);
    assert!(bodies.found.iter().all(|f| f.kind == Kind::Body));
    assert_eq!(find(&s, None, &[Kind::Component]).total, 2 + 4 + 3);
    assert_eq!(find(&s, None, &Kind::ALL).total, find(&s, Some(&pattern("*")), &[]).total);
}

#[test]
fn what_it_finds_selects() {
    let mut s = assembly();
    let found = run(&mut s, "browser.find", json!({"text": "m20:1"}));
    assert_eq!(found["total"], 3);
    assert_eq!(found["incomplete"], false);
    let items: Vec<Value> = found["found"].as_array().unwrap().iter().map(|f| f["item"].clone()).collect();
    assert!(items.iter().all(|i| i["type"] == "occurrence"));
    run(&mut s, "select.set", json!({"items": items}));
    assert_eq!(s.selection.len(), 3);
    assert!(s.selection.iter().all(|x| matches!(x, Sel::Occurrence { .. })));
    let shank = run(&mut s, "browser.find", json!({"text": "shank", "kinds": ["body"]}));
    run(&mut s, "select.set", json!({"items": shank["found"].as_array().unwrap().iter().map(|f| f["item"].clone()).collect::<Vec<_>>()}));
    assert_eq!(s.selection.len(), 6);
    // A query: no undo step, nothing changed.
    let undo = s.undo.len();
    let doc = s.doc.clone();
    run(&mut s, "browser.find", json!({"kinds": ["joint"]}));
    assert_eq!(s.undo.len(), undo);
    assert!(Arc::ptr_eq(&doc, &s.doc));
}

#[test]
fn listing_and_placements_are_capped_and_counted() {
    let s = assembly();
    let capped = find_within(&s, Some(&pattern("m20")), &[], 3, usize::MAX);
    assert_eq!((capped.found.len(), capped.total, capped.incomplete), (3, 7, false));
    assert_eq!(capped.found, find(&s, Some(&pattern("m20")), &[]).found[..3]);
    // Out of placements to look through: the search says it may have missed some.
    let short = find_within(&s, Some(&pattern("m20")), &[], usize::MAX, 2);
    assert!(short.incomplete);
    assert!(short.total < 7);
    assert!(!find_within(&s, Some(&pattern("m20")), &[], usize::MAX, 9).incomplete);
}

#[test]
fn a_component_placed_inside_itself_ends() {
    let mut s = assembly();
    let column = s.doc.find_component("Column").unwrap();
    let doc = Arc::make_mut(&mut s.doc);
    let mut inside = doc.occurrences.iter().find(|o| o.component == column).unwrap().clone();
    inside.id = 9_000;
    inside.parent = column;
    inside.name = "Column:3".into();
    doc.occurrences.push(inside);
    let started = Instant::now();
    let search = find(&s, Some(&pattern("column")), &[]);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(search.total >= 2);
    assert!(search.found.iter().all(|f| f.path.len() <= 64));
}

#[test]
fn browser_find_checks_its_params() {
    let mut s = assembly();
    for p in [
        json!({}),
        json!({"text": "   "}),
        json!({"text": 5}),
        json!({"text": ["bolt"]}),
        json!({"text": "a".repeat(MAX_PATTERN_CHARS + 1)}),
        json!({"kinds": "body"}),
        json!({"kinds": ["bodies"]}),
        json!({"kinds": [1]}),
        json!({"kinds": ["body", "body", "body", "body", "body", "body"]}),
        json!({"text": null, "kinds": null}),
        json!({"text": "bolt", "kinds": [null]}),
    ] {
        assert!(s.execute("browser.find", &p).is_err(), "{p}");
    }
    assert_eq!(run(&mut s, "browser.find", json!({"text": "   ", "kinds": ["sketch"]}))["total"], 1);
    assert_eq!(run(&mut s, "browser.find", json!({"text": "\0\u{FFFF}*?"}))["total"], 0);
    let found = run(&mut s, "browser.find", json!({"text": "washer", "kinds": ["component", "body", "joint"]}));
    assert_eq!(found["total"], 3);
    // The design's joints come before its placements, as in the browser.
    assert_eq!(found["found"][0]["kind"], "joint");
    assert_eq!(found["found"][1]["kind"], "component");
    assert_eq!(found["found"][1]["path"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["found"][2]["kind"], "body");
}

/// While a sketch is edited the timeline is rolled back to it, and a body a later feature removes
/// is in the model again but not in the browser (#1167): the search doesn't find it either.
#[test]
fn a_body_removed_later_is_not_found_while_a_sketch_is_edited() {
    let mut s = Session::default();
    for (name, x) in [("Kept block", 0), ("Gone block", 20)] {
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": format!("{name} sketch")}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [x, 0], "p1": [x + 10, 10]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": format!("{name} sketch"), "distance": 5, "body_name": name}));
    }
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": "30 mm", "name": "Edited"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [5, 5]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.remove", json!({"bodies": ["Gone block"]}));
    run(&mut s, "timeline.roll_to", json!({"feature": "Edited"}));
    run(&mut s, "sketch.edit", json!({"sketch": "Edited"}));
    assert!(s.model.state().body("Gone block").is_some() && s.body_removed_later("Gone block"));
    let found = find(&s, Some(&pattern("block")), &[Kind::Body]).found;
    assert_eq!(found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Kept block"]);
}
