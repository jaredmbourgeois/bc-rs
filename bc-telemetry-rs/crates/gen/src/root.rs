//! What only the root file may say: where the generated files go, and which
//! modules this tree no longer has.
//!
//! Both are read before anything else in the tree, because the walk that finds
//! the modules needs `output` to know which directories are the generator's own.
//! The rest of the root file — its `[module]` header and its declarations — is
//! lowered with every other module's.

use std::collections::{BTreeMap, BTreeSet};

use crate::adapters::Language;
use crate::diagnostics::{Code, Diagnostics, Reported, candidates_for, did_you_mean};
use crate::load::DefinitionFile;
use crate::model::{Dialect, Export, Keyword};
use crate::names::NameShape;
use crate::paths::relative_path_problem;
use crate::source::{Span, Spanned};
use crate::walker::TableWalker;

/// The build-owned half of the root file.
#[derive(Debug)]
pub(crate) struct RootConfig {
    /// Absent when the root has no `[generate]` table: the tree is validated
    /// and nothing is written.
    pub(crate) generate: Option<GenerateConfig>,
    pub(crate) retired: RetiredList,
}

/// The root's `[generate]` table.
#[derive(Debug)]
pub(crate) struct GenerateConfig {
    /// One path used in two places: under the root's directory for the shared
    /// artifacts and the manifest, and under each module's own directory for
    /// that module's emitting code.
    pub(crate) output: Spanned<String>,
    /// The storage engines a layout is generated for; none means the manifest
    /// alone.
    pub(crate) dialects: BTreeSet<Dialect>,
    /// The languages whose emitters every module gets; none means none.
    pub(crate) languages: BTreeSet<Language>,
    /// The Grafana export, when `exports` names it.
    pub(crate) grafana: Option<GrafanaExport>,
}

/// What `[generate.grafana]` says: which Grafana data source every generated
/// panel reads.
#[derive(Debug)]
pub(crate) struct GrafanaExport {
    pub(crate) data_source_uid: String,
}

/// The root's `[retired] modules`: the permanent list of module names this
/// tree once had and never declares again: a retired name is a retired error
/// category, whose codes stay spent with it.
#[derive(Debug)]
pub(crate) struct RetiredList {
    pub(crate) modules: Vec<Spanned<String>>,
    /// Where a finding about a name missing from the list points: the list
    /// itself, or the start of the root file when it has none.
    pub(crate) anchor: Span,
}

impl RetiredList {
    pub(crate) fn names(&self, name: &str) -> bool {
        self.modules.iter().any(|listed| listed.value == name)
    }
}

/// The `[generate]` keys later stages of this generator implement. They are
/// recognized — so a definition using one is told it is early, not that it
/// misspelled something — and refused.
const NOT_YET_SUPPORTED_GENERATE_KEYS: &[&str] = &["routing"];

/// Reads `[generate]` and `[retired]` from the root file. Its other keys are
/// the lowering's to read and to report on.
pub(crate) fn root_config(
    root: &DefinitionFile,
    context: &str,
    diagnostics: &mut Diagnostics,
) -> RootConfig {
    let mut walker = TableWalker::new(&root.table, context, diagnostics);
    let generate = generate_config(&mut walker);
    let retired = retired_list(&mut walker, Span::start_of(root.file));
    // Dropped without `finish`: the lowering walks this same table and is the
    // one that reports a key nobody knows.
    drop(walker);
    RootConfig { generate, retired }
}

fn generate_config(walker: &mut TableWalker<'_, '_>) -> Option<GenerateConfig> {
    let table = walker.optional_table("generate")?;
    let mut generate = TableWalker::new(table, "`[generate]`", walker.diagnostics());
    for key in NOT_YET_SUPPORTED_GENERATE_KEYS {
        if let Some(entry) = generate.take(key) {
            let message = format!(
                "`{key}` on `[generate]` is not implemented by bc-telemetry-gen {} yet",
                env!("CARGO_PKG_VERSION")
            );
            generate
                .diagnostics()
                .at(Code::NotYetSupported, entry.key_span, message);
        }
    }
    let output = generate.required_string("output");
    let dialects = keyword_list::<Dialect>(&mut generate, "dialects", "has a layout for");
    let exports = keyword_list::<Export>(&mut generate, "exports", "exports to");
    let languages = keyword_list::<Language>(&mut generate, "languages", "has an adapter for");
    let grafana_table = grafana_table(&mut generate);
    let mut known = vec!["output", "dialects", "exports", "grafana", "languages"];
    known.extend_from_slice(NOT_YET_SUPPORTED_GENERATE_KEYS);
    generate.finish(&known);

    // Every check runs before any finding ends the read, so one run reports
    // them all.
    let grafana = match (&dialects, &exports, grafana_table) {
        (Some(dialects), Some(exports), Ok(table)) => {
            grafana_export(dialects, exports, table, walker.diagnostics())
        }
        _ => Err(Reported),
    };
    let output = output?;
    if let Some(problem) = relative_path_problem(&output.value) {
        walker.diagnostics().at(
            Code::InvalidOutputPath,
            output.span,
            format!("`output` `{}` {problem}", output.value),
        );
        return None;
    }
    Some(GenerateConfig {
        output,
        dialects: dialects?.into_keys().collect(),
        languages: languages?.into_keys().collect(),
        grafana: grafana.ok()?,
    })
}

/// A list of keywords under `key`, each one this generator knows and each
/// once, with where each is written. Absent is none; any finding leaves the
/// whole list out. `knows` completes "this generator … `dialects`' keywords",
/// as a refusal says it.
fn keyword_list<K: Keyword + Ord>(
    generate: &mut TableWalker<'_, '_>,
    key: &str,
    knows: &str,
) -> Option<BTreeMap<K, Span>> {
    if !generate.has(key) {
        return Some(BTreeMap::new());
    }
    let listed = generate.optional_string_array(key)?;
    let diagnostics = generate.diagnostics();
    let mut members = BTreeMap::new();
    let mut every_entry_holds = true;
    for entry in listed {
        let Some(member) = K::from_keyword(&entry.value) else {
            let keywords = K::keywords();
            diagnostics.at(
                Code::InvalidValue,
                entry.span,
                format!(
                    "`{key}` on `[generate]` names `{}`; this generator {knows} {}{}",
                    entry.value,
                    keywords.join(" | "),
                    did_you_mean(&candidates_for(&entry.value, keywords.iter().copied()))
                ),
            );
            every_entry_holds = false;
            continue;
        };
        if members.insert(member, entry.span).is_some() {
            diagnostics.at(
                Code::DuplicateEntry,
                entry.span,
                format!("`{}` appears twice in `{key}`", entry.value),
            );
            every_entry_holds = false;
        }
    }
    every_entry_holds.then_some(members)
}

/// `[generate.grafana]`, if the root has one: its `datasource`, and where the
/// table is written.
fn grafana_table(
    generate: &mut TableWalker<'_, '_>,
) -> Result<Option<(Spanned<String>, Span)>, Reported> {
    if !generate.has("grafana") {
        return Ok(None);
    }
    let (table, key_span) = generate.optional_keyed_table("grafana").ok_or(Reported)?;
    let mut grafana = TableWalker::new(table, "`[generate.grafana]`", generate.diagnostics());
    let data_source = grafana.required_text("datasource");
    grafana.finish(&["datasource"]);
    Ok(Some((data_source.ok_or(Reported)?, key_span)))
}

/// The Grafana export, when `exports` names it and `[generate.grafana]` says
/// which data source it reads. The export reads the PostgreSQL layout, so it
/// needs that dialect; a table for an export nobody named is refused rather
/// than ignored, since it is either a forgotten export or a stale table.
fn grafana_export(
    dialects: &BTreeMap<Dialect, Span>,
    exports: &BTreeMap<Export, Span>,
    table: Option<(Spanned<String>, Span)>,
    diagnostics: &mut Diagnostics,
) -> Result<Option<GrafanaExport>, Reported> {
    match (exports.get(&Export::Grafana), table) {
        (None, None) => Ok(None),
        (None, Some((_, table_span))) => {
            diagnostics.at(
                Code::ExportMismatch,
                table_span,
                "`[generate.grafana]` configures the `grafana` export, which `exports` does not \
                 name; add it to `exports` or remove the table",
            );
            Err(Reported)
        }
        (Some(named), None) => {
            diagnostics.at(
                Code::ExportMismatch,
                *named,
                "`exports` names `grafana`, so `[generate.grafana]` must say which Grafana data \
                 source every panel reads: `datasource = \"<uid>\"`",
            );
            Err(Reported)
        }
        (Some(named), Some(_)) if !dialects.contains_key(&Dialect::Postgresql) => {
            diagnostics.at(
                Code::ExportMismatch,
                *named,
                "the `grafana` export reads the PostgreSQL layout, so `dialects` must name \
                 `postgresql`",
            );
            Err(Reported)
        }
        (Some(_), Some((data_source, _))) => Ok(Some(GrafanaExport {
            data_source_uid: data_source.value,
        })),
    }
}

fn retired_list(walker: &mut TableWalker<'_, '_>, start_of_file: Span) -> RetiredList {
    let no_list = RetiredList {
        modules: Vec::new(),
        anchor: start_of_file,
    };
    let Some((table, anchor)) = walker.optional_keyed_table("retired") else {
        return no_list;
    };
    let mut retired = TableWalker::new(table, "`[retired]`", walker.diagnostics());
    let listed = retired.required_string_array("modules");
    retired.finish(&["modules"]);
    let Some(listed) = listed else {
        return RetiredList {
            modules: Vec::new(),
            anchor,
        };
    };

    let diagnostics = walker.diagnostics();
    let mut seen = BTreeSet::new();
    let mut modules = Vec::new();
    for name in listed {
        if !NameShape::Snake.admits(&name.value) {
            diagnostics.at(
                Code::InvalidName,
                name.span,
                format!(
                    "retired module name `{}` is not {}",
                    name.value,
                    NameShape::Snake.describe()
                ),
            );
            continue;
        }
        if !seen.insert(name.value.clone()) {
            diagnostics.at(
                Code::DuplicateEntry,
                name.span,
                format!("module `{}` appears twice in `[retired]`", name.value),
            );
            continue;
        }
        modules.push(name);
    }
    RetiredList { modules, anchor }
}
