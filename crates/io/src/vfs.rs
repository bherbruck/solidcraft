//! File access for the engine. Natively it is the disk. In the browser (wasm32) there is no
//! file system to call synchronously, so files live in memory here: the web host loads them
//! (from the browser's private storage, a file picker or a drop) before a command reads them,
//! and takes what commands wrote ([`take_writes`]) to store it or hand it out as a download.

use std::io::{Error, ErrorKind, Result};

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::*;

    /// At most `max` bytes of a regular file. A character device such as `/dev/zero` reports a
    /// length of 0, so checking the metadata and then reading to the end is unbounded; a FIFO
    /// blocks the caller instead. Refuse anything that is not a regular file, and never read
    /// more than the caller's cap plus the byte that proves it was exceeded.
    pub fn read(path: &str, max: usize) -> Result<Vec<u8>> {
        read_with_progress(path, max, |done, total| solidcraft_doc::progress::work_step("reading", done, total))
    }

    /// Read a regular file, reporting bytes read and file size. Returning true stops the read.
    pub(super) fn read_with_progress(path: &str, max: usize, mut stop: impl FnMut(usize, usize) -> bool) -> Result<Vec<u8>> {
        use std::io::Read;

        check(path)?;
        // Cheap and non-blocking: refuse before opening, in case the open itself would block.
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(Error::new(ErrorKind::InvalidInput, "not a regular file"));
        }
        if meta.len() > max as u64 {
            return Err(Error::new(ErrorKind::InvalidData, "file too large"));
        }
        let f = std::fs::File::open(path)?;
        // The path may have been swapped between the two calls; the handle decides.
        let meta = f.metadata()?;
        if !meta.is_file() {
            return Err(Error::new(ErrorKind::InvalidInput, "not a regular file"));
        }
        if meta.len() > max as u64 {
            return Err(Error::new(ErrorKind::InvalidData, "file too large"));
        }
        let total = meta.len() as usize;
        if stop(0, total) {
            return Err(Error::new(ErrorKind::Interrupted, "file read cancelled"));
        }
        let mut out = Vec::with_capacity(meta.len().min(16 << 20) as usize);
        let mut reader = f.take((max as u64).saturating_add(1));
        loop {
            let n = reader.by_ref().take(256 << 10).read_to_end(&mut out)?;
            if out.len() > max {
                return Err(Error::new(ErrorKind::InvalidData, "file too large"));
            }
            if n == 0 {
                break;
            }
            if stop(out.len(), total) {
                return Err(Error::new(ErrorKind::Interrupted, "file read cancelled"));
            }
        }
        Ok(out)
    }
    pub fn write(path: &str, bytes: &[u8]) -> Result<()> {
        check(path)?;
        std::fs::write(path, bytes)
    }
    pub fn len(path: &str) -> Result<u64> {
        check(path)?;
        std::fs::metadata(path).map(|m| m.len())
    }
    pub fn exists(path: &str) -> bool {
        check(path).is_ok() && std::path::Path::new(path).exists()
    }
    pub fn remove(path: &str) -> Result<()> {
        check(path)?;
        std::fs::remove_file(path)
    }
    /// A folder and the folders above it, where missing.
    pub fn create_dir_all(path: &str) -> Result<()> {
        check(path)?;
        std::fs::create_dir_all(path)
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::*;

    thread_local! {
        static FILES: RefCell<BTreeMap<String, Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
        static WRITES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn missing(path: &str) -> Error {
        Error::new(ErrorKind::NotFound, format!("{path}: no such file in this browser"))
    }

    /// At most `max` bytes, to match the native signature.
    pub fn read(path: &str, max: usize) -> Result<Vec<u8>> {
        FILES.with(|f| {
            let files = f.borrow();
            let bytes = files.get(path).ok_or_else(|| missing(path))?;
            let total = bytes.len();
            if total > max {
                return Err(Error::new(ErrorKind::InvalidData, "file too large"));
            }
            if solidcraft_doc::progress::work_step("reading", 0, total) {
                return Err(Error::new(ErrorKind::Interrupted, "file read cancelled"));
            }
            let out = bytes.clone();
            if solidcraft_doc::progress::work_step("reading", total, total) {
                return Err(Error::new(ErrorKind::Interrupted, "file read cancelled"));
            }
            Ok(out)
        })
    }
    pub fn write(path: &str, bytes: &[u8]) -> Result<()> {
        FILES.with(|f| f.borrow_mut().insert(path.to_string(), bytes.to_vec()));
        WRITES.with(|w| {
            let mut w = w.borrow_mut();
            w.retain(|p| p != path);
            w.push(path.to_string());
        });
        Ok(())
    }
    pub fn len(path: &str) -> Result<u64> {
        FILES.with(|f| f.borrow().get(path).map(|b| b.len() as u64)).ok_or_else(|| missing(path))
    }
    pub fn exists(path: &str) -> bool {
        FILES.with(|f| f.borrow().contains_key(path))
    }
    pub fn remove(path: &str) -> Result<()> {
        FILES.with(|f| f.borrow_mut().remove(path)).map(|_| ()).ok_or_else(|| missing(path))
    }
    /// Files are keyed by their whole path here: a folder needs no making.
    pub fn create_dir_all(_path: &str) -> Result<()> {
        Ok(())
    }
    pub fn insert(path: &str, bytes: Vec<u8>) {
        FILES.with(|f| f.borrow_mut().insert(path.to_string(), bytes));
    }
    pub fn take_writes() -> Vec<(String, Vec<u8>)> {
        let paths = WRITES.with(|w| std::mem::take(&mut *w.borrow_mut()));
        paths.into_iter().filter_map(|p| FILES.with(|f| f.borrow().get(&p).cloned()).map(|b| (p, b))).collect()
    }
    pub fn list(prefix: &str) -> Vec<(String, u64)> {
        FILES.with(|f| f.borrow().iter().filter(|(p, _)| p.starts_with(prefix)).map(|(p, b)| (p.clone(), b.len() as u64)).collect())
    }
}

pub use imp::*;

/// The folders an agent's commands may read, write and delete in (an MCP server's, see
/// docs/mcp.md). While a scope is active on this thread ([`scoped`]), every file call here
/// refuses paths outside it; without one (the user's own file dialogs, the CLI) nothing changes.
///
/// Two kinds of folder: those named on purpose (`--allow-path`, the app's own recovery folder),
/// where everything is reachable, and the open design's folder, which counts only when
/// [`design_folder_refusal`] has nothing against it and never reaches hidden files or folders
/// (`.env`, `.git`) inside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathScope {
    roots: Vec<std::path::PathBuf>,
    design_roots: Vec<std::path::PathBuf>,
}

thread_local! {
    static SCOPE: std::cell::RefCell<Option<PathScope>> = const { std::cell::RefCell::new(None) };
}

/// `dir` resolved, if it exists.
fn canonical(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::canonicalize(dir).ok()
}

/// The user's home folder, resolved.
fn home_dir() -> Option<std::path::PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|h| !h.is_empty()).and_then(|h| canonical(std::path::Path::new(&h)))
}

/// A folder or file name an agent never reaches by default: hidden on Unix (`.ssh`, `.env`),
/// and on Windows also the per-user `AppData` folder.
fn hidden(name: &std::ffi::OsStr) -> bool {
    let n = name.to_string_lossy();
    n.starts_with('.') || (cfg!(windows) && n.eq_ignore_ascii_case("AppData"))
}

/// Why `dir` (an open design's folder) can't be one of an agent's folders by default, or `None`
/// when it can: it must lie strictly inside the home folder (or the temporary folder) and not
/// in a hidden folder. So a design saved in home itself, in `/`, on a drive root or in a system
/// folder never puts `~/.ssh`, `~/.bashrc` or anything else of the user's in reach.
pub fn design_folder_refusal(dir: &std::path::Path) -> Option<String> {
    design_folder_refusal_in(dir, home_dir().as_deref(), Some(&std::env::temp_dir()))
}

/// [`design_folder_refusal`] with the home and temporary folders given (tests).
pub fn design_folder_refusal_in(dir: &std::path::Path, home: Option<&std::path::Path>, temp: Option<&std::path::Path>) -> Option<String> {
    let Some(d) = canonical(dir) else { return Some("it doesn't exist".into()) };
    if d.parent().is_none() {
        return Some("it is a drive or file-system root".into());
    }
    let home = home.and_then(canonical);
    if home.as_ref().is_some_and(|h| h.starts_with(&d)) {
        return Some("it is your home folder or a folder above it".into());
    }
    let temp = temp.and_then(canonical).filter(|t| t.parent().is_some());
    // The closer of the two (a temporary folder may lie inside home, in a hidden folder).
    let Some(base) = [home, temp].into_iter().flatten().filter(|b| d.starts_with(b) && d != *b).max_by_key(|b| b.components().count()) else {
        return Some("it is not inside your home folder".into());
    };
    let rel = d.strip_prefix(&base).ok()?;
    if rel.components().any(|c| hidden(c.as_os_str())) {
        return Some("it is inside a hidden folder".into());
    }
    None
}

impl PathScope {
    /// Confine file access to `roots` (resolved now; folders that don't exist are left out, so an
    /// empty scope refuses every path).
    pub fn new(roots: impl IntoIterator<Item = std::path::PathBuf>) -> PathScope {
        let mut out = PathScope::default();
        for r in roots.into_iter().filter_map(|r| canonical(&r)) {
            if !out.roots.contains(&r) {
                out.roots.push(r);
            }
        }
        out
    }

    /// Also the folder of the design at `path` (the open design), when
    /// [`design_folder_refusal`] allows it; hidden entries inside it stay out of reach.
    pub fn with_design(mut self, path: Option<&str>) -> PathScope {
        let dir = path
            .map(std::path::Path::new)
            .and_then(std::path::Path::parent)
            .map(|d| if d.as_os_str().is_empty() { std::path::Path::new(".") } else { d });
        if let Some(d) = dir.filter(|d| design_folder_refusal(d).is_none()).and_then(canonical)
            && !self.design_roots.contains(&d)
        {
            self.design_roots.push(d);
        }
        self
    }

    /// The folders named on purpose.
    pub fn roots(&self) -> &[std::path::PathBuf] {
        &self.roots
    }

    /// The open design's folder, when it counts.
    pub fn design_roots(&self) -> &[std::path::PathBuf] {
        &self.design_roots
    }

    /// Whether `path` lies inside one of the folders, after following symlinks and `..`.
    pub fn check(&self, path: &str) -> Result<()> {
        let refused = || {
            let roots: Vec<String> = self.roots.iter().chain(&self.design_roots).map(|r| r.display().to_string()).collect();
            Error::new(
                ErrorKind::PermissionDenied,
                format!(
                    "{path}: outside the folders this agent may use ({}); allow another folder with `--allow-path DIR`",
                    if roots.is_empty() { "none".into() } else { roots.join(", ") }
                ),
            )
        };
        let windows = |why: &str| Error::new(ErrorKind::PermissionDenied, format!("{path}: not allowed for this agent: {why}"));
        // Judged from the text before any filesystem call: resolving a UNC path would already
        // make Windows connect out, and a device name opens the device.
        if cfg!(windows)
            && let Some(why) = windows_refusal(path)
        {
            return Err(windows(why));
        }
        let resolved = match resolve(std::path::Path::new(path)) {
            Ok(r) => r,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(self.missing_folder(path).unwrap_or_else(refused)),
            Err(_) => return Err(refused()),
        };
        // Judged again once resolved: a link or junction inside an allowed folder may lead to a
        // network share or a device even though the path given looked local.
        if cfg!(windows) && !local_drive_path(&resolved) {
            return Err(windows("only paths on a local drive"));
        }
        if self.contains(&resolved) { Ok(()) } else { Err(refused()) }
    }

    /// Whether the resolved path `p` lies inside one of the folders.
    fn contains(&self, p: &std::path::Path) -> bool {
        let inside_design = || self.design_roots.iter().any(|r| p.strip_prefix(r).is_ok_and(|rel| !rel.components().any(|c| hidden(c.as_os_str()))));
        self.roots.iter().any(|r| p.starts_with(r)) || inside_design()
    }

    /// The "folder does not exist" error for a path whose folder is missing, but only when the
    /// nearest folder that does exist is one of the scope's own: telling the user which folder is
    /// missing must not become a way to probe the disk outside it.
    fn missing_folder(&self, path: &str) -> Option<Error> {
        let given = std::path::Path::new(path);
        // A dangling link (the path itself, or in its folders) is not a missing folder: it leads
        // somewhere, and that is refused.
        let mut missing = given.parent()?;
        let existing = loop {
            let here = if missing.as_os_str().is_empty() { std::path::Path::new(".") } else { missing };
            if let Ok(c) = std::fs::canonicalize(here) {
                break c;
            }
            if std::fs::symlink_metadata(here).is_ok() {
                return None;
            }
            missing = missing.parent()?;
        };
        if std::fs::symlink_metadata(given).is_ok() {
            return None;
        }
        self.contains(&existing).then(|| {
            Error::new(
                ErrorKind::NotFound,
                format!("{path}: its folder does not exist (nothing exists below {}); create the folder first", existing.display()),
            )
        })
    }
}

/// Whether a resolved path is on a local drive (`C:\…` or `\\?\C:\…`) with no `:` or device
/// name in it. Always true off Windows, where these rules don't apply.
fn local_drive_path(p: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        let drive = matches!(
            p.components().next(),
            Some(std::path::Component::Prefix(x)) if matches!(x.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))
        );
        drive && windows_refusal(&p.to_string_lossy()).is_none()
    }
    #[cfg(not(windows))]
    {
        let _ = p;
        true
    }
}

/// Why Windows would not treat `path` as a plain file on a local drive, judged by Windows rules
/// from the text alone (so it is tested on every platform): UNC, device (`\\.\`), NT (`\??\`) and
/// verbatim paths other than a verbatim drive (`\\?\C:\`), drive-relative paths (`C:file`), a `:`
/// inside a name (alternate data streams), reserved device names (`NUL`, `COM1.txt`, …) and
/// short (8.3) names.
pub fn windows_refusal(path: &str) -> Option<&'static str> {
    let norm = path.replace('/', "\\");
    let drive = |s: &str| matches!(s.as_bytes(), [d, b':', ..] if d.is_ascii_alphabetic());
    let drive_abs = |s: &str| matches!(s.as_bytes(), [d, b':', b'\\', ..] if d.is_ascii_alphabetic());
    let body = if let Some(v) = norm.strip_prefix("\\\\?\\") {
        if !drive_abs(v) {
            return Some("only paths on a local drive (no UNC, device or volume paths)");
        }
        v.get(2..).unwrap_or("")
    } else if norm.starts_with("\\\\") || norm.starts_with("\\??\\") {
        return Some("network (UNC), device and NT namespace paths");
    } else if drive(&norm) {
        if !drive_abs(&norm) {
            return Some("drive-relative paths (`C:file`)");
        }
        norm.get(2..).unwrap_or("")
    } else {
        &norm
    };
    for name in body.split('\\').filter(|n| !n.is_empty()) {
        if name.contains(':') {
            return Some("`:` in a file name (alternate data streams)");
        }
        // A short (8.3) name such as `PROGRA~1` or `SSH~1` is another spelling of a folder whose
        // real name could be anything; canonicalising expands existing ones, and a new file's
        // name must not be one.
        if short_name(name) {
            return Some("short (8.3) names such as `PROGRA~1`");
        }
        let stem = name.split('.').next().unwrap_or("").trim_end_matches([' ', '.']).to_uppercase();
        let numbered =
            |p: &str| stem.strip_prefix(p).is_some_and(|n| matches!(n, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT") {
            return Some("reserved device names (CON, NUL, COM1, …)");
        }
    }
    None
}

/// Whether a file or folder name looks like a short (8.3) name: a `~` and a digit (`PROGRA~1`).
fn short_name(name: &str) -> bool {
    name.as_bytes().windows(2).any(|w| w[0] == b'~' && w[1].is_ascii_digit())
}

/// `path` spelled with long names, for the folders SolidCraft derives from the environment
/// itself (the temporary folder, the recovery folder). On Windows those may hold a short (8.3)
/// name: the temporary folder is often `…\NAME~1.XYZ\AppData\Local\Temp`, with the user folder's
/// short name, when that folder's name is long, and [`windows_refusal`] refuses short names in
/// every path an agent's commands reach, the server's own folders included. A path with a short
/// name is resolved as far as it exists (the rest is appended as given) and loses the `\\?\`
/// prefix resolving adds. It stays as it is when it has no short name, can't be resolved or
/// doesn't resolve to a local drive, and off Windows.
pub fn long_names(path: &std::path::Path) -> std::path::PathBuf {
    if !cfg!(windows) || !path.iter().any(|n| short_name(&n.to_string_lossy())) {
        return path.to_path_buf();
    }
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(full) = std::fs::canonicalize(at) {
            let local = full
                .to_str()
                .and_then(|f| f.strip_prefix(r"\\?\"))
                .filter(|f| matches!(f.as_bytes(), [d, b':', b'\\', ..] if d.is_ascii_alphabetic()));
            return local.map_or_else(|| path.to_path_buf(), |f| rest.iter().rev().fold(std::path::PathBuf::from(f), |p, n| p.join(n)));
        }
        let (Some(parent), Some(name)) = (at.parent(), at.file_name()) else { return path.to_path_buf() };
        rest.push(name);
        at = parent;
    }
}

/// The temporary folder spelled with long names ([`long_names`]), for the folders SolidCraft
/// makes in it.
pub fn temp_dir() -> std::path::PathBuf {
    long_names(&std::env::temp_dir())
}

/// `path` with symlinks and `..` resolved. A file that doesn't exist yet resolves through its
/// folder, which must exist; a symlink whose target is missing is refused (writing through it
/// would land wherever it points).
fn resolve(path: &std::path::Path) -> Result<std::path::PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == ErrorKind::NotFound && std::fs::symlink_metadata(path).is_err() => {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return Err(e) };
            let parent = if parent.as_os_str().is_empty() { std::path::Path::new(".") } else { parent };
            Ok(std::fs::canonicalize(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

/// Run `f` with file access confined to `scope` on this thread (`None`: unconfined).
pub fn scoped<T>(scope: Option<PathScope>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathScope>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            SCOPE.with(|s| *s.borrow_mut() = prev);
        }
    }
    // Sketch text reads the font files a design names, below this crate: it asks here first.
    solidcraft_doc::sketch::set_font_file_check(|p| check(&p.to_string_lossy()).map_err(|e| e.to_string()));
    let _restore = Restore(SCOPE.with(|s| s.replace(scope)));
    f()
}

/// The scope active on this thread.
pub fn current_scope() -> Option<PathScope> {
    SCOPE.with(|s| s.borrow().clone())
}

/// Refuse `path` if a scope is active and it lies outside. Code that reaches the disk without
/// going through this module calls it first. In the browser files are the host's, never the
/// disk, so there is nothing to confine.
// ponytail: checked then opened, so a local process racing a symlink swap in an allowed folder
// could still redirect one call; opening by handle (openat/O_NOFOLLOW) would close that.
pub fn check(path: &str) -> Result<()> {
    if cfg!(target_arch = "wasm32") {
        return Ok(());
    }
    SCOPE.with(|s| s.borrow().as_ref().map_or(Ok(()), |scope| scope.check(path)))
}

/// A file's text, at most `max` bytes.
pub fn read_to_string(path: &str, max: usize) -> Result<String> {
    String::from_utf8(read(path, max)?).map_err(|e| Error::new(ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_file_read_reports_bytes_before_completion_and_can_be_cancelled_between_chunks() {
        let path = std::env::temp_dir().join(format!("solidcraft-vfs-progress-{}", std::process::id()));
        let path = path.to_string_lossy();
        let bytes: Vec<_> = (0..1_000_003).map(|i| (i % 251) as u8).collect();
        super::write(&path, &bytes).unwrap();
        let mut reports = Vec::new();
        let read = super::imp::read_with_progress(&path, bytes.len(), |done, total| {
            reports.push((done, total));
            false
        })
        .unwrap();
        assert_eq!(read, bytes);
        assert_eq!(reports.first(), Some(&(0, bytes.len())));
        assert_eq!(reports.last(), Some(&(bytes.len(), bytes.len())));
        assert!(reports.iter().any(|&(done, total)| done > 0 && done < total));
        assert!(reports.windows(2).all(|p| p[0].0 < p[1].0 && p[1].1 == bytes.len()));

        let error = super::imp::read_with_progress(&path, bytes.len(), |done, _| done > 0).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        let progress = std::sync::Arc::new(solidcraft_doc::progress::Progress::default());
        solidcraft_doc::progress::watch(progress.clone(), || super::read(&path, bytes.len())).unwrap();
        assert_eq!(progress.work(), Some(("reading", bytes.len(), bytes.len())));
        progress.cancel();
        let error = solidcraft_doc::progress::watch(progress, || super::read(&path, bytes.len())).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        super::remove(&path).unwrap();
    }

    #[test]
    fn files_round_trip() {
        let dir = std::env::temp_dir().join(format!("solidcraft-vfs-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("a.txt").to_string_lossy().to_string();
        assert!(!super::exists(&p));
        assert!(super::read(&p, 16).is_err());
        super::write(&p, b"hello").unwrap();
        assert!(super::exists(&p));
        assert_eq!(super::len(&p).unwrap(), 5);
        assert_eq!(super::read_to_string(&p, 16).unwrap(), "hello");
        // The cap is the caller's, not the file's.
        assert!(super::read(&p, 4).is_err());
        assert_eq!(super::read(&p, 5).unwrap(), b"hello");
        super::remove(&p).unwrap();
        assert!(!super::exists(&p));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `metadata().len()` is 0 for a character device and for a FIFO, so a cap checked against
    /// it passes and the read then runs to an end that never comes. Refuse what is not a
    /// regular file, and do it before opening: opening a FIFO blocks until a writer appears.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn special_files_are_refused_without_blocking() {
        let e = super::read("/dev/zero", 1024).unwrap_err();
        assert!(e.to_string().contains("not a regular file"), "{e}");
        assert!(super::read_to_string("/dev/zero", 1024).is_err());

        let dir = std::env::temp_dir().join(format!("solidcraft-vfs-fifo-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let fifo = dir.join("p");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status().map(|s| s.success()).unwrap_or(false);
        if made {
            let path = fifo.to_string_lossy().to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(super::read(&path, 1024).is_err());
            });
            let refused = rx.recv_timeout(std::time::Duration::from_secs(5));
            assert_eq!(refused, Ok(true), "reading a FIFO must be refused, not block");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An agent's scope (#205): inside its folders files work as before; outside, through `..`
    /// or through a symlink that leads out, every call is refused with a clear error and the
    /// disk is left alone. Without a scope nothing is confined.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn a_scope_confines_every_call_to_its_folders() {
        use super::{PathScope, scoped};
        let base = std::env::temp_dir().join(format!("solidcraft-vfs-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, out) = (base.join("root"), base.join("out"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&out, root.join("link")).unwrap();
        std::os::unix::fs::symlink(out.join("missing.txt"), root.join("dangling")).unwrap();
        let s = |p: std::path::PathBuf| p.to_string_lossy().to_string();
        let scope = PathScope::new([root.clone()]);
        scoped(Some(scope.clone()), || {
            let inside = s(root.join("sub/a.txt"));
            super::write(&inside, b"ok").unwrap();
            assert_eq!(super::read(&inside, 16).unwrap(), b"ok");
            assert_eq!(super::len(&inside).unwrap(), 2);
            assert!(super::exists(&inside));
            super::remove(&inside).unwrap();
            for escape in [out.join("secret.txt"), root.join("../out/secret.txt"), root.join("link/secret.txt"), root.join("dangling")] {
                let p = s(escape);
                let e = super::read(&p, 16).unwrap_err();
                assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{p}: {e}");
                assert!(e.to_string().contains("outside the folders this agent may use"), "{e}");
                assert!(super::write(&p, b"x").is_err() && super::remove(&p).is_err() && super::len(&p).is_err() && !super::exists(&p), "{p}");
            }
            assert!(super::write(&s(out.join("new.txt")), b"x").is_err());
            let e = super::check(&s(root.join("dangling/x.txt"))).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
            assert!(super::check(&s(root.join("sub/../sub/b.txt"))).is_ok(), "`..` that stays inside is fine");
        });
        assert_eq!(std::fs::read_to_string(out.join("secret.txt")).unwrap(), "secret");
        assert!(!out.join("new.txt").exists() && !out.join("missing.txt").exists());
        // The scope ends with the call; an empty one refuses everything.
        assert!(super::current_scope().is_none());
        assert_eq!(super::read(&s(out.join("secret.txt")), 16).unwrap(), b"secret");
        assert!(PathScope::new([base.join("nonexistent")]).check(&s(root.join("sub/a.txt"))).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Review of #205: a root is a folder, not a string prefix (`a` doesn't admit `ab`); a new
    /// file needs an existing folder, and one whose folder is a symlink that leads out is refused.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn roots_are_folders_and_new_files_resolve_through_their_folder() {
        use super::PathScope;
        let base = std::env::temp_dir().join(format!("solidcraft-vfs-prefix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (a, ab, out) = (base.join("a"), base.join("ab"), base.join("out"));
        for d in [&a, &ab, &out] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::os::unix::fs::symlink(&out, a.join("link")).unwrap();
        let scope = PathScope::new([a.clone()]);
        let s = |p: std::path::PathBuf| p.to_string_lossy().to_string();
        assert!(scope.check(&s(a.join("new.txt"))).is_ok());
        for refused in [ab.join("x.txt"), a.join("link/new.txt"), base.join("missing/x.txt")] {
            let e = scope.check(&s(refused.clone())).unwrap_err();
            assert!(e.to_string().contains("outside the folders this agent may use"), "{}: {e}", refused.display());
        }
        // A folder that is missing inside an allowed one is reported as missing (#1077), not as
        // out of scope: widening `--allow-path` would not help. Nothing is created.
        for missing in [a.join("missing/x.txt"), a.join("missing/deeper/x.txt")] {
            let e = scope.check(&s(missing.clone())).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{}: {e}", missing.display());
            assert!(e.to_string().contains("its folder does not exist") && !e.to_string().contains("--allow-path"), "{e}");
        }
        super::scoped(Some(scope), || assert!(super::write(&s(a.join("link/new.txt")), b"x").is_err()));
        assert!(!out.join("new.txt").exists() && !a.join("missing").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Windows rules, judged from the text alone, on every platform (#205 review).
    #[test]
    fn windows_paths_that_are_not_plain_local_files_are_refused() {
        use super::windows_refusal as w;
        for ok in [
            r"C:\work\part.step",
            "C:/work/part.step",
            r"\\?\C:\work\part.step",
            r"work\part.step",
            "part.step",
            r"\work\a.b.step",
            r"C:\com10\x.txt",
            r"C:\console.txt",
            r"\\?\c:\work\part.step",
            r"C:\work\draft~v2.step",
        ] {
            assert_eq!(w(ok), None, "{ok}");
        }
        for bad in [
            r"\\server\share\x.step",
            "//server/share/x.step",
            r"\\?\UNC\server\share\x.step",
            r"\??\UNC\server\share\x.step",
            r"\??\C:\x.step",
            "/??/UNC/server/share/x.step",
            r"\\.\COM1",
            r"\\?\Volume{0}\x",
            r"\\?\GLOBALROOT\Device\x",
            "C:foo.step",
            r"C:\work\a.step:stream",
            r"C:\work\NUL",
            r"C:\work\nul.txt",
            r"C:\work\COM1.step",
            r"C:\work\lpt9",
            r"C:\work\Con .txt",
            r"C:\work\COM¹",
            r"C:\work\CONOUT$",
            r"work\AUX\x.step",
            // Every spelling of the NT, device and UNC prefixes.
            r"\\?\unc\server\share\x.step",
            "//?/UNC/server/share/x.step",
            r"/\?\UNC\server\share\x.step",
            r"\??/UNC/server/share/x.step",
            r"\\.\C:\x.step",
            "//./pipe/x",
            r"\\?\C:",
            r"\\server",
            // Device names with any extension, in any case.
            r"C:\work\aux.tar.gz",
            r"C:\work\Prn.",
            r"C:\work\lPt1.step",
            r"C:\work\conin$.txt",
            // Alternate data streams, and short (8.3) names.
            r"C:\work\a.step::$DATA",
            r"C:\Users\me\SSH~1\id_ed25519",
            r"C:\PROGRA~1\x.step",
        ] {
            assert!(w(bad).is_some(), "{bad}");
        }
    }

    /// The same, through a scope on Windows itself: refused before any filesystem call.
    #[cfg(windows)]
    #[test]
    fn windows_unc_verbatim_drive_relative_and_device_paths_are_refused() {
        let root = super::temp_dir().join(format!("solidcraft-vfs-win-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let scope = super::PathScope::new([root.clone()]);
        let inside = |f: &str| root.join(f).to_string_lossy().to_string();
        assert!(scope.check(&inside("ok.step")).is_ok());
        for bad in [
            r"\\server\share\x.step".to_string(),
            r"\\?\UNC\server\share\x.step".to_string(),
            r"\??\UNC\server\share\x.step".to_string(),
            r"\\.\COM1".to_string(),
            "C:foo.step".to_string(),
            inside("NUL"),
            inside("COM1.txt"),
            inside("con"),
            inside("a.step:stream"),
        ] {
            let e = scope.check(&bad).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{bad}: {e}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The short (8.3) spelling of an existing `path` as `cmd` gives it (`NAME~1.XYZ`), or `None`
    /// when it has no short name (the volume makes none).
    #[cfg(windows)]
    fn short_spelling(path: &std::path::Path) -> Option<std::path::PathBuf> {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("cmd").raw_arg(format!("/d /c for %I in (\"{}\") do @echo %~sI", path.display())).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| s.split('\\').any(super::short_name)).map(Into::into)
    }

    /// The folders SolidCraft derives from the environment are spelled with long names: on Windows
    /// the temporary folder's path often has a short (8.3) name (the user folder's `NAME~1.XYZ`
    /// when its name is long), and the server's own folders in it were refused like an agent's
    /// short names. An agent's own short spelling of a folder it may use stays refused.
    #[cfg(windows)]
    #[test]
    fn derived_folders_are_spelled_with_long_names_and_short_names_from_agents_stay_refused() {
        use super::{PathScope, long_names, short_name, temp_dir};
        assert!(!temp_dir().iter().any(|n| short_name(&n.to_string_lossy())), "{}", temp_dir().display());
        let long = temp_dir().join(format!("solidcraft-vfs-long-names-{}", std::process::id()));
        std::fs::create_dir_all(&long).unwrap();
        let Some(short) = short_spelling(&long) else {
            eprintln!("skipped: {} has no short (8.3) name on this volume", long.display());
            let _ = std::fs::remove_dir_all(&long);
            return;
        };
        let expanded = long_names(&short);
        assert_eq!(std::fs::canonicalize(&expanded).unwrap(), std::fs::canonicalize(&long).unwrap());
        assert!(super::windows_refusal(&expanded.to_string_lossy()).is_none(), "{}", expanded.display());
        // What doesn't exist yet is kept as given; a path without a short name is left alone.
        assert_eq!(long_names(&short.join("new folder").join("part.step")), expanded.join("new folder").join("part.step"));
        assert_eq!(long_names(&long), long);
        // The folder through a scope: its long spelling is in reach, an agent's short one is not.
        let scope = PathScope::new([long.clone()]);
        assert!(scope.check(&expanded.join("part.step").to_string_lossy()).is_ok());
        let e = scope.check(&short.join("part.step").to_string_lossy()).unwrap_err();
        assert!(e.to_string().contains("short (8.3) names"), "{e}");
        let _ = std::fs::remove_dir_all(&long);
    }

    /// The open design's folder counts only when it is safe (#205): strictly inside home (or the
    /// temporary folder), not home itself, a folder above it, a root or a hidden folder; and
    /// hidden files inside it stay out of reach. Folders named on purpose reach everything.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn the_designs_folder_counts_only_when_safe_and_never_reaches_hidden_files() {
        use super::{PathScope, design_folder_refusal_in as refusal};
        let base = std::env::temp_dir().join(format!("solidcraft-vfs-design-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        let (proj, dotdir) = (home.join("work/proj"), home.join(".config/x"));
        for d in [&proj.join("sub"), &dotdir, &home.join(".ssh"), &proj.join(".git")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let sibling = base.join("homex");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), "key").unwrap();
        std::fs::write(proj.join(".env"), "x").unwrap();
        std::os::unix::fs::symlink(home.join(".ssh"), proj.join("keys")).unwrap();
        let h = Some(home.as_path());
        assert_eq!(refusal(&proj, h, None), None);
        assert_eq!(refusal(&home.join("work/../work/proj"), h, None), None);
        for (dir, why) in [
            (home.clone(), "home"),
            (base.clone(), "above home"),
            ("/".into(), "root"),
            (sibling.clone(), "a name that only starts like home"),
            (dotdir.clone(), "hidden"),
            (base.join("missing"), "doesn't exist"),
        ] {
            assert!(refusal(&dir, h, None).is_some(), "{why}: {}", dir.display());
        }
        assert!(refusal(&proj, None, None).is_some(), "no home known: nothing implicit");
        assert_eq!(refusal(&base.join("homex"), None, Some(&base)), None, "inside the temporary folder");
        assert!(refusal(&base, None, Some(&base)).is_some(), "the temporary folder itself");

        // Through a scope: the design's folder reaches its own files, never hidden ones, and
        // never a link that leads to them.
        let s = |p: std::path::PathBuf| p.to_string_lossy().to_string();
        let mut scope = PathScope::new([]);
        // `with_design` judges with the real home; build the folder list directly instead.
        scope.design_roots.push(std::fs::canonicalize(&proj).unwrap());
        assert!(scope.check(&s(proj.join("part.step"))).is_ok());
        assert!(scope.check(&s(proj.join("sub/../part.step"))).is_ok());
        for bad in
            [proj.join(".env"), proj.join(".git/config"), proj.join("keys/id_ed25519"), proj.join("../../.ssh/id_ed25519"), home.join(".bashrc")]
        {
            assert!(scope.check(&s(bad.clone())).is_err(), "{}", bad.display());
        }
        // Named on purpose, a folder reaches its hidden files too.
        assert!(PathScope::new([proj.clone()]).check(&s(proj.join(".env"))).is_ok());
        // A design saved at `/` or in a missing folder adds nothing.
        assert!(PathScope::new([]).with_design(Some("/part.solidcraft")).design_roots().is_empty());
        assert!(PathScope::new([]).with_design(Some(&s(base.join("missing/p.solidcraft")))).design_roots().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A save follows a symbolic link to the file it points to (#845), so the link's target is
    /// what a scope judges: a link inside an allowed folder that leads out is refused, a chain of
    /// links that stays inside is fine.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn a_link_is_judged_by_where_it_leads() {
        use super::PathScope;
        let base = std::env::temp_dir().join(format!("solidcraft-vfs-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, out) = (base.join("root"), base.join("out"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("part.solidcraft"), "outside").unwrap();
        std::fs::write(root.join("real.solidcraft"), "inside").unwrap();
        std::os::unix::fs::symlink(out.join("part.solidcraft"), root.join("part.solidcraft")).unwrap();
        std::os::unix::fs::symlink("real.solidcraft", root.join("a.solidcraft")).unwrap();
        std::os::unix::fs::symlink("a.solidcraft", root.join("b.solidcraft")).unwrap();
        std::os::unix::fs::symlink("../out/new.solidcraft", root.join("dangling.solidcraft")).unwrap();
        let scope = PathScope::new([root.clone()]);
        let s = |p: std::path::PathBuf| p.to_string_lossy().to_string();
        assert!(scope.check(&s(root.join("part.solidcraft"))).is_err());
        assert!(scope.check(&s(root.join("dangling.solidcraft"))).is_err());
        assert!(scope.check(&s(root.join("b.solidcraft"))).is_ok());
        super::scoped(Some(scope), || {
            let path = s(root.join("part.solidcraft"));
            let refused = super::check(&path).and_then(|()| crate::safe_write::write_atomic(std::path::Path::new(&path), b"agent", false));
            assert!(refused.is_err());
        });
        assert_eq!(std::fs::read_to_string(out.join("part.solidcraft")).unwrap(), "outside");
        assert!(!out.join("new.solidcraft").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A directory is not a file either.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn directories_are_not_files() {
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        assert!(super::read(&dir, 1024).is_err());
    }
}
