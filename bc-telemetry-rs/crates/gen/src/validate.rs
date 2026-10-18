//! Reference integrity across the tree: every query measures a declared item,
//! groups by that item's own attributes and asks for an aggregation its measure
//! admits; every panel shows a declared query — or, rendering `logs`, the
//! records of a declared log or error.
//!
//! A reference may cross modules, so this runs after every module is lowered,
//! against every name *declared*: a reference to an item whose own declaration
//! had findings is not reported again as dangling.

use std::collections::BTreeMap;

use crate::diagnostics::{Code, Diagnostics, candidates_for, did_you_mean};
use crate::lower::{DeclarationKind, DeclaredNames};
use crate::model::{
    ErrorClass, ItemKind, ItemRef, Keyword, Measure, Panel, PanelSource, Query, QueryRef, Render,
    Tree,
};
use crate::root::RetiredList;

/// What the references are checked against: the lowered modules, and every
/// name each of them declared.
pub(crate) struct Declared<'run> {
    pub(crate) tree: &'run Tree,
    pub(crate) names: &'run BTreeMap<String, DeclaredNames>,
}

pub(crate) fn check_references(declared: &Declared<'_>, diagnostics: &mut Diagnostics) {
    for module in declared.tree.modules.values() {
        for (name, query) in &module.items.queries {
            check_query(name, query, declared, diagnostics);
        }
        for (name, dashboard) in &module.items.dashboards {
            for (index, panel) in dashboard.panels.iter().enumerate() {
                let context = format!("panel {} of dashboard `{name}`", index + 1);
                check_panel(&context, panel, declared, diagnostics);
            }
        }
    }
}

/// A name in the root's `[retired]` list is a module this tree no longer has;
/// one that is still declared contradicts the list. This holds whether
/// or not anything is generated.
pub(crate) fn check_retired_modules_are_gone(
    tree: &Tree,
    retired: &RetiredList,
    diagnostics: &mut Diagnostics,
) {
    for listed in &retired.modules {
        if tree.modules.contains_key(&listed.value) {
            diagnostics.at(
                Code::RetiredModuleDeclared,
                listed.span,
                format!(
                    "`[retired]` names `{}`, which this tree still declares; a module is retired \
                     once its definition is gone, and its name is never declared again",
                    listed.value
                ),
            );
        }
    }
}

fn check_query(name: &str, query: &Query, declared: &Declared<'_>, diagnostics: &mut Diagnostics) {
    let measure = &query.measure.value;
    let what = format!("query `{name}` measures `{}`", measure.spelled());
    let Some(measured) = measured_item(&what, measure, query.measure.span, declared, diagnostics)
    else {
        return;
    };
    let measured = match measured {
        Measured::Shape(shape) => shape,
        Measured::Refusal => {
            diagnostics.at(
                Code::InvalidValue,
                query.measure.span,
                format!(
                    "{what}, a refusal; nothing reports a refusal, so there is no record to \
                     measure"
                ),
            );
            return;
        }
    };

    let aggregation = query.aggregation.value;
    if !aggregation.admits(measured.measure) {
        let admitted: Vec<&str> = measured
            .measure
            .admitted()
            .into_iter()
            .map(Keyword::keyword)
            .collect();
        diagnostics.at(
            Code::InvalidAggregation,
            query.aggregation.span,
            format!(
                "query `{name}` takes `{}` of `{}`, {}, which admits {}",
                aggregation.keyword(),
                measure.spelled(),
                measured.measure.describe(),
                admitted.join(" | ")
            ),
        );
    }
    for group in &query.group_by {
        if !measured.attributes.contains(&group.value.as_str()) {
            diagnostics.at(
                Code::DanglingReference,
                group.span,
                format!(
                    "query `{name}` groups by `{}`, which is not {} of `{}`{}",
                    group.value,
                    measured.attribute_noun,
                    measure.spelled(),
                    did_you_mean(&candidates_for(
                        &group.value,
                        measured.attributes.iter().copied()
                    ))
                ),
            );
        }
    }
}

fn check_panel(
    context: &str,
    panel: &Panel,
    declared: &Declared<'_>,
    diagnostics: &mut Diagnostics,
) {
    let what = format!("{context} shows `{}`", panel.source.value.spelled());
    match &panel.source.value {
        PanelSource::Query(query) => {
            check_query_is_declared(&what, query, panel.source.span, declared, diagnostics);
        }
        PanelSource::Records(item) => {
            let measured = measured_item(&what, item, panel.source.span, declared, diagnostics);
            if matches!(measured, Some(Measured::Refusal)) {
                diagnostics.at(
                    Code::InvalidValue,
                    panel.source.span,
                    format!(
                        "{what}, a refusal; nothing reports a refusal, so there is no record to \
                         show"
                    ),
                );
            }
        }
    }

    let shows_records = matches!(panel.source.value, PanelSource::Records(_));
    let renders_logs = panel.render.value == Render::Logs;
    if renders_logs && !shows_records {
        diagnostics.at(
            Code::InvalidValue,
            panel.source.span,
            format!(
                "{context} renders `logs`, which shows a log's or an error's records; reference \
                 one as `log.<name>` or `error.<name>`"
            ),
        );
    } else if shows_records && !renders_logs {
        diagnostics.at(
            Code::InvalidValue,
            panel.source.span,
            format!(
                "{context} renders `{}`, which shows a query's result; reference one as \
                 `query.<name>`, or render the records with `logs`",
                panel.render.value.keyword()
            ),
        );
    }
}

/// What a query can ask of the item it measures.
struct Shape<'run> {
    measure: Measure,
    /// The item's attributes — or, for a log or an error, its fields.
    attributes: Vec<&'run str>,
    /// "an attribute" or "a field", as a message says it.
    attribute_noun: &'static str,
}

enum Measured<'run> {
    Shape(Shape<'run>),
    /// An error nothing ever reports, since a refusal is the caller's to
    /// correct: it has no record to read.
    Refusal,
}

/// The declared item `item` names, as far as a reference to it can be checked.
/// `None` when the reference dangles — which is reported here, opening with
/// `what` — or when the item's own declaration had findings, and nobody could
/// read what it declares.
fn measured_item<'run>(
    what: &str,
    item: &ItemRef,
    span: crate::source::Span,
    declared: &Declared<'run>,
    diagnostics: &mut Diagnostics,
) -> Option<Measured<'run>> {
    let Some(names) = declared.names.get(&item.module) else {
        refuse_undeclared_module(what, &item.module, span, declared, diagnostics);
        return None;
    };
    let kind = DeclarationKind::from(item.kind);
    if !names.contains(kind, &item.name) {
        // The commonest slip is the right name under the wrong kind.
        let declared_as: Vec<&str> = ItemKind::ALL
            .iter()
            .filter(|other| names.contains(DeclarationKind::from(**other), &item.name))
            .map(|other| other.keyword())
            .collect();
        let hint = match declared_as.as_slice() {
            [] => spelled_candidates(kind, &item.module, &item.name, names),
            kinds => format!(
                "; `{}` is declared in `{}` as a {}",
                item.name,
                item.module,
                kinds.join(" and as a ")
            ),
        };
        diagnostics.at(
            Code::DanglingReference,
            span,
            format!("{what}, which is not declared{hint}"),
        );
        return None;
    }

    let items = &declared.tree.modules.get(&item.module)?.items;
    let names_of = |map: &'run BTreeMap<String, _>| map.keys().map(String::as_str).collect();
    let shape = match item.kind {
        ItemKind::Span => Shape {
            measure: Measure::Span,
            attributes: names_of(&items.spans.get(&item.name)?.attributes),
            attribute_noun: "an attribute",
        },
        ItemKind::Metric => {
            let metric = items.metrics.get(&item.name)?;
            Shape {
                measure: Measure::of_metric(metric.kind),
                attributes: names_of(&metric.attributes),
                attribute_noun: "an attribute",
            }
        }
        ItemKind::Log => Shape {
            measure: Measure::Records,
            attributes: names_of(&items.logs.get(&item.name)?.fields),
            attribute_noun: "a field",
        },
        ItemKind::Error => {
            let error = items.errors.get(&item.name)?;
            if error.class == ErrorClass::Refusal {
                return Some(Measured::Refusal);
            }
            Shape {
                measure: Measure::Records,
                attributes: names_of(&error.fields),
                attribute_noun: "a field",
            }
        }
    };
    Some(Measured::Shape(shape))
}

fn check_query_is_declared(
    what: &str,
    query: &QueryRef,
    span: crate::source::Span,
    declared: &Declared<'_>,
    diagnostics: &mut Diagnostics,
) {
    let Some(names) = declared.names.get(&query.module) else {
        refuse_undeclared_module(what, &query.module, span, declared, diagnostics);
        return;
    };
    if !names.contains(DeclarationKind::Query, &query.name) {
        diagnostics.at(
            Code::DanglingReference,
            span,
            format!(
                "{what}, which is not declared{}",
                spelled_candidates(DeclarationKind::Query, &query.module, &query.name, names)
            ),
        );
    }
}

fn refuse_undeclared_module(
    what: &str,
    module: &str,
    span: crate::source::Span,
    declared: &Declared<'_>,
    diagnostics: &mut Diagnostics,
) {
    let modules = declared.names.keys().map(String::as_str);
    diagnostics.at(
        Code::DanglingReference,
        span,
        format!(
            "{what}, but no module `{module}` is declared{}",
            did_you_mean(&candidates_for(module, modules))
        ),
    );
}

/// "; did you mean `<kind>.<module>.<name>`?" over the names `module` declares
/// under `kind`.
fn spelled_candidates(
    kind: DeclarationKind,
    module: &str,
    name: &str,
    names: &DeclaredNames,
) -> String {
    let spelled: Vec<String> = candidates_for(name, names.of(kind))
        .into_iter()
        .map(|candidate| format!("{}.{module}.{candidate}", kind.key()))
        .collect();
    did_you_mean(&spelled)
}
