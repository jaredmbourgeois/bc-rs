//! The Rust adapter: one source file per module, emitting onto bc-telemetry-rs
//! and bc-error-rs.
//!
//! The file is `<module dir>/<output>/rust/<module>_telemetry.rs`, and it holds
//! a module's items, not a module: a consumer names the module where it brings
//! the file in, `mod payments { include!("…/payments_telemetry.rs"); }`. Through
//! `include!` the file is the generator's to format — `rustfmt` does not follow
//! it — and `--check` keeps it fresh.
//!
//! What it holds, per kind of item:
//!
//! - a span: a function returning the declared span, beside a module of its
//!   name holding that type and what its verbs yield. The verbs are `start`,
//!   `open`, `run` and `run_async`, and each takes the parent. `start` yields
//!   the runtime's own `SpanGuard`, so the lint that refuses a guard held
//!   across an `.await` sees it — a wrapper would hide it, since the lint reads
//!   a held value's own type; the held and running spans have a
//!   method per declared event, and `Running::of(guard.span())` is a guard's.
//! - a metric: a function returning a value whose verb is `record(parent,
//!   value)`.
//! - a log: a function returning a value whose verb is `emit(parent)`.
//! - an error: its kind in `error_kinds`, and in `errors` a constructor
//!   returning the error's own type, `errors::<error>::Error`, which holds the
//!   `bc_error::AnyError` with the declared fields as metadata and converts back
//!   from one (`try_from_error`, handing any other back in a `NotThisError`);
//!   for a failure only, its report calls in `report` and `report_degraded`,
//!   which take only that type, so no report names another error.
//!
//! An item's type lives in a module named for the item, so no two items'
//! types can collide, whatever their names. Every runtime path is spelled from
//! the crate root, `::bc_telemetry::…`, so no name a consumer has in scope
//! changes what the file means.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use bc_telemetry::{MetricKind, Severity, SpanKind};

use crate::adapters::NameRole;
use crate::model::{ErrorClass, Module, ScalarType};
use crate::output::GeneratedTree;
use crate::paths;

/// Rust's strict and reserved keywords, 2024 edition: none can name a function,
/// a module, a parameter or a method without a raw identifier, which the file
/// does not write.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// The methods and associated functions a span's handles already have, which
/// an event method may not shadow.
const SPAN_HANDLE_MEMBERS: &[&str] = &["into_span", "of", "span"];

/// Whether `name` cannot play `role` in the Rust this adapter writes.
pub(crate) fn is_reserved(role: NameRole, name: &str) -> bool {
    KEYWORDS.contains(&name)
        || match role {
            NameRole::Event => SPAN_HANDLE_MEMBERS.contains(&name),
            NameRole::Item(_) | NameRole::Attribute(_) => false,
        }
}

/// Where a module's file goes.
fn file_path(module_name: &str, module: &Module, output: &str) -> String {
    paths::under(
        &paths::under(&module.directory, output),
        &format!("rust/{module_name}_telemetry.rs"),
    )
}

/// Adds every module's file to what this run generates.
///
/// # Errors
///
/// Writing into a `String` failed, which it does not: the error is the
/// formatting machinery's, carried rather than discarded.
pub(crate) fn render(
    modules: &BTreeMap<String, Module>,
    output: &str,
    generated: &mut GeneratedTree,
) -> Result<(), fmt::Error> {
    for (module_name, module) in modules {
        generated.insert(
            file_path(module_name, module, output),
            source(module_name, module)?,
        );
    }
    Ok(())
}

/// A description as doc-comment lines at `indent`.
pub(crate) fn doc_comment(indent: &str, description: &str) -> Result<String, fmt::Error> {
    let mut comment = String::new();
    for line in description.lines() {
        if line.is_empty() {
            writeln!(comment, "{indent}///")?;
        } else {
            writeln!(comment, "{indent}/// {line}")?;
        }
    }
    Ok(comment)
}

/// `text` as a Rust string literal: `Debug` writes exactly that, escaping what
/// would end or break it.
pub(crate) fn string_literal(text: &str) -> String {
    format!("{text:?}")
}

/// A declared name as a Rust identifier, for a file that cannot refuse one: a
/// kebab name's hyphens become underscores, and a keyword gains a trailing `_`
/// (`type_`, `super_`). No declared name ends in `_` — its words are never
/// empty — so the suffix makes no two names one, which a raw identifier could
/// not promise: `r#super` does not compile.
pub(crate) fn identifier(name: &str) -> String {
    let snake = name.replace('-', "_");
    if KEYWORDS.contains(&snake.as_str()) {
        format!("{snake}_")
    } else {
        snake
    }
}

/// A snake-case name as a constant: `card_expired` → `CARD_EXPIRED`.
fn constant_name(name: &str) -> String {
    name.to_ascii_uppercase()
}

/// The type a constructor takes for an attribute of `scalar`.
const fn parameter_type(scalar: ScalarType) -> &'static str {
    match scalar {
        ScalarType::String | ScalarType::Digest => "impl Into<String>",
        ScalarType::Int | ScalarType::Duration => "i64",
        ScalarType::Float => "f64",
        ScalarType::Bool => "bool",
    }
}

/// `name` as the attribute value it becomes.
fn attribute_value(scalar: ScalarType, name: &str) -> String {
    match scalar {
        ScalarType::String | ScalarType::Digest => {
            format!("::bc_telemetry::AttributeValue::String({name}.into())")
        }
        ScalarType::Int | ScalarType::Duration => {
            format!("::bc_telemetry::AttributeValue::Int64({name})")
        }
        ScalarType::Float => format!("::bc_telemetry::AttributeValue::Float64({name})"),
        ScalarType::Bool => format!("::bc_telemetry::AttributeValue::Boolean({name})"),
    }
}

/// `name` as the metadata value it becomes.
fn metadata_value(scalar: ScalarType, name: &str) -> String {
    match scalar {
        ScalarType::String | ScalarType::Digest => {
            format!("::bc_error::MetadataValue::String({name}.into())")
        }
        ScalarType::Int | ScalarType::Duration => {
            format!("::bc_error::MetadataValue::Int64({name})")
        }
        ScalarType::Float => format!("::bc_error::MetadataValue::Float64({name})"),
        ScalarType::Bool => format!("::bc_error::MetadataValue::Boolean({name})"),
    }
}

/// How a report reads a field back from the metadata, and the attribute value
/// it makes of what it read.
const fn metadata_reader(scalar: ScalarType) -> (&'static str, &'static str) {
    match scalar {
        ScalarType::String | ScalarType::Digest => {
            ("as_string", "::bc_telemetry::AttributeValue::from(value)")
        }
        ScalarType::Int | ScalarType::Duration => {
            ("as_int64", "::bc_telemetry::AttributeValue::Int64(value)")
        }
        ScalarType::Float => (
            "as_float64",
            "::bc_telemetry::AttributeValue::Float64(value)",
        ),
        ScalarType::Bool => ("as_bool", "::bc_telemetry::AttributeValue::Boolean(value)"),
    }
}

fn parameters(attributes: &BTreeMap<String, ScalarType>) -> String {
    attributes
        .iter()
        .map(|(name, scalar)| format!("{name}: {}", parameter_type(*scalar)))
        .collect::<Vec<String>>()
        .join(", ")
}

/// The attribute list a constructor builds: the declared attributes, then the
/// reserved ones every generated record carries — a log's qualified name, since
/// its body is its description, and the module's schema version.
fn attribute_list(
    indent: &str,
    attributes: &BTreeMap<String, ScalarType>,
    log_name: Option<&str>,
) -> Result<String, fmt::Error> {
    let mut list = String::new();
    for (name, scalar) in attributes {
        writeln!(
            list,
            "{indent}({}.to_owned(), {}),",
            string_literal(name),
            attribute_value(*scalar, name)
        )?;
    }
    if let Some(log_name) = log_name {
        writeln!(
            list,
            "{indent}(\"bc.telemetry.log\".to_owned(), ::bc_telemetry::AttributeValue::from({})),",
            string_literal(log_name)
        )?;
    }
    writeln!(
        list,
        "{indent}(\"bc.telemetry.schema_version\".to_owned(), ::bc_telemetry::AttributeValue::Int64(super::SCHEMA_VERSION)),"
    )?;
    Ok(list)
}

const fn span_kind(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Internal => "Internal",
        SpanKind::Server => "Server",
        SpanKind::Client => "Client",
        SpanKind::Producer => "Producer",
        SpanKind::Consumer => "Consumer",
    }
}

const fn severity(severity: Severity) -> (&'static str, &'static str) {
    match severity {
        Severity::Debug => ("Debug", "debug"),
        Severity::Info => ("Info", "info"),
        Severity::Warn => ("Warn", "warn"),
        Severity::Error => ("Error", "error"),
        Severity::Fatal => ("Fatal", "fatal"),
    }
}

const OPENING: &str = "\
// Generated by bc-telemetry-gen. Do not edit.
//
// Bring it in as a module of the name you choose, in a crate that depends on bc-telemetry-rs and
// bc-error-rs: `mod payments { include!(\"…/payments_telemetry.rs\"); }`. Each declared item is a
// function in its kind's module that takes the item's attributes and returns a value whose verbs
// take every choice as an argument: a span's `start`, `open`, `run` and `run_async`, a metric's
// `record`, a log's `emit`. They emit onto bc-telemetry-rs and spell every argument it takes.
";

/// The whole file for one module.
fn source(module_name: &str, module: &Module) -> Result<String, fmt::Error> {
    let mut text = format!("// The telemetry of module `{module_name}`, for Rust.\n//\n{OPENING}");
    write!(
        text,
        "\n/// The module's name: its records' names start with it, and it is its errors' category.\n\
         pub const MODULE_NAME: &str = {};\n\n\
         /// The module's schema version, on every record it emits (`bc.telemetry.schema_version`).\n\
         pub const SCHEMA_VERSION: i64 = {};\n",
        string_literal(module_name),
        module.schema_version.value
    )?;
    let items = &module.items;
    if !items.spans.is_empty() {
        text.push_str("\n/// The module's spans.\npub mod spans {");
        for (name, span) in &items.spans {
            text.push_str(&span_section(module_name, name, span)?);
        }
        text.push_str("}\n");
    }
    if !items.metrics.is_empty() {
        text.push_str("\n/// The module's metrics.\npub mod metrics {");
        for (name, metric) in &items.metrics {
            text.push_str(&metric_section(module_name, name, metric)?);
        }
        text.push_str("}\n");
    }
    if !items.logs.is_empty() {
        text.push_str("\n/// The module's logs.\npub mod logs {");
        for (name, log) in &items.logs {
            text.push_str(&log_section(module_name, name, log)?);
        }
        text.push_str("}\n");
    }
    if !items.errors.is_empty() {
        text.push_str(&error_sections(module_name, module)?);
    }
    Ok(text)
}

/// The function a call site starts from, `name(attributes…) -> name::Type`,
/// building the item's attribute list.
fn constructor(
    description: &str,
    name: &str,
    type_name: &str,
    attributes: &BTreeMap<String, ScalarType>,
    log_name: Option<&str>,
) -> Result<String, fmt::Error> {
    let mut text = String::from("\n");
    text.push_str(&doc_comment("    ", description)?);
    write!(
        text,
        "    pub fn {name}({}) -> {name}::{type_name} {{\n        \
         {name}::{type_name} {{\n            attributes: vec![\n{}            ],\n        }}\n    }}\n",
        parameters(attributes),
        attribute_list("                ", attributes, log_name)?
    )?;
    Ok(text)
}

fn span_section(
    module_name: &str,
    name: &str,
    span: &crate::model::SpanDeclaration,
) -> Result<String, fmt::Error> {
    let events = |target: &str| -> Result<String, fmt::Error> {
        let mut methods = String::new();
        for event in &span.events {
            write!(
                methods,
                "\n            /// The span's `{event}` event, recorded on this span.\n            \
                 pub fn {event}(&self) {{\n                \
                 {target}.add_event({}, ::bc_telemetry::Attributes::new());\n            }}\n",
                string_literal(event)
            )?;
        }
        Ok(methods)
    };
    let mut text = constructor(&span.description, name, "Span", &span.attributes, None)?;
    write!(
        text,
        "\n    /// The `{name}` span, and what its verbs yield.\n    pub mod {name} {{\n        \
         const NAME: &str = {};\n        \
         const KIND: ::bc_telemetry::SpanKind = ::bc_telemetry::SpanKind::{};\n\n",
        string_literal(&format!("{module_name}.{name}")),
        span_kind(span.kind)
    )?;
    text.push_str(&doc_comment("        ", &span.description)?);
    write!(
        text,
        "        ///\n        /// The span before a verb starts it.\n        \
         #[must_use = \"a span starts only when one of its verbs is called\"]\n        \
         #[derive(Debug)]\n        pub struct Span {{\n            \
         pub(super) attributes: ::bc_telemetry::Attributes,\n        }}\n\n        \
         impl Span {{\n            \
         /// Starts the span under `parent`, current on this thread until the guard ends. The\n            \
         /// guard is the runtime's own, so the lint that refuses one held across an `.await`\n            \
         /// sees it; its events are `Running::of(guard.span())`'s.\n            \
         pub fn start(self, parent: ::bc_telemetry::Parent) -> ::bc_telemetry::SpanGuard {{\n                \
         ::bc_telemetry::start_span(parent, NAME, KIND, self.attributes)\n            \
         }}\n\n            \
         /// Opens the span under `parent`, current nowhere until made so.\n            \
         pub fn open(self, parent: ::bc_telemetry::Parent) -> Held {{\n                \
         Held {{\n                    \
         span: ::bc_telemetry::open_span(parent, NAME, KIND, self.attributes),\n                \
         }}\n            }}\n\n            \
         /// Starts the span under `parent`, runs `body` in it, and ends it with the status\n            \
         /// `end` gives `body`'s result. `body` takes the `Running` span.\n            \
         pub fn run<R>(\n                self,\n                parent: ::bc_telemetry::Parent,\n                \
         end: impl FnOnce(&R) -> ::bc_telemetry::SpanStatus,\n                \
         body: impl FnOnce(Running<'_>) -> R,\n            ) -> R {{\n                \
         ::bc_telemetry::running_span(parent, NAME, KIND, self.attributes, end, |span| {{\n                    \
         body(Running {{ span }})\n                }})\n            }}\n\n            \
         /// Starts the span under `parent` when first polled, runs `body` with it current for\n            \
         /// each poll, and ends it with the status `end` gives the output.\n            \
         pub async fn run_async<F: ::std::future::Future>(\n                self,\n                \
         parent: ::bc_telemetry::Parent,\n                \
         end: impl FnOnce(&F::Output) -> ::bc_telemetry::SpanStatus,\n                \
         body: F,\n            ) -> F::Output {{\n                \
         ::bc_telemetry::running_span_async(parent, NAME, KIND, self.attributes, end, body).await\n            \
         }}\n        }}\n\n"
    )?;
    write!(
        text,
        "        /// The span held, current nowhere until made so: what `open` yields.\n        \
         #[must_use = \"the span ends when its handle drops\"]\n        \
         #[derive(Debug)]\n        pub struct Held {{\n            \
         span: ::bc_telemetry::OpenSpan,\n        }}\n\n        impl Held {{{}\n            \
         /// The runtime's handle: to make the span current, set its status, or name it a parent.\n            \
         pub const fn span(&self) -> &::bc_telemetry::OpenSpan {{\n                \
         &self.span\n            }}\n\n            \
         /// The runtime's handle, to end the span with a status or send it elsewhere.\n            \
         pub fn into_span(self) -> ::bc_telemetry::OpenSpan {{\n                \
         self.span\n            }}\n        }}\n\n",
        events("self.span")?
    )?;
    write!(
        text,
        "        /// A span's events, on a span someone else holds: what a `run` body takes, and what\n        \
         /// `Running::of(guard.span())` makes of a `start`ed span's guard.\n        \
         #[derive(Debug)]\n        pub struct Running<'span> {{\n            \
         span: &'span ::bc_telemetry::OpenSpan,\n        }}\n\n        impl<'span> Running<'span> {{\n            \
         /// The events of `span`: a guard's, `guard.span()`, or a held span's.\n            \
         pub const fn of(span: &'span ::bc_telemetry::OpenSpan) -> Self {{\n                \
         Self {{ span }}\n            }}\n{}\n            \
         /// The runtime's handle.\n            \
         pub const fn span(&self) -> &::bc_telemetry::OpenSpan {{\n                \
         self.span\n            }}\n        }}\n    }}\n",
        events("self.span")?
    )?;
    Ok(text)
}

fn metric_section(
    module_name: &str,
    name: &str,
    metric: &crate::model::MetricDeclaration,
) -> Result<String, fmt::Error> {
    let (value_type, value_name, runtime_call, what) = match metric.kind {
        MetricKind::Counter => ("i64", "increment", "record_counter", "one increment"),
        MetricKind::Gauge => ("f64", "value", "record_gauge", "one reading"),
        MetricKind::Histogram => ("f64", "value", "record_histogram", "one observation"),
    };
    let mut text = constructor(
        &metric.description,
        name,
        "Metric",
        &metric.attributes,
        None,
    )?;
    write!(
        text,
        "\n    /// The `{name}` metric.\n    pub mod {name} {{\n        const NAME: &str = {};\n\n",
        string_literal(&format!("{module_name}.{name}"))
    )?;
    text.push_str(&doc_comment("        ", &metric.description)?);
    write!(
        text,
        "        #[must_use = \"nothing is recorded until `record` is called\"]\n        \
         #[derive(Debug)]\n        pub struct Metric {{\n            \
         pub(super) attributes: ::bc_telemetry::Attributes,\n        }}\n\n        \
         impl Metric {{\n            \
         /// Records {what}, in `{}`, under `parent`.\n            \
         pub fn record(self, parent: ::bc_telemetry::Parent, {value_name}: {value_type}) {{\n                \
         ::bc_telemetry::{runtime_call}(parent, NAME, {value_name}, self.attributes);\n            \
         }}\n        }}\n    }}\n",
        crate::model::Keyword::keyword(metric.unit),
    )?;
    Ok(text)
}

fn log_section(
    module_name: &str,
    name: &str,
    log: &crate::model::LogDeclaration,
) -> Result<String, fmt::Error> {
    let qualified = format!("{module_name}.{name}");
    let (variant, level) = severity(log.severity);
    let mut text = constructor(&log.description, name, "Log", &log.fields, Some(&qualified))?;
    write!(
        text,
        "\n    /// The `{name}` log.\n    pub mod {name} {{\n        const DESCRIPTION: &str = {};\n\n",
        string_literal(&log.description)
    )?;
    text.push_str(&doc_comment("        ", &log.description)?);
    write!(
        text,
        "        #[must_use = \"nothing is recorded until `emit` is called\"]\n        \
         #[derive(Debug)]\n        pub struct Log {{\n            \
         pub(super) attributes: ::bc_telemetry::Attributes,\n        }}\n\n        \
         impl Log {{\n            \
         /// Emits the record under `parent`, at `{level}`, its body the description.\n            \
         pub fn emit(self, parent: ::bc_telemetry::Parent) {{\n                \
         ::bc_telemetry::record_log(parent, ::bc_telemetry::Severity::{variant}, DESCRIPTION, self.attributes);\n            \
         }}\n        }}\n    }}\n",
    )?;
    Ok(text)
}

fn error_sections(module_name: &str, module: &Module) -> Result<String, fmt::Error> {
    let errors = &module.items.errors;
    let mut text = String::from("\n/// The module's error kinds.\npub mod error_kinds {");
    for (name, error) in errors {
        text.push('\n');
        text.push_str(&doc_comment("    ", &error.description)?);
        let descriptor = match error.class {
            ErrorClass::Failure => "AnyErrorKind",
            ErrorClass::Refusal => "RefusalKind",
        };
        writeln!(
            text,
            "    pub const {}: ::bc_error::{descriptor} =\n        ::bc_error::{descriptor}::new(super::MODULE_NAME, {});",
            constant_name(name),
            error.code.value
        )?;
    }
    text.push_str(
        "}\n\n/// The module's error constructors, and beside each the error type it builds.\npub mod errors {",
    );
    for (name, error) in errors {
        text.push_str(&error_constructor(name, error)?);
        text.push_str(&error_type(name, error)?);
    }
    text.push_str("}\n");

    text.push_str(&report_sections(module_name, module)?);
    text.push_str(
        "\n/// Records `error` on the current span, for every error the code meets — a refusal among them.\n\
         pub fn record_error(error: &::bc_error::AnyError) {\n    \
         ::bc_telemetry::record_error_telemetry(error);\n}\n",
    );
    Ok(text)
}

/// An error's constructor: the metadata is built in one expression, with no
/// local a declared field's parameter could share a name with.
fn error_constructor(
    name: &str,
    error: &crate::model::ErrorDeclaration,
) -> Result<String, fmt::Error> {
    let dictionary = if error.fields.is_empty() {
        "::bc_error::MetadataDictionary::new()".to_owned()
    } else {
        let mut entries = String::new();
        for (field, scalar) in &error.fields {
            writeln!(
                entries,
                "                ({}.to_owned(), {}),",
                string_literal(field),
                metadata_value(*scalar, field)
            )?;
        }
        format!("::bc_error::MetadataDictionary::from([\n{entries}            ])")
    };
    let mut text = String::from("\n");
    text.push_str(&doc_comment("    ", &error.description)?);
    write!(
        text,
        "    #[track_caller]\n    pub fn {name}({}) -> {name}::Error {{\n        \
         {name}::Error(::bc_error::AnyError::from_metadata(\n            \
         super::error_kinds::{},\n            {dictionary},\n        ))\n    }}\n",
        parameters(&error.fields),
        constant_name(name)
    )?;
    Ok(text)
}

/// An error's own type, in a module of its name: what its constructor builds
/// and the one error its reports take, so no report names another error, with
/// the conversion that hands any other `AnyError` back.
fn error_type(name: &str, error: &crate::model::ErrorDeclaration) -> Result<String, fmt::Error> {
    let mut checks = String::new();
    for (field, scalar) in &error.fields {
        let (reader, _) = metadata_reader(*scalar);
        write!(
            checks,
            "                if error.metadata().get({}).and_then(::bc_error::MetadataValue::{reader}).is_none() {{\n                    \
             return ::std::result::Result::Err(::bc_telemetry::NotThisError {{\n                        \
             error,\n                        \
             reason: ::bc_telemetry::NotThisErrorReason::MissingField({}),\n                    \
             }});\n                }}\n",
            string_literal(field),
            string_literal(field)
        )?;
    }
    let taken_by = match error.class {
        ErrorClass::Failure => {
            format!("the one error `report::{name}` and `report_degraded::{name}` take")
        }
        ErrorClass::Refusal => {
            "a refusal, the caller's to correct, so recorded and never reported".to_owned()
        }
    };
    let mut text = format!("\n    /// `{name}`'s type.\n    pub mod {name} {{\n");
    text.push_str(&doc_comment("        ", &error.description)?);
    write!(
        text,
        "        ///\n        /// Built by `errors::{name}`, or converted from an `AnyError` by\n        \
         /// `try_from_error`: {taken_by}.\n        \
         #[must_use = \"an error is recorded, reported or returned, never dropped unread\"]\n        \
         #[derive(Debug, Clone, PartialEq)]\n        \
         pub struct Error(pub(super) ::bc_error::AnyError);\n\n        \
         impl Error {{\n            \
         /// `error` as this error, when it is one — of this kind, with every declared field;\n            \
         /// otherwise it is handed back unchanged, with why.\n            \
         pub fn try_from_error(\n                \
         error: ::bc_error::AnyError,\n            \
         ) -> ::std::result::Result<Self, ::bc_telemetry::NotThisError> {{\n                \
         if !error.matches(&super::super::error_kinds::{}) {{\n                    \
         return ::std::result::Result::Err(::bc_telemetry::NotThisError {{\n                        \
         error,\n                        \
         reason: ::bc_telemetry::NotThisErrorReason::OtherKind,\n                    \
         }});\n                }}\n{checks}                \
         ::std::result::Result::Ok(Self(error))\n            }}\n\n            \
         /// The error itself: its identity and metadata, or to record it.\n            \
         pub const fn error(&self) -> &::bc_error::AnyError {{\n                \
         &self.0\n            }}\n\n            \
         /// The error itself, to return or propagate.\n            \
         pub fn into_error(self) -> ::bc_error::AnyError {{\n                \
         self.0\n            }}\n        }}\n\n        \
         impl ::std::convert::From<Error> for ::bc_error::AnyError {{\n            \
         fn from(error: Error) -> Self {{\n                \
         error.0\n            }}\n        }}\n    }}\n",
        constant_name(name)
    )?;
    Ok(text)
}

/// Each failure's report calls: the context a report logs, then `report` and
/// `report_degraded`. A refusal has none: one the caller can correct is not an
/// error of ours, and logging it would bury the ones that are.
fn report_sections(module_name: &str, module: &Module) -> Result<String, fmt::Error> {
    let mut text = String::new();
    let failures: Vec<(&String, &crate::model::ErrorDeclaration)> = module
        .items
        .errors
        .iter()
        .filter(|(_, error)| error.class == ErrorClass::Failure)
        .collect();
    if failures.is_empty() {
        return Ok(text);
    }
    text.push_str("\n/// What each failure's report logs.\nmod report_detail {");
    for (name, error) in &failures {
        write!(
            text,
            "\n    /// What a report of `{name}` logs beside the error's identity: its name, the\n    \
             /// module's schema version, and the declared fields, read from the metadata\n    \
             /// `errors::{name}` put them in.\n    \
             pub(super) fn {name}_context(error: &super::errors::{name}::Error) -> ::bc_telemetry::Attributes {{\n        \
             let mut context: ::bc_telemetry::Attributes = vec![\n            \
             (\"bc.telemetry.log\".to_owned(), ::bc_telemetry::AttributeValue::from({})),\n            \
             (\"bc.telemetry.schema_version\".to_owned(), ::bc_telemetry::AttributeValue::Int64(super::SCHEMA_VERSION)),\n        \
             ];\n",
            string_literal(&format!("{module_name}.{name}"))
        )?;
        for (field, scalar) in &error.fields {
            let (reader, value) = metadata_reader(*scalar);
            write!(
                text,
                "        if let Some(value) = error.error().metadata().get({}).and_then(::bc_error::MetadataValue::{reader}) {{\n            \
                 context.push(({}.to_owned(), {value}));\n        }}\n",
                string_literal(field),
                string_literal(field)
            )?;
        }
        write!(
            text,
            "        context\n    }}\n\n    pub(super) const {}_DESCRIPTION: &str = {};\n",
            constant_name(name),
            string_literal(&error.description)
        )?;
    }
    text.push_str("}\n");

    for (verb, runtime_call, answered, level) in [
        ("report", "report_error", "could not answer past", "error"),
        (
            "report_degraded",
            "report_degraded",
            "answered past with a lesser result",
            "warn",
        ),
    ] {
        write!(
            text,
            "\n/// Each failure's `{runtime_call}`.\npub mod {verb} {{"
        )?;
        for (name, _) in &failures {
            write!(
                text,
                "\n    /// Reports `error`, built by `errors::{name}`, as an error the code {answered}:\n    \
                 /// it is recorded on the current span, then logged at `{level}` with the declaration's\n    \
                 /// description as its body.\n    \
                 pub fn {name}(error: &super::errors::{name}::Error) {{\n        \
                 ::bc_telemetry::{runtime_call}(\n            \
                 super::report_detail::{}_DESCRIPTION,\n            \
                 error.error(),\n            \
                 super::report_detail::{name}_context(error),\n        );\n    }}\n",
                constant_name(name)
            )?;
        }
        text.push_str("}\n");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::{constant_name, identifier, is_reserved, string_literal};
    use crate::adapters::NameRole;
    use crate::model::ItemKind;

    #[test]
    fn a_string_literal_escapes_what_would_end_or_break_it() {
        assert_eq!(
            string_literal("a \"b\"\\c\nd\te\u{1}f"),
            "\"a \\\"b\\\"\\\\c\\nd\\te\\u{1}f\""
        );
        assert_eq!(string_literal("ünïcödé"), "\"ünïcödé\"");
    }

    #[test]
    fn a_declared_name_becomes_an_identifier_one_way() {
        assert_eq!(identifier("checkout-p95"), "checkout_p95");
        assert_eq!(identifier("payments"), "payments");
        assert_eq!(identifier("type"), "type_");
        assert_eq!(identifier("super"), "super_");
        assert_eq!(
            identifier("move-fast"),
            "move_fast",
            "only a whole keyword is one"
        );
    }

    #[test]
    fn a_snake_name_becomes_a_constant() {
        assert_eq!(constant_name("card_expired"), "CARD_EXPIRED");
        assert_eq!(constant_name("log_2x"), "LOG_2X");
    }

    #[test]
    fn a_name_is_reserved_by_the_role_it_plays() {
        assert!(is_reserved(NameRole::Item(ItemKind::Span), "type"));
        assert!(is_reserved(NameRole::Attribute(ItemKind::Metric), "gen"));
        assert!(!is_reserved(NameRole::Item(ItemKind::Span), "checkout"));
        assert!(is_reserved(NameRole::Event, "of"));
        assert!(is_reserved(NameRole::Event, "span"));
        assert!(!is_reserved(NameRole::Attribute(ItemKind::Span), "span"));
        // No location parameter: Rust captures the caller with `#[track_caller]`.
        assert!(!is_reserved(
            NameRole::Attribute(ItemKind::Error),
            "location"
        ));
    }
}
