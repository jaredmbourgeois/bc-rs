//! Reading definition files: a module's own file, and every file its `include`
//! names.
//!
//! This is the only module that reads definition files; everything after it
//! works on the parsed tables. An `include` entry is a path relative to the
//! module's own directory — literal, or with one `*` in its last segment. It
//! may not leave that directory, and it may not reach into a subdirectory that
//! is a module of its own: a file belongs to the module whose directory it is
//! in, and to no other.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostics::{Code, Diagnostics};
use crate::document::{Table, Value};
use crate::front::{self, Format};
use crate::paths::{self, relative_path_problem};
use crate::source::{FileId, Sources, Spanned};

/// One parsed definition file.
#[derive(Debug)]
pub(crate) struct DefinitionFile {
    pub(crate) file: FileId,
    pub(crate) table: Table,
}

/// One module's files: its definition file and the files that includes, in the
/// order it names them.
#[derive(Debug)]
pub(crate) struct LoadedModule {
    /// The module's directory relative to the root's.
    pub(crate) directory: String,
    /// Whether its definition file is the root file, which alone may carry
    /// `[generate]` and `[retired]` and may be a root without being a module.
    pub(crate) is_root: bool,
    pub(crate) file: DefinitionFile,
    pub(crate) includes: Vec<DefinitionFile>,
}

/// Reads and parses the definition file at `path`, printed as `display_path`.
/// `None` when it could not be read or parsed; the finding is in `diagnostics`.
pub(crate) fn read_definition(
    path: &Path,
    display_path: &Path,
    format: Format,
    sources: &mut Sources,
    diagnostics: &mut Diagnostics,
) -> Option<DefinitionFile> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            diagnostics.unplaced(
                Code::Io,
                format!("cannot read `{}`: {error}", display_path.display()),
            );
            return None;
        }
    };
    let file = sources.add(display_path.to_path_buf(), text);
    match front::parse(format, file, &sources.file(file).text) {
        Ok(table) => Some(DefinitionFile { file, table }),
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            None
        }
    }
}

/// Where a module's files are, on disk and as findings print them.
pub(crate) struct ModuleLocation<'run> {
    /// The module's directory relative to the root's.
    pub(crate) directory: &'run str,
    /// Its own definition file's name.
    pub(crate) file_name: &'run str,
    pub(crate) root_directory: &'run Path,
    pub(crate) display_directory: &'run Path,
    /// Every directory in the tree that holds a definition file, relative to
    /// the root: an include may not reach into another module's.
    pub(crate) module_directories: &'run BTreeSet<String>,
}

/// Every file `definition`'s `include` names. An include that could not be
/// read or parsed is a finding and is left out.
pub(crate) fn includes(
    definition: &DefinitionFile,
    location: &ModuleLocation<'_>,
    sources: &mut Sources,
    diagnostics: &mut Diagnostics,
) -> Vec<DefinitionFile> {
    let directory = paths::on_disk(location.root_directory, location.directory);
    let display_directory = paths::on_disk(location.display_directory, location.directory);

    let mut included = Vec::new();
    // The module's own file was just read, so it canonicalizes; were it ever not
    // to, a pattern reaching it would read it a second time and every item in it
    // would then be refused as declared twice — loud either way.
    let mut seen = BTreeSet::new();
    seen.extend(fs::canonicalize(directory.join(location.file_name)).ok());
    for pattern in include_patterns(&definition.table, diagnostics) {
        for relative in resolve(&pattern, &directory, diagnostics) {
            if let Some(other_module) = module_reached_into(&relative, location) {
                diagnostics.at(
                    Code::InvalidInclude,
                    pattern.span,
                    format!(
                        "include `{}` reaches `{}`, which is inside `{other_module}` — a module \
                         of its own, whose files are its alone",
                        pattern.value,
                        relative.display()
                    ),
                );
                continue;
            }
            let included_path = directory.join(&relative);
            // Two spellings of one file — a literal path and a pattern, or a
            // pattern that reaches the module's own file — are refused rather
            // than merged: a silent dedupe would hide that the layout says two
            // things.
            if let Ok(canonical) = fs::canonicalize(&included_path) {
                if !seen.insert(canonical) {
                    diagnostics.at(
                        Code::InvalidInclude,
                        pattern.span,
                        format!(
                            "include `{}` reaches `{}`, which is already part of this module",
                            pattern.value,
                            relative.display()
                        ),
                    );
                    continue;
                }
            }
            let Some(format) = Format::of(&included_path) else {
                diagnostics.at(
                    Code::UnsupportedFormat,
                    pattern.span,
                    format!(
                        "include `{}` reaches `{}`; a definition file is `.toml` or `.json`",
                        pattern.value,
                        relative.display()
                    ),
                );
                continue;
            };
            let display = display_directory.join(&relative);
            if let Some(file) =
                read_definition(&included_path, &display, format, sources, diagnostics)
            {
                included.push(file);
            }
        }
    }
    included
}

/// The other module whose directory `relative` — a path from this module's
/// directory — lies in, if any: the nearest directory above the file, below
/// this module's own, that holds a definition file.
fn module_reached_into(relative: &Path, location: &ModuleLocation<'_>) -> Option<String> {
    let mut directory = relative.parent();
    while let Some(current) = directory {
        let text = current.to_str()?;
        if text.is_empty() {
            return None;
        }
        let from_root = paths::under(location.directory, text);
        if location.module_directories.contains(&from_root) {
            return Some(from_root);
        }
        directory = current.parent();
    }
    None
}

/// A definition's `include` list. Type findings are reported here, because the
/// loader reads the key before anything walks the table; the lowering marks it
/// read and says nothing more about it.
fn include_patterns(definition: &Table, diagnostics: &mut Diagnostics) -> Vec<Spanned<String>> {
    let Some(entry) = definition.get("include") else {
        return Vec::new();
    };
    let Value::Array(elements) = &entry.value.value else {
        diagnostics.at(
            Code::TypeMismatch,
            entry.value.span,
            format!(
                "`include` must be an array of paths, got {}",
                entry.value.value.describe()
            ),
        );
        return Vec::new();
    };
    let mut patterns = Vec::new();
    for element in elements {
        match &element.value {
            Value::String(text) => patterns.push(Spanned {
                value: text.clone(),
                span: element.span,
            }),
            other => diagnostics.at(
                Code::TypeMismatch,
                element.span,
                format!(
                    "every element of `include` must be a path, got {}",
                    other.describe()
                ),
            ),
        }
    }
    patterns
}

/// The files `pattern` names, relative to the module's directory, in name
/// order.
fn resolve(
    pattern: &Spanned<String>,
    directory: &Path,
    diagnostics: &mut Diagnostics,
) -> Vec<PathBuf> {
    let refuse = |diagnostics: &mut Diagnostics, why: &str| {
        diagnostics.at(
            Code::InvalidInclude,
            pattern.span,
            format!("include `{}` {why}", pattern.value),
        );
    };
    let text = pattern.value.as_str();
    if let Some(problem) = relative_path_problem(text) {
        refuse(diagnostics, problem);
        return Vec::new();
    }
    let relative = Path::new(text);
    if text.contains(['?', '[', ']', '{', '}']) {
        refuse(diagnostics, "uses glob syntax beyond `*`");
        return Vec::new();
    }

    let (directory_part, last_segment) = match text.rsplit_once('/') {
        Some((parent, last)) => (Some(parent), last),
        None => (None, text),
    };
    if directory_part.is_some_and(|parent| parent.contains('*')) {
        refuse(diagnostics, "has a `*` outside its last path segment");
        return Vec::new();
    }

    let Some((prefix, suffix)) = last_segment.split_once('*') else {
        // A literal path: it must name a file.
        if directory.join(relative).is_file() {
            return vec![relative.to_path_buf()];
        }
        refuse(diagnostics, "names no file");
        return Vec::new();
    };
    if suffix.contains('*') {
        refuse(diagnostics, "has more than one `*`");
        return Vec::new();
    }

    let relative_directory = match directory_part {
        Some(parent) => PathBuf::from(parent),
        None => PathBuf::new(),
    };
    let listing = match fs::read_dir(directory.join(&relative_directory)) {
        Ok(listing) => listing,
        Err(error) => {
            refuse(diagnostics, &format!("cannot be listed: {error}"));
            return Vec::new();
        }
    };
    let mut names = Vec::new();
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                refuse(diagnostics, &format!("cannot be listed: {error}"));
                return Vec::new();
            }
        };
        // A name that is not UTF-8 cannot have been meant by a UTF-8 pattern.
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let matches = name.len() >= prefix.len() + suffix.len()
            && name.starts_with(prefix)
            && name.ends_with(suffix);
        if matches && entry.path().is_file() {
            names.push(name);
        }
    }
    if names.is_empty() {
        diagnostics.at(
            Code::EmptyInclude,
            pattern.span,
            format!("include `{}` matches no file", pattern.value),
        );
        return Vec::new();
    }
    names.sort();
    names
        .into_iter()
        .map(|name| relative_directory.join(name))
        .collect()
}
