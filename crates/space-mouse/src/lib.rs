//! 3D mouse input that doesn't depend on how the device is read or what the motion drives. The
//! apps open the device their own way (hidraw on Linux, hidapi on Windows), pass its report
//! descriptor to [`Descriptor::parse`] and each input report to [`Layout::decode`], and apply the
//! six axes and the buttons they get back to their own camera and commands.
//!
//! Any device that declares a HID Multi-axis Controller is read the same way: 3Dconnexion's,
//! which send translation and rotation in two reports and their buttons in a third, and homemade
//! ones, which often send all six axes in one report, sometimes after their buttons. The
//! descriptor says where each axis and button is.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod descriptor;

pub use descriptor::{Descriptor, FULL_SCALE, Field, Layout, MAX_BUTTONS, Report};
