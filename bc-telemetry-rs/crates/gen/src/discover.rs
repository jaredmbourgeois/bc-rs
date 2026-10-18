//! Finding the tree's definition files.
//!
//! A run starts at a root directory and walks every directory beneath it; each
//! one that holds a definition file is a module. The walk reads no file's
//! contents — only names — and its rules are the ones that keep one tree from
//! reading as two:
//!
//! - a directory whose name begins with `.` is not entered;
//! - a directory holding a `CACHEDIR.TAG` is not entered: build tools mark their
//!   output trees with one, and cargo's holds a copy of every packaged module;
//! - the generator's own output directories are not entered, so nothing it
//!   wrote is ever read back as a definition;
//! - a symbolic link to a directory is not followed: it could loop, leave the
//!   root, or show one module twice;
//! - a file is a definition only under its exact name. One that differs by case
//!   alone is refused, because a case-insensitive file system would read it and
//!   a case-sensitive one would not.
//!
//! Each directory is listed once and its names sorted, so the order modules are
//! found in — which is the order findings print in — is the same on every file
//! system.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::diagnostics::{Code, Diagnostics};
use crate::paths;

/// The two names a definition file may have, in the order a message names
/// them. The format is the extension.
pub(crate) const DEFINITION_FILE_NAMES: [&str; 2] = ["bc-telemetry.toml", "bc-telemetry.json"];

/// The marker a build tool leaves in a directory of its own output.
const CACHE_DIRECTORY_TAG: &str = "CACHEDIR.TAG";

/// One module's definition file, found by the walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefinitionSite {
    /// The module's directory relative to the root's, `/`-separated.
    pub(crate) directory: String,
    pub(crate) file_name: &'static str,
}

/// What one directory holds by way of a definition file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    Nothing,
    One(&'static str),
    /// The definition in both syntaxes: a refusal, never a pick.
    Both,
    /// A file whose name is a definition file's in every way but its case:
    /// the name found, and the exact one it resembles.
    Misnamed {
        found: String,
        exact: &'static str,
    },
}

/// Classifies a directory by the names of the files in it.
pub(crate) fn held_by<'names>(file_names: impl IntoIterator<Item = &'names str>) -> Held {
    let mut exact: Vec<&'static str> = Vec::new();
    for name in file_names {
        for definition_name in DEFINITION_FILE_NAMES {
            if name == definition_name {
                exact.push(definition_name);
            } else if name.eq_ignore_ascii_case(definition_name) {
                return Held::Misnamed {
                    found: name.to_string(),
                    exact: definition_name,
                };
            }
        }
    }
    match exact.as_slice() {
        [] => Held::Nothing,
        [only] => Held::One(only),
        _ => Held::Both,
    }
}

/// The file and directory names in one directory, each sorted.
struct Listing {
    files: Vec<String>,
    directories: Vec<String>,
}

/// Lists `directory`. A symbolic link counts as a file when it leads to one —
/// reading it reads the file — and is otherwise left out, so a link to a
/// directory is never walked.
///
/// # Errors
///
/// Why the directory could not be listed, already worded for a diagnostic.
fn list(directory: &Path, display: &Path) -> Result<Listing, String> {
    let cannot_read =
        |error: std::io::Error| format!("cannot read `{}`: {error}", display_or_here(display));
    let mut listing = Listing {
        files: Vec::new(),
        directories: Vec::new(),
    };
    for entry in fs::read_dir(directory).map_err(cannot_read)? {
        let entry = entry.map_err(cannot_read)?;
        let file_type = entry.file_type().map_err(cannot_read)?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            if file_type.is_dir() {
                return Err(format!(
                    "`{}` holds a directory whose name is not UTF-8; a module's directory is \
                     recorded as UTF-8, so the tree beneath it cannot be walked",
                    display_or_here(display)
                ));
            }
            // A file under such a name cannot be a definition file, whose two
            // names are ASCII.
            continue;
        };
        if file_type.is_dir() {
            listing.directories.push(name);
        } else if file_type.is_file() || entry.path().is_file() {
            listing.files.push(name);
        }
    }
    listing.files.sort();
    listing.directories.sort();
    Ok(listing)
}

/// A directory as a message names it; the directory the run started in has an
/// empty relative path, which would print as nothing.
fn display_or_here(display: &Path) -> String {
    if display.as_os_str().is_empty() {
        ".".to_string()
    } else {
        display.display().to_string()
    }
}

/// The root directory's definition file. `named` is the file `--file` chose,
/// already known to carry one of the two names; without it the directory must
/// hold exactly one.
pub(crate) fn root_definition(
    root_directory: &Path,
    display_directory: &Path,
    named: Option<&'static str>,
    diagnostics: &mut Diagnostics,
) -> Option<&'static str> {
    let listing = match list(root_directory, display_directory) {
        Ok(listing) => listing,
        Err(problem) => {
            diagnostics.unplaced(Code::Io, problem);
            return None;
        }
    };
    let [toml, json] = DEFINITION_FILE_NAMES;
    match held_by(listing.files.iter().map(String::as_str)) {
        Held::Misnamed { found, exact } => {
            refuse_misnamed(display_directory, &found, exact, diagnostics);
            None
        }
        Held::Both => {
            diagnostics.unplaced(
                Code::AmbiguousDefinition,
                format!(
                    "both `{toml}` and `{json}` are in `{}`; a directory holds its definition in \
                     one syntax",
                    root_directory.display()
                ),
            );
            None
        }
        // The named file may be absent; reading it then says so.
        Held::One(_) | Held::Nothing if named.is_some() => named,
        Held::One(only) => Some(only),
        Held::Nothing => {
            diagnostics.unplaced(
                Code::NoDefinition,
                format!(
                    "no `{toml}` or `{json}` in `{}`; run where the root definition is, or name \
                     it with `--file <path>`",
                    root_directory.display()
                ),
            );
            None
        }
    }
}

fn refuse_misnamed(
    display_directory: &Path,
    name: &str,
    exact: &str,
    diagnostics: &mut Diagnostics,
) {
    diagnostics.unplaced(
        Code::MisnamedDefinition,
        format!(
            "`{}` differs from `{exact}` by case alone; a definition file carries that exact \
             name, so every file system reads the same tree",
            display_directory.join(name).display()
        ),
    );
}

/// Every module below the root, in the order the walk meets them: a directory
/// before the directories under it, and those in name order. The root's own
/// definition is not among them — [`root_definition`] found it.
///
/// `output` is the root's `[generate] output`, when it has one: the path, under
/// each definition's directory, that the generator owns and the walk skips.
pub(crate) fn modules_below(
    root_directory: &Path,
    display_directory: &Path,
    output: Option<&str>,
    diagnostics: &mut Diagnostics,
) -> Vec<DefinitionSite> {
    let mut walk = Walk {
        root_directory,
        display_directory,
        output,
        owned: BTreeSet::new(),
        sites: Vec::new(),
        diagnostics,
    };
    // The root holds a definition by the time the walk starts, so its output
    // directory is owned before any directory is entered.
    walk.own_output_of(paths::ROOT);
    walk.enter_children_of(paths::ROOT);
    walk.sites
}

struct Walk<'run> {
    root_directory: &'run Path,
    display_directory: &'run Path,
    output: Option<&'run str>,
    /// The generator's output directories met so far, relative to the root. A
    /// definition's directory is always visited before the directories under
    /// it, so its output directory is here before the walk could enter it.
    owned: BTreeSet<String>,
    sites: Vec<DefinitionSite>,
    diagnostics: &'run mut Diagnostics,
}

impl Walk<'_> {
    fn own_output_of(&mut self, directory: &str) {
        if let Some(output) = self.output {
            self.owned.insert(paths::under(directory, output));
        }
    }

    fn enter_children_of(&mut self, directory: &str) {
        let Some(listing) = self.listing_of(directory) else {
            return;
        };
        for name in &listing.directories {
            if name.starts_with('.') {
                continue;
            }
            let child = paths::under(directory, name);
            if self.owned.contains(&child) {
                continue;
            }
            self.visit(&child);
        }
    }

    fn visit(&mut self, directory: &str) {
        let Some(listing) = self.listing_of(directory) else {
            return;
        };
        if listing.files.iter().any(|name| name == CACHE_DIRECTORY_TAG) {
            return;
        }
        let display = self.display_directory.join(directory);
        match held_by(listing.files.iter().map(String::as_str)) {
            Held::Nothing => {}
            Held::One(file_name) => {
                self.sites.push(DefinitionSite {
                    directory: directory.to_string(),
                    file_name,
                });
                self.own_output_of(directory);
            }
            Held::Both => {
                let [toml, json] = DEFINITION_FILE_NAMES;
                self.diagnostics.unplaced(
                    Code::AmbiguousDefinition,
                    format!(
                        "both `{toml}` and `{json}` are in `{}`; a directory holds its definition \
                         in one syntax",
                        display.display()
                    ),
                );
            }
            Held::Misnamed { found, exact } => {
                refuse_misnamed(&display, &found, exact, self.diagnostics);
            }
        }
        self.enter_children_of(directory);
    }

    fn listing_of(&mut self, directory: &str) -> Option<Listing> {
        let path = paths::on_disk(self.root_directory, directory);
        let display = paths::on_disk(self.display_directory, directory);
        match list(&path, &display) {
            Ok(listing) => Some(listing),
            Err(problem) => {
                self.diagnostics.unplaced(Code::Io, problem);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Held, held_by};

    #[test]
    fn a_directory_holds_a_definition_only_under_its_exact_name() {
        assert_eq!(held_by(["README.md", "Cargo.toml"]), Held::Nothing);
        assert_eq!(
            held_by(["Cargo.toml", "bc-telemetry.toml"]),
            Held::One("bc-telemetry.toml")
        );
        assert_eq!(
            held_by(["bc-telemetry.json"]),
            Held::One("bc-telemetry.json")
        );
        assert_eq!(
            held_by(["bc-telemetry.json", "bc-telemetry.toml"]),
            Held::Both
        );
    }

    #[test]
    fn a_name_that_differs_by_case_alone_is_misnamed_whatever_else_is_there() {
        assert_eq!(
            held_by(["BC-Telemetry.toml"]),
            Held::Misnamed {
                found: "BC-Telemetry.toml".to_string(),
                exact: "bc-telemetry.toml"
            }
        );
        assert_eq!(
            held_by(["bc-telemetry.toml", "bc-telemetry.JSON"]),
            Held::Misnamed {
                found: "bc-telemetry.JSON".to_string(),
                exact: "bc-telemetry.json"
            }
        );
        // Another extension is another file, not a misspelling of this one.
        assert_eq!(held_by(["bc-telemetry.yaml"]), Held::Nothing);
    }
}
