//! The tree's one manifest.
//!
//! It sits under the root's `output`: every module's lowered vocabulary, the
//! codes each module retired, the modules the tree retired, and every file the
//! run produced. It is the language-independent artifact — the one a tree with
//! no adapter configured still generates — and it is what the next run reads
//! back for the questions nothing else can answer:
//!
//! - which files under the generator's directories are its own to remove;
//! - whether a module's new definition breaks readers of the old one without
//!   bumping its `schema_version`;
//! - which error codes and module names are spent, and may never be declared
//!   again.
//!
//! The retired codes live nowhere else. A manifest that is deleted, or merged
//! badly, forgets them, and no later run can notice; it is reviewed as source.

use std::collections::{BTreeMap, BTreeSet};

use bc_json::{JsonWriteError, JsonWriter};

use crate::diagnostics::{Code, Diagnostics};
use crate::document::{Node, Table, Value};
use crate::model::{ItemKind, Items, Keyword, MAXIMUM_ERROR_CODE, ScalarType, Tree};
use crate::names::NameShape;
use crate::paths::{self, relative_path_problem};
use crate::source::{Span, Spanned};
use crate::walker::TableWalker;

/// The manifest's file name, under the root's `output`.
pub(crate) const MANIFEST_FILE_NAME: &str = "manifest.json";

/// The manifest's own format version. A manifest of any other version is
/// refused rather than half-read.
const MANIFEST_VERSION: i64 = 2;

/// The single-domain format the first release wrote. Named so its refusal can
/// say what it is: nothing in one must survive, so there is no migration.
const SINGLE_DOMAIN_MANIFEST_VERSION: i64 = 1;

pub(crate) const GENERATOR: &str = concat!("bc-telemetry-gen ", env!("CARGO_PKG_VERSION"));

/// What the tree has given up for good: the codes each live module retired,
/// and the modules that are gone.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Retired {
    /// By module name. A module that never retired a code may be absent.
    pub(crate) codes: BTreeMap<String, BTreeSet<u32>>,
    pub(crate) modules: BTreeSet<String>,
}

/// The manifest for `tree`, listing `files` — every other file the run
/// generates, relative to the root — as pretty-printed JSON a review can read
/// field by field.
pub(crate) fn render(
    tree: &Tree,
    retired: &Retired,
    output: &str,
    files: &[&str],
) -> Result<String, JsonWriteError> {
    let mut writer = JsonWriter::pretty();
    writer.object(|writer| {
        writer.field("manifest_version", MANIFEST_VERSION);
        writer.field("generator", GENERATOR);
        writer.field("output", output);
        writer.nested_object("modules", |writer| {
            for (name, module) in &tree.modules {
                writer.nested_object(name, |writer| {
                    writer.field("directory", &module.directory);
                    writer.field("schema_version", module.schema_version.value);
                    write_items(writer, &module.items);
                    writer.array("retired_codes", |writer| {
                        for code in retired.codes.get(name).into_iter().flatten() {
                            writer.array_element(*code);
                        }
                    });
                });
            }
        });
        writer.string_array("retired_modules", &retired.modules);
        writer.string_array("files", files);
    });
    let mut text = writer.finish()?;
    text.push('\n');
    Ok(text)
}

fn write_items(writer: &mut JsonWriter, items: &Items) {
    writer.nested_object("spans", |writer| {
        for (name, span) in &items.spans {
            writer.nested_object(name, |writer| {
                writer.field("description", &span.description);
                writer.field("kind", span.kind.keyword());
                write_scalar_map(writer, "attributes", &span.attributes);
                writer.string_array("events", &span.events);
            });
        }
    });
    writer.nested_object("metrics", |writer| {
        for (name, metric) in &items.metrics {
            writer.nested_object(name, |writer| {
                writer.field("description", &metric.description);
                writer.field("type", metric.kind.keyword());
                writer.field("unit", metric.unit.keyword());
                write_scalar_map(writer, "attributes", &metric.attributes);
            });
        }
    });
    writer.nested_object("logs", |writer| {
        for (name, log) in &items.logs {
            writer.nested_object(name, |writer| {
                writer.field("description", &log.description);
                writer.field("level", log.severity.keyword());
                write_scalar_map(writer, "fields", &log.fields);
            });
        }
    });
    writer.nested_object("errors", |writer| {
        for (name, error) in &items.errors {
            writer.nested_object(name, |writer| {
                writer.field("description", &error.description);
                writer.field("class", error.class.keyword());
                writer.field("code", error.code.value);
                write_scalar_map(writer, "fields", &error.fields);
            });
        }
    });
    writer.nested_object("queries", |writer| {
        for (name, query) in &items.queries {
            writer.nested_object(name, |writer| {
                writer.field("description", &query.description);
                writer.field("measure", query.measure.value.spelled());
                writer.field("aggregation", query.aggregation.value.keyword());
                writer.field("window_seconds", query.window_seconds);
                writer.string_array(
                    "group_by",
                    query.group_by.iter().map(|group| group.value.as_str()),
                );
            });
        }
    });
    writer.nested_object("dashboards", |writer| {
        for (name, dashboard) in &items.dashboards {
            writer.nested_object(name, |writer| {
                writer.field("title", &dashboard.title);
                writer.array("panels", |writer| {
                    for panel in &dashboard.panels {
                        writer.array_value(|writer| {
                            writer.object(|writer| {
                                writer.field("title", &panel.title);
                                writer.field("source", panel.source.value.spelled());
                                writer.field("render", panel.render.value.keyword());
                            });
                        });
                    }
                });
            });
        }
    });
}

fn write_scalar_map(writer: &mut JsonWriter, key: &str, map: &BTreeMap<String, ScalarType>) {
    writer.nested_object(key, |writer| {
        for (name, scalar) in map {
            writer.field(name, scalar.keyword());
        }
    });
}

/// What readers of a module's records depend on: each emitted item's kind,
/// attribute names and types, events and unit, and each error's code, class and
/// fields. Queries and dashboards are consumers of the vocabulary, not part of
/// it, so changing them never breaks a reader.
///
/// Keywords are kept as the text the manifest spells them with, so a manifest
/// written by a release that knows a keyword this one does not is still
/// compared, and a difference still worded.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Vocabulary {
    /// Spans, metrics and logs, by kind and then name.
    pub(crate) items: BTreeMap<(ItemKind, String), Shape>,
    pub(crate) errors: BTreeMap<String, ErrorShape>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Shape {
    /// Attribute — or, for a log, field — name to its type keyword.
    pub(crate) attributes: BTreeMap<String, String>,
    pub(crate) events: BTreeSet<String>,
    /// A metric's `type` and `unit` keywords; `None` for spans and logs.
    pub(crate) metric: Option<MetricShape>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct MetricShape {
    pub(crate) kind: String,
    pub(crate) unit: String,
}

#[derive(Debug, PartialEq)]
pub(crate) struct ErrorShape {
    pub(crate) code: u32,
    pub(crate) class: String,
    pub(crate) fields: BTreeMap<String, String>,
}

impl Vocabulary {
    pub(crate) fn of(items: &Items) -> Self {
        let keywords = |map: &BTreeMap<String, ScalarType>| -> BTreeMap<String, String> {
            map.iter()
                .map(|(name, scalar)| (name.clone(), scalar.keyword().to_string()))
                .collect()
        };
        let mut vocabulary = Self::default();
        for (name, span) in &items.spans {
            let shape = Shape {
                attributes: keywords(&span.attributes),
                events: span.events.clone(),
                metric: None,
            };
            vocabulary
                .items
                .insert((ItemKind::Span, name.clone()), shape);
        }
        for (name, metric) in &items.metrics {
            let shape = Shape {
                attributes: keywords(&metric.attributes),
                events: BTreeSet::new(),
                metric: Some(MetricShape {
                    kind: metric.kind.keyword().to_string(),
                    unit: metric.unit.keyword().to_string(),
                }),
            };
            vocabulary
                .items
                .insert((ItemKind::Metric, name.clone()), shape);
        }
        for (name, log) in &items.logs {
            let shape = Shape {
                attributes: keywords(&log.fields),
                events: BTreeSet::new(),
                metric: None,
            };
            vocabulary
                .items
                .insert((ItemKind::Log, name.clone()), shape);
        }
        for (name, error) in &items.errors {
            let shape = ErrorShape {
                code: error.code.value,
                class: error.class.keyword().to_string(),
                fields: keywords(&error.fields),
            };
            vocabulary.errors.insert(name.clone(), shape);
        }
        vocabulary
    }
}

/// What a committed manifest says about the run that wrote it.
#[derive(Debug)]
pub(crate) struct PreviousManifest {
    /// That run's `output`, which names the directories its files are in.
    pub(crate) output: String,
    pub(crate) modules: BTreeMap<String, PreviousModule>,
    pub(crate) retired_modules: BTreeSet<String>,
    /// Every file that run generated, relative to the root, the manifest
    /// included.
    pub(crate) files: BTreeSet<String>,
}

#[derive(Debug)]
pub(crate) struct PreviousModule {
    /// Where the module was, relative to the root.
    pub(crate) directory: String,
    pub(crate) schema_version: u32,
    pub(crate) vocabulary: Vocabulary,
    pub(crate) retired_codes: BTreeSet<u32>,
}

impl PreviousManifest {
    /// Every directory that run owned, relative to the root: its `output`, and
    /// the same path under each module's directory.
    pub(crate) fn owned_directories(&self) -> BTreeSet<String> {
        let mut owned = BTreeSet::new();
        owned.insert(self.output.clone());
        for module in self.modules.values() {
            owned.insert(paths::under(&module.directory, &self.output));
        }
        owned
    }
}

/// Reads a committed manifest — parsed by the JSON front end like any other
/// file, so a hand-edit that broke it is refused with its position. `None`
/// when it has findings, every one of which is in `diagnostics`.
pub(crate) fn read(
    table: &Table,
    context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<PreviousManifest> {
    let mut walker = TableWalker::new(table, context, diagnostics);
    let version = walker.required_integer("manifest_version")?;
    if version.value != MANIFEST_VERSION {
        let message = if version.value == SINGLE_DOMAIN_MANIFEST_VERSION {
            format!(
                "{context} is manifest version {SINGLE_DOMAIN_MANIFEST_VERSION}, the \
                 single-domain format; there is no migration, and a manifest of that version \
                 records nothing that must survive — remove it and regenerate"
            )
        } else {
            // No advice to remove it: a later manifest may hold retired codes,
            // which exist nowhere else.
            format!(
                "{context} is manifest version {}, which {GENERATOR} does not read (it reads \
                 {MANIFEST_VERSION}); regenerate with the release that wrote it",
                version.value
            )
        };
        walker
            .diagnostics()
            .at(Code::ManifestUnreadable, version.span, message);
        return None;
    }
    walker.consume("generator");
    let output = walker.required_string("output");
    let modules = walker.required_table("modules");
    let retired_modules = walker.required_string_array("retired_modules");
    let files = walker.required_string_array("files");
    walker.finish(&[
        "manifest_version",
        "generator",
        "output",
        "modules",
        "retired_modules",
        "files",
    ]);

    let output = output.and_then(|output| {
        let problem = relative_path_problem(&output.value);
        if let Some(problem) = problem {
            unreadable(
                diagnostics,
                output.span,
                format!(
                    "{context} records `output` `{}`, which {problem}",
                    output.value
                ),
            );
        }
        problem.is_none().then_some(output.value)
    });
    let modules = read_modules(modules?, context, diagnostics);
    let retired_modules = retired_modules
        .and_then(|names| read_retired_modules(names, modules.as_ref(), context, diagnostics));
    let mut manifest = PreviousManifest {
        output: output?,
        modules: modules?,
        retired_modules: retired_modules?,
        files: BTreeSet::new(),
    };
    manifest.files = read_files(files?, &manifest, context, diagnostics)?;
    Some(manifest)
}

fn read_modules(
    table: &Table,
    context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<BTreeMap<String, PreviousModule>> {
    let mut modules = BTreeMap::new();
    let mut every_module_read = true;
    for entry in &table.entries {
        if !NameShape::Snake.admits(&entry.key) {
            unreadable(
                diagnostics,
                entry.key_span,
                format!(
                    "{context} records a module named `{}`, which no definition can declare",
                    entry.key
                ),
            );
            every_module_read = false;
            continue;
        }
        match read_module(&entry.key, &entry.value, context, diagnostics) {
            Some(module) => {
                modules.insert(entry.key.clone(), module);
            }
            None => every_module_read = false,
        }
    }
    every_module_read.then_some(modules)
}

/// The retired module names: each once, and none a module the manifest also
/// records as live. `modules` is absent when the live ones could not be read;
/// the list is then held only to itself.
fn read_retired_modules(
    names: Vec<Spanned<String>>,
    modules: Option<&BTreeMap<String, PreviousModule>>,
    context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<BTreeSet<String>> {
    let mut retired = BTreeSet::new();
    let mut the_list_holds = true;
    for name in names {
        let problem = if modules.is_some_and(|modules| modules.contains_key(&name.value)) {
            Some("as retired and as a module")
        } else if retired.contains(&name.value) {
            Some("as retired twice")
        } else {
            None
        };
        if let Some(problem) = problem {
            unreadable(
                diagnostics,
                name.span,
                format!("{context} records `{}` {problem}", name.value),
            );
            the_list_holds = false;
        } else {
            retired.insert(name.value);
        }
    }
    the_list_holds.then_some(retired)
}

/// The files a run generated, the manifest among them. A listed file is one a
/// run may remove, so each is held to the directories this manifest's own
/// `output` and modules name: an edited manifest cannot direct a removal
/// elsewhere under the root.
fn read_files(
    listed: Vec<Spanned<String>>,
    manifest: &PreviousManifest,
    context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<BTreeSet<String>> {
    let owned = manifest.owned_directories();
    let manifest_path = paths::under(&manifest.output, MANIFEST_FILE_NAME);
    let mut files = BTreeSet::new();
    let mut every_file_holds = true;
    for file in listed {
        let problem = if relative_path_problem(&file.value).is_some()
            || !owned
                .iter()
                .any(|directory| is_inside(&file.value, directory))
        {
            Some("which is outside every directory this manifest's `output` and modules name")
        } else if file.value == manifest_path {
            Some("which is the manifest itself")
        } else if files.contains(&file.value) {
            Some("twice")
        } else {
            None
        };
        if let Some(problem) = problem {
            unreadable(
                diagnostics,
                file.span,
                format!("{context} lists the file `{}`, {problem}", file.value),
            );
            every_file_holds = false;
        } else {
            files.insert(file.value);
        }
    }
    files.insert(manifest_path);
    every_file_holds.then_some(files)
}

/// Whether `file` is strictly inside `directory`: beneath it, and not it.
fn is_inside(file: &str, directory: &str) -> bool {
    file != directory && paths::is_within(file, directory)
}

fn unreadable(diagnostics: &mut Diagnostics, span: Span, message: String) {
    diagnostics.at(Code::ManifestUnreadable, span, message);
}

fn read_module(
    name: &str,
    value: &Node,
    context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<PreviousModule> {
    let Value::Table(table) = &value.value else {
        unreadable(
            diagnostics,
            value.span,
            format!(
                "{context} records module `{name}` as {}, not an object",
                value.value.describe()
            ),
        );
        return None;
    };
    let module_context = format!("module `{name}` in {context}");
    let mut walker = TableWalker::new(table, module_context.as_str(), diagnostics);
    let directory = walker.required_string("directory");
    let schema_version = walker.required_integer("schema_version");
    let spans = walker.required_table("spans");
    let metrics = walker.required_table("metrics");
    let logs = walker.required_table("logs");
    let errors = walker.required_table("errors");
    // Queries and dashboards consume the vocabulary; nothing here reads them.
    walker.consume("queries");
    walker.consume("dashboards");
    let retired_codes = walker.required_array("retired_codes");
    walker.finish(&[
        "directory",
        "schema_version",
        "spans",
        "metrics",
        "logs",
        "errors",
        "queries",
        "dashboards",
        "retired_codes",
    ]);

    let directory = directory.and_then(|directory| {
        let problem = if directory.value == paths::ROOT {
            None
        } else {
            relative_path_problem(&directory.value)
        };
        if let Some(problem) = problem {
            unreadable(
                diagnostics,
                directory.span,
                format!(
                    "{module_context} records the directory `{}`, which {problem}",
                    directory.value
                ),
            );
        }
        problem.is_none().then_some(directory.value)
    });
    let schema_version = schema_version.and_then(|version| {
        let in_range = u32::try_from(version.value)
            .ok()
            .filter(|number| *number >= 1);
        if in_range.is_none() {
            unreadable(
                diagnostics,
                version.span,
                format!(
                    "{module_context} records `schema_version` {}, which no definition can \
                     declare",
                    version.value
                ),
            );
        }
        in_range
    });

    let items = read_items(
        [
            (ItemKind::Span, spans),
            (ItemKind::Metric, metrics),
            (ItemKind::Log, logs),
        ],
        &module_context,
        diagnostics,
    );
    let errors = errors.and_then(|table| read_errors(table, &module_context, diagnostics));
    let retired_codes = retired_codes.and_then(|(elements, _)| {
        read_retired_codes(elements, errors.as_ref(), &module_context, diagnostics)
    });
    Some(PreviousModule {
        directory: directory?,
        schema_version: schema_version?,
        vocabulary: Vocabulary {
            items: items?,
            errors: errors?,
        },
        retired_codes: retired_codes?,
    })
}

/// The spans, metrics and logs a module records. A table that is absent has
/// already been reported as missing.
fn read_items(
    tables: [(ItemKind, Option<&Table>); 3],
    module_context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<BTreeMap<(ItemKind, String), Shape>> {
    let mut items = BTreeMap::new();
    let mut every_item_read = true;
    for (kind, table) in tables {
        let Some(table) = table else {
            every_item_read = false;
            continue;
        };
        for entry in &table.entries {
            if let Some(shape) = read_shape(kind, &entry.key, &entry.value.value, diagnostics) {
                items.insert((kind, entry.key.clone()), shape);
            } else {
                unreadable(
                    diagnostics,
                    entry.key_span,
                    format!(
                        "{module_context} records {} `{}` in a shape this generator does not read",
                        kind.keyword(),
                        entry.key
                    ),
                );
                every_item_read = false;
            }
        }
    }
    every_item_read.then_some(items)
}

/// The errors a module records, no two under one code.
fn read_errors(
    table: &Table,
    module_context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<BTreeMap<String, ErrorShape>> {
    let mut errors = BTreeMap::new();
    // Which error holds each code.
    let mut coded: BTreeMap<u32, String> = BTreeMap::new();
    let mut every_error_read = true;
    for entry in &table.entries {
        let Some(shape) = read_error(&entry.key, &entry.value.value, diagnostics) else {
            unreadable(
                diagnostics,
                entry.key_span,
                format!(
                    "{module_context} records error `{}` in a shape this generator does not read",
                    entry.key
                ),
            );
            every_error_read = false;
            continue;
        };
        if let Some(holder) = coded.insert(shape.code, entry.key.clone()) {
            unreadable(
                diagnostics,
                entry.key_span,
                format!(
                    "{module_context} records code {} for both error `{holder}` and error `{}`",
                    shape.code, entry.key
                ),
            );
            every_error_read = false;
        }
        errors.insert(entry.key.clone(), shape);
    }
    every_error_read.then_some(errors)
}

/// The codes a module retired: each a code, each once, and none one of its
/// errors still declares. `errors` is absent when they could not be read; the
/// list is then held only to itself.
fn read_retired_codes(
    elements: &[Node],
    errors: Option<&BTreeMap<String, ErrorShape>>,
    module_context: &str,
    diagnostics: &mut Diagnostics,
) -> Option<BTreeSet<u32>> {
    let holder_of = |code: u32| {
        errors?
            .iter()
            .find(|(_, error)| error.code == code)
            .map(|(name, _)| name.as_str())
    };
    let mut retired = BTreeSet::new();
    let mut every_code_holds = true;
    for element in elements {
        let code = match &element.value {
            Value::Integer(number) => u32::try_from(*number)
                .ok()
                .filter(|code| *code <= MAXIMUM_ERROR_CODE),
            _ => None,
        };
        let problem = match code {
            None => Some("holds a retired code that is not a code".to_string()),
            Some(code) if retired.contains(&code) => Some(format!("retires code {code} twice")),
            Some(code) => holder_of(code).map(|holder| {
                format!("retires code {code}, which error `{holder}` still declares")
            }),
        };
        if let Some(problem) = problem {
            unreadable(
                diagnostics,
                element.span,
                format!("{module_context} {problem}"),
            );
            every_code_holds = false;
        } else if let Some(code) = code {
            retired.insert(code);
        }
    }
    every_code_holds.then_some(retired)
}

/// One span's, metric's or log's shape, read leniently — a description is not
/// part of what readers depend on — but completely for what is.
fn read_shape(
    kind: ItemKind,
    name: &str,
    value: &Value,
    diagnostics: &mut Diagnostics,
) -> Option<Shape> {
    let Value::Table(table) = value else {
        return None;
    };
    let mut walker = TableWalker::new(table, format!("{} `{name}`", kind.keyword()), diagnostics);
    let attributes_key = if kind == ItemKind::Log {
        "fields"
    } else {
        "attributes"
    };
    let attributes = walker.optional_table(attributes_key);
    let events = if walker.has("events") {
        walker.optional_string_array("events")?
    } else {
        Vec::new()
    };
    let metric_kind = walker.optional_string("type");
    let unit = walker.optional_string("unit");
    // What only describes — a description, a span's kind, a log's level — is
    // not compared: it may change at any version.
    walker.consume("description");
    walker.consume("kind");
    walker.consume("level");
    walker.finish(&[
        "description",
        "kind",
        attributes_key,
        "events",
        "type",
        "unit",
        "level",
    ]);

    let metric = if kind == ItemKind::Metric {
        Some(MetricShape {
            kind: metric_kind?.value,
            unit: unit?.value,
        })
    } else {
        None
    };
    Some(Shape {
        attributes: read_keywords(attributes?)?,
        events: events.into_iter().map(|event| event.value).collect(),
        metric,
    })
}

fn read_error(name: &str, value: &Value, diagnostics: &mut Diagnostics) -> Option<ErrorShape> {
    let Value::Table(table) = value else {
        return None;
    };
    let mut walker = TableWalker::new(table, format!("error `{name}`"), diagnostics);
    let code = walker.required_integer("code");
    let class = walker.required_string("class");
    let fields = walker.required_table("fields");
    walker.consume("description");
    walker.finish(&["description", "class", "code", "fields"]);

    let code = u32::try_from(code?.value)
        .ok()
        .filter(|code| *code <= MAXIMUM_ERROR_CODE)?;
    Some(ErrorShape {
        code,
        class: class?.value,
        fields: read_keywords(fields?)?,
    })
}

/// A table of `name: "<type keyword>"` entries.
fn read_keywords(table: &Table) -> Option<BTreeMap<String, String>> {
    let mut keywords = BTreeMap::new();
    for entry in &table.entries {
        let Value::String(keyword) = &entry.value.value else {
            return None;
        };
        keywords.insert(entry.key.clone(), keyword.clone());
    }
    Some(keywords)
}
