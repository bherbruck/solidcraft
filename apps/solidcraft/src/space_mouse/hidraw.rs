//! Finding a 3D mouse on Linux. The kernel exposes every HID interface as `/dev/hidrawN`, with
//! its report descriptor under `/sys/class/hidraw/hidrawN/device/`. Reading it is plain file
//! I/O, so no udev or hidapi library is needed.

use std::path::{Path, PathBuf};

use solidcraft_space_mouse::{Descriptor, Layout};

const SYSFS: &str = "/sys/class/hidraw";
const DEV: &str = "/dev";

pub struct Device {
    pub path: PathBuf,
    pub layout: Layout,
}

/// The first hidraw interface that is a multi-axis controller with motion axes, whoever made it.
pub fn find() -> Option<Device> {
    let mut names: Vec<_> = std::fs::read_dir(SYSFS).ok()?.filter_map(Result::ok).map(|entry| entry.file_name()).collect();
    names.sort();
    names.into_iter().find_map(|name| probe(&Path::new(SYSFS).join(&name), Path::new(DEV).join(&name)))
}

fn probe(sysfs: &Path, path: PathBuf) -> Option<Device> {
    let descriptor = Descriptor::parse(&std::fs::read(sysfs.join("device/report_descriptor")).ok()?);
    // A controller that declares no motion axis (buttons only, say) isn't read.
    let layout = descriptor.is_multi_axis_controller().then(|| descriptor.layout()).flatten()?;
    Some(Device { path, layout })
}
