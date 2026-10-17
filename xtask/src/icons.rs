//! Original UI icon standards and review-tool regression checks.
use std::path::Path;
use std::process::Command;

pub fn run(root: &Path) -> Result<(), String> {
    for args in [
        vec!["-B", "assets/icons/ui/design/tools/lint_icons.py"],
        vec!["-B", "assets/icons/ui/design/tools/icon_status.py", "--check"],
        vec!["-B", "-m", "unittest", "discover", "-s", "tests/tools", "-p", "test_icon*.py"],
    ] {
        let mut command = Command::new("python3");
        command.args(args).current_dir(root);
        super::run(command, "UI icon standards (Python 3.11+)")?;
    }
    Ok(())
}
