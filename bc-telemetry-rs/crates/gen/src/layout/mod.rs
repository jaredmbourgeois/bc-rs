//! The storage layout the generator owns: for PostgreSQL, the DDL of the
//! tables and of the one ingest step every write goes through, the typed views
//! of the tree's items, and the conformance corpus every writer is held to.
//!
//! The DDL is the same for every tree — it is the layout — and is two files of
//! this crate, written out as they are: the data, applied once and versioned,
//! and the functions, which are code and are applied again whenever they
//! change. The views and the corpus are rendered from the tree.

pub(crate) mod corpus;
pub(crate) mod queries;
pub(crate) mod views;

use bc_json::JsonWriteError;

use crate::model::Tree;
use crate::output::GeneratedTree;
use crate::paths;

/// The layout's own version: the version of its data. Adding a column, an
/// index or a view is additive, and so is a function's new body, which
/// [`POSTGRESQL_FUNCTIONS`] carries to a database by being applied again;
/// anything else is a new version, with a migration rendered beside it.
pub(crate) const LAYOUT_VERSION: u32 = 1;

/// The DDL that brings an empty database to [`LAYOUT_VERSION`].
const POSTGRESQL_LAYOUT: &str = include_str!("postgresql_v1.sql");

/// The maintenance, erasure and ingest functions, applied after the layout and
/// again over themselves whenever they change.
const POSTGRESQL_FUNCTIONS: &str = include_str!("postgresql_functions.sql");

/// Where the layout's files go, under the root's `output`.
const POSTGRESQL_DIRECTORY: &str = "postgresql";

/// Where each module's queries go, under the layout's directory.
const QUERIES_DIRECTORY: &str = "queries";

/// Why the generated tree could not be rendered. Every case is a defect in the
/// generator: what it renders was validated first, and every value it writes
/// is its own.
#[derive(Debug)]
pub(crate) enum RenderError {
    /// The JSON writer refused a value.
    Json(JsonWriteError),
    /// The model holds what validation should have refused.
    Unvalidated(String),
    /// Something the generator relies on did not hold.
    Defect(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Unvalidated(what) => {
                write!(
                    formatter,
                    "{what} reached rendering without being validated"
                )
            }
            Self::Defect(what) => write!(formatter, "{what}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Unvalidated(_) | Self::Defect(_) => None,
        }
    }
}

/// Adds the PostgreSQL layout's files for `tree` to what this run generates:
/// the layout, its functions, the views, the corpus, and each module's
/// queries.
///
/// # Errors
///
/// A defect in the generator, as [`RenderError`] says.
pub(crate) fn render_postgresql(
    tree: &Tree,
    output: &str,
    generated: &mut GeneratedTree,
) -> Result<(), RenderError> {
    let directory = paths::under(output, POSTGRESQL_DIRECTORY);
    generated.insert(paths::under(&directory, "layout.sql"), POSTGRESQL_LAYOUT);
    generated.insert(
        paths::under(&directory, "functions.sql"),
        POSTGRESQL_FUNCTIONS,
    );
    generated.insert(paths::under(&directory, "views.sql"), views::render(tree));
    generated.insert(
        paths::under(&directory, "corpus.json"),
        corpus::render(tree).map_err(RenderError::Json)?,
    );
    let queries_directory = paths::under(&directory, QUERIES_DIRECTORY);
    for (module_name, file) in queries::render(tree)? {
        generated.insert(
            paths::under(&queries_directory, &format!("{module_name}.sql")),
            file,
        );
    }
    Ok(())
}
