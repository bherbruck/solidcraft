//! An explicit help request prints usage to stdout and succeeds; an unknown command still fails.

use std::process::Command;

#[test]
fn help_prints_usage_and_succeeds() {
    for arg in ["--help", "-h", "help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_solidcraft-cli")).arg(arg).output().unwrap();
        assert!(output.status.success(), "{arg}: {}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("usage:"), "{arg}: {stdout}");
        assert!(stdout.contains("solidcraft-cli run"), "{arg}: {stdout}");
        assert!(output.stderr.is_empty(), "{arg}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn unknown_command_still_fails_with_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_solidcraft-cli")).arg("--not-a-command").output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("error: usage:"));
}
