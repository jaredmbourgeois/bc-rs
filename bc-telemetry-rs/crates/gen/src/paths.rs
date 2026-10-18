//! Paths relative to the root directory, as the model and the manifest hold
//! them: `/`-separated text, with [`ROOT`] for the root directory itself.
//!
//! They are text rather than `Path` because they are data — a module's
//! directory and every generated file's path are written into the manifest and
//! compared across runs and platforms — and they meet the file system in one
//! place, [`on_disk`].

use std::path::{Component, Path, PathBuf};

/// The root directory, relative to itself.
pub(crate) const ROOT: &str = ".";

/// `relative` taken from `directory`: `services/payments` and `generated` give
/// `services/payments/generated`, and the root gives `generated`.
pub(crate) fn under(directory: &str, relative: &str) -> String {
    if directory == ROOT {
        relative.to_string()
    } else {
        format!("{directory}/{relative}")
    }
}

/// The directory or file `relative` names beneath `base`.
pub(crate) fn on_disk(base: &Path, relative: &str) -> PathBuf {
    if relative == ROOT {
        base.to_path_buf()
    } else {
        base.join(relative)
    }
}

/// Whether `path` is `directory` or lies beneath it.
pub(crate) fn is_within(path: &str, directory: &str) -> bool {
    directory == ROOT
        || path == directory
        || path
            .strip_prefix(directory)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Why `text` is not a path that stays inside the directory it is taken from,
/// or `None` when it is. Every path a definition or a manifest writes — an
/// `include`, the `output`, a recorded directory, a listed file — is held to
/// it.
pub(crate) fn relative_path_problem(text: &str) -> Option<&'static str> {
    if text.is_empty() {
        return Some("is empty");
    }
    if text.contains('\\') {
        return Some("uses `\\`; paths are written with `/`");
    }
    let leaves_the_directory = Path::new(text)
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir));
    if leaves_the_directory || text.split('/').any(str::is_empty) {
        return Some(
            "must stay inside the directory it is taken from: no `..`, no leading `/` and no \
             empty segment",
        );
    }
    if text.split('/').any(|segment| segment == ROOT) {
        return Some("holds a `.` segment; write the path without it");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ROOT, is_within, relative_path_problem, under};

    #[test]
    fn a_path_is_taken_from_a_directory_and_the_root_adds_nothing() {
        assert_eq!(under(ROOT, "generated/telemetry"), "generated/telemetry");
        assert_eq!(
            under("services/payments", "generated"),
            "services/payments/generated"
        );
    }

    #[test]
    fn a_path_is_within_a_directory_by_whole_segments() {
        assert!(is_within("generated/a.rs", "generated"));
        assert!(is_within("generated", "generated"));
        assert!(is_within("anything/at/all", ROOT));
        // A sibling that merely starts with the same letters is not inside.
        assert!(!is_within("generated-old/a.rs", "generated"));
        assert!(!is_within("gen", "generated"));
    }

    #[test]
    fn a_relative_path_names_something_inside_and_nothing_else() {
        for path in ["generated", "generated/telemetry", "a.b/c-d_e"] {
            assert_eq!(relative_path_problem(path), None, "{path}");
        }
        for path in [
            "",
            "..",
            "../outside",
            "a/../b",
            "/absolute",
            "a//b",
            "trailing/",
            "back\\slash",
            ".",
            "./a",
            "a/./b",
        ] {
            assert!(relative_path_problem(path).is_some(), "{path}");
        }
    }
}
