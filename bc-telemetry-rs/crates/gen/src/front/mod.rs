//! The syntax front ends: a definition file's text in, a positioned
//! [`Table`](crate::document::Table) out.

mod json;
mod toml;

use std::path::Path;

use crate::diagnostics::Diagnostic;
use crate::document::Table;
use crate::source::FileId;

/// A definition file's syntax, which is its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Toml,
    Json,
}

impl Format {
    /// The format `path` names, or `None` for any other extension.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "toml" => Some(Self::Toml),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Parses `text` — the contents of `file` — into its root table.
///
/// # Errors
///
/// The syntax error, positioned where the parser found it.
pub(crate) fn parse(format: Format, file: FileId, text: &str) -> Result<Table, Diagnostic> {
    match format {
        Format::Toml => toml::parse(file, text),
        Format::Json => json::parse(file, text),
    }
}
