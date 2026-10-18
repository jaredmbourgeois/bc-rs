//! The Grafana export: each module's declarative dashboards, lowered into
//! bc-telemetry-dashboard's model and rendered as Grafana's classic dashboard
//! JSON; and the tree's query handles for a program that builds dashboards in
//! code — in TypeScript with Grafana's Foundation SDK, or in Rust with
//! bc-telemetry-dashboard's builder.
//!
//! Both read the queries the PostgreSQL dialect writes, so a panel shows
//! the same statement the module's query file holds, in the unit its query
//! answers in.

use std::fmt::{self, Write as _};

use bc_json::JsonWriter;
use bc_telemetry_dashboard::{
    Dashboard as GrafanaDashboard, DashboardId, DataSource, Logs, MeasureQuery,
    Panel as GrafanaPanel, RecordsQuery, ResultUnit, Stat, Table, Timeseries, Unit,
};

use crate::adapters::rust;

use crate::layout::RenderError;
use crate::layout::queries::{self, Handle};
use crate::model::{Dashboard, ErrorClass, ItemKind, ItemRef, PanelSource, Render, Tree};
use crate::names::dashboard_uid;
use crate::output::GeneratedTree;
use crate::paths;
use crate::root::GrafanaExport;

/// Where the export's files go, under the root's `output`.
const GRAFANA_DIRECTORY: &str = "grafana";

/// Adds the Grafana export's files for `tree` to what this run generates:
/// `dashboards/<uid>.json` for every declarative dashboard, and the query
/// handles for a program that builds dashboards in code: `queries.ts` and
/// `queries.rs`.
///
/// # Errors
///
/// A defect in the generator, as [`RenderError`] says: what this renders was
/// validated first.
pub(crate) fn render(
    tree: &Tree,
    output: &str,
    export: &GrafanaExport,
    generated: &mut GeneratedTree,
) -> Result<(), RenderError> {
    let directory = paths::under(output, GRAFANA_DIRECTORY);
    let data_source = DataSource::postgresql(export.data_source_uid.as_str());
    for (module_name, module) in &tree.modules {
        for (dashboard_name, dashboard) in &module.items.dashboards {
            let uid = dashboard_uid(module_name, dashboard_name);
            let lowered = lower(tree, &uid, dashboard, &data_source)?;
            generated.insert(
                paths::under(&directory, &format!("dashboards/{uid}.json")),
                lowered.to_json().map_err(RenderError::Json)?,
            );
        }
    }
    generated.insert(
        paths::under(&directory, "queries.ts"),
        typescript_handles(tree, &export.data_source_uid)?,
    );
    generated.insert(
        paths::under(&directory, "queries.rs"),
        rust_handles(tree, &export.data_source_uid)?,
    );
    Ok(())
}

/// A declarative dashboard in the model: its panels in declared order, each
/// reading the handle of what it names.
fn lower(
    tree: &Tree,
    uid: &str,
    dashboard: &Dashboard,
    data_source: &DataSource,
) -> Result<GrafanaDashboard, RenderError> {
    let id = DashboardId::new(uid).map_err(|error| RenderError::Unvalidated(error.to_string()))?;
    let mut lowered = GrafanaDashboard::new(id, dashboard.title.as_str());
    for panel in &dashboard.panels {
        let unvalidated = || {
            RenderError::Unvalidated(format!(
                "panel `{}` of dashboard `{uid}` over `{}`",
                panel.title,
                panel.source.value.spelled()
            ))
        };
        let lowered_panel: GrafanaPanel = match (&panel.source.value, panel.render.value) {
            (PanelSource::Query(query), render) => {
                let declared = tree
                    .modules
                    .get(&query.module)
                    .and_then(|module| module.items.queries.get(&query.name))
                    .ok_or_else(unvalidated)?;
                let handle = queries::query_handle(tree, &query.module, &query.name, declared)?;
                let measure = measure_query(handle, data_source).ok_or_else(unvalidated)?;
                let title = panel.title.as_str();
                match render {
                    Render::Timeseries => Timeseries::new(title, measure).into(),
                    Render::Stat => Stat::new(title, measure).into(),
                    Render::Table => Table::new(title, measure).into(),
                    Render::Logs => return Err(unvalidated()),
                }
            }
            (PanelSource::Records(item), Render::Logs) => {
                let handle = records_handle(tree, item).ok_or_else(unvalidated)?;
                Logs::new(
                    panel.title.as_str(),
                    RecordsQuery::new(handle.description, handle.sql, data_source.clone()),
                )
                .into()
            }
            (PanelSource::Records(_), Render::Timeseries | Render::Stat | Render::Table) => {
                return Err(unvalidated());
            }
        };
        lowered = lowered.panel(lowered_panel);
    }
    Ok(lowered)
}

/// A query handle as the model's measure query: every query's handle has a unit.
fn measure_query(handle: Handle, data_source: &DataSource) -> Option<MeasureQuery> {
    let unit = handle.unit?;
    Some(MeasureQuery::new(
        handle.description,
        handle.sql,
        unit,
        data_source.clone(),
    ))
}

/// The handle that reads the records of the log or failure error `item`
/// names; `None` for anything else.
fn records_handle(tree: &Tree, item: &ItemRef) -> Option<Handle> {
    let items = &tree.modules.get(&item.module)?.items;
    match item.kind {
        ItemKind::Log => {
            let log = items.logs.get(&item.name)?;
            Some(queries::records_handle(
                item,
                &log.description,
                log.fields.keys(),
            ))
        }
        ItemKind::Error => {
            let error = items
                .errors
                .get(&item.name)
                .filter(|error| error.class == ErrorClass::Failure)?;
            Some(queries::records_handle(
                item,
                &error.description,
                error.fields.keys(),
            ))
        }
        ItemKind::Span | ItemKind::Metric => None,
    }
}

/// A declared name as a TypeScript property: `store_server` → `storeServer`,
/// `checkout-p95` → `checkoutP95`. A separator before a digit becomes `_`, so
/// `p-95` and `p95` stay two names.
pub(crate) fn camel_case(name: &str) -> String {
    let mut spelled = String::with_capacity(name.len());
    let mut after_separator = false;
    for character in name.chars() {
        if matches!(character, '_' | '-') {
            after_separator = true;
            continue;
        }
        if after_separator && character.is_ascii_digit() {
            spelled.push('_');
            spelled.push(character);
        } else if after_separator {
            spelled.push(character.to_ascii_uppercase());
        } else {
            spelled.push(character);
        }
        after_separator = false;
    }
    spelled
}

/// `text` as a TypeScript string literal. A JSON string is one, and the
/// family's writer spells it: a one-element array, whose brackets come off.
fn string_literal(text: &str) -> Result<String, RenderError> {
    let mut json = JsonWriter::new();
    json.bare_array(|json| json.array_element(text));
    let array = json.finish().map_err(RenderError::Json)?;
    match array
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        Some(literal) => Ok(literal.to_string()),
        None => Err(RenderError::Defect(format!(
            "the JSON writer spelled a one-string array as `{array}`"
        ))),
    }
}

/// How the handles file opens: what it is, and the two classes every handle is
/// one of.
const TYPESCRIPT_OPENING: &str = r#"// The query handles of this tree, for building Grafana dashboards with Grafana's Foundation SDK.
//
// Generated by bc-telemetry-gen. Do not edit.
// Each handle is a target the SDK's panel builders take, `.withTarget(handle)`: raw SQL read
// from Grafana's PostgreSQL data source, the statement the module's query file holds. A
// measure's panel takes `handle.unit`, the unit the query answers in.

import type * as cog from "@grafana/grafana-foundation-sdk/cog";

/** How Grafana's PostgreSQL data source shapes a query's answer. */
export type Format = "time_series" | "table";

/** The target Grafana's PostgreSQL data source reads: `rawSql`, as code. */
function target(rawSql: string, format: Format): cog.Dataquery {
  const query = {
    _implementsDataqueryVariant(): void {},
    refId: "A",
    datasource: dataSource,
    editorMode: "code",
    format,
    rawQuery: true,
    rawSql,
  };
  return query;
}

/** A query whose answer is a measurement over time: a timeseries, stat or table panel's. */
export class MeasureQuery implements cog.Builder<cog.Dataquery> {
  constructor(
    readonly description: string,
    readonly rawSql: string,
    /** Grafana's id for the unit the query answers in. */
    readonly unit: string,
    readonly format: Format,
  ) {}

  /** The same query, its answer shaped as a table rather than a time series. */
  asTable(): MeasureQuery {
    return new MeasureQuery(this.description, this.rawSql, this.unit, "table");
  }

  build(): cog.Dataquery {
    return target(this.rawSql, this.format);
  }
}

/** A query that reads a log's or an error's records, newest first: a logs panel's. */
export class RecordsQuery implements cog.Builder<cog.Dataquery> {
  constructor(
    readonly description: string,
    readonly rawSql: string,
  ) {}

  build(): cog.Dataquery {
    return target(this.rawSql, "table");
  }
}
"#;

/// The tree's query handles as a TypeScript module: the data source, then per
/// module its queries and the records of its logs and failure errors.
fn typescript_handles(tree: &Tree, data_source_uid: &str) -> Result<String, RenderError> {
    let mut file = String::from(TYPESCRIPT_OPENING);
    file.push_str(
        "\n/** The data source every query reads. */\nexport const dataSource = {\n  type: \
         \"grafana-postgresql-datasource\",\n  uid: ",
    );
    file.push_str(&string_literal(data_source_uid)?);
    file.push_str(",\n} as const;\n");
    file.push_str("\n/** Every module's queries, and the records of its logs and errors. */\n");
    file.push_str("export const queries = {\n");
    for (module_name, module) in &tree.modules {
        file.push_str("  ");
        file.push_str(&camel_case(module_name));
        file.push_str(": {\n    queries: {\n");
        for (query_name, query) in &module.items.queries {
            let handle = queries::query_handle(tree, module_name, query_name, query)?;
            let Some(unit) = handle.unit else {
                return Err(RenderError::Defect(format!(
                    "query `{module_name}.{query_name}` has no unit"
                )));
            };
            file.push_str(&measure_line(
                &camel_case(query_name),
                &handle,
                unit.grafana_id(),
            )?);
        }
        file.push_str("    },\n    records: {\n");
        let mut records: Vec<(String, Handle)> = records_of(module_name, module)
            .into_iter()
            .map(|(name, handle)| (camel_case(&name), handle))
            .collect();
        // Sorted, so logs and errors interleave by name.
        records.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (key, handle) in &records {
            file.push_str(&records_line(key, handle)?);
        }
        file.push_str("    },\n  },\n");
    }
    file.push_str("} as const;\n");
    Ok(file)
}

/// The handles that read a module's records: each log's, and each failure
/// error's (a refusal is never logged), by declared name. A log and an error
/// of one module never share a name, since both are stored as log records, so
/// the names are distinct.
fn records_of(module_name: &str, module: &crate::model::Module) -> Vec<(String, Handle)> {
    let item = |kind: ItemKind, name: &str| ItemRef {
        kind,
        module: module_name.to_owned(),
        name: name.to_owned(),
    };
    let mut records: Vec<(String, Handle)> = Vec::new();
    for (name, log) in &module.items.logs {
        records.push((
            name.clone(),
            queries::records_handle(
                &item(ItemKind::Log, name),
                &log.description,
                log.fields.keys(),
            ),
        ));
    }
    for (name, error) in &module.items.errors {
        if error.class == ErrorClass::Failure {
            records.push((
                name.clone(),
                queries::records_handle(
                    &item(ItemKind::Error, name),
                    &error.description,
                    error.fields.keys(),
                ),
            ));
        }
    }
    records
}

/// How the Rust handles file opens: what it is, and how a dashboard program
/// brings it in.
const RUST_OPENING: &str = "\
// The query handles of this tree, for building Grafana dashboards with bc-telemetry-dashboard.
//
// Generated by bc-telemetry-gen. Do not edit.
// Bring it in as a module in a crate that depends on bc-telemetry-dashboard:
// `mod queries { include!(\"…/grafana/queries.rs\"); }`. Each handle is what the builder's panels
// take, reading the data source below with the statement its module's query file holds: a measure
// for a timeseries, stat or table panel, a log's or a failure's records for a logs panel. A name
// that is a Rust keyword is written with a trailing `_`.
";

/// The tree's query handles as a Rust module: the data source, then per module
/// its queries and the records of its logs and failure errors — the TypeScript
/// handles' shape, in the model the declarative dashboards render through.
fn rust_handles(tree: &Tree, data_source_uid: &str) -> Result<String, RenderError> {
    let mut file = String::from(RUST_OPENING);
    write!(
        file,
        "\n/// The data source every query reads.\n#[must_use]\n\
         pub fn data_source() -> ::bc_telemetry_dashboard::DataSource {{\n    \
         ::bc_telemetry_dashboard::DataSource::postgresql({})\n}}\n",
        rust::string_literal(data_source_uid)
    )
    .map_err(unformatted)?;
    for (module_name, module) in &tree.modules {
        let mut measures = String::new();
        for (query_name, query) in &module.items.queries {
            let handle = queries::query_handle(tree, module_name, query_name, query)?;
            let Some(unit) = handle.unit else {
                return Err(RenderError::Defect(format!(
                    "query `{module_name}.{query_name}` has no unit"
                )));
            };
            measures.push_str(&rust_handle(
                query_name,
                &handle,
                "MeasureQuery",
                &format!("{},\n                ", rust_unit(unit)),
            )?);
        }
        let mut records = records_of(module_name, module);
        records.sort_by_key(|(name, _)| rust::identifier(name));
        let mut readers = String::new();
        for (name, handle) in &records {
            readers.push_str(&rust_handle(name, handle, "RecordsQuery", "")?);
        }
        write!(
            file,
            "\n/// The `{module_name}` module's queries, and the records of its logs and failure errors.\n\
             pub mod {} {{\n    /// Its queries.\n    pub mod queries {{{}}}\n\n    \
             /// The records of its logs and failure errors.\n    pub mod records {{{}}}\n}}\n",
            rust::identifier(module_name),
            module_body(&measures),
            module_body(&readers),
        )
        .map_err(unformatted)?;
    }
    Ok(file)
}

/// A failed write into a `String`, which a `Display` that keeps its contract
/// never makes: carried as the generator's defect rather than discarded.
fn unformatted(error: fmt::Error) -> RenderError {
    RenderError::Defect(format!("a generated file did not format: {error}"))
}

/// A nested module's body: its functions and the indent its closing brace
/// takes, or nothing, so an empty module reads `{}`.
fn module_body(functions: &str) -> String {
    if functions.is_empty() {
        String::new()
    } else {
        format!("{functions}    ")
    }
}

/// One handle's function, `name() -> Type`, built from its description, its
/// statement, `unit` (written before the data source; empty for records) and
/// the file's data source.
fn rust_handle(
    name: &str,
    handle: &Handle,
    query_type: &str,
    unit: &str,
) -> Result<String, RenderError> {
    let doc = rust::doc_comment("        ", &handle.description).map_err(unformatted)?;
    Ok(format!(
        "\n{doc}        #[must_use]\n        pub fn {}() -> ::bc_telemetry_dashboard::{query_type} {{\n            \
         ::bc_telemetry_dashboard::{query_type}::new(\n                {},\n                {},\n                \
         {unit}super::super::data_source(),\n            )\n        }}\n",
        rust::identifier(name),
        rust::string_literal(&handle.description),
        rust::string_literal(&handle.sql),
    ))
}

/// A result unit as the Rust expression that builds it.
fn rust_unit(unit: ResultUnit) -> String {
    let (wrapper, measured) = match unit {
        ResultUnit::Of(measured) => ("Of", measured),
        ResultUnit::PerSecond(measured) => ("PerSecond", measured),
    };
    let variant = match measured {
        Unit::Unitless => "Unitless",
        Unit::Milliseconds => "Milliseconds",
        Unit::Seconds => "Seconds",
        Unit::Bytes => "Bytes",
        Unit::Percent => "Percent",
    };
    format!(
        "::bc_telemetry_dashboard::ResultUnit::{wrapper}(::bc_telemetry_dashboard::Unit::{variant})"
    )
}

/// One measure handle's property:
/// `key: new MeasureQuery(description, sql, unit, "time_series"),`.
fn measure_line(key: &str, handle: &Handle, unit: &str) -> Result<String, RenderError> {
    Ok(format!(
        "      {key}: new MeasureQuery(\n        {},\n        {},\n        {},\n        \
         \"time_series\",\n      ),\n",
        string_literal(&handle.description)?,
        string_literal(&handle.sql)?,
        string_literal(unit)?,
    ))
}

/// One records handle's property: `key: new RecordsQuery(description, sql),`.
fn records_line(key: &str, handle: &Handle) -> Result<String, RenderError> {
    Ok(format!(
        "      {key}: new RecordsQuery(\n        {},\n        {},\n      ),\n",
        string_literal(&handle.description)?,
        string_literal(&handle.sql)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{camel_case, string_literal};

    #[test]
    fn a_declared_name_becomes_a_typescript_property_one_way() {
        assert_eq!(camel_case("store_server"), "storeServer");
        assert_eq!(camel_case("checkout-p95"), "checkoutP95");
        assert_eq!(camel_case("orders-per-second"), "ordersPerSecond");
        assert_eq!(camel_case("payments"), "payments");
        assert_eq!(camel_case("p-95"), "p_95");
        assert_eq!(camel_case("p95"), "p95");
        assert_eq!(camel_case("log_2_tries"), "log_2Tries");
    }

    #[test]
    fn a_string_literal_escapes_what_ends_or_breaks_one() {
        assert_eq!(
            string_literal("a \"quote\"\nand \\ more").expect("a literal"),
            r#""a \"quote\"\nand \\ more""#
        );
    }
}
