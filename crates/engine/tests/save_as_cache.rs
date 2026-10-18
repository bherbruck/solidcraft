//! Optional save caches survive a filename-driven rename, without warming or
//! certifying placements that do not match the document being saved.
use serde_json::json;
use solidcraft_doc::Document;
use solidcraft_engine::Session;

fn assembly() -> Session {
    let mut session = Session::new(Document::new("Untitled"));
    session.execute("component.create", &json!({"name":"PlacedPart"})).unwrap();
    session.execute("solid.box", &json!({"length":2,"width":3,"height":4})).unwrap();
    session
}

fn warm(session: &Session) {
    for body in &session.world_state().bodies {
        body.mesh();
    }
}

fn save(session: &mut Session, label: &str) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("solidcraft-save-as-{label}-{}.solidcraft", std::process::id()));
    session.execute("file.save_as", &json!({"path":path,"cache":true,"preview":true})).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(solidcraft_io::read_design(&bytes).unwrap().name, path.file_stem().unwrap().to_string_lossy());
    std::fs::remove_file(&path).unwrap();
    let _ = std::fs::remove_file(format!("{}.bak", path.display()));
    bytes
}

#[test]
fn a_filename_rename_preserves_opted_in_warm_assembly_caches() {
    let mut session = assembly();
    warm(&session);
    for label in ["first-warm", "renamed-warm"] {
        let bytes = save(&mut session, label);
        let geometry = solidcraft_io::read_design_geometry(&bytes).expect("warm geometry survives the rename");
        assert_eq!(geometry.len(), 1);
        assert!((solidcraft_kernel::measure(&geometry[0].body).unwrap().volume - 24.0).abs() < 1e-9);
        assert!(solidcraft_io::read_design_preview(&bytes).is_some(), "warm display meshes survive the rename");
        warm(&session);
    }
}

#[test]
fn naming_a_design_does_not_warm_cold_or_revalidate_stale_placements() {
    let original = assembly();
    let mut cold = Session::new((*original.doc).clone());
    let bytes = save(&mut cold, "cold");
    assert!(solidcraft_io::read_design_geometry(&bytes).is_none());
    assert!(solidcraft_io::read_design_preview(&bytes).is_none());

    let mut stale = assembly();
    warm(&stale);
    stale.doc_mut().occurrences.first_mut().unwrap().transform[3][0] = 25.0;
    let bytes = save(&mut stale, "stale");
    assert!(solidcraft_io::read_design_geometry(&bytes).is_none(), "old placements must not be certified for a changed document");
    assert!(solidcraft_io::read_design_preview(&bytes).is_none());
}
