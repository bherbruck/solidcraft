//! An agent builds parts through MCP only and verifies them through MCP only.

use std::f64::consts::PI;

use serde_json::{Value, json};

use crate::{Headless, PROTOCOL_VERSION, Server, tool_definitions};

fn server() -> Server {
    Server::new(Box::new(Headless::default()))
}

/// Screenshots are drawn on the GPU by the renderer the host installs (`solidcraft-cli mcp`); the
/// tests install the same one. `false`, with the reason printed, on a machine without a GPU
/// adapter: the screenshots are then left out of the test.
fn renders() -> bool {
    match solidcraft_gpu::capture::shared() {
        Ok(c) => {
            solidcraft_engine::capture::install(std::sync::Arc::new(move |scene, cam, w, h| {
                c.rgba(scene, cam, w, h).map(|rgba| solidcraft_engine::capture::Picture { rgba, renderer: c.renderer() }).map_err(|e| e.to_string())
            }));
            true
        }
        Err(e @ solidcraft_gpu::GpuError::NoAdapter(_)) if std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none() => {
            eprintln!("screenshots left out: {e}");
            false
        }
        Err(e) => panic!("{e}"),
    }
}

fn call(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let r = s.handle_line(&line).expect("reply");
    serde_json::from_str(&r).unwrap()
}

fn raw_tool(s: &mut Server, name: &str, args: Value) -> Value {
    call(s, 99, "tools/call", json!({"name": name, "arguments": args}))
}

/// Call a tool that must succeed; returns its JSON text (or the first content block).
fn tool(s: &mut Server, name: &str, args: Value) -> Value {
    let r = raw_tool(s, name, args);
    let c = &r["result"]["content"][0];
    assert_ne!(r["result"]["isError"], json!(true), "{name} failed: {c}");
    assert!(r.get("error").is_none(), "{name}: {r}");
    if c["type"] == "text" { serde_json::from_str(c["text"].as_str().unwrap()).unwrap_or(Value::Null) } else { c.clone() }
}

#[test]
fn crash_reports_are_listed_and_read_without_changing_the_design() {
    let mut s = server();
    let list = tool(&mut s, "crash_reports", json!({"limit": 3}));
    assert!(list["reports"].as_array().is_some_and(|r| r.len() <= 3), "{list}");
    assert!(list["dir"].is_string(), "{list}");
    let r = raw_tool(&mut s, "crash_reports", json!({"id": "../../etc"}));
    assert_eq!(r["result"]["isError"], json!(true), "{r}");
    let r = raw_tool(&mut s, "crash_reports", json!({"id": 5}));
    assert!(r.get("error").is_some() || r["result"]["isError"] == json!(true), "{r}");
}

fn volume(s: &mut Server) -> f64 {
    tool(s, "measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap()
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-12)
}

#[test]
fn handshake_and_tools_list() {
    let mut s = server();
    let init =
        call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "solidcraft");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert!(init["result"]["instructions"].as_str().unwrap().contains("headless"));
    // An unknown version gets ours.
    let init = call(&mut s, 2, "initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert!(s.is_initialized());
    assert_eq!(call(&mut s, 3, "ping", json!({}))["result"], json!({}));

    let tools = call(&mut s, 4, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "list_commands",
        "execute",
        "batch",
        "inspect_design",
        "measure",
        "body_topology",
        "set_parameter",
        "screenshot",
        "export",
        "undo",
        "redo",
        "crash_reports",
        "new_design",
        "open",
        "save",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    for t in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(t["description"].as_str().unwrap().len() > 20, "{t}");
    }

    let cmds = tool(&mut s, "list_commands", json!({"filter": "fillet"}));
    assert!(cmds.as_array().unwrap().iter().any(|c| c["id"] == "solid.fillet"), "{cmds}");
    assert!(cmds.as_array().unwrap().len() < 5);

    let res = call(&mut s, 5, "resources/list", json!({}));
    assert_eq!(res["result"]["resources"].as_array().unwrap().len(), 2);
    let d = call(&mut s, 6, "resources/read", json!({"uri": "solidcraft://design"}));
    assert!(d["result"]["contents"][0]["text"].as_str().unwrap().contains("timeline"));
    assert_eq!(call(&mut s, 7, "resources/read", json!({"uri": "solidcraft://nope"}))["error"]["code"], -32002);
    assert_eq!(call(&mut s, 8, "nope/nope", json!({}))["error"]["code"], -32601);
}

/// A URDF goes through `export` with its own options (#719).
#[test]
fn export_tool_writes_a_urdf_package() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}));
    let dir = std::env::temp_dir().join(format!("solidcraft-mcp-urdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let urdf = dir.join("robot.urdf");
    let r = tool(&mut s, "export", json!({"path": urdf.to_string_lossy(), "mesh_prefix": "package://robot/meshes/"}));
    assert_eq!(r["format"], "Urdf", "{r}");
    assert!(std::fs::read_to_string(&urdf).unwrap().contains(r#"filename="package://robot/meshes/robot-base.stl""#));
    assert!(dir.join("robot-base.stl").exists());
    // effort and velocity reach file.export, which refuses a bad one.
    let r = raw_tool(&mut s, "export", json!({"path": urdf.to_string_lossy(), "effort": -1}));
    assert_eq!(r["result"]["isError"], json!(true), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Box → fillet → hole in one batch, then verify with measure / inspect_design and look at it.
#[test]
fn agent_builds_box_fillet_hole() {
    let mut s = server();
    let r = tool(
        &mut s,
        "batch",
        json!({"commands": [
            {"command": "sketch.create", "params": {"plane": "XY"}},
            {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [40, 30]}},
            {"command": "sketch.finish"},
            {"command": "solid.extrude", "params": {"distance": 20}},
            {"command": "solid.fillet", "params": {"edges": [[0, 0, 10]], "radius": 3}},
            {"command": "sketch.create", "params": {"plane": "XY"}},
            {"command": "sketch.circle.center", "params": {"center": [20, 15], "radius": 5}},
            {"command": "solid.extrude", "params": {"distance": 20, "operation": "cut"}},
        ]}),
    );
    assert_eq!(r["completed"], 8, "{r}");
    let expect = 24000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0 - PI * 25.0 * 20.0;
    let v = volume(&mut s);
    assert!(rel(v, expect) < 5e-4, "{v} vs {expect}");
    let m = tool(&mut s, "measure", json!({"body": "Body1"}));
    // 6 box faces + fillet + hole wall (the kernel may keep the wall as two halves).
    assert!((8..=9).contains(&m["bodies"][0]["faces"].as_u64().unwrap()), "{m}");
    assert!(rel(m["bodies"][0]["bbox"]["max"][0].as_f64().unwrap(), 40.0) < 1e-6, "{m}");

    let d = tool(&mut s, "inspect_design", json!({}));
    assert_eq!(d["timeline"].as_array().unwrap().len(), 5, "{d}");
    assert!(d["timeline"].as_array().unwrap().iter().all(|f| f["error"].is_null()), "{d}");
    assert_eq!(d["sketches"][0]["curves"].as_array().unwrap().len(), 4, "{d}");
    assert!(d["sketches"][0]["status"].is_string(), "{d}");
    assert!(d["bodies"][0]["volume_mm3"].as_f64().unwrap() > 0.0);

    let topo = tool(&mut s, "body_topology", json!({"body": "Body1"}));
    assert!(topo["edges"].as_array().unwrap().len() >= 12, "{topo}");
    assert!(topo["faces"].as_array().unwrap().len() >= 8, "{topo}");

    // Undo the hole, redo it.
    tool(&mut s, "undo", json!({}));
    assert!(rel(volume(&mut s), 24000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0) < 5e-4);
    tool(&mut s, "redo", json!({}));
    assert!(rel(volume(&mut s), expect) < 5e-4);

    // Look at it from the top: a real PNG comes back as an image block.
    if renders() {
        let img = tool(&mut s, "screenshot", json!({"view": "top", "width": 320, "height": 200}));
        assert_eq!(img["type"], "image");
        assert_eq!(img["mimeType"], "image/png");
        assert!(img["data"].as_str().unwrap().starts_with("iVBORw0KGgo"), "not a PNG");
    }

    // Export, save, start over, open.
    let dir = std::env::temp_dir().join(format!("solidcraft-mcp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let step = dir.join("part.step");
    tool(&mut s, "export", json!({"path": step.to_string_lossy()}));
    assert!(std::fs::read_to_string(&step).unwrap().contains("ISO-10303-21"));
    let stl = dir.join("part.stl");
    tool(&mut s, "export", json!({"path": stl.to_string_lossy(), "format": "stl"}));
    assert!(std::fs::metadata(&stl).unwrap().len() > 1000);
    let shot = dir.join("shot.png");
    if renders() {
        tool(&mut s, "screenshot", json!({"path": shot.to_string_lossy(), "width": 64, "height": 64}));
        assert!(std::fs::metadata(&shot).unwrap().len() > 100);
    }
    let design = dir.join("part.solidcraft");
    tool(&mut s, "save", json!({"path": design.to_string_lossy()}));
    tool(&mut s, "new_design", json!({"name": "Empty"}));
    assert_eq!(tool(&mut s, "measure", json!({}))["body_count"], 0);
    tool(&mut s, "open", json!({"path": design.to_string_lossy()}));
    assert!(rel(volume(&mut s), expect) < 5e-4);
    // A STEP file opens as a new design with the same body.
    let r = tool(&mut s, "open", json!({"path": step.to_string_lossy()}));
    assert_eq!(r["features"], 1, "{r}");
    assert!(rel(volume(&mut s), expect) < 5e-4);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `open` hands `up` and `names` to the STEP import, and the tool's schema refuses other values.
#[test]
fn open_stands_a_y_up_step_file_up() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 40, "height": 5}}));
    let dir = std::env::temp_dir().join(format!("solidcraft-mcp-up-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let step = dir.join("tall.step").to_string_lossy().to_string();
    tool(&mut s, "export", json!({"path": step}));
    let height = |s: &mut Server| {
        let bbox = tool(s, "measure", json!({}))["bodies"][0]["bbox"].clone();
        bbox["max"][2].as_f64().unwrap() - bbox["min"][2].as_f64().unwrap()
    };

    let r = tool(&mut s, "open", json!({"path": step}));
    assert_eq!(r["features"], 1, "{r}");
    assert!((height(&mut s) - 5.0).abs() < 1e-6, "as written");
    let r = tool(&mut s, "open", json!({"path": step, "up": "y", "names": "product"}));
    assert_eq!(r["features"], 1, "the import, its component turned: {r}");
    assert!((height(&mut s) - 40.0).abs() < 1e-6, "Y became Z");

    for args in [json!({"path": step, "up": "x"}), json!({"path": step, "names": "label"}), json!({"path": step, "up": 1})] {
        let r = raw_tool(&mut s, "open", args.clone());
        assert_eq!(r["error"]["code"], -32602, "{args}: {r}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_selects_container_or_json_and_reopens_by_contents() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 20, "height": 30}}));
    let dir = std::env::temp_dir().join(format!("solidcraft-mcp-encodings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, format, is_json) in [
        ("default.solidcraft", None, false),
        ("explicit.solidcraft", Some("json"), true),
        ("legacy.solvecraft", None, false),
        ("inferred.json", None, true),
        ("container.json", Some("container"), false),
    ] {
        let path = dir.join(name);
        let mut args = json!({"path": path.to_string_lossy()});
        if let Some(format) = format {
            args["format"] = json!(format);
        }
        tool(&mut s, "save", args);
        let bytes = std::fs::read(&path).unwrap();
        if is_json {
            assert!(serde_json::from_slice::<Value>(&bytes).unwrap().is_object());
        } else {
            assert!(bytes.starts_with(b"PK\x03\x04"));
        }
        tool(&mut s, "new_design", json!({}));
        tool(&mut s, "open", json!({"path": path.to_string_lossy()}));
        assert!(rel(volume(&mut s), 6000.0) < 1e-9);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn parameters_drive_the_design() {
    let mut s = server();
    tool(&mut s, "set_parameter", json!({"name": "len", "value": "40 mm"}));
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": "len", "width": 30, "height": "len / 2", "corner": [0, 0, 0]}}));
    assert!(rel(volume(&mut s), 40.0 * 30.0 * 20.0) < 1e-9);
    let r = tool(&mut s, "set_parameter", json!({"name": "len", "value": 50}));
    assert_eq!(r["value"], 50.0, "{r}");
    assert_eq!(r["errors"].as_array().unwrap().len(), 0, "{r}");
    assert!(rel(volume(&mut s), 50.0 * 30.0 * 25.0) < 1e-9);
    let d = tool(&mut s, "inspect_design", json!({"measure": false, "sketches": false}));
    assert!(d["params"].as_array().unwrap().iter().any(|p| p["name"] == "len" && p["value"] == 50.0), "{d}");
}

/// Design properties (#1015) are set through `execute` and come back in `inspect_design`.
#[test]
fn design_properties_show_in_inspect_design() {
    let mut s = server();
    let d = tool(&mut s, "inspect_design", json!({"measure": false, "sketches": false}));
    assert_eq!(d["metadata"], json!({}), "{d}");
    let params = json!({"values": {"author": "Agent", "com.example.agent": {"session": 7}}});
    tool(&mut s, "execute", json!({"command": "document.metadata.set", "params": params}));
    let d = tool(&mut s, "inspect_design", json!({"measure": false, "sketches": false}));
    assert_eq!(d["metadata"], json!({"author": "Agent", "com.example.agent": {"session": 7}}), "{d}");
}

#[test]
fn batch_stops_at_first_error_and_rolls_back() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}));
    let r = raw_tool(
        &mut s,
        "batch",
        json!({"commands": [
            {"command": "solid.sphere", "params": {"radius": 3, "center": [50, 0, 0]}},
            {"command": "solid.extrude", "params": {"distance": "bogus +"}},
            {"command": "solid.sphere", "params": {"radius": 3, "center": [80, 0, 0]}},
        ]}),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    let body: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["failed"]["index"], 1, "{body}");
    assert_eq!(body["failed"]["command"], "solid.extrude");
    assert_eq!(body["undone"], 1, "{body}");
    assert_eq!(body["results"].as_array().unwrap().len(), 1);
    // Only the first box is left, and its undo step is intact.
    assert_eq!(tool(&mut s, "measure", json!({}))["body_count"], 1);
    assert_eq!(tool(&mut s, "inspect_design", json!({"measure": false}))["undo"], 1);

    // Without rollback the completed steps stay.
    let r = raw_tool(
        &mut s,
        "batch",
        json!({"rollback": false, "commands": [{"command": "solid.sphere", "params": {"radius": 3, "center": [50, 0, 0]}}, {"command": "NoSuchCommand"}]}),
    );
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("NoSuchCommand"));
    assert_eq!(tool(&mut s, "measure", json!({}))["body_count"], 2);
}

/// Regression: with a full undo history a failed batch reported `undone: 0` and left its
/// sketches behind (an empty "Sketch1", two sketches named "TapHole").
#[test]
fn batch_rolls_back_with_a_full_undo_history() {
    let mut s = server();
    let fill: Vec<Value> =
        (0..230).map(|i| json!({"command": "parameters.add", "params": {"name": format!("p{i}"), "expression": "1 mm"}})).collect();
    tool(&mut s, "batch", json!({"commands": fill}));
    let undo = tool(&mut s, "inspect_design", json!({"measure": false}))["undo"].clone();
    for _ in 0..2 {
        let r = raw_tool(
            &mut s,
            "batch",
            json!({"commands": [
                {"command": "sketch.create", "params": {"plane": "XY", "name": "TapHole"}},
                {"command": "sketch.circle.center", "params": {"center": [0, 0], "radius": 2}},
                {"command": "sketch.dimension", "params": {"entities": ["nope"], "value": 3}},
            ]}),
        );
        let body: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["failed"]["index"], 2, "{body}");
        assert_eq!(body["undone"], 2, "{body}");
    }
    let d = tool(&mut s, "inspect_design", json!({"measure": false, "sketches": false}));
    assert_eq!(d["sketches"].as_array().map(Vec::len), Some(0), "{d}");
    assert!(d["active_sketch"].is_null(), "{d}");
    assert_eq!(d["undo"], undo);
}

#[test]
fn helper_sketches_can_be_hidden() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 40, "width": 40, "height": 2}}));
    let sk = tool(&mut s, "execute", json!({"command": "sketch.create", "params": {"plane": "XY"}}))["sketch"].clone();
    tool(&mut s, "execute", json!({"command": "sketch.line", "params": {"points": [[0, 0], [40, 0]], "construction": true}}));
    tool(&mut s, "execute", json!({"command": "sketch.finish"}));
    let visible = |s: &mut Server| tool(s, "inspect_design", json!({"measure": false}))["sketches"][0]["visible"].clone();
    assert_eq!(visible(&mut s), true);
    tool(&mut s, "execute", json!({"command": "browser.visibility", "params": {"items": [sk], "visible": false}}));
    assert_eq!(visible(&mut s), false);
    let r = raw_tool(&mut s, "execute", json!({"command": "browser.visibility", "params": {"items": ["Nope"], "visible": false}}));
    assert_eq!(r["result"]["isError"], true);
    tool(&mut s, "execute", json!({"command": "browser.visibility", "params": {"folder": "sketches", "visible": true}}));
    assert_eq!(visible(&mut s), true);
}

#[test]
fn unknown_tools_and_bad_arguments_are_clean_errors() {
    let mut s = server();
    let r = raw_tool(&mut s, "make_coffee", json!({}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("make_coffee"));
    assert_eq!(raw_tool(&mut s, "execute", json!({}))["error"]["code"], -32602);
    assert_eq!(raw_tool(&mut s, "execute", json!({"command": 5}))["error"]["code"], -32602);
    assert_eq!(raw_tool(&mut s, "measure", json!({"bogus": 1}))["error"]["code"], -32602);
    assert_eq!(raw_tool(&mut s, "screenshot", json!({"source": "sideways"}))["error"]["code"], -32602);
    // `view` also takes saved named views, so an unknown name is a tool error naming it.
    let r = raw_tool(&mut s, "screenshot", json!({"view": "sideways"}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("sideways"), "{r}");
    assert_eq!(call(&mut s, 1, "tools/call", json!({"arguments": {}}))["error"]["code"], -32602);
    // Engine errors are tool errors the model can read.
    let r = raw_tool(&mut s, "execute", json!({"command": "solid.extrude", "params": {"distance": 10}}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    let r = raw_tool(&mut s, "execute", json!({"command": "NoSuchCommand"}));
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("unknown command"));
    // Framing errors.
    assert!(s.handle_line("{not json").unwrap().contains("-32700"));
    assert!(s.handle_line("[]").unwrap().contains("-32600"));
    assert!(s.handle_line("42").unwrap().contains("-32600"));
    assert!(s.handle_line("   ").is_none());
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).is_none());
    let both = s.handle_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/x"}]"#).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&both).unwrap().as_array().unwrap().len(), 1);
}

fn hostile_values() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!(-1),
        json!(0),
        json!(1e308),
        json!(-1e308),
        json!(""),
        json!("nope"),
        json!("../../../../../../nonexistent/dir/x.step"),
        json!("1/0"),
        json!([]),
        json!([1e308, -1e308]),
        json!([{"command": "solid.extrude", "params": {"distance": 1e308}}]),
        json!([{"command": 7}, null, "x"]),
        json!({}),
        json!({"x": 1}),
    ]
}

/// Every tool, every argument it documents, every hostile value: replies are well-formed
/// JSON-RPC and nothing panics.
#[test]
fn hostile_arguments_never_panic() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}));
    let defs = tool_definitions();
    let mut n = 0;
    for def in &defs {
        let name = def["name"].as_str().unwrap();
        // Hostile strings are relative paths: don't let the sweep write files into the crate.
        let writes = matches!(name, "save" | "screenshot" | "export");
        let keys: Vec<String> = def["inputSchema"]["properties"].as_object().unwrap().keys().filter(|k| !(writes && *k == "path")).cloned().collect();
        for k in &keys {
            for v in hostile_values() {
                let mut args = json!({});
                // Fill the other required arguments with something plausible.
                for r in def["inputSchema"]["required"].as_array().into_iter().flatten() {
                    args[r.as_str().unwrap()] = json!("Body1");
                }
                args[k.as_str()] = v;
                if name == "screenshot" && (k == "width" || k == "height") {
                    args["view"] = json!("front");
                }
                let r = raw_tool(&mut s, name, args.clone());
                assert!(r.get("result").is_some() || r["error"]["code"].is_i64(), "{name} {args}: {r}");
                n += 1;
            }
        }
    }
    assert!(n > 100);
    // Execute every command with hostile params through MCP too.
    let cmds = tool(&mut s, "list_commands", json!({}));
    for c in cmds.as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        if matches!(id, "doc.open" | "file.save" | "file.save_as" | "file.export" | "file.save_mesh") {
            continue;
        }
        for v in hostile_values() {
            let r = raw_tool(&mut s, "execute", json!({"command": id, "params": {"distance": v.clone(), "edges": v.clone(), "plane": v}}));
            assert!(r.get("result").is_some(), "{id}: {r}");
        }
    }
    // Arguments that are not an object.
    for v in hostile_values().into_iter().filter(|v| !v.is_object() && !v.is_null()) {
        let r = call(&mut s, 1, "tools/call", json!({"name": "measure", "arguments": v}));
        assert_eq!(r["error"]["code"], -32602, "{r}");
    }
}

#[test]
fn base64_matches_rfc4648() {
    use crate::tools::base64;
    for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
        assert_eq!(base64(i.as_bytes()), o);
    }
}

/// Filtering by a command id returns that command, not every command whose docs mention it (#269).
#[test]
fn list_commands_filter_by_a_command_id_matches_ids() {
    let mut s = server();
    let cmds = tool(&mut s, "list_commands", json!({"filter": "sketch.delete"}));
    let ids: Vec<&str> = cmds.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"sketch.delete"), "{ids:?}");
    assert!(ids.iter().all(|id| id.contains("sketch.delete")), "{} commands matched: {ids:?}", ids.len());
}

/// Compatibility shims (docs/compatibility.md): a design saved before the rename (legacy
/// extension and format tag) opens through MCP, and the legacy resource URIs still read.
#[test]
fn legacy_design_files_and_resource_uris() {
    let mut s = server();
    let saved = String::from_utf8(solidcraft_engine::io::write_design(&solidcraft_engine::doc::Document::default())).unwrap();
    assert!(saved.contains("\"solidcraft/"), "{saved}");
    let legacy = saved.replace("\"solidcraft/", "\"solvecraft/");
    let path = std::env::temp_dir().join(format!("solidcraft-mcp-legacy-{}.solvecraft", std::process::id()));
    std::fs::write(&path, legacy).unwrap();
    tool(&mut s, "open", json!({"path": path.to_string_lossy()}));
    let _ = std::fs::remove_file(&path);
    let d = call(&mut s, 1, "resources/read", json!({"uri": "solvecraft://design"}));
    assert!(d["result"]["contents"][0]["text"].as_str().unwrap().contains("timeline"));
}

/// The default folders (#205): the recovery folder works out of the box, `screenshot` without a
/// `path` uses the server's own scratch folder, and the open design's folder counts while the
/// design is open there, without its hidden files. The working folder doesn't count at all.
#[test]
fn the_default_folders_are_the_designs_the_recovery_folder_and_the_scratch_folder() {
    let base = solidcraft_engine::io::vfs::temp_dir().join(format!("solidcraft-mcp-defaults-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (proj, other, recovery) = (base.join("proj"), base.join("other"), base.join("recovery"));
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(proj.join(".env"), "TOKEN=secret").unwrap();
    let at = |d: &std::path::Path, f: &str| d.join(f).to_string_lossy().to_string();
    let scope = crate::agent_scope_with(Some(recovery.clone()), []);
    let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    assert!(!scope.roots().contains(&cwd), "the working folder is not a default folder: {scope:?}");
    assert!(scope.roots().contains(&std::fs::canonicalize(&recovery).unwrap()), "{scope:?}");
    assert!(scope.roots().contains(&std::fs::canonicalize(crate::scratch_dir().unwrap()).unwrap()), "{scope:?}");

    let mut session = solidcraft_engine::Session::default();
    session.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
    session.execute("file.save_as", &json!({"path": at(&proj, "part.solidcraft")})).unwrap();
    let mut s = Server::new(Box::new(Headless::new(session))).with_path_scope(Some(scope));
    // Legitimate work in the design's folder: export, save, open, screenshots, the recovery folder.
    tool(&mut s, "export", json!({"path": at(&proj, "part.step")}));
    tool(&mut s, "execute", json!({"command": "parameters.export", "params": {"path": at(&proj, "params.csv")}}));
    tool(&mut s, "save", json!({"path": at(&proj, "copy.solidcraft")}));
    tool(&mut s, "open", json!({"path": at(&proj, "part.solidcraft")}));
    if renders() {
        tool(&mut s, "screenshot", json!({"width": 32, "height": 32}));
    }
    tool(&mut s, "execute", json!({"command": "doc.recovery_list", "params": {"dir": recovery.to_string_lossy()}}));
    assert!(proj.join("part.step").exists() && proj.join("copy.solidcraft").exists());
    // Hidden files in it, and folders beside it, stay out of reach.
    for (name, args) in [
        ("export", json!({"path": at(&proj, ".env")})),
        ("execute", json!({"command": "parameters.import", "params": {"path": at(&proj, ".env")}})),
        ("export", json!({"path": at(&proj, ".git/x.step")})),
        ("export", json!({"path": at(&other, "x.step")})),
    ] {
        let r = raw_tool(&mut s, name, args);
        assert_eq!(r["result"]["isError"], json!(true), "{name}: {r}");
    }
    assert_eq!(std::fs::read_to_string(proj.join(".env")).unwrap(), "TOKEN=secret");
    assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&base);
}

/// Tool calls reach files only inside the server's folders (#205): the usual workflow (save,
/// export, open, screenshots) works there; a path outside, directly, through `..` or through a
/// symlink that leads out, is refused with a clear error and nothing is written.
#[test]
fn tool_calls_stay_inside_the_agents_folders() {
    use solidcraft_engine::io::vfs::PathScope;
    let base = solidcraft_engine::io::vfs::temp_dir().join(format!("solidcraft-mcp-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, out) = (base.join("root"), base.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let at = |d: &std::path::Path, f: &str| d.join(f).to_string_lossy().to_string();
    let defaults = crate::agent_scope_with(None, [root.clone()]);
    assert!(defaults.roots().contains(&std::fs::canonicalize(&root).unwrap()), "{defaults:?}");
    assert!(!defaults.roots().contains(&std::fs::canonicalize(std::env::temp_dir()).unwrap()), "{defaults:?}");
    // Only `root` here, to tell inside from outside within the temporary folder.
    let mut s = Server::new(Box::new(Headless::default())).with_path_scope(Some(PathScope::new([root.clone()])));
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}));
    tool(&mut s, "save", json!({"path": at(&root, "part.solidcraft")}));
    tool(&mut s, "export", json!({"path": at(&root, "part.step")}));
    tool(&mut s, "open", json!({"path": at(&root, "part.solidcraft")}));
    if renders() {
        tool(&mut s, "screenshot", json!({"path": at(&root, "shot.png"), "width": 64, "height": 64}));
    }
    let escapes = vec![at(&out, "x.step"), at(&root, "../out/x.step")];
    #[cfg(unix)]
    let mut escapes = escapes;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&out, root.join("link")).unwrap();
        escapes.push(at(&root, "link/x.step"));
    }
    for p in escapes {
        for (name, args) in [
            ("export", json!({"path": p})),
            ("save", json!({"path": p})),
            ("open", json!({"path": p})),
            ("screenshot", json!({"path": p, "width": 64, "height": 64})),
            ("execute", json!({"command": "file.save_as", "params": {"path": p}})),
            ("batch", json!({"commands": [{"command": "file.export", "params": {"path": p}}]})),
        ] {
            let r = raw_tool(&mut s, name, args);
            assert_eq!(r["result"]["isError"], json!(true), "{name} {p}: {r}");
            assert!(r.to_string().contains("outside the folders this agent may use"), "{name} {p}: {r}");
        }
    }
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing written outside");
    let _ = std::fs::remove_dir_all(&base);
}

/// The short (8.3) spelling of an existing `path` as `cmd` gives it (`NAME~1.XYZ`), or `None`
/// when it has no short name (the volume makes none).
#[cfg(windows)]
fn short_spelling(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("cmd").raw_arg(format!("/d /c for %I in (\"{}\") do @echo %~sI", path.display())).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| solidcraft_engine::io::vfs::windows_refusal(s).is_some_and(|why| why.contains("8.3")))
        .map(Into::into)
}

/// On Windows the temporary folder's path often has a short (8.3) name (the user folder's
/// `NAME~1.XYZ` when its name is long). The server's own folders are spelled with long names
/// there, so `screenshot` without a `path` (the scratch folder) and `doc.recovery_list` (the
/// recovery folder) work; on such a machine both were refused as short names. Run in a child
/// process whose `TMP`, `TEMP` and `SOLIDCRAFT_RECOVERY_DIR` are short spellings.
#[cfg(windows)]
#[test]
fn the_servers_own_folders_work_when_the_temporary_folder_has_a_short_name() {
    let base = solidcraft_engine::io::vfs::temp_dir().join(format!("solidcraft-mcp-short-names-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (temp, recovery) = (base.join("temporary"), base.join("recovery folder"));
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::create_dir_all(&recovery).unwrap();
    let (Some(short_temp), Some(short_recovery)) = (short_spelling(&temp), short_spelling(&recovery)) else {
        eprintln!("skipped: {} has no short (8.3) name on this volume", base.display());
        let _ = std::fs::remove_dir_all(&base);
        return;
    };
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::short_names_child", "--test-threads=1", "--nocapture"])
        .env("TMP", &short_temp)
        .env("TEMP", &short_temp)
        .env("SOLIDCRAFT_RECOVERY_DIR", &short_recovery)
        .env("SOLIDCRAFT_MCP_SHORT_NAMES_CHILD", "1")
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains("short names: ok"), "{text}");
    let _ = std::fs::remove_dir_all(&base);
}

/// The child process of the test above; does nothing on its own.
#[cfg(windows)]
#[test]
fn short_names_child() {
    if std::env::var_os("SOLIDCRAFT_MCP_SHORT_NAMES_CHILD").is_none() {
        return;
    }
    use solidcraft_engine::io::vfs::windows_refusal;
    let temp = std::env::temp_dir();
    assert!(windows_refusal(&temp.to_string_lossy()).is_some_and(|why| why.contains("8.3")), "{}", temp.display());
    let mut s = Server::new(Box::new(Headless::default())).with_path_scope(Some(crate::agent_scope([])));
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}}));
    if renders() {
        tool(&mut s, "screenshot", json!({"width": 32, "height": 32}));
    }
    tool(&mut s, "execute", json!({"command": "doc.recovery_list", "params": {}}));
    // An agent's own short spelling of the scratch folder stays refused.
    let scratch = crate::scratch_dir().unwrap();
    let short = std::env::temp_dir().join(scratch.file_name().unwrap()).join("shot.png");
    let r = raw_tool(&mut s, "screenshot", json!({"path": short.to_string_lossy(), "width": 32, "height": 32}));
    assert!(r.to_string().contains("short (8.3) names"), "{r}");
    println!("short names: ok");
}

/// A bridged server sends the agent's folders with every request, so the app confines the
/// request's files the same way (`allow_paths`, crates/ui-egui/src/control.rs).
#[test]
fn a_bridged_request_carries_the_agents_folders() {
    use std::io::{BufRead, BufReader, Write};

    use solidcraft_engine::io::vfs::{PathScope, scoped};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let echo = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
        let req: Value = serde_json::from_str(&line).unwrap();
        let mut result = req["params"].clone();
        result["sent_token"] = req["token"].clone();
        writeln!(&stream, "{}", json!({"id": req["id"], "ok": true, "result": result})).unwrap();
    });
    // The app's control token goes with the request too (#205).
    let mut b = crate::Remote::connect_with_token(&addr, Some("0123456789abcdef0123456789abcdef")).unwrap();
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let got = scoped(Some(PathScope::new([dir.clone()])), || crate::Backend::call(&mut b, "engine.execute", json!({"command": "file.new"}))).unwrap();
    assert_eq!(got["allow_paths"], json!([dir.to_string_lossy()]), "{got}");
    assert_eq!(got["command"], "file.new");
    assert_eq!(got["sent_token"], "0123456789abcdef0123456789abcdef");
    echo.join().unwrap();
}

/// #528: an agent saves the angle of a product photo as a named view once, then renders the
/// model from that view after each change. The render must use the saved camera, not a
/// standard view.
#[test]
fn screenshot_renders_a_saved_named_view() {
    if !renders() {
        return;
    }
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 40, "width": 20, "height": 10}}));
    let camera = json!({"target": [20, 10, 5], "yaw": 0.5, "pitch": -0.6, "distance": 150});
    tool(&mut s, "execute", json!({"command": "view.save", "params": {"name": "Photo", "camera": camera.clone()}}));
    let shot = |s: &mut Server, view: &str| {
        let img = tool(s, "screenshot", json!({"view": view, "width": 96, "height": 64}));
        img["data"].as_str().unwrap_or_default().to_string()
    };
    let iso = shot(&mut s, "iso");
    let photo = shot(&mut s, "Photo");
    assert!(photo.starts_with("iVBORw0KGgo"), "not a PNG");
    assert_ne!(photo, iso, "the named view rendered like the standard iso view");
    // The same camera given directly renders the same picture; a standard view turns back.
    let direct = tool(&mut s, "screenshot", json!({"camera": camera, "width": 96, "height": 64}));
    assert_eq!(direct["data"].as_str().unwrap_or_default(), photo);
    assert_eq!(shot(&mut s, "iso"), iso);
}

/// The ground grid lies on Z 0 and cuts through a part that straddles it: `grid: false` and
/// `axes: false` leave it and the origin's axes out of the render. An axis hidden with
/// `browser.visibility` stays out whatever `axes` says.
#[test]
fn screenshot_can_leave_the_grid_and_the_axes_out() {
    if !renders() {
        return;
    }
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 40, "width": 30, "height": 20, "corner": [10, -15, -10]}}));
    let shot = |s: &mut Server, extra: Value| {
        let mut args = json!({"view": "front", "width": 96, "height": 64});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        tool(s, "screenshot", args)["data"].as_str().unwrap_or_default().to_string()
    };
    let all = shot(&mut s, json!({}));
    let no_grid = shot(&mut s, json!({"grid": false}));
    let no_axes = shot(&mut s, json!({"axes": false}));
    let bare = shot(&mut s, json!({"grid": false, "axes": false}));
    assert!(all.starts_with("iVBORw0KGgo"), "not a PNG");
    let pictures = [&all, &no_grid, &no_axes, &bare];
    for (i, a) in pictures.iter().enumerate() {
        for b in &pictures[i + 1..] {
            assert_ne!(a, b, "two of the pictures are the same");
        }
    }
    // Asking for them is the default.
    assert_eq!(shot(&mut s, json!({"grid": true, "axes": true})), all);
    // Hiding the origin leaves the axes out, with or without `axes`; the grid is not the origin.
    tool(&mut s, "execute", json!({"command": "browser.visibility", "params": {"folder": "origin", "visible": false}}));
    assert_eq!(shot(&mut s, json!({})), no_axes);
    assert_eq!(shot(&mut s, json!({"axes": true})), no_axes);
    assert_eq!(shot(&mut s, json!({"grid": false})), bare);
    // Other than booleans are schema errors.
    for bad in [json!({"grid": "no"}), json!({"axes": 0})] {
        assert_eq!(raw_tool(&mut s, "screenshot", bad)["error"]["code"], -32602);
    }
}

/// Bridged to an app, `grid` and `axes` ask for a model render and travel with it; without them
/// the window is captured as before; the window cannot be asked to leave things out.
#[test]
fn bridged_screenshots_with_grid_or_axes_are_model_renders() {
    struct App {
        calls: Vec<(String, Value)>,
    }
    impl crate::backend::Backend for App {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            self.calls.push((method.to_string(), params.clone()));
            match method {
                "ui.render" | "ui.screenshot" => {
                    let path = params["path"].as_str().ok_or("missing `path`")?;
                    std::fs::write(path, b"\x89PNG").map_err(|e| e.to_string())?;
                    Ok(json!({"path": path, "width": 64, "height": 48}))
                }
                other => Err(format!("unknown method `{other}`")),
            }
        }
        fn has_ui(&self) -> bool {
            true
        }
        fn describe(&self) -> String {
            "app".into()
        }
    }
    let dir = std::env::temp_dir().join(format!("solidcraft-mcp-guides-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shot.png").to_string_lossy().to_string();
    let shoot = |extra: Value| {
        let mut args = json!({"path": path, "width": 64, "height": 48});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let mut app = App { calls: Vec::new() };
        let r = crate::tools::call_tool(&mut app, "screenshot", &args);
        (app.calls, r)
    };
    let (calls, r) = shoot(json!({}));
    assert!(r["isError"] != json!(true), "{r}");
    assert_eq!(calls.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["ui.screenshot"]);
    let (calls, r) = shoot(json!({"grid": false, "axes": false}));
    assert!(r["isError"] != json!(true), "{r}");
    assert_eq!(calls, [("ui.render".to_string(), json!({"path": path, "width": 64, "height": 48, "grid": false, "axes": false}))]);
    let (calls, _) = shoot(json!({"axes": true}));
    assert_eq!(calls, [("ui.render".to_string(), json!({"path": path, "width": 64, "height": 48, "axes": true}))]);
    let (calls, r) = shoot(json!({"source": "model", "grid": false}));
    assert!(r["isError"] != json!(true), "{r}");
    assert_eq!(calls, [("ui.render".to_string(), json!({"path": path, "width": 64, "height": 48, "grid": false}))]);
    // The window shows what it shows: nothing is called, and the error says what to ask for.
    let (calls, r) = shoot(json!({"source": "window", "grid": false, "view": "top"}));
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(r["content"][0]["text"].as_str().unwrap().contains("source"), "{r}");
    assert!(calls.is_empty(), "{calls:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #1677: a hidden window gives no picture. With no `source`, a model render comes back and says
/// it is one (`fallback`); asked for the window, the tool says why there is none, at once.
#[test]
fn a_hidden_window_screenshot_says_so() {
    struct Hidden {
        calls: Vec<String>,
    }
    impl crate::backend::Backend for Hidden {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            self.calls.push(method.to_string());
            match method {
                "ui.screenshot" => Err("no frame was presented (window hidden?); use ui.render".into()),
                "ui.render" => {
                    let path = params["path"].as_str().ok_or("missing `path`")?;
                    std::fs::write(path, b"\x89PNG").map_err(|e| e.to_string())?;
                    Ok(json!({"path": path, "width": 64, "height": 48}))
                }
                other => Err(format!("unknown method `{other}`")),
            }
        }
        fn has_ui(&self) -> bool {
            true
        }
        fn describe(&self) -> String {
            "app".into()
        }
    }
    let dir = std::env::temp_dir().join(format!("solidcraft-mcp-hidden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shot.png").to_string_lossy().to_string();
    let shoot = |extra: Value| {
        let mut args = json!({"path": path, "width": 64, "height": 48});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let mut app = Hidden { calls: Vec::new() };
        let r = crate::tools::call_tool(&mut app, "screenshot", &args);
        (app.calls, r)
    };
    // The default: the window first, then a model render that says why.
    let (calls, r) = shoot(json!({}));
    assert!(r["isError"] != json!(true), "{r}");
    assert_eq!(calls, ["ui.screenshot", "ui.render"]);
    let info: Value = serde_json::from_str(r["content"][1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(info["source"], "model", "{info}");
    assert!(info["fallback"].as_str().is_some_and(|f| f.contains("window hidden")), "{info}");
    // Asked for the window: an error naming the cause and the way out, and no render.
    let (calls, r) = shoot(json!({"source": "window"}));
    assert_eq!(r["isError"], json!(true), "{r}");
    let text = r["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("window hidden") && text.contains("source \"model\""), "{text}");
    assert_eq!(calls, ["ui.screenshot"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #530: in a design with many bodies every feature result lists all of them, so a batch reply
/// is mostly the same body list once per step. A compact batch reply names only what each step
/// added or changed.
#[test]
fn compact_batch_reply_does_not_repeat_the_body_list() {
    let mut s = server();
    let cube =
        |x: i64, y: i64| json!({"command": "solid.box", "params": {"length": 1, "width": 1, "height": 1, "corner": [x, y, 0], "operation": "new"}});
    let existing: Vec<Value> = (0..50).map(|i| cube(2 * i, 0)).collect();
    tool(&mut s, "batch", json!({"commands": existing}));
    let more: Vec<Value> = (0..3).map(|i| cube(2 * i, 5)).collect();
    let r = raw_tool(&mut s, "batch", json!({"commands": more, "verbose": false}));
    assert!(r.get("error").is_none() && r["result"]["isError"] != json!(true), "{r}");
    let reply = r["result"]["content"][0]["text"].as_str().unwrap_or_default();
    assert_eq!(tool(&mut s, "measure", json!({}))["body_count"], 53);
    // The new bodies are named; the 50 untouched ones are not repeated.
    for new in ["Body51", "Body52", "Body53"] {
        assert!(reply.contains(&format!("\"{new}\"")), "{new} missing from the compact reply: {reply}");
    }
    for old in ["Body1", "Body25", "Body50"] {
        assert!(!reply.contains(&format!("\"{old}\"")), "untouched {old} repeated in the compact reply ({} bytes): {reply}", reply.len());
    }
}

/// A compact batch reply keeps what each step did: the body a cut changed, and a sketch
/// command's new ids once (under `created`); the default reply is unchanged.
#[test]
fn compact_batch_reply_keeps_changed_bodies_and_created_ids() {
    let mut s = server();
    let steps = json!([
        {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "body_name": "Base"}},
        {"command": "solid.box", "params": {"length": 4, "width": 4, "height": 10, "corner": [3, 3, 5], "operation": "cut", "targets": ["Base"]}},
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [20, 0], "p1": [30, 10]}},
    ]);
    let full = tool(&mut s, "batch", json!({"commands": steps}));
    assert_eq!(full["results"][1]["bodies"], json!(["Base"]), "{full}");
    assert_eq!(full["results"][3]["curves"], full["results"][3]["created"]["curves"], "{full}");
    tool(&mut s, "new_design", json!({}));
    let r = tool(&mut s, "batch", json!({"commands": steps, "verbose": false}));
    let cut = &r["results"][1];
    assert!(cut.get("bodies").is_none(), "{r}");
    assert_eq!(cut["changed_bodies"], json!(["Base"]), "{r}");
    let rect = &r["results"][3];
    assert!(rect.get("curves").is_none() && rect.get("constraints").is_none(), "{r}");
    assert_eq!(rect["created"]["curves"].as_array().map(Vec::len), Some(4), "{r}");
    assert!(rect["sides"].is_object(), "results that are not repeated stay: {r}");
}

/// #531: a product model keeps an optional variant hidden in its default presentation. Hiding
/// it must survive saving and reopening the design (suppressing it would change the design).
#[test]
fn hidden_body_stays_hidden_after_save_and_reopen() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "body_name": "Base"}}));
    tool(
        &mut s,
        "execute",
        json!({"command": "solid.box", "params": {"length": 5, "width": 5, "height": 5, "corner": [20, 0, 0], "operation": "new", "body_name": "Variant"}}),
    );
    tool(&mut s, "execute", json!({"command": "browser.visibility", "params": {"items": ["Variant"], "visible": false}}));
    let visible = |s: &mut Server, body: &str| {
        let d = tool(s, "inspect_design", json!({"measure": false}));
        d["bodies"].as_array().into_iter().flatten().find(|b| b["name"] == body).map(|b| b["visible"].clone()).unwrap_or(Value::Null)
    };
    assert_eq!(visible(&mut s, "Variant"), false);

    let dir = std::env::temp_dir().join(format!("solidcraft-mcp-visibility-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let design = dir.join("variants.solidcraft");
    tool(&mut s, "save", json!({"path": design.to_string_lossy()}));
    tool(&mut s, "new_design", json!({"name": "Empty"}));
    tool(&mut s, "open", json!({"path": design.to_string_lossy()}));
    let (base, variant) = (visible(&mut s, "Base"), visible(&mut s, "Variant"));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(base, true);
    assert_eq!(variant, false, "the hidden body is visible again after reopening");
}

/// Single execute calls share batch compaction without changing geometry or inspection.
#[test]
fn compact_execute_keeps_changes_ids_and_default_replies() {
    let mut s = server();
    let commands = json!([
        {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10, "body_name": "Base"}},
        {"command": "solid.box", "params": {"length": 4, "width": 4, "height": 10, "corner": [3, 3, 5], "operation": "cut", "targets": ["Base"]}},
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [20, 0], "p1": [30, 10]}}
    ]);
    let full: Vec<Value> = commands.as_array().unwrap().iter().map(|a| tool(&mut s, "execute", a.clone())).collect();
    assert_eq!(full[0]["bodies"], json!(["Base"]));
    assert_eq!(full[3]["curves"], full[3]["created"]["curves"]);
    let expected_volume = volume(&mut s);
    for verbose in [true, false] {
        tool(&mut s, "new_design", json!({}));
        for (a, expected) in commands.as_array().unwrap().iter().zip(&full) {
            let mut a = a.clone();
            a["verbose"] = json!(verbose);
            let mut actual = tool(&mut s, "execute", a);
            let mut expected = expected.clone();
            // Independent executions have different timings; the field must still be present.
            if expected.get("ms").is_some() {
                assert!(actual["ms"].is_number());
                actual.as_object_mut().unwrap().remove("ms");
                expected.as_object_mut().unwrap().remove("ms");
            }
            if !verbose {
                // Compare against the full result with only the documented repeated fields removed.
                let o = expected.as_object_mut().unwrap();
                if o.contains_key("created_bodies") {
                    o.remove("bodies");
                }
                for key in ["points", "curves", "constraints", "dimensions"] {
                    if o.get(key).is_some_and(|v| o.get("created").and_then(|c| c.get(key)) == Some(v)) {
                        o.remove(key);
                    }
                }
            }
            assert_eq!(actual, expected);
        }
        assert!((volume(&mut s) - expected_volume).abs() < 1e-9);
        let inspect = tool(&mut s, "execute", json!({"command": "sketch.inspect", "verbose": false}));
        assert_eq!(inspect["curves"].as_array().map(Vec::len), Some(4));
        let invalid = raw_tool(&mut s, "execute", json!({"command": "solid.box", "verbose": "false"}));
        assert_eq!(invalid["error"]["code"], -32602);
        assert!((volume(&mut s) - expected_volume).abs() < 1e-9);
        let error = raw_tool(&mut s, "execute", json!({"command": "not.a.command", "verbose": false}));
        assert_eq!(error["result"]["isError"], true);
    }
}

/// A connected app sees exactly one engine call; no inventory inspection is needed.
#[test]
fn compact_execute_uses_one_backend_call_and_keeps_removed_bodies() {
    struct ReplyOnly {
        calls: usize,
    }
    impl crate::backend::Backend for ReplyOnly {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            assert_eq!(method, "engine.execute");
            assert_eq!(params, json!({"command": "test.feature", "params": {}}));
            self.calls += 1;
            Ok(
                json!({"feature": 7, "name": "Combine1", "bodies": ["Base"], "created_bodies": [], "changed_bodies": ["Base"], "removed_bodies": ["Tool"]}),
            )
        }
        fn has_ui(&self) -> bool {
            true
        }
        fn describe(&self) -> String {
            "test".into()
        }
    }
    let mut b = ReplyOnly { calls: 0 };
    let r = crate::tools::call_tool(&mut b, "execute", &json!({"command": "test.feature", "verbose": false}));
    let v: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(b.calls, 1);
    assert_eq!(v, json!({"feature": 7, "name": "Combine1", "created_bodies": [], "changed_bodies": ["Base"], "removed_bodies": ["Tool"]}));
}

/// The worked example in `skills/solidcraft-modeling/reference.md` is what agents copy, so it has
/// to keep building with the volume the file documents, and follow a parameter change.
#[test]
fn skill_reference_example_builds_as_documented() {
    let doc = include_str!("../../../skills/solidcraft-modeling/reference.md");
    let start = doc.find("{\"commands\":[").expect("the reference has a worked example");
    let end = start + doc[start..].find("```").expect("the example's code block ends");
    let example: Value = serde_json::from_str(&doc[start..end]).expect("the example is JSON");
    let mut s = server();
    tool(&mut s, "batch", example);
    // Plate with four r5 corners, minus four counterbored holes (Ø3.4 through, Ø6.5 × 3.3).
    let expected = |l: f64| {
        let (w, t) = (50.0, 6.0);
        let plate = l * w * t - 4.0 * 25.0 * (1.0 - PI / 4.0) * t;
        let hole = PI * 1.7 * 1.7 * (t - 3.3) + PI * 3.25 * 3.25 * 3.3;
        plate - 4.0 * hole
    };
    assert!((expected(80.0) - 23335.2).abs() < 0.05, "the reference states {}", expected(80.0));
    // Volume can't see where the holes are: each counterbore floor (z = T - 3.3) is centred on its
    // hole, at (±(L/2 - inset), ±(W/2 - inset)).
    let holes_at = |s: &mut Server, x: f64| {
        let topo = tool(s, "body_topology", json!({"body": "Body1"}));
        let mut floors: Vec<(i64, i64)> = topo["faces"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["centroid"].as_array())
            .filter(|c| (c[2].as_f64().unwrap() - 2.7).abs() < 1e-6)
            .map(|c| ((c[0].as_f64().unwrap() * 1000.0).round() as i64, (c[1].as_f64().unwrap() * 1000.0).round() as i64))
            .collect();
        floors.sort_unstable();
        let x = (x * 1000.0).round() as i64;
        assert_eq!(floors, vec![(-x, -17000), (-x, 17000), (x, -17000), (x, 17000)], "{topo}");
    };
    let m = tool(&mut s, "measure", json!({}));
    assert!(rel(m["total"]["volume_mm3"].as_f64().unwrap(), expected(80.0)) < 1e-5, "{m}");
    assert_eq!(m["bodies"][0]["faces"], json!(22), "{m}");
    holes_at(&mut s, 32.0);
    let r = tool(&mut s, "set_parameter", json!({"name": "L", "value": "120 mm"}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    assert!(rel(volume(&mut s), expected(120.0)) < 1e-5);
    holes_at(&mut s, 52.0);
}

/// The skill reference's sketch-plane table: where sketch (10, 20) lands and which way +distance goes.
#[test]
fn sketch_plane_axes_match_the_skill_reference() {
    let mut s = server();
    for plane in ["XY", "XZ", "YZ"] {
        tool(
            &mut s,
            "batch",
            json!({"commands": [
                {"command": "sketch.create", "params": {"plane": plane}},
                {"command": "sketch.circle.center", "params": {"center": [10, 20], "radius": 1}},
                {"command": "sketch.finish"},
                {"command": "solid.extrude", "params": {"distance": 3, "operation": "new"}},
            ]}),
        );
    }
    let m = tool(&mut s, "measure", json!({}));
    let centres: Vec<Vec<f64>> = m["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["center_of_mass"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect())
        .collect();
    let expected = [[10.0, 20.0, 1.5], [10.0, 1.5, -20.0], [1.5, 10.0, 20.0]];
    assert_eq!(centres.len(), 3, "{m}");
    for (got, want) in centres.iter().zip(expected) {
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6), "{got:?} != {want:?}");
    }
}

/// The worked example in `skills/solidcraft-mechanisms/reference.md` is what agents copy: the
/// actuator has to follow the hinge with the stroke the file computes, the mirrored panel its link,
/// and the drive has to stop where the rod reaches its limit.
#[test]
fn mechanisms_skill_example_moves_as_documented() {
    let doc = include_str!("../../../skills/solidcraft-mechanisms/reference.md");
    let start = doc.find("{\"commands\":[").expect("the reference has a worked example");
    let end = start + doc[start..].find("```").expect("the example's code block ends");
    let example: Value = serde_json::from_str(&doc[start..end]).expect("the example is JSON");
    let mut s = server();
    tool(&mut s, "batch", example);
    let exec = |s: &mut Server, command: &str, params: Value| tool(s, "execute", json!({"command": command, "params": params}));
    let value = |list: &Value, joint: &str| {
        let j = list["joints"].as_array().unwrap().iter().find(|j| j["name"] == joint).unwrap_or_else(|| panic!("no joint {joint}: {list}"));
        j["values"][0]["value"].as_f64().unwrap()
    };
    let com = |s: &mut Server, occurrence: &str| -> Vec<f64> {
        let p = exec(s, "occurrence.properties", json!({"occurrence": occurrence}));
        p["total"]["center_of_mass"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
    };
    // Frame 0, the panels and the actuator one motion between them; nothing open.
    let list = exec(&mut s, "joint.list", json!({}));
    assert_eq!(list["conflicts"], json!([]), "{list}");
    let mut dof: Vec<u64> = list["dof"].as_array().unwrap().iter().map(|d| d["dof"].as_u64().unwrap()).collect();
    dof.sort_unstable();
    assert_eq!(dof, [0, 0, 1, 1, 1, 1], "root, Frame, then Wing, Wing 2, barrel and rod: {list}");
    let near = |got: &[f64], want: [f64; 3]| got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-3);
    let wing = com(&mut s, "Wing:1");
    assert!(near(&wing, [0.0, 20.0, 1.5]), "joined where it was modelled: {wing:?}");

    // The tip at (0, 30 cos θ, 30 sin θ), the root at (0, 0, -40): L² = 2500 + 2400 sin θ.
    let stroke = |deg: f64| (2500.0 + 2400.0 * deg.to_radians().sin()).sqrt() - 50.0;
    assert!((stroke(20.0) - 7.6268).abs() < 5e-5, "the reference states {}", stroke(20.0));
    exec(&mut s, "joint.drive", json!({"joint": "Hinge", "value": 20}));
    let list = exec(&mut s, "joint.list", json!({}));
    assert!((value(&list, "Stroke") - stroke(20.0)).abs() < 1e-6, "{list}");
    assert!((value(&list, "Hinge 2") + 20.0).abs() < 1e-9, "{list}");
    let (wing, wing2) = (com(&mut s, "Wing:1"), com(&mut s, "Wing 2:1"));
    assert!(near(&wing, [0.0, 18.281, 8.250]), "{wing:?}");
    assert!(near(&wing2, [0.0, -18.281, 8.250]), "{wing2:?}");

    // The rod reaches +10 mm where L = 60: sin θ = 11/24.
    let stop = (11.0f64 / 24.0).asin().to_degrees();
    assert!((stop - 27.280).abs() < 5e-4, "the reference states {stop}");
    let r = exec(&mut s, "joint.drive", json!({"joint": "Hinge", "value": 45}));
    assert_eq!(r["stopped_by_loop"]["joint"], "Stroke", "{r}");
    let list = exec(&mut s, "joint.list", json!({}));
    let hinge = value(&list, "Hinge");
    assert!((hinge - stop).abs() < 0.01, "stopped at {hinge}, the rod's limit is at {stop}");
    assert!(value(&list, "Stroke") <= 10.0 + 1e-6 && (value(&list, "Stroke") - stroke(hinge)).abs() < 1e-6, "{list}");
}

/// Every tool and command the agent skills call exists, so a rename or removal can't leave the
/// skills sending agents to names the server no longer has.
#[test]
fn skills_name_only_real_tools_and_commands() {
    let docs = [
        include_str!("../../../skills/solidcraft-modeling/SKILL.md"),
        include_str!("../../../skills/solidcraft-modeling/reference.md"),
        include_str!("../../../skills/solidcraft-editing/SKILL.md"),
        include_str!("../../../skills/solidcraft-checking/SKILL.md"),
        include_str!("../../../skills/solidcraft-checking/reference/porting.md"),
        include_str!("../../../skills/solidcraft-mechanisms/SKILL.md"),
        include_str!("../../../skills/solidcraft-mechanisms/reference.md"),
    ];
    let mut s = server();
    let tools: Vec<String> = tool_definitions().iter().filter_map(|t| t["name"].as_str()).map(str::to_owned).collect();
    let commands: Vec<String> =
        tool(&mut s, "list_commands", json!({})).as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).map(str::to_owned).collect();
    let mut checked = 0;
    for doc in docs {
        // A call in backticks is `name {}`, `name {"json":…}` or `name {placeholder}`: a tool when the
        // name is snake_case, a command when it is a dotted id. Field lists such as
        // `sides {bottom,right,top,left}` and `created: {…}` aren't calls. JSON examples name
        // commands as "command":"id".
        let ident = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
        let calls = doc.split('`').skip(1).step_by(2).filter_map(|span| {
            let (head, args) = span.split_once(" {")?;
            let name = head.rsplit(' ').next()?;
            let args = args.strip_suffix('}')?;
            let call_like = args.is_empty() || args.starts_with('"') || args.chars().all(|c| c.is_ascii_lowercase() || c == '_');
            (ident(name) && call_like).then_some(name)
        });
        let in_json = doc.split("\"command\":\"").skip(1).filter_map(|rest| rest.split('"').next());
        for name in calls.chain(in_json) {
            let known = if name.contains('.') { commands.iter().any(|c| c == name) } else { tools.iter().any(|t| t == name) };
            assert!(known, "a skill calls `{name}`, which is neither a tool nor a command");
            checked += 1;
        }
    }
    assert!(checked > 30, "only {checked} tool and command names found in the skills");
}

/// Every tool the server instructions name (a bare `snake_case` word in backticks) is a real tool.
#[test]
fn instructions_name_only_real_tools() {
    let mut s = server();
    let init = call(&mut s, 1, "initialize", json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}));
    let text = init["result"]["instructions"].as_str().unwrap().to_owned();
    let tools: Vec<String> = tool_definitions().iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect();
    let named: Vec<&str> =
        text.split('`').skip(1).step_by(2).filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '_')).collect();
    assert!(named.contains(&"body_topology"), "{named:?}");
    for name in named {
        assert!(tools.iter().any(|t| t == name), "the instructions name `{name}`, which is not a tool");
    }
}

/// A fake app that records what it is asked and answers `engine.batch` as configured.
struct Recorder {
    calls: Vec<(String, Value)>,
    batch: Result<Value, String>,
}

impl crate::backend::Backend for Recorder {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.calls.push((method.to_string(), params.clone()));
        match method {
            "engine.batch" => self.batch.clone(),
            "engine.execute" => match params["command"].as_str() {
                Some("edit.checkpoint") => Ok(json!({"checkpoint": 3})),
                Some(c) => Ok(json!({"ran": c})),
                None => Err("missing `command`".into()),
            },
            "document.inspect" => Ok(json!({"undo": 0})),
            other => Err(format!("unknown method `{other}`")),
        }
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "recorder".into()
    }
}

fn recorded_batch(batch: Result<Value, String>, args: Value) -> (Recorder, Value, bool) {
    let mut b = Recorder { calls: Vec::new(), batch };
    let r = crate::tools::call_tool(&mut b, "batch", &args);
    let body = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap_or_else(|_| r["content"][0]["text"].clone());
    let is_error = r["isError"] == json!(true);
    (b, body, is_error)
}

fn methods(b: &Recorder) -> Vec<&str> {
    b.calls.iter().map(|(m, _)| m.as_str()).collect()
}

/// #743: bridged to an app that has `engine.batch`, a batch is one control request.
#[test]
fn batch_is_one_engine_batch_call() {
    let reply = json!({"completed": 2, "results": [{"feature": 1, "bodies": ["B"], "created_bodies": ["B"]}, {"x": 2}], "undone": 0});
    let steps = json!([{"command": "a.one", "params": {"v": 1}}, {"command": "a.two"}]);
    let (b, body, is_error) = recorded_batch(Ok(reply.clone()), json!({"commands": steps}));
    assert_eq!(methods(&b), ["engine.batch"]);
    assert_eq!(b.calls[0].1, json!({"commands": steps, "rollback": true}));
    assert!(!is_error);
    assert_eq!(body, json!({"ok": true, "completed": 2, "results": reply["results"]}));
    // `verbose: false` compacts each step's result as the per-step path does.
    let (_, body, _) = recorded_batch(Ok(reply), json!({"commands": steps, "verbose": false}));
    assert_eq!(body["results"][0], json!({"feature": 1, "created_bodies": ["B"]}), "{body}");
}

#[test]
fn batch_reports_a_failed_engine_batch_like_before() {
    let reply = json!({"completed": 1, "results": [{"x": 1}], "failed": {"index": 1, "command": "a.two", "error": "boom"}, "undone": 1});
    let (b, body, is_error) = recorded_batch(Ok(reply), json!({"commands": [{"command": "a.one"}, {"command": "a.two"}]}));
    assert_eq!(methods(&b), ["engine.batch"]);
    assert!(is_error);
    assert_eq!(
        body,
        json!({"ok": false, "completed": 1, "failed": {"index": 1, "command": "a.two", "error": "boom"}, "rolled_back": true, "undone": 1, "results": [{"x": 1}]})
    );
}

#[test]
fn batch_falls_back_when_the_app_lacks_engine_batch() {
    let (b, body, is_error) =
        recorded_batch(Err("unknown method `engine.batch`".into()), json!({"commands": [{"command": "a.one"}, {"command": "a.two"}]}));
    assert!(!is_error, "{body}");
    let ran: Vec<&str> = b.calls.iter().filter(|(m, _)| m == "engine.execute").map(|(_, p)| p["command"].as_str().unwrap_or("")).collect();
    assert!(ran.contains(&"a.one") && ran.contains(&"a.two"), "{ran:?}");
    assert_eq!(body["results"], json!([{"ran": "a.one"}, {"ran": "a.two"}]));
}

#[test]
fn batch_falls_back_while_the_agent_cursor_is_shown() {
    let (b, body, is_error) =
        recorded_batch(Err("engine.batch unavailable: the agent cursor is shown".into()), json!({"commands": [{"command": "a.one"}]}));
    assert!(!is_error, "{body}");
    assert!(b.calls.iter().any(|(m, p)| m == "engine.execute" && p["command"] == "a.one"), "{:?}", b.calls);
}

/// The app may have run the batch: sending the steps again would apply them twice.
#[test]
fn batch_does_not_resend_after_a_transport_error() {
    let (b, body, is_error) =
        recorded_batch(Err("SolidCraft at 127.0.0.1:1: timed out".into()), json!({"commands": [{"command": "a.one"}, {"command": "a.two"}]}));
    assert_eq!(methods(&b), ["engine.batch"]);
    assert!(is_error);
    assert!(body.as_str().is_some_and(|t| t.contains("may have run") && t.contains("timed out")), "{body}");
}

/// A batch longer than one control line goes step by step instead of being refused unrun.
#[test]
fn batch_too_large_for_one_control_line_goes_per_step() {
    let big = "x".repeat(solidcraft_engine::MAX_LINE);
    let (b, body, is_error) = recorded_batch(Ok(json!({})), json!({"commands": [{"command": "a.one", "params": {"text": big}}]}));
    assert!(!is_error, "{body}");
    assert!(!methods(&b).contains(&"engine.batch"), "{:?}", methods(&b));
    assert!(b.calls.iter().any(|(m, p)| m == "engine.execute" && p["command"] == "a.one"));
}

/// A fake app on a loopback port: counts connections and `engine.batch` lines, never replies to
/// them (`close_first`: close a connection as soon as it is accepted).
fn silent_app(close_first: bool) -> (String, std::sync::Arc<std::sync::Mutex<(usize, usize)>>) {
    use std::io::BufRead;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let seen = std::sync::Arc::new(std::sync::Mutex::new((0usize, 0usize)));
    let counts = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counts.lock().unwrap().0 += 1;
            let counts = counts.clone();
            std::thread::spawn(move || {
                if close_first && counts.lock().unwrap().0 > 1 {
                    return;
                }
                let mut r = std::io::BufReader::new(stream);
                let mut line = String::new();
                while r.read_line(&mut line).is_ok_and(|n| n > 0) {
                    if line.contains("engine.batch") {
                        counts.lock().unwrap().1 += 1;
                        return;
                    }
                    line.clear();
                }
            });
        }
    });
    (addr, seen)
}

#[test]
fn remote_engine_batch_is_not_resent_after_a_lost_reply() {
    use crate::backend::Backend;
    for close_first in [false, true] {
        let (addr, seen) = silent_app(close_first);
        let mut remote = crate::Remote::connect(&addr).unwrap();
        let r = remote.call("engine.batch", json!({"commands": [{"command": "solid.box"}]}));
        assert!(r.is_err(), "{r:?}");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (connections, batches) = *seen.lock().unwrap();
        // The connection from `connect`, and one fresh one for the batch: no resend.
        assert_eq!(connections, 2, "close_first {close_first}: {connections} connections");
        assert!(batches <= 1, "close_first {close_first}: the batch arrived {batches} times");
        if !close_first {
            assert_eq!(batches, 1);
        }
    }
}

/// A bridge made before the app runs keeps its tools (#270): calls say the app is not running and
/// how to start it (a batch was never sent, so it may not "have run"), and reach the app once it
/// listens.
#[test]
fn a_bridge_made_before_the_app_runs_connects_once_it_does() {
    use std::io::{BufRead, BufReader, Write};

    use crate::backend::Backend;
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    let mut remote = crate::Remote::new(&addr, Some("0123456789abcdef0123456789abcdef"));
    // Still "connected to the SolidCraft app", which the skills read as bridged.
    assert!(remote.describe().starts_with("connected to the SolidCraft app") && remote.describe().contains("not running"), "{}", remote.describe());
    let e = remote.call("engine.execute", json!({"command": "inspect.measure"})).unwrap_err();
    assert!(e.starts_with(crate::backend::NOT_RUNNING) && e.contains(&format!("solidcraft --control {port}")), "{e}");
    let batch = crate::call_tool(&mut remote, "batch", &json!({"commands": [{"command": "solid.box"}]}));
    let text = batch["content"][0]["text"].as_str().unwrap_or_default();
    assert_eq!(batch["isError"], true);
    assert!(text.starts_with(crate::backend::NOT_RUNNING) && !text.contains("may have run"), "{text}");

    let listener = std::net::TcpListener::bind(&addr).unwrap();
    let app = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
        let req: Value = serde_json::from_str(&line).unwrap();
        writeln!(&stream, "{}", json!({"id": req["id"], "ok": true, "result": {"answered": req["params"]["command"]}})).unwrap();
    });
    let got = remote.call("engine.execute", json!({"command": "inspect.measure"})).unwrap();
    assert_eq!(got["answered"], "inspect.measure");
    assert!(remote.describe().starts_with("connected") && !remote.describe().contains("not running"), "{}", remote.describe());
    app.join().unwrap();
}

/// An app that quits while it runs a request may have applied it: the retry then finds nothing
/// listening, and the error says the call may have run, not just that the app is not running.
#[test]
fn an_app_that_quits_during_a_call_may_have_run_it() {
    use std::io::{BufRead, BufReader};

    use crate::backend::Backend;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let app = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
        // Quit without answering: the port closes first, so the retry is refused.
        drop(listener);
        drop(stream);
    });
    let mut remote = crate::Remote::connect(&addr).unwrap();
    let e = remote.call("engine.execute", json!({"command": "solid.box"})).unwrap_err();
    app.join().unwrap();
    assert!(!e.starts_with(crate::backend::NOT_RUNNING) && e.contains("may have quit") && e.contains("Inspect the design"), "{e}");
}

/// Tool annotations never understate what a tool may do (#205): `screenshot` writes a file when
/// given a `path`, and `execute`, `batch`, `save`, `export` and `open` can overwrite or delete
/// files, so none of them is read-only and all are destructive and reach the file system.
#[test]
fn tool_annotations_tell_the_whole_truth() {
    let tools = crate::tool_definitions();
    let hints = |name: &str| {
        let t = tools.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("no tool {name}"));
        let a = &t["annotations"];
        (a["readOnlyHint"].as_bool().unwrap(), a["destructiveHint"].as_bool().unwrap(), a["openWorldHint"].as_bool().unwrap())
    };
    for name in ["execute", "batch", "save", "export", "open", "screenshot"] {
        assert_eq!(hints(name), (false, true, true), "{name}");
    }
    for name in ["list_commands", "inspect_design", "measure", "body_topology", "crash_reports"] {
        assert_eq!(hints(name), (true, false, false), "{name}");
    }
    assert_eq!(hints("new_design"), (false, true, false));
    for name in ["set_parameter", "undo", "redo"] {
        assert_eq!(hints(name), (false, false, false), "{name}");
    }
    assert!(tools.iter().all(|t| t["annotations"]["readOnlyHint"].is_boolean()), "every tool is annotated");
}

/// `measure` with `items` (#1392): a sketch circle's diameter and radius, and two circles'
/// centre-to-centre with the closest and farthest distances.
#[test]
fn measure_items_gives_diameters_and_centre_distances() {
    let mut s = server();
    tool(&mut s, "execute", json!({"command": "sketch.create", "params": {"plane": "XY"}}));
    let a = tool(&mut s, "execute", json!({"command": "sketch.circle.center", "params": {"center": [0, 0], "radius": 3}}));
    let b = tool(&mut s, "execute", json!({"command": "sketch.circle.center", "params": {"center": [30, 40], "radius": 2}}));
    let id = |r: &Value| r["curves"][0].as_str().or(r["result"]["curves"][0].as_str()).unwrap().to_string();
    let (a, b) = (id(&a), id(&b));
    let one = tool(&mut s, "measure", json!({"items": [{"type": "sketch_curve", "id": a}]}));
    assert_eq!((one["diameter"].as_f64(), one["radius"].as_f64()), (Some(6.0), Some(3.0)), "{one}");
    let two = tool(&mut s, "measure", json!({"items": [{"type": "sketch_curve", "id": a}, {"type": "sketch_curve", "id": b}]}));
    assert!((two["center_distance"].as_f64().unwrap() - 50.0).abs() < 1e-9, "{two}");
    assert!((two["min"].as_f64().unwrap() - 45.0).abs() < 1e-9 && (two["max"].as_f64().unwrap() - 55.0).abs() < 1e-9, "{two}");
    assert!(two["distance_mm"].is_number() && two["from"].is_array(), "{two}");
}

/// #1833: `timings` / `profile` time every step run, the failing one included, headless with the
/// kernel's share; bridged, the app is asked to time them and its numbers are passed on.
#[test]
fn batch_reports_timings_and_profile() {
    let mut s = server();
    let steps = json!([
        {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}},
        {"command": "solid.box", "params": {"length": 4, "width": 4, "height": 30, "operation": "join"}},
    ]);
    let r = raw_tool(&mut s, "batch", json!({"commands": steps, "profile": true}));
    let body: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let rows = body["timings"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{body}");
    assert_eq!((rows[1]["index"].as_u64(), rows[1]["command"].as_str()), (Some(1), Some("solid.box")));
    assert!(rows[1]["profile"]["boolean_ms"].as_f64().unwrap() > 0.0, "{body}");
    assert!(body["total_ms"].as_f64().is_some(), "{body}");

    let r = raw_tool(
        &mut s,
        "batch",
        json!({"timings": true, "commands": [{"command": "solid.box", "params": {"length": 1, "width": 1, "height": 1}}, {"command": "NoSuchCommand"}]}),
    );
    let body: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["timings"].as_array().map(Vec::len), Some(2), "the failing step is timed too: {body}");
    assert!(body["timings"][0].get("profile").is_none());
    // Not asked: no timings.
    let d = tool(&mut s, "batch", json!({"commands": [{"command": "solid.box", "params": {"length": 1, "width": 1, "height": 1}}]}));
    assert!(d.get("timings").is_none(), "{d}");

    let reply =
        json!({"completed": 1, "results": [{"x": 1}], "undone": 0, "timings": [{"index": 0, "command": "a.one", "ms": 2.5}], "total_ms": 2.6});
    let (b, body, _) = recorded_batch(Ok(reply), json!({"commands": [{"command": "a.one"}], "profile": true}));
    assert_eq!(b.calls[0].1["timings"], true);
    assert_eq!(b.calls[0].1["profile"], true);
    assert_eq!((body["timings"][0]["ms"].as_f64(), body["total_ms"].as_f64()), (Some(2.5), Some(2.6)), "{body}");
    // An app without engine.batch is timed step by step here, without a kernel split it can't measure.
    let (_, body, _) = recorded_batch(Err("unknown method `engine.batch`".into()), json!({"commands": [{"command": "a.one"}], "profile": true}));
    assert_eq!(body["timings"][0]["command"], "a.one", "{body}");
    assert!(body["timings"][0].get("profile").is_none(), "{body}");
}
