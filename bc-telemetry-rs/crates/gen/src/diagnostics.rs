//! Findings, and how they are printed.
//!
//! Validation is total before anything is written: every pass pushes into one
//! [`Diagnostics`] sink rather than returning at its first problem, so one run
//! reports everything wrong with a definition.

use crate::source::{Sources, Span};

/// What kind of finding a diagnostic is. Each has a stable printed name, so a
/// script — or a test — can tell one refusal from another without parsing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Code {
    /// A file could not be read.
    Io,
    /// An included file's extension is neither `.toml` nor `.json`.
    UnsupportedFormat,
    /// No definition file in the root directory.
    NoDefinition,
    /// One directory holds the definition in both syntaxes.
    AmbiguousDefinition,
    /// A file that is not named as a definition file is, or one that differs
    /// from that name by case alone.
    MisnamedDefinition,
    TomlSyntax,
    JsonSyntax,
    UnknownField,
    MissingField,
    TypeMismatch,
    /// A value of the right type outside what its key accepts.
    InvalidValue,
    /// A declared name outside its grammar: `snake_case` for a module, an item,
    /// an attribute, a field or an event, `kebab-case` for a query or a
    /// dashboard.
    InvalidName,
    /// The same item declared twice across a module's files, or an error and
    /// a log of one module sharing a name.
    DuplicateItem,
    /// Two definition files declare one module name.
    DuplicateModule,
    /// Two errors of one module declare one code.
    OneCodeTwoErrors,
    /// The same entry twice in one list (an event, a `group_by` attribute).
    DuplicateEntry,
    /// A reference to nothing declared.
    DanglingReference,
    /// An included file's `[module]` names another module.
    ModuleMismatch,
    /// An included file's `schema_version` differs from its module's.
    VersionMismatch,
    /// A key only the root definition file may carry, in another file.
    RootOnly,
    /// An attribute or field named as a column every view of its kind carries,
    /// or as the `level` a log's or an error's records statement adds.
    ReservedName,
    /// A derived name past the limit of the engine that carries it: a view name
    /// past PostgreSQL's 63 bytes, a dashboard uid past Grafana's 40 characters.
    NameTooLong,
    /// An aggregation its measure does not admit, by the aggregation table in
    /// this crate's README.
    InvalidAggregation,
    InvalidInclude,
    /// An include pattern that matched no file.
    EmptyInclude,
    /// A key this generator recognizes and does not yet implement.
    NotYetSupported,
    /// An export configured and not named, named and not configured, or named
    /// without the dialect it reads.
    ExportMismatch,
    InvalidOutputPath,
    /// A generated file whose committed content differs from what the
    /// definition generates.
    OutputChanged,
    /// A generated file absent from the committed tree.
    OutputMissing,
    /// A file a previous run generated that this one no longer produces.
    OutputStale,
    /// A file under the output root that no run generated.
    OutputUnexpected,
    ManifestUnreadable,
    /// A change existing readers cannot absorb, without a `schema_version` bump.
    BreakingChange,
    /// A `schema_version` lower than the committed tree's.
    VersionRegression,
    /// A declared error code the module retired: a removed error's code is
    /// never declared again.
    RetiredCodeDeclared,
    /// An error that kept its name and changed its code.
    ErrorRenumbered,
    /// A module the manifest holds, the tree lacks, and `[retired]` does not
    /// name.
    ModuleMissing,
    /// A module declared under a retired name.
    RetiredModuleDeclared,
    /// A `[retired]` entry the manifest never held.
    UnknownRetiredModule,
    /// A retired module's name dropped from `[retired]`.
    RetirementRemoved,
    /// A defect in this generator rather than in the definition.
    Internal,
}

impl Code {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::UnsupportedFormat => "unsupported-format",
            Self::NoDefinition => "no-definition",
            Self::AmbiguousDefinition => "ambiguous-definition",
            Self::MisnamedDefinition => "misnamed-definition",
            Self::TomlSyntax => "toml-syntax",
            Self::JsonSyntax => "json-syntax",
            Self::UnknownField => "unknown-field",
            Self::MissingField => "missing-field",
            Self::TypeMismatch => "type-mismatch",
            Self::InvalidValue => "invalid-value",
            Self::InvalidName => "invalid-name",
            Self::DuplicateItem => "duplicate-item",
            Self::DuplicateModule => "duplicate-module",
            Self::OneCodeTwoErrors => "duplicate-code",
            Self::DuplicateEntry => "duplicate-entry",
            Self::DanglingReference => "dangling-reference",
            Self::ModuleMismatch => "module-mismatch",
            Self::VersionMismatch => "version-mismatch",
            Self::RootOnly => "root-only",
            Self::ReservedName => "reserved-name",
            Self::NameTooLong => "name-too-long",
            Self::InvalidAggregation => "invalid-aggregation",
            Self::InvalidInclude => "invalid-include",
            Self::EmptyInclude => "empty-include",
            Self::NotYetSupported => "not-yet-supported",
            Self::ExportMismatch => "export-mismatch",
            Self::InvalidOutputPath => "invalid-output-path",
            Self::OutputChanged => "output-changed",
            Self::OutputMissing => "output-missing",
            Self::OutputStale => "output-stale",
            Self::OutputUnexpected => "output-unexpected",
            Self::ManifestUnreadable => "manifest-unreadable",
            Self::BreakingChange => "breaking-change",
            Self::VersionRegression => "version-regression",
            Self::RetiredCodeDeclared => "retired-code",
            Self::ErrorRenumbered => "code-changed",
            Self::ModuleMissing => "module-missing",
            Self::RetiredModuleDeclared => "retired-module-declared",
            Self::UnknownRetiredModule => "unknown-retired-module",
            Self::RetirementRemoved => "retirement-removed",
            Self::Internal => "internal",
        }
    }
}

/// One finding: what, why, and — when it is about something written in a file —
/// where.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Diagnostic {
    pub(crate) code: Code,
    pub(crate) message: String,
    pub(crate) span: Option<Span>,
}

/// The error of a step whose finding is already in the [`Diagnostics`] sink:
/// the caller has nothing to add, only a path to stop on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reported;

/// The sink every pass reports into.
#[derive(Debug, Default)]
pub(crate) struct Diagnostics {
    list: Vec<Diagnostic>,
}

impl Diagnostics {
    pub(crate) fn at(&mut self, code: Code, span: Span, message: impl Into<String>) {
        self.list.push(Diagnostic {
            code,
            message: message.into(),
            span: Some(span),
        });
    }

    /// A finding about no particular place in any file — a missing file, a
    /// generated tree as a whole.
    pub(crate) fn unplaced(&mut self, code: Code, message: impl Into<String>) {
        self.list.push(Diagnostic {
            code,
            message: message.into(),
            span: None,
        });
    }

    pub(crate) fn push(&mut self, diagnostic: Diagnostic) {
        self.list.push(diagnostic);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Every finding, rendered one per line in reading order: unplaced findings
    /// first, then by file and position. Findings at one position keep the order
    /// they were reported in, so the output is deterministic.
    pub(crate) fn render(&self, sources: &Sources) -> String {
        let mut ordered: Vec<&Diagnostic> = self.list.iter().collect();
        ordered.sort_by_key(|diagnostic| diagnostic.span.map(|span| (span.file, span.start)));
        let mut rendered = String::new();
        for diagnostic in ordered {
            rendered.push_str(&render_one(diagnostic, sources));
            rendered.push('\n');
        }
        rendered
    }
}

fn render_one(diagnostic: &Diagnostic, sources: &Sources) -> String {
    let code = diagnostic.code.name();
    match diagnostic.span {
        Some(span) => {
            let position = sources.line_column(span);
            format!(
                "{}:{}:{}: error[{code}]: {}",
                sources.display_path(span.file).display(),
                position.line,
                position.column,
                diagnostic.message
            )
        }
        None => format!("error[{code}]: {}", diagnostic.message),
    }
}

/// Candidates from `pool` within a small edit distance of `unknown`, best
/// first; ties break alphabetically so diagnostics are deterministic.
pub(crate) fn candidates_for<'pool>(
    unknown: &str,
    pool: impl IntoIterator<Item = &'pool str>,
) -> Vec<String> {
    // Distance ≤ 2 catches typos without suggesting unrelated names. Short
    // names get a tighter budget — never more edits than would rewrite the
    // whole name, so "a" suggests nothing rather than every other short name.
    let unknown_length = unknown.chars().count();
    let budget_for_length = if unknown_length <= 3 { 1 } else { 2 };
    let maximum_distance = budget_for_length.min(unknown_length.saturating_sub(1));

    let mut scored: Vec<(usize, &str)> = pool
        .into_iter()
        .filter_map(|candidate| {
            let distance = edit_distance(unknown, candidate);
            (distance <= maximum_distance).then_some((distance, candidate))
        })
        .collect();
    scored.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
    scored
        .into_iter()
        .map(|(_, candidate)| candidate.to_string())
        .collect()
}

/// A "; did you mean …?" suffix, or nothing when there are no candidates —
/// kept in one place so every diagnostic words it identically.
pub(crate) fn did_you_mean(candidates: &[String]) -> String {
    match candidates {
        [] => String::new(),
        [only] => format!("; did you mean `{only}`?"),
        several => {
            let quoted: Vec<String> = several
                .iter()
                .map(|candidate| format!("`{candidate}`"))
                .collect();
            format!("; did you mean one of {}?", quoted.join(", "))
        }
    }
}

/// Levenshtein distance over characters — the classic two-row dynamic
/// program: `previous_row[j]` holds the distance from the first `i-1` source
/// characters to the first `j` target characters.
fn edit_distance(source: &str, target: &str) -> usize {
    let source_characters: Vec<char> = source.chars().collect();
    let target_characters: Vec<char> = target.chars().collect();

    let mut previous_row: Vec<usize> = (0..=target_characters.len()).collect();
    for (source_index, source_character) in source_characters.iter().enumerate() {
        let mut current_row = Vec::with_capacity(target_characters.len() + 1);
        current_row.push(source_index + 1);
        for (target_index, target_character) in target_characters.iter().enumerate() {
            let substitution_cost = usize::from(source_character != target_character);
            let substitution = previous_row[target_index] + substitution_cost;
            let deletion = previous_row[target_index + 1] + 1;
            let insertion = current_row[target_index] + 1;
            current_row.push(substitution.min(deletion).min(insertion));
        }
        previous_row = current_row;
    }
    previous_row[target_characters.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typo_suggests_the_nearest_name_and_an_unrelated_word_suggests_nothing() {
        let pool = ["attributes", "description", "events"];
        assert_eq!(candidates_for("atributes", pool), vec!["attributes"]);
        let unrelated = candidates_for("colour", pool);
        assert!(unrelated.is_empty(), "{unrelated:?}");
    }

    #[test]
    fn a_short_name_gets_a_tight_budget() {
        // Two edits would rewrite most of a three-letter name.
        let two_edits_away = candidates_for("abc", ["xyz", "ayz"]);
        assert!(two_edits_away.is_empty(), "{two_edits_away:?}");
        assert_eq!(candidates_for("abc", ["abd"]), vec!["abd"]);
    }

    #[test]
    fn the_edit_distance_counts_insertions_deletions_and_substitutions() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("flaw", "lawn"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }

    #[test]
    fn the_suffix_lists_every_candidate_in_order() {
        assert_eq!(did_you_mean(&[]), "");
        assert_eq!(did_you_mean(&["a".to_string()]), "; did you mean `a`?");
        assert_eq!(
            did_you_mean(&["a".to_string(), "b".to_string()]),
            "; did you mean one of `a`, `b`?"
        );
    }
}
