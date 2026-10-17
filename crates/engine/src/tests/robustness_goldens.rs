//! Seeded goldens: what every robustness seed builds (`tests/robustness/goldens.json`).
//!
//! Each seed's random design (`fuzz::run`) is recorded: its steps, how it ended (ok, or the
//! kind and start of the error it was refused with) and, for what it left, the bodies' total
//! volume and area (within a relative 1e-6) and their face, edge and shell counts (exact). Any
//! change fails, the better and the worse alike: a seed that used to be refused and now builds
//! is recorded on purpose, with the reason. To record: `ROBUSTNESS_GOLDENS_BLESS="why" cargo
//! test --profile ci -p solidcraft-engine --lib robustness_goldens::` (docs/robustness.md); run
//! it with `cargo test`, whose tests share one process and so one lock on the file, not with
//! nextest. The reason goes into the file's `notes` with the seeds it changed.
//!
//! Each seed is its own test (`seed_00` … `seed_95`), so the runner spreads them over the CI
//! shards and their cores: a few seeds take minutes, and grouped four ways they made one test
//! of half an hour. Pull requests that touch `crates/kernel` run every 8th seed (ci.yml);
//! pushes to main and the nightly run every seed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::fuzz;

/// Seeds 0..SEEDS (about ten minutes for all of them on one core), each with up to this many steps (as `fixed_seeds_run_clean`).
const SEEDS: u64 = 96;
const MAX_STEPS: usize = 8;
const REL: f64 = 1e-6;

/// Tests share the file when they record.
static FILE: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/robustness/goldens.json")
}

fn load() -> Value {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({}))
}

/// How `got` differs from `want` (None when it matches within the tolerances).
fn differs(want: &Value, got: &Value) -> Option<String> {
    let mut out = Vec::new();
    for (k, g) in got.as_object().into_iter().flatten() {
        let w = &want[k];
        let same = match (w.as_f64(), g.as_f64()) {
            (Some(a), Some(b)) if matches!(k.as_str(), "volume" | "area") => (a - b).abs() <= REL * a.abs().max(b.abs()).max(1.0),
            _ => w == g,
        };
        if !same {
            out.push(format!("{k}: {w} -> {g}"));
        }
    }
    (!out.is_empty()).then(|| out.join("; "))
}

fn check(seed: u64) {
    let got: BTreeMap<u64, Value> = BTreeMap::from([(seed, fuzz::golden(seed, MAX_STEPS))]);
    let _lock = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = load();
    let changed: Vec<(u64, String)> = got
        .iter()
        .filter_map(|(seed, g)| {
            let w = &file["seeds"][seed.to_string()];
            if w.is_null() { Some((*seed, "new".to_string())) } else { differs(w, g).map(|d| (*seed, d)) }
        })
        .collect();
    if let Ok(reason) = std::env::var("ROBUSTNESS_GOLDENS_BLESS") {
        assert!(!reason.trim().is_empty(), "ROBUSTNESS_GOLDENS_BLESS needs the reason for the change");
        if changed.is_empty() {
            return;
        }
        if file.get("about").is_none() {
            file["about"] = json!(
                "What each robustness seed builds (crates/engine/src/tests/robustness_goldens.rs, docs/robustness.md). Record changes with ROBUSTNESS_GOLDENS_BLESS=\"why\"."
            );
        }
        for (seed, g) in &got {
            file["seeds"][seed.to_string()] = g.clone();
        }
        let mut notes = file["notes"].as_array().cloned().unwrap_or_default();
        let seeds: Vec<u64> = changed.iter().map(|(s, _)| *s).collect();
        // The parts of one record (the tests run side by side) share one note.
        match notes.last_mut() {
            Some(last) if last["reason"] == json!(reason) => {
                let mut all: Vec<u64> = last["seeds"].as_array().into_iter().flatten().filter_map(Value::as_u64).chain(seeds).collect();
                all.sort_unstable();
                all.dedup();
                last["seeds"] = json!(all);
            }
            _ => notes.push(json!({"reason": reason, "seeds": seeds})),
        }
        file["notes"] = Value::Array(notes);
        // Seeds in order, one per line, so a change is a small diff.
        let seeds: BTreeMap<u64, Value> =
            file["seeds"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.parse::<u64>().ok()?, v.clone()))).collect();
        let mut s = format!("{{\n \"about\": {},\n \"seeds\": {{\n", file["about"]);
        let n = seeds.len();
        for (i, (k, v)) in seeds.iter().enumerate() {
            s += &format!("  \"{k}\": {}{}\n", v, if i + 1 < n { "," } else { "" });
        }
        let notes: Vec<String> = file["notes"].as_array().into_iter().flatten().map(|n| format!("  {n}")).collect();
        s += &format!(" }},\n \"notes\": [\n{}\n ]\n}}\n", notes.join(",\n"));
        std::fs::write(path(), s).unwrap_or_else(|e| panic!("{}: {e}", path().display()));
        return;
    }
    assert!(
        changed.is_empty(),
        "{} seed(s) build differently from tests/robustness/goldens.json (if that is intended, record it with ROBUSTNESS_GOLDENS_BLESS=\"why\" and say why in the pull request):\n{}",
        changed.len(),
        changed.iter().map(|(s, d)| format!("seed {s}: {d}")).collect::<Vec<_>>().join("\n")
    );
}

/// One test per seed, and the list of them.
macro_rules! seeds {
    ($($name:ident = $seed:literal),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                check($seed);
            }
        )*
        const LISTED: &[u64] = &[$($seed),*];
    };
}

seeds!(
    seed_00 = 0,
    seed_01 = 1,
    seed_02 = 2,
    seed_03 = 3,
    seed_04 = 4,
    seed_05 = 5,
    seed_06 = 6,
    seed_07 = 7,
    seed_08 = 8,
    seed_09 = 9,
    seed_10 = 10,
    seed_11 = 11,
    seed_12 = 12,
    seed_13 = 13,
    seed_14 = 14,
    seed_15 = 15,
    seed_16 = 16,
    seed_17 = 17,
    seed_18 = 18,
    seed_19 = 19,
    seed_20 = 20,
    seed_21 = 21,
    seed_22 = 22,
    seed_23 = 23,
    seed_24 = 24,
    seed_25 = 25,
    seed_26 = 26,
    seed_27 = 27,
    seed_28 = 28,
    seed_29 = 29,
    seed_30 = 30,
    seed_31 = 31,
    seed_32 = 32,
    seed_33 = 33,
    seed_34 = 34,
    seed_35 = 35,
    seed_36 = 36,
    seed_37 = 37,
    seed_38 = 38,
    seed_39 = 39,
    seed_40 = 40,
    seed_41 = 41,
    seed_42 = 42,
    seed_43 = 43,
    seed_44 = 44,
    seed_45 = 45,
    seed_46 = 46,
    seed_47 = 47,
    seed_48 = 48,
    seed_49 = 49,
    seed_50 = 50,
    seed_51 = 51,
    seed_52 = 52,
    seed_53 = 53,
    seed_54 = 54,
    seed_55 = 55,
    seed_56 = 56,
    seed_57 = 57,
    seed_58 = 58,
    seed_59 = 59,
    seed_60 = 60,
    seed_61 = 61,
    seed_62 = 62,
    seed_63 = 63,
    seed_64 = 64,
    seed_65 = 65,
    seed_66 = 66,
    seed_67 = 67,
    seed_68 = 68,
    seed_69 = 69,
    seed_70 = 70,
    seed_71 = 71,
    seed_72 = 72,
    seed_73 = 73,
    seed_74 = 74,
    seed_75 = 75,
    seed_76 = 76,
    seed_77 = 77,
    seed_78 = 78,
    seed_79 = 79,
    seed_80 = 80,
    seed_81 = 81,
    seed_82 = 82,
    seed_83 = 83,
    seed_84 = 84,
    seed_85 = 85,
    seed_86 = 86,
    seed_87 = 87,
    seed_88 = 88,
    seed_89 = 89,
    seed_90 = 90,
    seed_91 = 91,
    seed_92 = 92,
    seed_93 = 93,
    seed_94 = 94,
    seed_95 = 95,
);

/// Every seed in 0..SEEDS has its test, once, and every recorded seed is one of them.
#[test]
fn every_seed_has_a_test() {
    assert_eq!(LISTED, (0..SEEDS).collect::<Vec<_>>().as_slice());
    let file = load();
    let recorded: Vec<u64> = file["seeds"].as_object().into_iter().flatten().filter_map(|(k, _)| k.parse().ok()).collect();
    assert!(recorded.iter().all(|s| *s < SEEDS), "{recorded:?}");
}
