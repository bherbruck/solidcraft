//! A client of the 3Dconnexion driver (3DxWare) on macOS.
//!
//! The driver's helper app opens the device exclusively while it runs, so reading the HID device
//! directly fails then. The driver offers its own client API instead, in
//! `3DconnexionClient.framework`: an app registers with the driver and gets the cap's six axes
//! (after the user's 3DxWare settings) and the buttons as messages on a run loop.
//!
//! The framework is loaded with `dlopen` when [`start`] is called, so nothing links against it and
//! the app runs without the driver installed ([`Error::NotInstalled`]). The framework is never
//! bundled or redistributed; its declarations here are our own. On other platforms [`start`]
//! returns [`Error::Unsupported`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;

#[cfg(target_os = "macos")]
mod macos;

/// What the driver reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A 3D mouse was plugged in or the driver connected to one.
    DeviceAdded,
    /// The 3D mouse was unplugged or the driver went away.
    DeviceRemoved,
    /// The cap moved: push left/right, forward/back, lift/press, then tilt forward/back,
    /// tilt left/right and twist, each about -350..350 as the HID device reports them.
    Axes { translation: [i16; 3], rotation: [i16; 3] },
    /// The buttons that are down, one bit each.
    Buttons(u32),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not macOS.
    Unsupported,
    /// The driver's framework is not installed (or not loadable).
    NotInstalled,
    /// The framework lacks a function this client needs (another driver version).
    MissingSymbol(&'static str),
    /// The driver refused the handlers (its `OSErr`).
    Handlers(i16),
    /// The driver refused the client registration.
    Register,
    /// [`start`] was called before.
    AlreadyStarted,
    /// The client's thread could not be started.
    Thread(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported => write!(f, "the 3Dconnexion driver client only exists on macOS"),
            Error::NotInstalled => write!(f, "the 3Dconnexion driver is not installed"),
            Error::MissingSymbol(name) => write!(f, "the 3Dconnexion driver lacks {name}"),
            Error::Handlers(code) => write!(f, "the 3Dconnexion driver refused the handlers (error {code})"),
            Error::Register => write!(f, "the 3Dconnexion driver refused the client"),
            Error::AlreadyStarted => write!(f, "the 3Dconnexion client is already started"),
            Error::Thread(reason) => write!(f, "the 3Dconnexion client thread did not start: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

/// Connects to the driver and calls `sink` (on the client's own thread) for everything it reports.
/// `name` is how the driver lists this app. Only one client per process; the connection lasts
/// until the process ends.
pub fn start(name: &str, sink: impl Fn(Event) + Send + Sync + 'static) -> Result<(), Error> {
    #[cfg(target_os = "macos")]
    {
        macos::start(name, Box::new(sink))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (name, sink);
        Err(Error::Unsupported)
    }
}
