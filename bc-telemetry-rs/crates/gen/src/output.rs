//! The generated tree, the committed one, and the difference between them.
//!
//! Generation is pure: a run produces a [`GeneratedTree`] — path to bytes — in
//! memory, and only then compares it with what is on disk. `--check` stops at
//! the comparison (no temporary tree, nothing written); a generating run
//! applies it.
//!
//! The generator owns a set of directories — the root's `output`, and the same
//! path under each module's directory — and nothing else in them: a file the
//! previous manifest lists and this run no longer produces is removed, and a
//! file no run produced is refused rather than overwritten or deleted. Every
//! path here is relative to the root directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use crate::paths;

/// What one run generates: `/`-separated paths relative to the root directory,
/// in path order.
#[derive(Debug, Default)]
pub(crate) struct GeneratedTree {
    files: BTreeMap<String, Vec<u8>>,
}

impl GeneratedTree {
    pub(crate) fn insert(&mut self, path: impl Into<String>, contents: impl Into<Vec<u8>>) {
        self.files.insert(path.into(), contents.into());
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub(crate) fn len(&self) -> usize {
        self.files.len()
    }
}

/// The committed tree: every regular file under each directory in `owned`,
/// read in full and keyed by its path from the root. An absent directory holds
/// nothing.
///
/// # Errors
///
/// Why the tree could not be read, already worded for a diagnostic: an I/O
/// failure, a name that is not UTF-8, or something other than a regular file or
/// a directory — a generated tree holds neither links nor devices, so meeting
/// one means the directory is not only the generator's. Paths in the message
/// are printed from `display_directory`.
pub(crate) fn read_committed(
    root_directory: &Path,
    display_directory: &Path,
    owned: &BTreeSet<String>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    for directory in owned {
        let path = paths::on_disk(root_directory, directory);
        let display = paths::on_disk(display_directory, directory);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot read `{}`: {error}", display.display())),
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "`{}` is not a directory; the output path names the directory the generator \
                     owns",
                    display.display()
                ));
            }
            Ok(_) => read_directory(&path, &display, directory, &mut files)?,
        }
    }
    Ok(files)
}

fn read_directory(
    directory: &Path,
    display: &Path,
    from_root: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let cannot_read = |error: io::Error| format!("cannot read `{}`: {error}", display.display());
    for entry in fs::read_dir(directory).map_err(cannot_read)? {
        let entry = entry.map_err(cannot_read)?;
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            return Err(format!(
                "`{}` holds a name that is not UTF-8; no run generates one",
                display.display()
            ));
        };
        let entry_display = display.join(&name);
        let entry_from_root = paths::under(from_root, &name);
        let file_type = entry.file_type().map_err(cannot_read)?;
        if file_type.is_dir() {
            read_directory(&path, &entry_display, &entry_from_root, files)?;
        } else if file_type.is_file() {
            let contents = fs::read(&path)
                .map_err(|error| format!("cannot read `{}`: {error}", entry_display.display()))?;
            files.insert(entry_from_root, contents);
        } else {
            return Err(format!(
                "`{}` is neither a regular file nor a directory; a generated tree holds only those",
                entry_display.display()
            ));
        }
    }
    Ok(())
}

/// How the committed tree differs from the generated one. Each list is in path
/// order.
#[derive(Debug, Default)]
pub(crate) struct Difference {
    /// Generated, committed by an earlier run, and different.
    pub(crate) changed: Vec<String>,
    /// Generated and not committed.
    pub(crate) missing: Vec<String>,
    /// Committed, listed by the previous manifest, and no longer generated.
    pub(crate) stale: Vec<String>,
    /// Committed, and written by no run — whether or not this run would
    /// generate a file at that path.
    pub(crate) unexpected: Vec<String>,
    /// Generated and committed byte-identical.
    pub(crate) unchanged: usize,
}

pub(crate) fn compare(
    generated: &GeneratedTree,
    committed: &BTreeMap<String, Vec<u8>>,
    previously_generated: &BTreeSet<String>,
) -> Difference {
    let mut difference = Difference::default();
    for (path, contents) in &generated.files {
        match committed.get(path) {
            Some(existing) if existing == contents => difference.unchanged += 1,
            Some(_) if previously_generated.contains(path) => difference.changed.push(path.clone()),
            // A differing file no run wrote is someone else's, even where this
            // run would put one of its own: refused, never overwritten.
            Some(_) => difference.unexpected.push(path.clone()),
            None => difference.missing.push(path.clone()),
        }
    }
    for path in committed.keys() {
        if generated.files.contains_key(path) {
            continue;
        }
        if previously_generated.contains(path) {
            difference.stale.push(path.clone());
        } else {
            difference.unexpected.push(path.clone());
        }
    }
    difference.unexpected.sort();
    difference
}

/// Brings the committed tree to the generated one: writes every changed and
/// missing file, removes every stale one and any directory that leaves empty —
/// up to, and never including, a directory in `owned`. The caller refuses an
/// unexpected file before getting here.
///
/// # Errors
///
/// The first file that could not be written or removed. What was already
/// written stays; the next run's comparison reports whatever is left.
pub(crate) fn apply(
    root_directory: &Path,
    generated: &GeneratedTree,
    difference: &Difference,
    owned: &BTreeSet<String>,
) -> io::Result<()> {
    debug_assert!(
        difference.unexpected.is_empty(),
        "an unexpected file is refused before anything is written"
    );
    for path in difference.changed.iter().chain(&difference.missing) {
        let target = root_directory.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, &generated.files[path])?;
    }
    for path in &difference.stale {
        fs::remove_file(root_directory.join(path))?;
        // Prune the directories this removal emptied, innermost first.
        let mut directory = parent_of(path);
        while let Some(current) = directory {
            let on_disk = root_directory.join(current);
            if owned.contains(current) || fs::read_dir(&on_disk)?.next().is_some() {
                break;
            }
            fs::remove_dir(&on_disk)?;
            directory = parent_of(current);
        }
    }
    Ok(())
}

/// The directory holding `path`, or `None` when that is the root directory.
fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{GeneratedTree, compare, parent_of};

    #[test]
    fn a_tree_lists_each_path_once_in_path_order() {
        let mut tree = GeneratedTree::default();
        tree.insert("generated/rust/lib.rs", "first");
        tree.insert("generated/manifest.json", "manifest");
        tree.insert("generated/rust/lib.rs", "second");
        tree.insert("generated/go/telemetry.go", "go");
        assert_eq!(
            tree.paths().collect::<Vec<_>>(),
            [
                "generated/go/telemetry.go",
                "generated/manifest.json",
                "generated/rust/lib.rs"
            ]
        );
    }

    #[test]
    fn a_tree_counts_its_distinct_paths() {
        let mut tree = GeneratedTree::default();
        assert_eq!(tree.len(), 0);
        tree.insert("generated/manifest.json", "first");
        tree.insert("generated/rust/lib.rs", "rust");
        tree.insert("generated/manifest.json", "second");
        assert_eq!(tree.len(), 2);
    }

    /// A file that differs from what this run generates is the generator's to
    /// rewrite only if an earlier run wrote it.
    #[test]
    fn a_differing_file_is_changed_only_when_an_earlier_run_wrote_it() {
        let mut tree = GeneratedTree::default();
        tree.insert("generated/a.rs", "new");
        tree.insert("generated/b.rs", "new");
        tree.insert("generated/c.rs", "same");
        tree.insert("generated/d.rs", "new");
        let committed: BTreeMap<String, Vec<u8>> = [
            ("generated/a.rs", "old"),
            ("generated/b.rs", "someone else's"),
            ("generated/c.rs", "same"),
            ("generated/gone.rs", "old"),
            ("generated/stray.rs", "someone else's"),
        ]
        .into_iter()
        .map(|(path, contents)| (path.to_string(), contents.as_bytes().to_vec()))
        .collect();
        let previously_generated: BTreeSet<String> = ["generated/a.rs", "generated/gone.rs"]
            .into_iter()
            .map(str::to_string)
            .collect();

        let difference = compare(&tree, &committed, &previously_generated);
        assert_eq!(difference.changed, ["generated/a.rs"]);
        assert_eq!(difference.missing, ["generated/d.rs"]);
        assert_eq!(difference.stale, ["generated/gone.rs"]);
        assert_eq!(
            difference.unexpected,
            ["generated/b.rs", "generated/stray.rs"]
        );
        assert_eq!(difference.unchanged, 1);
    }

    #[test]
    fn a_path_at_the_root_has_no_parent_directory_to_prune() {
        assert_eq!(parent_of("generated/rust/lib.rs"), Some("generated/rust"));
        assert_eq!(parent_of("generated"), None);
    }
}
