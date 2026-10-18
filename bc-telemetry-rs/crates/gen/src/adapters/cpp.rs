//! The C++ adapter: one header per module, emitting onto `bourgeois_concern`'s
//! telemetry and error-telemetry libraries.
//!
//! The header is `<module dir>/<output>/cpp/<module>_telemetry.hpp`. Its
//! basename is unique across the tree, because a C++ build's include
//! directories form one flat namespace of names; so is its namespace,
//! `<module>_telemetry`, which shadows no `bc::` namespace from inside one.
//!
//! What it holds, per kind of item:
//!
//! - a span: a type and a function returning it, whose verbs are `start`,
//!   `open` and `run` and each take the parent; every handle a verb yields has a
//!   method per declared event, recorded on that span. C++ has no asynchronous
//!   functions in bc's code, so no `run_async`.
//! - a metric: a type whose verb is `record(parent, value)`.
//! - a log: a type whose verb is `emit(parent)`.
//! - an error: its bc-error kind in `error_kinds`, a constructor in `errors`
//!   returning the `bc::AnyError` with the declared fields as metadata, and —
//!   for a failure only — its report calls in `report` and `report_degraded`.
//!
//! Every runtime call spells every argument the runtime takes, so the header
//! compiles unchanged once bc's own declarations lose their defaults. The one
//! default it writes is `std::source_location::current()`, which captures the
//! caller's line and is the house rule's own exception.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use bc_telemetry::{MetricKind, Severity, SpanKind};

use crate::adapters::NameRole;
use crate::model::{ErrorClass, ItemKind, Module, ScalarType};
use crate::output::GeneratedTree;
use crate::paths;

/// C++20's keywords and alternative tokens, and the standard library's two
/// lowercase macros a name could meet: none of them can name a function, a
/// parameter or a method.
const KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "assert",
    "auto",
    "bitand",
    "bitor",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "char8_t",
    "char16_t",
    "char32_t",
    "class",
    "co_await",
    "co_return",
    "co_yield",
    "compl",
    "concept",
    "const",
    "const_cast",
    "consteval",
    "constexpr",
    "constinit",
    "continue",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "dynamic_cast",
    "else",
    "enum",
    "errno",
    "explicit",
    "export",
    "extern",
    "false",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "private",
    "protected",
    "public",
    "register",
    "reinterpret_cast",
    "requires",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "static_cast",
    "struct",
    "switch",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typedef",
    "typeid",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "wchar_t",
    "while",
    "xor",
    "xor_eq",
];

/// The members a span's handles already have, which an event method may not
/// shadow.
const SPAN_HANDLE_MEMBERS: &[&str] = &["guard", "span"];

/// The handle classes nested in a span's class, which the span may not be
/// named for: a span `scope` would be `class Scope` holding a `class Scope`,
/// and a nested class may not share its enclosing class's name.
const SPAN_HANDLE_CLASSES: &[&str] = &["scope", "held", "running"];

/// The one class every header's `errors` namespace declares beside the
/// errors' own, which an error may not be named for (`NOT_THIS_ERROR`).
const ERRORS_NAMESPACE_CLASS: &str = "not_this_error";

/// The parameter an error's constructor takes after its fields.
const ERROR_LOCATION_PARAMETER: &str = "location";

/// Whether `name` cannot play `role` in the C++ this adapter writes.
pub(crate) fn is_reserved(role: NameRole, name: &str) -> bool {
    KEYWORDS.contains(&name)
        || match role {
            NameRole::Event => SPAN_HANDLE_MEMBERS.contains(&name),
            NameRole::Attribute(ItemKind::Error) => name == ERROR_LOCATION_PARAMETER,
            NameRole::Item(ItemKind::Span) => SPAN_HANDLE_CLASSES.contains(&name),
            NameRole::Item(ItemKind::Error) => name == ERRORS_NAMESPACE_CLASS,
            NameRole::Item(_) | NameRole::Attribute(_) => false,
        }
}

/// Where a module's header goes: its basename is unique across the tree.
fn header_path(module_name: &str, module: &Module, output: &str) -> String {
    paths::under(
        &paths::under(&module.directory, output),
        &format!("cpp/{module_name}_telemetry.hpp"),
    )
}

/// Adds every module's header to what this run generates.
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
            header_path(module_name, module, output),
            header(module_name, module)?,
        );
    }
    Ok(())
}

/// A snake-case name as a C++ type: `request_duration` → `RequestDuration`. A
/// separator before a digit stays, so `log_2x` and `log2x` remain two types.
pub(crate) fn type_name(name: &str) -> String {
    let mut spelled = String::with_capacity(name.len());
    let mut at_word_start = true;
    for character in name.chars() {
        if character == '_' {
            at_word_start = true;
            continue;
        }
        if at_word_start && character.is_ascii_digit() && !spelled.is_empty() {
            spelled.push('_');
            spelled.push(character);
        } else if at_word_start {
            spelled.push(character.to_ascii_uppercase());
        } else {
            spelled.push(character);
        }
        at_word_start = false;
    }
    spelled
}

/// `text` as a C++ string literal. Control characters are written as
/// three-digit octal escapes, which — unlike `\x` — end where they say.
pub(crate) fn string_literal(text: &str) -> Result<String, fmt::Error> {
    let mut literal = String::from('"');
    for character in text.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            control if control.is_control() && u32::from(control) < 0x80 => {
                write!(literal, "\\{:03o}", u32::from(control))?;
            }
            other => literal.push(other),
        }
    }
    literal.push('"');
    Ok(literal)
}

fn cpp_type(scalar: ScalarType) -> &'static str {
    match scalar {
        ScalarType::String | ScalarType::Digest => "std::string",
        ScalarType::Int | ScalarType::Duration => "bc::i64",
        ScalarType::Float => "bc::f64",
        ScalarType::Bool => "bool",
    }
}

/// How a parameter is handed on: a string moves, a number copies.
fn passed(scalar: ScalarType, name: &str) -> String {
    match scalar {
        ScalarType::String | ScalarType::Digest => format!("std::move({name})"),
        ScalarType::Int | ScalarType::Duration | ScalarType::Float | ScalarType::Bool => {
            name.to_string()
        }
    }
}

fn parameters(attributes: &BTreeMap<String, ScalarType>) -> Vec<String> {
    attributes
        .iter()
        .map(|(name, scalar)| format!("{} {name}", cpp_type(*scalar)))
        .collect()
}

fn arguments(attributes: &BTreeMap<String, ScalarType>) -> Vec<String> {
    attributes
        .iter()
        .map(|(name, scalar)| passed(*scalar, name))
        .collect()
}

/// A description as doc-comment lines at `indent`.
fn doc_comment(indent: &str, description: &str) -> Result<String, fmt::Error> {
    let mut comment = String::new();
    for line in description.lines() {
        writeln!(comment, "{indent}/// {line}")?;
    }
    Ok(comment)
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
            "{indent}    {{\"{name}\", {}}},",
            passed(*scalar, name)
        )?;
    }
    if let Some(log_name) = log_name {
        writeln!(
            list,
            "{indent}    {{\"bc.telemetry.log\", std::string{{\"{log_name}\"}}}},"
        )?;
    }
    writeln!(
        list,
        "{indent}    {{\"bc.telemetry.schema_version\", schema_version}},"
    )?;
    Ok(list)
}

const fn span_kind(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Internal => "internal",
        SpanKind::Server => "server",
        SpanKind::Client => "client",
        SpanKind::Producer => "producer",
        SpanKind::Consumer => "consumer",
    }
}

const fn log_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
        Severity::Fatal => "fatal",
    }
}

const OPENING: &str = "\
// Generated by bc-telemetry-gen. Do not edit.
//
// Each declared item is a function in its kind's namespace that takes the item's attributes and
// returns a value whose verbs take every choice as an argument: a span's `start`, `open` and
// `run`, a metric's `record`, a log's `emit`. They emit onto bourgeois_concern's telemetry and
// error-telemetry libraries and spell every argument those take.
#pragma once

#include <map>
#include <optional>
#include <source_location>
#include <string>
#include <string_view>
#include <utility>

#include \"any_error_kind.hpp\"
#include \"error.hpp\"
#include \"log.hpp\"
#include \"record_error_telemetry.hpp\"
#include \"result.hpp\"
#include \"telemetry.hpp\"
";

/// The whole header for one module.
fn header(module_name: &str, module: &Module) -> Result<String, fmt::Error> {
    let namespace = format!("{module_name}_telemetry");
    let mut text = format!("// The telemetry of module `{module_name}`, for C++.\n//\n{OPENING}");
    write!(
        text,
        "\nnamespace {namespace} {{\n\n    \
         /// The module's name: its records' names start with it, and it is its errors' category.\n    \
         inline constexpr std::string_view module_name = \"{module_name}\";\n\n    \
         /// The module's schema version, on every record it emits (`bc.telemetry.schema_version`).\n    \
         inline constexpr bc::i64 schema_version = {};\n\n}} // namespace {namespace}\n",
        module.schema_version.value
    )?;
    let items = &module.items;
    if !items.spans.is_empty() {
        write!(text, "\nnamespace {namespace}::spans {{\n")?;
        for (name, span) in &items.spans {
            text.push_str(&span_section(module_name, name, span)?);
        }
        writeln!(text, "}} // namespace {namespace}::spans")?;
    }
    if !items.metrics.is_empty() {
        write!(text, "\nnamespace {namespace}::metrics {{\n")?;
        for (name, metric) in &items.metrics {
            text.push_str(&metric_section(module_name, name, metric)?);
        }
        writeln!(text, "}} // namespace {namespace}::metrics")?;
    }
    if !items.logs.is_empty() {
        write!(text, "\nnamespace {namespace}::logs {{\n")?;
        for (name, log) in &items.logs {
            text.push_str(&log_section(module_name, name, log)?);
        }
        writeln!(text, "}} // namespace {namespace}::logs")?;
    }
    if !items.errors.is_empty() {
        text.push_str(&error_sections(module_name, &namespace, module)?);
    }
    Ok(text)
}

fn span_section(
    module_name: &str,
    name: &str,
    span: &crate::model::SpanDeclaration,
) -> Result<String, fmt::Error> {
    let type_name = type_name(name);
    let events = |indent: &str, target: &str| -> Result<String, fmt::Error> {
        let mut methods = String::new();
        for event in &span.events {
            write!(
                methods,
                "\n{indent}/// The span's `{event}` event, recorded on this span.\n\
                 {indent}void {event}(std::source_location location = std::source_location::current()) const {{\n\
                 {indent}    {target}.add_event(\"{event}\", bc::telemetry::Attributes{{}}, location);\n\
                 {indent}}}\n"
            )?;
        }
        Ok(methods)
    };
    let mut text = String::new();
    text.push('\n');
    text.push_str(&doc_comment("    ", &span.description)?);
    write!(
        text,
        "    class {type_name} {{\n    public:\n\
         \x20       /// The span current in a scope until it ends: what `start` yields.\n\
         \x20       class Scope {{\n        public:\n\
         \x20           explicit Scope(bc::telemetry::SpanGuard guard) : guard_{{std::move(guard)}} {{}}\n\
         {}\n\
         \x20           /// The runtime's guard: to set the status, end the span early, or hand it over.\n\
         \x20           [[nodiscard]] auto guard() & -> bc::telemetry::SpanGuard& {{ return guard_; }}\n\n\
         \x20       private:\n            bc::telemetry::SpanGuard guard_;\n        }};\n\n\
         \x20       /// The span held, current nowhere until made so: what `open` yields.\n\
         \x20       class Held {{\n        public:\n\
         \x20           explicit Held(bc::telemetry::OpenSpan span) : span_{{std::move(span)}} {{}}\n\
         {}\n\
         \x20           /// The runtime's handle: to make the span current, end it, or parent another.\n\
         \x20           [[nodiscard]] auto span() & -> bc::telemetry::OpenSpan& {{ return span_; }}\n\n\
         \x20       private:\n            bc::telemetry::OpenSpan span_;\n        }};\n\n\
         \x20       /// The span a `run` body runs in.\n\
         \x20       class Running {{\n        public:\n\
         \x20           explicit Running(const bc::telemetry::OpenSpan& span) : span_{{span}} {{}}\n\
         {}\n\
         \x20           /// The runtime's handle.\n\
         \x20           [[nodiscard]] auto span() const -> const bc::telemetry::OpenSpan& {{ return span_; }}\n\n\
         \x20       private:\n            const bc::telemetry::OpenSpan& span_;\n        }};\n\n",
        events("            ", "guard_")?,
        events("            ", "span_")?,
        events("            ", "span_")?,
    )?;
    write!(
        text,
        "        {}\n            : attributes_{{\n{}              }} {{}}\n\n",
        constructor_head(&type_name, &span.attributes),
        attribute_list("              ", &span.attributes, None)?,
    )?;
    write!(
        text,
        "        /// Starts the span under `parent`; it is current in this scope until the guard ends.\n\
         \x20       [[nodiscard]] auto start(\n\
         \x20           bc::telemetry::Parent parent,\n\
         \x20           std::source_location location = std::source_location::current()\n\
         \x20       ) && -> Scope {{\n\
         \x20           return Scope{{bc::telemetry::start_span(\n\
         \x20               std::move(parent), std::string{{name}}, kind, std::move(attributes_), location)}};\n\
         \x20       }}\n\n\
         \x20       /// Opens the span under `parent`, current nowhere until made so.\n\
         \x20       [[nodiscard]] auto open(bc::telemetry::Parent parent) && -> Held {{\n\
         \x20           return Held{{bc::telemetry::open_span(\n\
         \x20               std::move(parent), std::string{{name}}, kind, std::move(attributes_))}};\n\
         \x20       }}\n\n\
         \x20       /// Starts the span under `parent`, runs `body` in it, and ends it with the status\n\
         \x20       /// `end` gives `body`'s result. `body` takes the `Running` span.\n\
         \x20       template <typename End, typename Body>\n\
         \x20       auto run(bc::telemetry::Parent parent, End&& end, Body&& body) && {{\n\
         \x20           return bc::telemetry::running_span(\n\
         \x20               std::string{{name}},\n\
         \x20               bc::telemetry::SpanOptions{{\n\
         \x20                   .parent = std::move(parent), .kind = kind, .attributes = std::move(attributes_)}},\n\
         \x20               [&body](const bc::telemetry::OpenSpan& span) {{\n\
         \x20                   return std::forward<Body>(body)(Running{{span}});\n\
         \x20               }},\n\
         \x20               std::forward<End>(end));\n\
         \x20       }}\n\n\
         \x20   private:\n\
         \x20       static constexpr std::string_view name = \"{module_name}.{name}\";\n\
         \x20       static constexpr bc::telemetry::SpanKind kind = bc::telemetry::SpanKind::{};\n\
         \x20       bc::telemetry::Attributes attributes_;\n\
         \x20   }};\n\n",
        span_kind(span.kind)
    )?;
    text.push_str(&doc_comment("    ", &span.description)?);
    text.push_str(&factory(name, &type_name, &span.attributes));
    Ok(text)
}

/// `TypeName(std::string a, bc::i64 b)` — or `TypeName()` for no attributes.
fn constructor_head(type_name: &str, attributes: &BTreeMap<String, ScalarType>) -> String {
    let parameters = parameters(attributes);
    if parameters.is_empty() {
        format!("{type_name}()")
    } else {
        format!("explicit {type_name}({})", parameters.join(", "))
    }
}

/// The function a call site starts from: `name(attributes…) -> TypeName`.
fn factory(name: &str, type_name: &str, attributes: &BTreeMap<String, ScalarType>) -> String {
    format!(
        "    [[nodiscard]] inline auto {name}({}) -> {type_name} {{\n        return {type_name}{{{}}};\n    }}\n",
        parameters(attributes).join(", "),
        arguments(attributes).join(", ")
    )
}

fn metric_section(
    module_name: &str,
    name: &str,
    metric: &crate::model::MetricDeclaration,
) -> Result<String, fmt::Error> {
    let type_name = type_name(name);
    let (value_type, value_name, runtime_call, what) = match metric.kind {
        MetricKind::Counter => ("bc::i64", "increment", "record_counter", "one increment"),
        MetricKind::Gauge => ("bc::f64", "value", "record_gauge", "one reading"),
        MetricKind::Histogram => ("bc::f64", "value", "record_histogram", "one observation"),
    };
    let mut text = String::from("\n");
    text.push_str(&doc_comment("    ", &metric.description)?);
    write!(
        text,
        "    class {type_name} {{\n    public:\n\
         \x20       {}\n            : attributes_{{\n{}              }} {{}}\n\n\
         \x20       /// Records {what}, in `{}`, under `parent`.\n\
         \x20       void record(bc::telemetry::Parent parent, {value_type} {value_name}) && {{\n\
         \x20           bc::telemetry::{runtime_call}(\n\
         \x20               std::move(parent), std::string{{name}}, {value_name}, std::move(attributes_));\n\
         \x20       }}\n\n\
         \x20   private:\n\
         \x20       static constexpr std::string_view name = \"{module_name}.{name}\";\n\
         \x20       bc::telemetry::Attributes attributes_;\n\
         \x20   }};\n\n",
        constructor_head(&type_name, &metric.attributes),
        attribute_list("              ", &metric.attributes, None)?,
        crate::model::Keyword::keyword(metric.unit),
    )?;
    text.push_str(&doc_comment("    ", &metric.description)?);
    text.push_str(&factory(name, &type_name, &metric.attributes));
    Ok(text)
}

fn log_section(
    module_name: &str,
    name: &str,
    log: &crate::model::LogDeclaration,
) -> Result<String, fmt::Error> {
    let type_name = type_name(name);
    let qualified = format!("{module_name}.{name}");
    let mut text = String::from("\n");
    text.push_str(&doc_comment("    ", &log.description)?);
    write!(
        text,
        "    class {type_name} {{\n    public:\n\
         \x20       {}\n            : attributes_{{\n{}              }} {{}}\n\n\
         \x20       /// Emits the record under `parent`, at `{}`, its body the description.\n\
         \x20       void emit(bc::telemetry::Parent parent) && {{\n\
         \x20           bc::telemetry::record_log(\n\
         \x20               std::move(parent), bc::LogLevel::{}, std::string{{description}}, std::move(attributes_));\n\
         \x20       }}\n\n\
         \x20   private:\n\
         \x20       static constexpr std::string_view description = {};\n\
         \x20       bc::telemetry::Attributes attributes_;\n\
         \x20   }};\n\n",
        constructor_head(&type_name, &log.fields),
        attribute_list("              ", &log.fields, Some(&qualified))?,
        log_level(log.severity),
        log_level(log.severity),
        string_literal(&log.description)?,
    )?;
    text.push_str(&doc_comment("    ", &log.description)?);
    text.push_str(&factory(name, &type_name, &log.fields));
    Ok(text)
}

/// The metadata value a field becomes, and how a report reads it back.
fn metadata_reader(scalar: ScalarType) -> (&'static str, &'static str) {
    match scalar {
        ScalarType::String | ScalarType::Digest => ("as_string", "std::string{*value}"),
        ScalarType::Int | ScalarType::Duration => ("as_int64", "*value"),
        ScalarType::Float => ("as_float64", "*value"),
        ScalarType::Bool => ("as_bool", "*value"),
    }
}

fn error_sections(
    module_name: &str,
    namespace: &str,
    module: &Module,
) -> Result<String, fmt::Error> {
    let errors = &module.items.errors;
    let mut text = format!("\nnamespace {namespace}::error_kinds {{\n");
    for (name, error) in errors {
        text.push('\n');
        text.push_str(&doc_comment("    ", &error.description)?);
        let descriptor = match error.class {
            ErrorClass::Failure => "bc::AnyErrorKind",
            ErrorClass::Refusal => "bc::RefusalKind",
        };
        writeln!(
            text,
            "    inline constexpr {descriptor} {name}{{.category = module_name, .value = {}}};",
            error.code.value
        )?;
    }
    writeln!(text, "\n}} // namespace {namespace}::error_kinds")?;

    write!(text, "\nnamespace {namespace}::errors {{\n{NOT_THIS_ERROR}")?;
    for (name, error) in errors {
        text.push_str(&error_class(name, error)?);
    }
    writeln!(text, "\n}} // namespace {namespace}::errors")?;

    text.push_str(&report_sections(module_name, namespace, module)?);
    write!(
        text,
        "\nnamespace {namespace} {{\n\n    \
         /// Records `error` on the current span, for every error the code meets — a refusal among them.\n    \
         inline void record_error(const bc::AnyError& error) {{\n        \
         bc::error_telemetry::record_error_telemetry(error);\n    }}\n\n}} // namespace {namespace}\n"
    )?;
    Ok(text)
}

/// What a module's conversions hand back: one per header, so an error may not
/// be named for it (`is_reserved`).
const NOT_THIS_ERROR: &str = "
    /// An error a conversion was handed that is not the declared error it converts to: handed
    /// back unchanged, with why, so the caller still holds it to record or return.
    struct NotThisError {
        /// The error, unchanged.
        bc::AnyError error;
        /// The declared field it lacks — so built by hand rather than through its constructor —
        /// or none, when it is of another kind.
        std::optional<std::string_view> missing_field;
    };
";

/// An error's own class and its constructor: the class holds the
/// `bc::AnyError`, its constructor from one is private to the generated
/// constructor, and `from_error` converts any other error or hands it back.
fn error_class(name: &str, error: &crate::model::ErrorDeclaration) -> Result<String, fmt::Error> {
    let class = type_name(name);
    let mut declared = parameters(&error.fields);
    declared.push(format!("std::source_location {ERROR_LOCATION_PARAMETER}"));
    let mut defaulted = parameters(&error.fields);
    defaulted.push(format!(
        "std::source_location {ERROR_LOCATION_PARAMETER} = std::source_location::current()"
    ));
    let mut metadata = String::new();
    let mut checks = String::new();
    for (field, scalar) in &error.fields {
        writeln!(
            metadata,
            "                {{\"{field}\", bc::MetadataValue::from({})}},",
            passed(*scalar, field)
        )?;
        let (reader, _) = metadata_reader(*scalar);
        write!(
            checks,
            "            if (const auto found = metadata.find(\"{field}\");\n\
             \x20               found == metadata.end() || !found->second.{reader}()) {{\n\
             \x20               return Converted::err(\n\
             \x20                   NotThisError{{.error = std::move(error), .missing_field = \"{field}\"}});\n\
             \x20           }}\n"
        )?;
    }
    // A declaration with no fields reads no metadata, and an unread binding is a warning.
    let reads = if checks.is_empty() {
        String::new()
    } else {
        format!("            const auto& metadata = error.metadata();\n{checks}")
    };
    let taken_by = match error.class {
        ErrorClass::Failure => {
            format!("the one error `report::{name}` and `report_degraded::{name}` take")
        }
        ErrorClass::Refusal => {
            "a refusal, the caller's to correct, so recorded and never reported".to_owned()
        }
    };
    let mut text = String::from("\n");
    text.push_str(&doc_comment("    ", &error.description)?);
    write!(
        text,
        "    ///\n    /// Built by `{name}`, or converted from a `bc::AnyError` by `from_error`: {taken_by}.\n\
         \x20   class {class} {{\n    public:\n\
         \x20       /// `error` as this error, when it is one — of this kind, with every declared field;\n\
         \x20       /// otherwise it is handed back unchanged, with why.\n\
         \x20       [[nodiscard]] static auto from_error(bc::AnyError error) -> bc::Result<{class}, NotThisError> {{\n\
         \x20           using Converted = bc::Result<{class}, NotThisError>;\n\
         \x20           if (!error.matches(error_kinds::{name})) {{\n\
         \x20               return Converted::err(NotThisError{{.error = std::move(error), .missing_field = std::nullopt}});\n\
         \x20           }}\n\
         {reads}\
         \x20           return Converted::ok({class}{{std::move(error)}});\n\
         \x20       }}\n\n\
         \x20       /// The error itself: its identity and metadata, or to record it.\n\
         \x20       [[nodiscard]] auto error() const noexcept -> const bc::AnyError& {{ return error_; }}\n\n\
         \x20       /// The error itself, to return or propagate.\n\
         \x20       [[nodiscard]] auto into_error() && -> bc::AnyError {{ return std::move(error_); }}\n\n\
         \x20   private:\n\
         \x20       explicit {class}(bc::AnyError error) : error_{{std::move(error)}} {{}}\n\n\
         \x20       friend auto {name}({}) -> {class};\n\n\
         \x20       bc::AnyError error_;\n\
         \x20   }};\n\n",
        declared.join(", ")
    )?;
    text.push_str(&doc_comment("    ", &error.description)?);
    write!(
        text,
        "    [[nodiscard]] inline auto {name}({}) -> {class} {{\n\
         \x20       return {class}{{bc::AnyError::from_metadata(\n\
         \x20           error_kinds::{name},\n\
         \x20           std::map<std::string, bc::MetadataValue>{{\n{metadata}            }},\n\
         \x20           {ERROR_LOCATION_PARAMETER})}};\n    }}\n",
        defaulted.join(", ")
    )?;
    Ok(text)
}

/// Each failure's report calls: the context a report logs, then `report` and
/// `report_degraded`. A refusal has none: one the caller can correct is not an
/// error of ours, and logging it would bury the ones that are.
fn report_sections(
    module_name: &str,
    namespace: &str,
    module: &Module,
) -> Result<String, fmt::Error> {
    let mut text = String::new();
    let failures: Vec<(&String, &crate::model::ErrorDeclaration)> = module
        .items
        .errors
        .iter()
        .filter(|(_, error)| error.class == ErrorClass::Failure)
        .collect();
    if !failures.is_empty() {
        write!(text, "\nnamespace {namespace}::report_detail {{\n")?;
        for (name, error) in &failures {
            write!(
                text,
                "\n    /// What a report of `{name}` logs beside the error's identity: its name, the\n\
                 \x20   /// module's schema version, and the declared fields, read from the metadata\n\
                 \x20   /// `errors::{name}` put them in.\n\
                 \x20   inline auto {name}_context(const errors::{class}& error) -> bc::telemetry::Attributes {{\n\
                 \x20       auto context = bc::telemetry::Attributes{{\n\
                 \x20           {{\"bc.telemetry.log\", std::string{{\"{module_name}.{name}\"}}}},\n\
                 \x20           {{\"bc.telemetry.schema_version\", schema_version}},\n\
                 \x20       }};\n\
                 \x20       const auto& metadata = error.error().metadata();\n",
                class = type_name(name)
            )?;
            for (field, scalar) in &error.fields {
                let (reader, value) = metadata_reader(*scalar);
                write!(
                    text,
                    "        if (const auto found = metadata.find(\"{field}\"); found != metadata.end()) {{\n\
                     \x20           if (const auto value = found->second.{reader}()) {{\n\
                     \x20               context.emplace_back(\"{field}\", {value});\n\
                     \x20           }}\n        }}\n"
                )?;
            }
            write!(
                text,
                "        return context;\n    }}\n\n    inline constexpr std::string_view {name}_description = {};\n",
                string_literal(&error.description)?
            )?;
        }
        writeln!(text, "\n}} // namespace {namespace}::report_detail")?;

        for (verb, runtime_call, what) in [
            (
                "report",
                "report_error",
                "an error the code could not answer past: an error-level log record",
            ),
            (
                "report_degraded",
                "report_degraded",
                "an error the code answered past with a lesser result: a warn-level log record",
            ),
        ] {
            write!(text, "\nnamespace {namespace}::{verb} {{\n")?;
            for (name, _) in &failures {
                write!(
                    text,
                    "\n    /// Reports `error`, built by `errors::{name}`, as {what} whose body is the\n\
                     \x20   /// declaration's description, after recording it on the current span.\n\
                     \x20   inline void {name}(const errors::{class}& error) {{\n\
                     \x20       bc::error_telemetry::{runtime_call}(\n\
                     \x20           report_detail::{name}_description, error.error(), report_detail::{name}_context(error));\n\
                     \x20   }}\n",
                    class = type_name(name)
                )?;
            }
            writeln!(text, "\n}} // namespace {namespace}::{verb}")?;
        }
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::{is_reserved, string_literal, type_name};
    use crate::adapters::NameRole;
    use crate::model::ItemKind;

    #[test]
    fn a_snake_name_becomes_a_type_one_way() {
        assert_eq!(type_name("request_duration"), "RequestDuration");
        assert_eq!(type_name("checkout"), "Checkout");
        assert_eq!(type_name("log_2x"), "Log_2x");
        assert_eq!(type_name("log2x"), "Log2x");
    }

    #[test]
    fn a_string_literal_escapes_what_would_end_or_break_it() {
        assert_eq!(
            string_literal("a \"b\"\\c\nd\te\u{1}f").expect("a literal"),
            "\"a \\\"b\\\"\\\\c\\nd\\te\\001f\""
        );
        assert_eq!(string_literal("ünïcödé").expect("a literal"), "\"ünïcödé\"");
        // DEL is the last control an octal escape spells; a C1 control is a
        // UTF-8 character like any other.
        assert_eq!(
            string_literal("\u{7f}\u{80}").expect("a literal"),
            "\"\\177\u{80}\""
        );
    }

    #[test]
    fn a_name_is_reserved_by_the_role_it_plays() {
        assert!(is_reserved(NameRole::Item(ItemKind::Span), "class"));
        assert!(is_reserved(
            NameRole::Attribute(ItemKind::Metric),
            "default"
        ));
        assert!(!is_reserved(NameRole::Item(ItemKind::Span), "checkout"));
        assert!(is_reserved(NameRole::Event, "guard"));
        assert!(!is_reserved(NameRole::Attribute(ItemKind::Span), "guard"));
        assert!(is_reserved(
            NameRole::Attribute(ItemKind::Error),
            "location"
        ));
        assert!(!is_reserved(NameRole::Attribute(ItemKind::Log), "location"));
        // A span named for a handle class nested in it, or an error named for the
        // header's one shared class, would not compile.
        for handle in ["scope", "held", "running"] {
            assert!(is_reserved(NameRole::Item(ItemKind::Span), handle));
            assert!(!is_reserved(NameRole::Item(ItemKind::Metric), handle));
        }
        assert!(is_reserved(
            NameRole::Item(ItemKind::Error),
            "not_this_error"
        ));
        assert!(!is_reserved(
            NameRole::Item(ItemKind::Span),
            "not_this_error"
        ));
    }
}
