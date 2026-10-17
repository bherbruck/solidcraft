//! `mcp --connect` to a port where no app listens yet (#270): the server still starts, its tools
//! say how to start the app, and once the app runs they reach it with that launch's token.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[test]
fn mcp_connect_serves_before_the_app_runs_and_reaches_it_once_it_does() {
    // This test needs the port closed until its second phase. A port obtained
    // with bind(:0) and released can immediately be reused by another parallel
    // test's mock server. Stay below the usual ephemeral ranges,
    // with a process-specific starting point, and skip any occupied ports.
    let port = (0..512u32)
        .find_map(|offset| {
            let port = 16000 + (std::process::id() + offset) % 16000;
            TcpListener::bind(("127.0.0.1", port as u16)).ok().map(|listener| listener.local_addr().unwrap().port())
        })
        .expect("an unused non-ephemeral test port");
    let addr = format!("127.0.0.1:{port}");
    let control_dir = std::env::temp_dir().join(format!("solidcraft-mcp-connect-{}-{port}", std::process::id()));
    std::fs::create_dir_all(&control_dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_solidcraft-cli"))
        .args(["mcp", "--connect", &addr, "--allow-any-path"])
        .env("SOLIDCRAFT_CONTROL_DIR", &control_dir)
        .env_remove("SOLIDCRAFT_CONTROL_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |id: u64, method: &str, params: Value| -> Value {
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    };
    let measure = json!({"name": "measure", "arguments": {}});
    let text = |r: &Value| r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();

    // The server answers without the app, and its instructions say the app isn't running.
    let init = ask(1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    let instructions = init["result"]["instructions"].as_str().unwrap_or_default();
    assert!(instructions.contains("not running") && instructions.contains(&format!("--control {port}")), "{instructions}");
    let tools = ask(2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().is_some_and(|t| !t.is_empty()), "{tools}");

    // A tool call says the app is not running and how to start it.
    let r = ask(3, "tools/call", measure.clone());
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(text(&r).contains("not running") && text(&r).contains(&format!("solidcraft --control {port}")), "{r}");

    // The app starts: a token file for this launch, and its control port.
    let token = "0123456789abcdef".repeat(4);
    std::fs::write(control_dir.join(format!("{port}.token")), &token).unwrap();
    let listener = TcpListener::bind(&addr).unwrap();
    let app = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
        let req: Value = serde_json::from_str(&line).unwrap();
        let reply = if req["token"] == json!(token) {
            json!({"id": req["id"], "ok": true, "result": {"bodies": [], "method": req["method"]}})
        } else {
            json!({"id": req["id"], "ok": false, "error": format!("unauthorized: token {}", req["token"])})
        };
        writeln!(&stream, "{reply}").unwrap();
    });
    let r = ask(4, "tools/call", measure);
    assert_ne!(r["result"]["isError"], true, "{r}");
    assert!(text(&r).contains("engine.execute"), "{r}");
    app.join().unwrap();

    drop(stdin);
    assert!(child.wait().unwrap().success());
    std::fs::remove_dir_all(&control_dir).ok();
}

#[test]
fn mcp_connect_without_a_port_is_refused_at_start() {
    for addr in ["localhost", "127.0.0.1:0"] {
        let output = Command::new(env!("CARGO_BIN_EXE_solidcraft-cli")).args(["mcp", "--connect", addr]).stdin(Stdio::null()).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{addr}: {stderr}");
        assert!(stderr.contains("HOST:PORT"), "{addr}: {stderr}");
    }
}
