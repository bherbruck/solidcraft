//! Commands run for an agent (#205): with a `vfs::PathScope` active, every command that reads,
//! writes or deletes a file it was given a path for stays inside the scope's folders.

use serde_json::json;
use solidcraft_io::vfs::{PathScope, scoped};

use crate::*;

#[test]
fn file_commands_stay_inside_the_agents_folders() {
    let base = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-path-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, out) = (base.join("root"), base.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let at = |d: &std::path::Path, f: &str| d.join(f).to_string_lossy().to_string();
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
    std::fs::write(out.join("part.solidcraft"), solidcraft_io::write_design(&s.doc)).unwrap();
    std::fs::write(out.join("p.csv"), "name,expression\nw,5 mm\n").unwrap();

    scoped(Some(PathScope::new([root.clone()])), || {
        // Inside: the usual workflow.
        s.execute("file.save_as", &json!({"path": at(&root, "part.solidcraft")})).unwrap();
        s.execute("file.export", &json!({"path": at(&root, "part.step")})).unwrap();
        s.execute("doc.open", &json!({"path": at(&root, "part.solidcraft")})).unwrap();
        // Outside, directly or through `..`: refused, with the reason.
        for (cmd, p) in [
            ("file.save_as", json!({"path": at(&out, "x.solidcraft")})),
            ("file.save", json!({"path": at(&root, "../out/x.solidcraft")})),
            ("file.export", json!({"path": at(&out, "x.stl")})),
            ("file.export_view_dxf", json!({"path": at(&out, "x.dxf"),"camera":render::Camera::default()})),
            ("inspect.section_curves", json!({"path": at(&out, "x.svg"), "plane": "XY", "at": 5})),
            ("file.export", json!({"path": at(&out, "x.json"), "format": "json"})),
            ("file.save_as", json!({"path": at(&out, "x.json"), "format": "json", "preview": true})),
            ("parameters.export", json!({"path": at(&out, "x.csv")})),
            ("doc.open", json!({"path": at(&out, "part.solidcraft")})),
            ("parameters.import", json!({"path": at(&out, "p.csv")})),
            ("sketch.insert_dxf", json!({"path": at(&out, "a.dxf")})),
            ("doc.recovery_list", json!({"dir": out.to_string_lossy()})),
            ("doc.recovery_discard", json!({"dir": out.to_string_lossy(), "id": "part"})),
            ("doc.recover", json!({"dir": out.to_string_lossy(), "id": "part", "discard": true})),
        ] {
            let e = s.execute(cmd, &p).unwrap_err().to_string();
            assert!(e.contains("outside the folders this agent may use"), "{cmd}: {e}");
        }
        // A font file named by path is a file read too; the built-in font still works.
        s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
        let e = s.execute("sketch.text", &json!({"text": "A", "at": [0, 0], "font": at(&out, "f.ttf")})).unwrap_err().to_string();
        assert!(e.contains("outside the folders this agent may use"), "sketch.text: {e}");
        s.execute("sketch.text", &json!({"text": "A", "at": [0, 0]})).unwrap();
        s.execute("sketch.finish", &json!({})).unwrap();
        // Saving follows a link to its file (#845), so a link that leads out is refused.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(out.join("part.solidcraft"), root.join("linked.solidcraft")).unwrap();
            let e = s.execute("file.save_as", &json!({"path": at(&root, "linked.solidcraft")})).unwrap_err().to_string();
            assert!(e.contains("outside the folders this agent may use"), "save through a link: {e}");
        }
        // The default recovery folder isn't one of the agent's folders either.
        assert!(s.execute("doc.recovery_list", &json!({})).is_err());
    });
    let left: Vec<_> = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left.len(), 2, "nothing written or deleted outside: {left:?}");
    // Without a scope (the user's own dialogs) nothing changes.
    s.execute("file.export", &json!({"path": at(&out, "x.stl")})).unwrap();
    let _ = std::fs::remove_dir_all(&base);
}

/// A design names the font file of its text, and opening it reads that file: for an agent, a
/// font outside its folders stays unread, even one already read (and cached) for the user.
#[test]
fn a_designs_font_file_stays_inside_the_agents_folders() {
    let base = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-font-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, out) = (base.join("root"), base.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let font = out.join("f.ttf");
    std::fs::write(&font, solidcraft_sketch::font_data(None).unwrap().bytes()).unwrap();
    let mut s = Session::default();
    s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
    s.execute("sketch.text", &json!({"text": "A", "at": [0, 0], "font": font.to_string_lossy()})).unwrap();
    s.execute("sketch.finish", &json!({})).unwrap();
    let design = root.join("text.solidcraft");
    std::fs::write(&design, solidcraft_io::write_design(&s.doc)).unwrap();
    let open = json!({"path": design.to_string_lossy()});
    // The sketch's warning says what its text couldn't read.
    let warnings = |s: &Session| s.model.results.iter().filter_map(|r| r.warning.clone()).collect::<Vec<_>>().join("; ");
    s.execute("doc.open", &open).unwrap();
    assert!(warnings(&s).is_empty(), "the user's own open reads it: {}", warnings(&s));
    scoped(Some(PathScope::new([root.clone()])), || {
        s.execute("doc.open", &open).unwrap();
        assert!(warnings(&s).contains("outside the folders this agent may use"), "{}", warnings(&s));
    });
    let _ = std::fs::remove_dir_all(&base);
}
