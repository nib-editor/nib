//! `nib plugin new`: writes a plugin that builds and passes its tests, to
//! start from (docs/plugin-dev.md).

use std::fs;
use std::path::{Path, PathBuf};

use nib_core::API_VERSION;

/// The Rust SDK's tag for this nib's API. Bump with `sdk/rust`'s version.
const RUST_SDK_TAG: &str = "sdk/rust/v0.5.2";
/// The Go SDK's version for this nib's API. Bump with its tags `sdk/go/v*`.
const GO_SDK_VERSION: &str = "v0.5.2";

/// Paths in the new plugin and their templates. `NAME` in a path is the
/// plugin's name; dotfiles are stored without their dot.
type Template = &'static [(&'static str, &'static str)];

const RUST: Template = &[
    ("plugin.toml", include_str!("../templates/rust/plugin.toml")),
    // Named so cargo does not take the template for a package of its own
    // and leave it out of nib's.
    (
        "Cargo.toml",
        include_str!("../templates/rust/Cargo.toml.in"),
    ),
    ("src/lib.rs", include_str!("../templates/rust/src/lib.rs")),
    (
        "tests/NAME.toml",
        include_str!("../templates/rust/tests/NAME.toml"),
    ),
    ("README.md", include_str!("../templates/rust/README.md")),
    ("AGENTS.md", include_str!("../templates/rust/AGENTS.md")),
    (".gitignore", include_str!("../templates/rust/gitignore")),
    (
        ".github/workflows/release.yml",
        include_str!("../templates/rust/.github/workflows/release.yml"),
    ),
];

const GO: Template = &[
    ("plugin.toml", include_str!("../templates/go/plugin.toml")),
    ("go.mod", include_str!("../templates/go/go.mod")),
    ("main.go", include_str!("../templates/go/main.go")),
    (
        "tests/NAME.toml",
        include_str!("../templates/go/tests/NAME.toml"),
    ),
    ("README.md", include_str!("../templates/go/README.md")),
    ("AGENTS.md", include_str!("../templates/go/AGENTS.md")),
    (".gitignore", include_str!("../templates/go/gitignore")),
    (
        ".github/workflows/release.yml",
        include_str!("../templates/go/.github/workflows/release.yml"),
    ),
];

/// Writes a new plugin named `name` into `dir`, or `./name`. Returns the
/// directory.
pub fn run(name: &str, dir: Option<&Path>, go: bool) -> Result<PathBuf, String> {
    check_name(name)?;
    let dir = dir.map_or_else(|| PathBuf::from(name), Path::to_path_buf);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let template = if go { GO } else { RUST };
    for (path, text) in template {
        let path = dir.join(path.replace("NAME", name));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
        }
        fs::write(&path, fill(text, name)).map_err(|err| format!("{}: {err}", path.display()))?;
    }
    Ok(dir)
}

/// Fills in the template's fields. Only these are replaced, so other
/// `{{...}}`, as in GitHub's and Go's templates, stay.
fn fill(text: &str, name: &str) -> String {
    text.replace("{{name}}", name)
        .replace("{{crate}}", &name.replace('-', "_"))
        .replace("{{api}}", API_VERSION)
        .replace("{{rust_sdk_tag}}", RUST_SDK_TAG)
        .replace("{{go_sdk_version}}", GO_SDK_VERSION)
}

/// A manifest's name: it prefixes the plugin's commands.
fn check_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(format!(
            "{name:?}: a plugin's name is lowercase letters, digits, and '-', starting with a letter"
        ));
    }
    if ["buffer", "editor", "view"].contains(&name) {
        return Err(format!("{name:?} is reserved for the core's commands"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rust_sdk_tag_is_its_version() {
        let manifest = include_str!("../../sdk/rust/Cargo.toml");
        let version = manifest
            .lines()
            .find_map(|line| line.strip_prefix("version = \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap();
        assert_eq!(RUST_SDK_TAG, format!("sdk/rust/v{version}"));
    }

    #[test]
    fn names_are_checked() {
        assert!(check_name("word-count2").is_ok());
        for bad in ["", "Word", "2words", "word_count", "editor"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fields_are_filled_and_others_kept() {
        let text = fill("{{name}} {{crate}} ${{ github.token }} {{.Dir}}", "a-b");
        assert_eq!(text, "a-b a_b ${{ github.token }} {{.Dir}}");
    }
}
