//! Update notifications (docs/book/src/guide/updates.md). At every start, and once a day after
//! that while the app stays open, a background thread asks GitHub for the latest stable release (`releases/latest`; nightlies and
//! release candidates are never "latest"). When it is newer than this build, an Update button
//! appears in the application bar beside Report a bug; it opens a small window with the version,
//! the release notes and Download, Skip this version and Remind me later.
//!
//! The request is the only thing sent: no identifiers, no usage data. A short timeout keeps it
//! from hanging, a failed check (offline, rate limited) stays silent and is retried later, and
//! development builds never check. Turning off Preferences ▸ General ▸ Check for updates
//! automatically stops the automatic checks; Check now still works.
//!
//! The host provides the HTTP client (`Services::http_get`), so this crate stays portable and
//! the browser build never checks (it updates itself through its service worker).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{FontId, RichText, text::LayoutJob};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::SolveApp;
use crate::theme::Tokens;

/// The repository releases come from.
pub const REPO: &str = "bherbruck/solidcraft";
/// The one request an update check makes.
pub const LATEST_API: &str = "https://api.github.com/repos/bherbruck/solidcraft/releases/latest";
/// Where Download goes when no package fits this installation.
pub const RELEASES_PAGE: &str = "https://github.com/bherbruck/solidcraft/releases/latest";
/// Release pages and assets must be on GitHub, under this repository. The repository is moving
/// to the `tangency-labs` organisation, so releases may name either owner (the API request stays
/// on the old name, which GitHub redirects after a transfer).
const RELEASE_PREFIXES: [&str; 2] = ["https://github.com/bherbruck/solidcraft/releases/", "https://github.com/tangency-labs/solidcraft/releases/"];

/// A release page or asset URL: github.com, one of the two owners, exactly `solidcraft`, then
/// `/releases/` (the trailing slash rules out look-alike names such as `solidcraft-evil`).
fn is_release_url(url: &str) -> bool {
    RELEASE_PREFIXES.iter().any(|p| url.starts_with(p))
}

/// While the app stays open, automatic checks run once a day (every start checks too).
pub const CHECK_INTERVAL_S: f64 = 24.0 * 3600.0;
/// Remind me later hides the button for three days.
pub const REMIND_DELAY_S: f64 = 3.0 * 24.0 * 3600.0;
/// Check now can't be pressed more often than this (GitHub allows 60 anonymous requests an hour).
pub const MANUAL_COOLDOWN_S: f64 = 30.0;
/// After a failed automatic check, the open app tries again after this long (an hour).
pub const RETRY_AFTER_FAILURE_S: f64 = 3600.0;
/// Caps on what a release response may hold (it comes from the network).
const MAX_NOTES: usize = 64 * 1024;
const MAX_ASSETS: usize = 400;
/// The room the Update pill's long label leaves for the document tabs.
const TAB_ROOM: f32 = 300.0;
/// The Update window's width, and its release notes' wrap width.
const WINDOW_W: f32 = 460.0;
const NOTES_W: f32 = WINDOW_W - 30.0;

/// The Linux app ID (Flatpak, desktop files).
pub const APP_ID: &str = "ai.storyteller.solidcraft";

/// Why a check failed. Automatic checks never show it; Check now does.
#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    /// GitHub answered 403/429 with a rate limit: don't ask again for this many seconds.
    RateLimited { retry_after_s: u64 },
    /// Offline, timed out, a bad answer, ...
    Failed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::RateLimited { retry_after_s } => {
                write!(f, "GitHub's rate limit was reached; try again in {} min", retry_after_s.div_ceil(60))
            }
            FetchError::Failed(e) => f.write_str(e),
        }
    }
}

/// The host's HTTP GET (HTTPS, short timeout): the body of a 200 answer, or why not.
pub type HttpGet = Arc<dyn Fn(&str) -> Result<Vec<u8>, FetchError> + Send + Sync>;

// ---- versions ---------------------------------------------------------------------------------

/// A version as SolidCraft's tags and builds spell it: `0.10.0`, `v0.11.0-rc.1`,
/// `0.9.0-nightly.20261014.36.1`.
pub fn parse_version(v: &str) -> Option<semver::Version> {
    let v = v.trim();
    semver::Version::parse(v.strip_prefix('v').unwrap_or(v)).ok()
}

/// What a build is compared as. A nightly is built from `main` after the release it is
/// numbered from (`0.9.0-nightly.…` contains everything in 0.9.0 and more), so it counts as that
/// release, not as something before it: it is offered only a strictly newer stable release.
pub fn comparable(v: &semver::Version) -> semver::Version {
    let mut v = v.clone();
    if v.pre.as_str().starts_with("nightly") {
        v.pre = semver::Prerelease::EMPTY;
    }
    v.build = semver::BuildMetadata::EMPTY;
    v
}

/// `latest` (a release tag) is a stable release newer than the `current` build.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(latest), Some(current)) = (parse_version(latest), parse_version(current)) else { return false };
    latest.pre.is_empty() && latest > comparable(&current)
}

/// This build's version: the release workflow's `SOLIDCRAFT_VERSION` (with any nightly suffix)
/// when it built this binary, else the crate version.
pub fn current_version() -> &'static str {
    option_env!("SOLIDCRAFT_VERSION").filter(|v| !v.is_empty()).unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// The release workflow built this binary (`SOLIDCRAFT_VERSION` is set there). Anything else is a
/// development build, which never checks for updates.
pub fn release_build() -> bool {
    option_env!("SOLIDCRAFT_VERSION").is_some_and(|v| !v.is_empty()) && !cfg!(debug_assertions)
}

// ---- installations ----------------------------------------------------------------------------

/// How this copy of SolidCraft was installed: decides which file Download fetches and whether
/// the system's package manager is the way to update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Built from source (or by something other than the release workflow): never checks.
    Dev,
    /// The browser app (updates itself).
    Web,
    /// An AppImage, at this path.
    AppImage(PathBuf),
    /// A Flatpak, with this app ID. It has no network access and Flatpak updates it: never checks.
    Flatpak(String),
    /// A Snap: from the Snap Store (`store`), or a downloaded `.snap` installed with `--dangerous`.
    Snap {
        store: bool,
    },
    Deb,
    Rpm,
    /// The plain `.tar.gz`, unpacked somewhere.
    LinuxTarball,
    /// Installed by the `.msi` (under Program Files, or in the folder the installer recorded).
    WindowsMsi,
    /// The portable `.zip`, unpacked somewhere.
    WindowsPortable,
    /// The lone `SolidCraft-<version>-windows-x64.exe`.
    WindowsExe,
    /// `SolidCraft.app`, at this path (the bundle).
    MacApp(PathBuf),
    /// A release build in a place none of the above explains (FreeBSD, a copied binary).
    Other,
}

/// What [`detect`] looks at, so tests can describe any machine.
pub struct Probe<'a> {
    /// `std::env::consts::OS`.
    pub os: &'a str,
    pub release_build: bool,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// The running executable.
    pub exe: Option<PathBuf>,
    pub exists: &'a dyn Fn(&Path) -> bool,
    /// Where the Windows installer put SolidCraft (`HKLM\Software\SolidCraft`, `InstallDir`): a
    /// folder chosen in its wizard can be anywhere, not only under Program Files (#1870).
    pub msi_dir: Option<PathBuf>,
}

pub fn detect(p: &Probe) -> InstallKind {
    if p.os == "unknown" || cfg!(target_arch = "wasm32") {
        return InstallKind::Web;
    }
    if !p.release_build {
        return InstallKind::Dev;
    }
    let env = |k: &str| (p.env)(k).filter(|v| !v.trim().is_empty());
    let Some(exe) = p.exe.clone() else { return InstallKind::Other };
    match p.os {
        "linux" => {
            if let Some(id) = env("FLATPAK_ID") {
                return InstallKind::Flatpak(id);
            }
            if env("SNAP").is_some() && env("SNAP_NAME").is_none_or(|n| n.starts_with("solidcraft")) {
                // Locally installed (unasserted) snaps have revisions x1, x2, …
                return InstallKind::Snap { store: !env("SNAP_REVISION").is_some_and(|r| r.starts_with('x')) };
            }
            if let Some(image) = env("APPIMAGE") {
                return InstallKind::AppImage(PathBuf::from(image));
            }
            if exe.starts_with("/usr") {
                // nfpm installs both packages under /usr; dpkg keeps a file list per package.
                if (p.exists)(Path::new("/var/lib/dpkg/info/solidcraft.list")) {
                    return InstallKind::Deb;
                }
                if (p.exists)(Path::new("/var/lib/rpm")) || (p.exists)(Path::new("/usr/lib/sysimage/rpm")) {
                    return InstallKind::Rpm;
                }
                return InstallKind::Other;
            }
            InstallKind::LinuxTarball
        }
        "windows" => {
            let under = |k: &str| env(k).is_some_and(|dir| starts_with_ci(&exe, Path::new(&dir)));
            let installed = p.msi_dir.as_deref().is_some_and(|dir| starts_with_ci(&exe, dir));
            if installed || under("ProgramFiles") || under("ProgramW6432") || under("ProgramFiles(x86)") {
                return InstallKind::WindowsMsi;
            }
            if exe.parent().is_some_and(|d| (p.exists)(&d.join("solidcraft-cli.exe"))) {
                return InstallKind::WindowsPortable;
            }
            InstallKind::WindowsExe
        }
        "macos" => app_bundle(&exe).map_or(InstallKind::Other, InstallKind::MacApp),
        _ => InstallKind::Other,
    }
}

/// `path` is inside `dir` (Windows paths: case-insensitive).
fn starts_with_ci(path: &Path, dir: &Path) -> bool {
    let (p, d) = (path.to_string_lossy().to_lowercase().replace('/', "\\"), dir.to_string_lossy().to_lowercase().replace('/', "\\"));
    let d = d.trim_end_matches('\\');
    !d.is_empty() && p.strip_prefix(d).is_some_and(|rest| rest.starts_with('\\'))
}

/// The `.app` bundle an executable at `…/X.app/Contents/MacOS/x` belongs to.
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// This machine's installation, looked up once.
pub fn this_install() -> InstallKind {
    #[cfg(target_arch = "wasm32")]
    {
        InstallKind::Web
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let env = |k: &str| std::env::var(k).ok();
        let exists = |p: &Path| p.exists();
        let msi_dir = msi_install_dir();
        detect(&Probe {
            os: std::env::consts::OS,
            release_build: release_build(),
            env: &env,
            exe: std::env::current_exe().ok(),
            exists: &exists,
            msi_dir,
        })
    }
}

/// The folder the Windows installer recorded (solidcraft.wxs writes it for upgrades), if any.
#[cfg(windows)]
fn msi_install_dir() -> Option<PathBuf> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE).open_subkey(r"Software\SolidCraft").ok()?;
    key.get_value::<String, _>("InstallDir").ok().map(PathBuf::from)
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
fn msi_install_dir() -> Option<PathBuf> {
    None
}

impl InstallKind {
    /// The package manager updates it: Download isn't the way (the window shows the command).
    pub fn system_managed(&self) -> bool {
        matches!(self, InstallKind::Flatpak(_) | InstallKind::Snap { store: true })
    }

    /// A short name for Preferences and docs.
    pub fn label(&self) -> &'static str {
        match self {
            InstallKind::Dev => "development build",
            InstallKind::Web => "browser app",
            InstallKind::AppImage(_) => "AppImage",
            InstallKind::Flatpak(_) => "Flatpak",
            InstallKind::Snap { store: true } => "Snap",
            InstallKind::Snap { store: false } => "Snap (installed from a file)",
            InstallKind::Deb => ".deb package",
            InstallKind::Rpm => ".rpm package",
            InstallKind::LinuxTarball => ".tar.gz",
            InstallKind::WindowsMsi => "Windows installer (.msi)",
            InstallKind::WindowsPortable => "portable .zip",
            InstallKind::WindowsExe => "standalone .exe",
            InstallKind::MacApp(_) => "macOS app",
            InstallKind::Other => "other",
        }
    }

    /// Release builds check; development builds, the browser app and the Flatpak (no network
    /// permission; Flatpak updates it) don't.
    pub fn checks(&self) -> bool {
        !matches!(self, InstallKind::Dev | InstallKind::Web | InstallKind::Flatpak(_))
    }

    /// The command that updates a package-managed installation, if there is one.
    pub fn update_command(&self, asset: Option<&str>) -> Option<String> {
        match self {
            InstallKind::Flatpak(id) => Some(format!("flatpak update {id}")),
            InstallKind::Snap { store: true } => Some("sudo snap refresh solidcraft".into()),
            InstallKind::Snap { store: false } => {
                Some(format!("sudo snap install --dangerous ./{}", asset.unwrap_or("solidcraft-linux-x86_64.snap")))
            }
            InstallKind::Deb => Some(format!("sudo apt install ./{}", asset.unwrap_or("solidcraft-linux-x86_64.deb"))),
            InstallKind::Rpm => Some(format!("sudo dnf install ./{}", asset.unwrap_or("solidcraft-linux-x86_64.rpm"))),
            _ => None,
        }
    }
}

/// The release asset that installs or replaces this installation, by its fixed name
/// (docs/releasing.md: `solidcraft-windows-x64.msi`, `solidcraft-linux-x86_64.AppImage`, …), or
/// the versioned name for the packages without one. `arch` is `std::env::consts::ARCH`.
pub fn asset_name(kind: &InstallKind, arch: &str, version: &str) -> Option<String> {
    let linux = match arch {
        "x86_64" | "aarch64" => Some(arch),
        _ => None,
    };
    let windows = match arch {
        "x86_64" => Some("x64"),
        "aarch64" => Some("arm64"),
        "x86" => Some("x86"),
        _ => None,
    };
    let version = version.trim_start_matches('v');
    Some(match kind {
        InstallKind::AppImage(_) => format!("solidcraft-linux-{}.AppImage", linux?),
        InstallKind::Flatpak(_) => format!("solidcraft-linux-{}.flatpak", linux?),
        InstallKind::Snap { .. } => format!("solidcraft-linux-{}.snap", linux?),
        InstallKind::Deb => format!("solidcraft-linux-{}.deb", linux?),
        InstallKind::Rpm => format!("solidcraft-linux-{}.rpm", linux?),
        InstallKind::LinuxTarball => format!("solidcraft-{version}-linux-{}.tar.gz", linux?),
        InstallKind::WindowsMsi => format!("solidcraft-windows-{}.msi", windows?),
        InstallKind::WindowsPortable => format!("solidcraft-windows-{}-portable.zip", windows?),
        // Only x64 has the lone .exe; elsewhere the portable zip is the same thing.
        InstallKind::WindowsExe if windows == Some("x64") => format!("SolidCraft-{version}-windows-x64.exe"),
        InstallKind::WindowsExe => format!("solidcraft-windows-{}-portable.zip", windows?),
        InstallKind::MacApp(_) => "solidcraft-macos-universal.dmg".into(),
        InstallKind::Dev | InstallKind::Web | InstallKind::Other => return None,
    })
}

// ---- releases ---------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// The parts of a GitHub release an update needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub name: String,
    /// Release notes (Markdown).
    pub notes: String,
    /// The release's page.
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// A `releases/latest` answer. Drafts and prereleases are refused (GitHub never returns them
    /// there, but the answer comes from the network).
    pub fn parse(json: &[u8]) -> Result<Release, FetchError> {
        let v: Value = serde_json::from_slice(json).map_err(|e| FetchError::Failed(format!("unexpected answer from GitHub: {e}")))?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        if v.get("draft").and_then(Value::as_bool).unwrap_or(false) || v.get("prerelease").and_then(Value::as_bool).unwrap_or(false) {
            return Err(FetchError::Failed("the latest release is a prerelease".into()));
        }
        let tag = s("tag_name");
        if parse_version(&tag).is_none() {
            return Err(FetchError::Failed(format!("the latest release's tag `{}` isn't a version", tag.chars().take(40).collect::<String>())));
        }
        let mut notes = s("body");
        if notes.len() > MAX_NOTES {
            let mut cut = MAX_NOTES;
            while !notes.is_char_boundary(cut) {
                cut -= 1;
            }
            notes.truncate(cut);
            notes.push_str("\n\n…");
        }
        let page = Some(s("html_url")).filter(|u| is_release_url(u)).unwrap_or_else(|| RELEASES_PAGE.to_string());
        let assets = v
            .get("assets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(MAX_ASSETS)
            .filter_map(|a| {
                let name = a.get("name")?.as_str()?.to_string();
                let url = a.get("browser_download_url")?.as_str()?.to_string();
                is_release_url(&url).then(|| Asset { name, url, size: a.get("size").and_then(Value::as_u64).unwrap_or(0) })
            })
            .collect();
        Ok(Release {
            name: Some(s("name")).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("SolidCraft {tag}")),
            tag,
            notes,
            page,
            assets,
        })
    }

    /// The version without the `v`.
    pub fn version(&self) -> &str {
        self.tag.trim_start_matches('v')
    }

    /// The asset that fits `kind` on `arch`, if this release has it.
    pub fn asset_for(&self, kind: &InstallKind, arch: &str) -> Option<&Asset> {
        let name = asset_name(kind, arch, self.version())?;
        self.assets.iter().find(|a| a.name == name)
    }

    /// What Download opens: the fitting file, else the release page. Package-managed
    /// installations get the page (their package manager does the update).
    pub fn download_url(&self, kind: &InstallKind, arch: &str) -> &str {
        if kind.system_managed() {
            return &self.page;
        }
        self.asset_for(kind, arch).map_or(self.page.as_str(), |a| a.url.as_str())
    }
}

// ---- what is remembered -----------------------------------------------------------------------

/// Update bookkeeping, kept with the preferences (not design data). Times are Unix seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memo {
    /// When the last check finished (successfully or not).
    pub last_check: f64,
    /// The next automatic check while the app stays open: a day after a successful one, an hour
    /// after a failed one. A new start doesn't wait for it.
    pub next_check: f64,
    /// GitHub's rate limit was reached: no request at all before this, not even Check now.
    pub rate_limited_until: f64,
    /// Skip this version: its button never shows (a later version's does).
    pub skipped: String,
    /// Remind me later: no button before this time.
    pub remind_after: f64,
}

impl Memo {
    /// An automatic check is due. `launch`: no automatic check has run since SolidCraft started,
    /// so one is due whatever the daily schedule says (someone who started yesterday's version
    /// this morning hears about today's release at the next start). GitHub's rate limit always
    /// holds. A clock set back by more than a day counts as due.
    pub fn due(&self, now: f64, launch: bool) -> bool {
        let limited = now < self.rate_limited_until && self.rate_limited_until - now <= CHECK_INTERVAL_S;
        !limited && (launch || now >= self.next_check || self.next_check - now > CHECK_INTERVAL_S)
    }

    /// The Update button shows for `release`.
    pub fn offers(&self, release: &Release, current: &str, now: f64) -> bool {
        is_newer(&release.tag, current) && !same_version(&self.skipped, &release.tag) && now >= self.remind_after
    }

    pub fn skip(&mut self, release: &Release) {
        self.skipped = release.version().to_string();
    }

    pub fn remind_later(&mut self, now: f64) {
        self.remind_after = now + REMIND_DELAY_S;
    }

    /// A check finished: the next one is a day later after a success, an hour after a failure,
    /// and after a rate limit as long as GitHub said (at least a minute, at most a day).
    pub fn checked(&mut self, now: f64, result: &Result<Release, FetchError>) {
        self.last_check = now;
        self.next_check = now
            + match result {
                Ok(_) => CHECK_INTERVAL_S,
                Err(FetchError::Failed(_)) => RETRY_AFTER_FAILURE_S,
                Err(FetchError::RateLimited { retry_after_s }) => (*retry_after_s as f64).clamp(60.0, CHECK_INTERVAL_S),
            };
        if let Err(FetchError::RateLimited { .. }) = result {
            self.rate_limited_until = self.next_check;
        }
    }
}

fn same_version(a: &str, b: &str) -> bool {
    !a.is_empty() && a.trim_start_matches('v') == b.trim_start_matches('v')
}

/// Now, in Unix seconds (works in the browser too).
pub fn unix_now() -> f64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

// ---- the running state ------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Failed(String),
}

#[derive(Default)]
pub struct Updater {
    /// The installation, looked up at the first frame.
    pub kind: Option<InstallKind>,
    pub status: Status,
    /// The newest release found (whether or not it is offered).
    pub release: Option<Release>,
    /// The Update window is open.
    pub open: bool,
    /// The check in flight, and whether Check now started it.
    rx: Option<(Receiver<Result<Release, FetchError>>, bool)>,
    started: bool,
    /// An automatic check has started since launch (until then one is due at once).
    launch_checked: bool,
    /// `SOLIDCRAFT_UPDATE_CHECK=0`: no checks at all.
    off: bool,
    /// When Check now was last pressed (Unix seconds).
    last_manual: f64,
    /// A release given by `SOLIDCRAFT_UPDATE_FEED` (a file), for tests and screenshots.
    feed: Option<PathBuf>,
    /// The one-click update in progress (`install`).
    job: Option<(Receiver<Result<install::Outcome, String>>, Arc<install::Progress>)>,
    /// The last one-click update's result.
    pub installed: Option<Result<install::Outcome, String>>,
    /// A program for the host to start as SolidCraft exits (restart, installer).
    launch: Option<(PathBuf, Vec<String>)>,
}

impl Updater {
    /// The installation (looked up on first use).
    pub fn kind(&mut self) -> &InstallKind {
        self.kind.get_or_insert_with(this_install)
    }
}

/// The Update button shows.
pub fn offered(app: &SolveApp) -> Option<&Release> {
    let r = app.updater.release.as_ref()?;
    app.preferences.update.offers(r, current_version(), unix_now()).then_some(r)
}

/// Checks may run here: a release build with the host's HTTP client, or a test feed.
pub fn available(app: &mut SolveApp) -> bool {
    !app.updater.off && (app.updater.feed.is_some() || (app.services.http_get.is_some() && app.updater.kind().checks()))
}

/// Per frame: the first frame of each launch starts an automatic check, the open app checks
/// again once a day (an hour after a failure), and a finished check is collected.
pub fn tick(app: &mut SolveApp, ctx: &egui::Context) {
    poll_install(app);
    // Quitting to restart or install was cancelled (a "save changes?" answered Cancel).
    if app.updater.launch.is_some() && app.docs.quitting.is_none() && !app.quit_requested {
        app.updater.launch = None;
    }
    if !app.updater.started {
        app.updater.started = true;
        #[cfg(not(target_arch = "wasm32"))]
        {
            install::clean_up_after_update(std::env::current_exe().ok().as_deref());
            if std::env::var("SOLIDCRAFT_UPDATE_CHECK").is_ok_and(|v| v == "0") {
                app.updater.off = true;
                return;
            }
            app.updater.feed = std::env::var_os("SOLIDCRAFT_UPDATE_FEED").map(PathBuf::from);
        }
    }
    poll(app);
    let now = unix_now();
    let launch = !app.updater.launch_checked;
    if app.updater.rx.is_none() && app.preferences.check_updates && app.preferences.update.due(now, launch) && available(app) {
        app.updater.launch_checked = true;
        start(app, ctx, false);
    }
}

/// Check now (Preferences): ignores the daily interval, not GitHub's rate limit.
pub fn check_now(app: &mut SolveApp, ctx: &egui::Context) {
    let now = unix_now();
    if app.updater.rx.is_some() || now - app.updater.last_manual < MANUAL_COOLDOWN_S || !available(app) {
        return;
    }
    if now < app.preferences.update.rate_limited_until {
        let wait = (app.preferences.update.rate_limited_until - now).max(0.0) as u64;
        app.updater.status = Status::Failed(FetchError::RateLimited { retry_after_s: wait }.to_string());
        return;
    }
    app.updater.last_manual = now;
    start(app, ctx, true);
}

fn start(app: &mut SolveApp, ctx: &egui::Context, manual: bool) {
    let (tx, rx) = std::sync::mpsc::channel();
    app.updater.status = Status::Checking;
    app.updater.rx = Some((rx, manual));
    let feed = app.updater.feed.clone();
    let http = app.services.http_get.clone();
    let fetch = move || -> Result<Release, FetchError> {
        let body = match (&feed, &http) {
            (Some(path), _) => std::fs::read(path).map_err(|e| FetchError::Failed(format!("{}: {e}", path.display())))?,
            (None, Some(get)) => get(LATEST_API)?,
            (None, None) => return Err(FetchError::Failed("no network client".into())),
        };
        Release::parse(&body)
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(fetch());
            ctx.request_repaint();
        });
        if spawned.is_err() {
            app.updater.rx = None;
            app.updater.status = Status::Idle;
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (ctx, tx, fetch);
        app.updater.rx = None;
        app.updater.status = Status::Idle;
    }
}

fn poll(app: &mut SolveApp) {
    let Some((rx, manual)) = &app.updater.rx else { return };
    let manual = *manual;
    let result = match rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(FetchError::Failed("the check stopped".into())),
    };
    app.updater.rx = None;
    app.preferences.update.checked(unix_now(), &result);
    match result {
        Ok(release) => {
            let newer = is_newer(&release.tag, current_version());
            app.updater.status = if newer { Status::Idle } else { Status::UpToDate };
            if newer && manual {
                // Asked for: show it even if it was skipped or put off.
                app.preferences.update.skipped.clear();
                app.preferences.update.remind_after = 0.0;
                app.updater.open = true;
            }
            app.updater.release = Some(release);
        }
        Err(e) => {
            log::info!("update check: {e}");
            app.updater.status = if manual { Status::Failed(e.to_string()) } else { Status::Idle };
        }
    }
}

// ---- one-click updates ------------------------------------------------------------------------

/// How this installation updates in place, when the host can download (None: Download only).
pub fn install_method(app: &mut SolveApp) -> Option<install::Method> {
    if app.services.http_get.is_none() || app.services.http_download.is_none() {
        return None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let exe = std::env::current_exe().ok();
        install::method(app.updater.kind(), exe.as_deref())
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Download, verify and apply the release found (Update now).
pub fn start_install(app: &mut SolveApp, ctx: &egui::Context) {
    let (Some(release), Some(method), Some(get), Some(download)) =
        (app.updater.release.clone(), install_method(app), app.services.http_get.clone(), app.services.http_download.clone())
    else {
        return;
    };
    if app.updater.job.is_some() {
        return;
    }
    let kind = app.updater.kind().clone();
    let progress = Arc::new(install::Progress::default());
    let (tx, rx) = std::sync::mpsc::channel();
    app.updater.installed = None;
    app.updater.job = Some((rx, progress.clone()));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ctx = ctx.clone();
        let staging = std::env::temp_dir().join("solidcraft-update");
        let net = install::Net { get, download };
        let spawned = std::thread::Builder::new().name("update-install".into()).spawn(move || {
            let result = install::run(&release, &kind, &method, std::env::consts::ARCH, &net, &staging, &progress);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            app.updater.job = None;
            app.updater.installed = Some(Err(e.to_string()));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (ctx, tx, release, method, get, download, kind);
        app.updater.job = None;
    }
}

fn poll_install(app: &mut SolveApp) {
    let Some((rx, _)) = &app.updater.job else { return };
    let result = match rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err("the update stopped".into()),
    };
    app.updater.job = None;
    match &result {
        Ok(install::Outcome::Installer(program, args)) => {
            // The installer replaces this program: start it once SolidCraft has closed. On
            // Windows the installer is followed by the new version (see install::relaunch_after).
            app.updater.launch = Some(match std::env::current_exe() {
                #[cfg(windows)]
                Ok(exe) => install::relaunch_after(program, args, &exe),
                _ => (program.clone(), args.clone()),
            });
            crate::documents::request_quit(app);
        }
        Ok(install::Outcome::Opened(dmg)) => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = std::process::Command::new("open").arg(dmg).spawn();
            }
            let _ = dmg;
        }
        Ok(install::Outcome::Restart(_)) => {}
        Err(e) => log::warn!("update: {e}"),
    }
    app.updater.installed = Some(result);
}

/// Restart now: close (asking about unsaved designs), then start the new version.
pub fn restart(app: &mut SolveApp) {
    if let Some(Ok(install::Outcome::Restart(program))) = &app.updater.installed {
        app.updater.launch = Some((program.clone(), Vec::new()));
        crate::documents::request_quit(app);
    }
}

/// For the host, as SolidCraft exits: the program to start (the new version, an installer).
pub fn take_launch(app: &mut SolveApp) -> Option<(PathBuf, Vec<String>)> {
    app.updater.launch.take()
}

// ---- user interface ---------------------------------------------------------------------------

/// The application bar's Update pill ("Update to 0.14.0"), at the right of `span`: it opens and
/// closes the Update window. Returns its rect (None: not shown).
pub fn button(app: &mut SolveApp, ui: &mut egui::Ui, span: egui::Rect, labeled: bool) -> Option<egui::Rect> {
    let version = offered(app)?.version().to_string();
    // "Update to 0.14.0" while the document tabs keep room; a short "Update" when they would be
    // squeezed; the icon alone in a narrow bar.
    let full = crate::i18n::format("update.button_version", &[("version", &version)]);
    let labels = [full.as_str(), crate::i18n::tr("update.button")];
    let (rect, response) = crate::toolbar::offer_pill(ui, span, labeled, &labels, TAB_ROOM, "update", "titlebar:update")?;
    if response.on_hover_text(crate::i18n::format("update.button_tip", &[("version", &version)])).clicked() {
        app.updater.open = !app.updater.open;
    }
    Some(rect)
}

/// The Update window.
pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    if !app.updater.open {
        return;
    }
    let Some(release) = app.updater.release.clone() else {
        app.updater.open = false;
        return;
    };
    let kind = app.updater.kind().clone();
    let install = install_method(app);
    let arch = std::env::consts::ARCH;
    let t = Tokens::get();
    let mut open = true;
    let mut action = None;
    let shown = crate::frame::window(ctx, crate::i18n::tr("update.title"), crate::frame::Width::Wide)
        .id(egui::Id::new("sc_update"))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.set_width(WINDOW_W);
            ui.set_max_width(WINDOW_W);
            ui.label(RichText::new(crate::i18n::format("update.available", &[("version", release.version())])).strong().size(15.0));
            ui.label(RichText::new(crate::i18n::format("update.you_have", &[("version", current_version())])).color(t.text_dim));
            ui.add_space(6.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_max_width(NOTES_W + 12.0);
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                    if release.notes.trim().is_empty() {
                        ui.label(RichText::new(crate::i18n::tr("update.no_notes")).color(t.text_dim));
                    } else {
                        markdown(ui, &release.notes, &t);
                    }
                });
            });
            ui.add_space(6.0);
            let asset = release.asset_for(&kind, arch).map(|a| a.name.clone());
            if let Some(cmd) = kind.update_command(asset.as_deref()) {
                let how = match kind {
                    InstallKind::Deb | InstallKind::Rpm | InstallKind::Snap { store: false } => "update.package_hint",
                    _ => "update.system_hint",
                };
                ui.add(egui::Label::new(crate::i18n::format(how, &[("kind", kind.label())])).wrap());
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(&cmd).monospace()).selectable(true));
                    if ui.small_button(crate::i18n::tr("update.copy")).clicked() {
                        ui.ctx().copy_text(cmd.clone());
                    }
                });
                ui.add_space(4.0);
            } else if let Some(name) = asset.as_ref().filter(|_| app.updater.installed.as_ref().is_none_or(|r| r.is_err())) {
                ui.label(RichText::new(crate::i18n::format("update.file", &[("file", name)])).size(11.0).color(t.text_dim));
            }
            if let Some((_, progress)) = &app.updater.job {
                let done = progress.done.load(std::sync::atomic::Ordering::Relaxed);
                let total = progress.total.load(std::sync::atomic::Ordering::Relaxed).max(1);
                let mb = |b: u64| format!("{:.1}", b as f64 / 1_048_576.0);
                ui.add(
                    egui::ProgressBar::new((done as f32 / total as f32).clamp(0.0, 1.0))
                        .text(crate::i18n::format("update.downloading", &[("done", &mb(done)), ("total", &mb(total))])),
                );
                if ui.button(crate::i18n::tr("ui.cancel")).clicked() {
                    progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            match &app.updater.installed {
                Some(Ok(install::Outcome::Restart(_))) => {
                    ui.label(RichText::new(crate::i18n::format("update.installed", &[("version", release.version())])).strong());
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::Button::new(RichText::new(crate::i18n::tr("update.restart")).strong()).fill(t.accent));
                        crate::scenario::publish_rect("update:restart", r.rect);
                        if r.clicked() {
                            action = Some("restart");
                        }
                        if ui.button(crate::i18n::tr("update.restart_later")).clicked() {
                            action = Some("close");
                        }
                    });
                    return;
                }
                Some(Ok(install::Outcome::Installer(..))) => {
                    ui.label(crate::i18n::tr("update.installer_started"));
                    return;
                }
                Some(Ok(install::Outcome::Opened(_))) => {
                    ui.add(egui::Label::new(crate::i18n::tr("update.dmg_opened")).wrap());
                    return;
                }
                Some(Err(e)) => {
                    ui.add(egui::Label::new(RichText::new(crate::i18n::format("update.install_failed", &[("error", e)])).color(t.error)).wrap());
                }
                None => {}
            }
            ui.horizontal(|ui| {
                let one_click = install.is_some() && !matches!(app.updater.installed, Some(Err(_)));
                if let Some(m) = install.as_ref().filter(|_| one_click) {
                    let label = match m {
                        install::Method::Msi => crate::i18n::tr("update.install_msi"),
                        install::Method::Dmg => crate::i18n::tr("update.download_open"),
                        _ => crate::i18n::tr("update.update_now"),
                    };
                    let r = ui.add(egui::Button::new(RichText::new(label).strong()).fill(t.accent));
                    crate::scenario::publish_rect("update:install", r.rect);
                    if r.clicked() {
                        action = Some("install");
                    }
                }
                let download = if kind.system_managed() { crate::i18n::tr("update.release_page") } else { crate::i18n::tr("update.download") };
                let r = if one_click { ui.button(download) } else { ui.add(egui::Button::new(RichText::new(download).strong()).fill(t.accent)) };
                crate::scenario::publish_rect("update:download", r.rect);
                if r.clicked() {
                    action = Some("download");
                }
                if ui.button(crate::i18n::tr("update.skip")).clicked() {
                    action = Some("skip");
                }
                if ui.button(crate::i18n::tr("update.later")).clicked() {
                    action = Some("later");
                }
            });
        });
    if let Some(w) = &shown {
        crate::scenario::publish_rect("update:window", w.response.rect);
    }
    let now = unix_now();
    match action {
        Some("download") => {
            let url = release.download_url(&kind, arch).to_string();
            ctx.open_url(egui::OpenUrl::new_tab(url));
            app.updater.open = false;
        }
        Some("skip") => {
            app.preferences.update.skip(&release);
            app.updater.open = false;
        }
        Some("later") => {
            app.preferences.update.remind_later(now);
            app.updater.open = false;
        }
        Some("install") => start_install(app, ctx),
        Some("restart") => restart(app),
        Some("close") => app.updater.open = false,
        _ if !open => app.updater.open = false,
        _ => {}
    }
}

/// Preferences ▸ General: the automatic check and Check now.
pub fn prefs_rows(app: &mut SolveApp, ui: &mut egui::Ui, t: &Tokens) {
    let can = available(app);
    let kind = app.updater.kind().clone();
    ui.label(crate::i18n::tr("update.prefs_label"));
    ui.add_enabled(can, egui::Checkbox::new(&mut app.preferences.check_updates, crate::i18n::tr("update.check_automatically")));
    ui.end_row();
    ui.label("");
    ui.horizontal(|ui| {
        let busy = app.updater.status == Status::Checking;
        let r = ui.add_enabled(can && !busy, egui::Button::new(crate::i18n::tr("update.check_now")));
        crate::scenario::publish_rect("prefs:check_now", r.rect);
        if r.clicked() {
            let ctx = ui.ctx().clone();
            check_now(app, &ctx);
        }
        let status = match (&app.updater.status, offered_or_newer(app)) {
            _ if !can => String::new(),
            (Status::Checking, _) => crate::i18n::tr("update.checking").to_string(),
            (Status::Failed(e), _) => crate::i18n::format("update.failed", &[("error", e)]),
            (_, Some(v)) => crate::i18n::format("update.available", &[("version", &v)]),
            (Status::UpToDate, None) => crate::i18n::tr("update.up_to_date").to_string(),
            (Status::Idle, None) => String::new(),
        };
        if !status.is_empty() {
            ui.label(RichText::new(status).color(t.text_dim));
        }
    });
    ui.end_row();
    ui.label("");
    let note = if can {
        crate::i18n::format("update.prefs_note", &[("version", current_version()), ("kind", kind.label())])
    } else if let InstallKind::Flatpak(id) = &kind {
        crate::i18n::format("update.prefs_flatpak", &[("version", current_version()), ("command", &format!("flatpak update {id}"))])
    } else {
        crate::i18n::format("update.prefs_dev", &[("version", current_version())])
    };
    ui.add(egui::Label::new(RichText::new(note).size(11.0).color(t.text_dim)).wrap());
    ui.end_row();
    if let Some(v) = offered_or_newer(app)
        && offered(app).is_none()
        && ui.link(crate::i18n::format("update.show", &[("version", &v)])).clicked()
    {
        app.updater.open = true;
    }
}

/// The newer version found, if any (offered or skipped).
fn offered_or_newer(app: &SolveApp) -> Option<String> {
    app.updater.release.as_ref().filter(|r| is_newer(&r.tag, current_version())).map(|r| r.version().to_string())
}

// ---- release notes ----------------------------------------------------------------------------

/// A span of inline Markdown.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text(String),
    Strong(String),
    Code(String),
    /// A link's text (the target is dropped: the release page has them).
    Link(String),
}

/// A block of the release notes.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(Vec<Span>),
    Paragraph(Vec<Span>),
    /// A list item, at a nesting depth.
    Item(usize, Vec<Span>),
}

/// The Markdown GitHub release notes use, enough to read them: headings, paragraphs (soft line
/// breaks joined), `-`/`*` lists, `**strong**`, `` `code` ``, `[links](…)` and bare pull-request
/// URLs (shown as `#123`). Anything else shows as written.
pub fn parse_markdown(src: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para = String::new();
    let mut item: Option<(usize, String)> = None;
    let flush = |blocks: &mut Vec<Block>, para: &mut String, item: &mut Option<(usize, String)>| {
        if !para.trim().is_empty() {
            blocks.push(Block::Paragraph(inline(para.trim())));
        }
        para.clear();
        if let Some((depth, text)) = item.take() {
            blocks.push(Block::Item(depth, inline(text.trim())));
        }
    };
    for line in src.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() {
            flush(&mut blocks, &mut para, &mut item);
        } else if let Some(h) = trimmed.strip_prefix('#') {
            flush(&mut blocks, &mut para, &mut item);
            blocks.push(Block::Heading(inline(h.trim_start_matches('#').trim())));
        } else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            flush(&mut blocks, &mut para, &mut item);
            item = Some((indent / 2, rest.to_string()));
        } else if let Some((_, text)) = &mut item {
            text.push(' ');
            text.push_str(trimmed);
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(trimmed);
        }
    }
    flush(&mut blocks, &mut para, &mut item);
    blocks
}

/// Inline spans of one block's text.
pub fn inline(text: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    let push_plain = |out: &mut Vec<Span>, plain: &mut String| {
        if !plain.is_empty() {
            out.push(Span::Text(std::mem::take(plain)));
        }
    };
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
            && end > 0
        {
            push_plain(&mut out, &mut plain);
            let inner = after.get(..end).unwrap_or_default();
            out.push(Span::Strong(strip_links(inner)));
            rest = after.get(end + 2..).unwrap_or_default();
        } else if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            push_plain(&mut out, &mut plain);
            out.push(Span::Code(after.get(..end).unwrap_or_default().to_string()));
            rest = after.get(end + 1..).unwrap_or_default();
        } else if let Some(after) = rest.strip_prefix('[')
            && let Some(close) = after.find("](")
            && let Some(end) = after.get(close + 2..).and_then(|s| s.find(')'))
        {
            push_plain(&mut out, &mut plain);
            out.push(Span::Link(strip_links(after.get(..close).unwrap_or_default())));
            rest = after.get(close + 2 + end + 1..).unwrap_or_default();
        } else if rest.starts_with("https://github.com/") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = rest.get(..end).unwrap_or_default();
            let tail = url.trim_end_matches(['.', ',', ')', ';']);
            let number = tail.rsplit('/').next().filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            let kind = tail.rsplit('/').nth(1);
            match (number, kind) {
                (Some(n), Some("pull" | "issues")) => {
                    push_plain(&mut out, &mut plain);
                    out.push(Span::Link(format!("#{n}")));
                    rest = rest.get(tail.len()..).unwrap_or_default();
                }
                _ => {
                    plain.push_str(url);
                    rest = rest.get(end..).unwrap_or_default();
                }
            }
        } else {
            plain.push(c);
            rest = rest.get(c.len_utf8()..).unwrap_or_default();
        }
    }
    push_plain(&mut out, &mut plain);
    out
}

/// Link text inside bold: `**[x](y)**` → `x`.
fn strip_links(s: &str) -> String {
    inline(s)
        .into_iter()
        .map(|span| match span {
            Span::Text(t) | Span::Strong(t) | Span::Code(t) | Span::Link(t) => t,
        })
        .collect()
}

fn markdown(ui: &mut egui::Ui, src: &str, t: &Tokens) {
    let body = ui.visuals().text_color();
    let job = |spans: &[Span], size: f32, strong: bool, wrap: f32| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = wrap;
        for span in spans {
            let (text, font, color) = match span {
                Span::Text(s) => (s.as_str(), FontId::proportional(size), if strong { t.text } else { body }),
                Span::Strong(s) => (s.as_str(), FontId::proportional(size), t.text),
                Span::Code(s) => (s.as_str(), FontId::monospace(size - 1.0), body),
                Span::Link(s) => (s.as_str(), FontId::proportional(size), t.accent),
            };
            job.append(text, 0.0, egui::TextFormat { font_id: font, color, ..Default::default() });
        }
        job
    };
    // Fixed widths: the notes never widen the window, whatever their lines.
    let width = NOTES_W;
    for block in parse_markdown(src) {
        match block {
            Block::Heading(spans) => {
                ui.add_space(4.0);
                ui.add(egui::Label::new(job(&spans, 14.0, true, width)).wrap());
            }
            Block::Paragraph(spans) => {
                ui.add(egui::Label::new(job(&spans, 12.5, false, width)).wrap());
                ui.add_space(3.0);
            }
            Block::Item(depth, spans) => {
                let indent = 6.0 + 12.0 * depth.min(4) as f32;
                ui.horizontal_top(|ui| {
                    ui.add_space(indent);
                    ui.label(RichText::new("•").size(12.5));
                    ui.add(egui::Label::new(job(&spans, 12.5, false, width - indent - 14.0)).wrap());
                });
            }
        }
    }
}

pub mod install;

#[cfg(test)]
mod install_tests;
#[cfg(test)]
mod tests;
