//! The command line, besides `nib plugin`.

use std::process::Command;

#[test]
fn says_its_version() {
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_nib"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        let expected = format!("nib {}\n", env!("CARGO_PKG_VERSION"));
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    }
}
