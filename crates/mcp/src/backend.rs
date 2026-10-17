//! Where MCP tool calls end up: a control-channel method call (`docs/control-protocol.md`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::render::StandardView;
use solidcraft_engine::view::Guides;

/// Something that answers control-channel methods: `engine.execute`, `engine.commands`,
/// `document.inspect`, `ui.view`, `ui.render`, `ui.screenshot`.
pub trait Backend {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    /// True when a real window is attached (`ui.screenshot` can capture it).
    fn has_ui(&self) -> bool;
    fn describe(&self) -> String;
    /// The open design's file, when this side knows it (its folder joins the agent's folders,
    /// see `vfs::PathScope::with_design`). A running app adds its own, so `Remote` says nothing.
    fn design_path(&self) -> Option<String> {
        None
    }
    /// Commands run in this process, on this thread (the kernel's time in them can be measured
    /// here, #1833).
    fn in_process(&self) -> bool {
        false
    }
}

/// Longest reply line accepted from the app.
const MAX_REPLY: u64 = 64 << 20;

/// How long a reply may take: kernel operations (booleans, fillets) on big parts can take a while.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the app may take to run an `engine.batch` (its control server waits as long).
const BATCH_TIMEOUT: Duration = Duration::from_secs(3600);

/// A running SolidCraft app reached through its loopback control port
/// (`solidcraft --control PORT`).
pub struct Remote {
    addr: String,
    port: u16,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: u64,
    /// `--token`, when given. Otherwise each request looks the token up again, since every
    /// launch of the app makes a new one (docs/control-protocol.md).
    explicit_token: Option<String>,
}

/// How a tool error starts when no app listens at the bridged address. The agent reads it, and
/// `batch` uses it to tell "never sent" from "sent, reply lost".
pub(crate) const NOT_RUNNING: &str = "no SolidCraft app is listening at";

impl Remote {
    /// Connect to the app at `addr`, with its control token found the usual way
    /// (`SOLIDCRAFT_CONTROL_TOKEN`, else the app's token file for that port).
    pub fn connect(addr: &str) -> std::io::Result<Self> {
        Self::connect_with_token(addr, None)
    }

    /// [`Remote::connect`] with the token given (`--token`); `None` finds it the usual way.
    pub fn connect_with_token(addr: &str, token: Option<&str>) -> std::io::Result<Self> {
        let mut r = Self::new(addr, token);
        r.reconnect()?;
        Ok(r)
    }

    /// A bridge to the app at `addr` that connects when a call needs it: an MCP server started
    /// before the app keeps its tools, and they reach the app once it runs.
    pub fn new(addr: &str, token: Option<&str>) -> Self {
        let port = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
        let explicit_token = token.map(str::to_string);
        Self { addr: addr.to_string(), port, conn: None, next_id: 1, explicit_token }
    }

    /// Connect now, if the app is there (`new` does not); the error says why not.
    pub fn try_connect(&mut self) -> std::io::Result<()> {
        self.reconnect()
    }

    /// Failing to reach the app at all is `NotConnected`, so [`Remote::explain`] can tell it from
    /// a connection lost during a call.
    fn reconnect(&mut self) -> std::io::Result<()> {
        self.conn = None;
        let not_connected = |e: std::io::Error| std::io::Error::new(std::io::ErrorKind::NotConnected, e);
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("cannot resolve {}", self.addr));
        for sa in self.addr.to_socket_addrs().map_err(not_connected)? {
            match TcpStream::connect_timeout(&sa, Duration::from_millis(800)) {
                Ok(s) => {
                    s.set_nodelay(true).ok();
                    s.set_read_timeout(Some(READ_TIMEOUT)).ok();
                    let read = s.try_clone()?;
                    self.conn = Some((BufReader::new(read), s));
                    return Ok(());
                }
                Err(e) => last = e,
            }
        }
        Err(not_connected(last))
    }

    /// The tool error for a failed call: whether the app is not running (and how to start it) or
    /// went away during the call.
    fn explain(&self, e: &std::io::Error) -> String {
        let (addr, port) = (&self.addr, self.port);
        if e.kind() == std::io::ErrorKind::NotConnected {
            format!(
                "{NOT_RUNNING} {addr} ({e}): it is not running, or it quit. Start it with `solidcraft --control {port}` \
                 (or ask the user to); the next call connects to it"
            )
        } else {
            format!(
                "the SolidCraft app at {addr} did not answer ({e}): it may have quit or crashed during this call, after                  running it or not. Inspect the design before sending a change again; if the app quit, start it with                  `solidcraft --control {port}`"
            )
        }
    }

    fn roundtrip(&mut self, line: &str) -> std::io::Result<String> {
        if self.conn.is_none() {
            self.reconnect()?;
        }
        let Some((reader, writer)) = self.conn.as_mut() else { return Err(std::io::Error::other("not connected")) };
        // One write: a request that loses its newline still reaches the app as a request.
        let sent = writer.write_all(format!("{line}\n").as_bytes()).and_then(|()| writer.flush());
        if let Err(e) = sent {
            self.conn = None;
            return Err(e);
        }
        let mut reply = String::new();
        let n = match reader.by_ref().take(MAX_REPLY).read_line(&mut reply) {
            Ok(n) => n,
            Err(e) => {
                // A timed-out read leaves a late reply on the stream: start afresh next time.
                self.conn = None;
                return Err(e);
            }
        };
        if n == 0 || !reply.ends_with('\n') {
            self.conn = None;
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed"));
        }
        Ok(reply)
    }

    fn set_read_timeout(&self, t: Duration) {
        if let Some((_, writer)) = &self.conn {
            writer.set_read_timeout(Some(t)).ok();
        }
    }

    /// `engine.batch` is never sent twice (#743): it goes on a fresh connection (an idle one may
    /// have been closed by an app restart; connecting again sends nothing), and once it is being
    /// written any error is final, as the app may have run it.
    fn send_batch(&mut self, line: &str) -> std::io::Result<String> {
        self.reconnect().or_else(|_| self.reconnect())?;
        self.set_read_timeout(BATCH_TIMEOUT);
        let r = self.roundtrip(line);
        self.set_read_timeout(READ_TIMEOUT);
        r
    }
}

impl Backend for Remote {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut params = params;
        // The app confines the files this request reaches to the agent's folders (control.rs).
        if let (Some(scope), Some(o)) = (solidcraft_engine::io::vfs::current_scope(), params.as_object_mut()) {
            o.insert("allow_paths".into(), json!(scope.roots().iter().map(|r| r.to_string_lossy()).collect::<Vec<_>>()));
        }
        let mut msg = json!({"id": id, "method": method, "params": params});
        // The token goes with every request (an app restarted mid-session has a new one).
        if let Some(t) = solidcraft_engine::control_auth::client_token(self.port, self.explicit_token.as_deref()) {
            msg["token"] = json!(t);
        }
        let line = msg.to_string();
        let reply = if method == "engine.batch" {
            self.send_batch(&line)
        } else {
            // One retry on a fresh connection (the app may have restarted). When nothing listens
            // any more, the app went away, maybe while running this request: say that, not
            // just "not running".
            self.roundtrip(&line).or_else(|lost| {
                self.roundtrip(&line).map_err(|e| if e.kind() == std::io::ErrorKind::NotConnected && lost.kind() != e.kind() { lost } else { e })
            })
        }
        .map_err(|e| self.explain(&e))?;
        let v: Value = serde_json::from_str(&reply).map_err(|e| format!("bad reply from the app: {e}"))?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string())
        }
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        if self.conn.is_some() {
            format!("connected to the SolidCraft app at {} (the user watches the viewport)", self.addr)
        } else {
            // Still "connected to the SolidCraft app": the skills tell bridged from headless by it.
            format!(
                "connected to the SolidCraft app at {} once it runs (the user watches the viewport). It was not running \
                 when this server started; until it is (`solidcraft --control {}`), tool calls say so",
                self.addr, self.port
            )
        }
    }
}

/// An in-process headless session (no window); renders with the CPU rasterizer.
pub struct Headless {
    pub session: Session,
    /// Orientation for renders; the camera is refitted to the model every time.
    pub view: StandardView,
}

impl Default for Headless {
    fn default() -> Self {
        Headless::new(Session::default())
    }
}

impl Headless {
    pub fn new(mut session: Session) -> Self {
        match solidcraft_engine::render::job::CpuHost::for_platform() {
            Ok(host) => {
                if let Err(e) = session.set_render_job_host(Some(host)) {
                    session.log.push(format!("image host unavailable: {e}"));
                }
            }
            Err(e) => session.log.push(format!("image host unavailable: {e}")),
        }
        Headless { session, view: StandardView::Iso }
    }
}

fn dim(p: &Value, k: &str, default: f64) -> u32 {
    p.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(default).clamp(16.0, 4096.0) as u32
}

impl Backend for Headless {
    fn call(&mut self, method: &str, p: Value) -> Result<Value, String> {
        let s = &mut self.session;
        let str_p = |k: &str| p.get(k).and_then(Value::as_str);
        match method {
            "engine.execute" | "command" => {
                let id = str_p("command").ok_or("missing `command`")?;
                let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
                s.execute(id, &params).map_err(|e| e.to_string())
            }
            "engine.script" => s.run_script(&p).map(Value::from).map_err(|e| e.to_string()),
            "engine.commands" => s.execute("engine.commands", &json!({})).map_err(|e| e.to_string()),
            "document.inspect" => s.execute("document.inspect", &p).map_err(|e| e.to_string()),
            "ui.view" => {
                let v = str_p("view").unwrap_or("home");
                match v {
                    // Fit keeps the orientation, also of a camera `view.apply` chose.
                    "fit" => {
                        let b = solidcraft_engine::view::bounds(s);
                        if let Some(c) = s.camera.as_mut() {
                            c.fit(&b);
                        }
                    }
                    _ => {
                        self.view = StandardView::parse(v).ok_or_else(|| format!("unknown view `{v}`"))?;
                        s.camera = None;
                    }
                }
                Ok(json!({"view": self.view}))
            }
            "ui.render" => {
                let (w, h) = (dim(&p, "width", 1280.0), dim(&p, "height", 800.0));
                // The camera `view.apply` chose, else the standard view framing the model.
                let cam = s.camera.unwrap_or_else(|| {
                    let mut cam = solidcraft_engine::view::home_camera(s);
                    cam.set_view(self.view);
                    cam
                });
                // The grid and the axes are drawn unless the request leaves them out.
                let shown = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(true);
                let guides = Guides { grid: shown("grid"), axes: shown("axes") };
                let path = str_p("path").ok_or("missing `path`")?;
                // A path outside the agent's folders is refused before anything is drawn.
                solidcraft_engine::io::vfs::check(path).map_err(|e| format!("{path}: {e}"))?;
                let scene = solidcraft_engine::view::scene_with_guides(s, &cam, s.section, guides);
                // Drawn on the GPU by the renderer the host installed (`solidcraft-cli mcp`).
                let png = solidcraft_engine::capture::png(&scene, &cam, w, h)?;
                solidcraft_engine::io::vfs::write(path, &png.bytes).map_err(|e| format!("{path}: {e}"))?;
                Ok(json!({
                    "path": path,
                    "bytes": png.bytes.len(),
                    "width": w,
                    "height": h,
                    "renderer": png.renderer,
                    "warning": solidcraft_engine::capture::warning(png.renderer),
                }))
            }
            "ui.screenshot" => Err("no window in headless mode; use ui.render".into()),
            other => Err(format!("unknown method `{other}`")),
        }
    }
    fn has_ui(&self) -> bool {
        false
    }
    fn describe(&self) -> String {
        "headless in-process SolidCraft session".into()
    }
    fn design_path(&self) -> Option<String> {
        self.session.path.clone()
    }
    fn in_process(&self) -> bool {
        true
    }
}
