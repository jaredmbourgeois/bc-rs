//! How a tree may change between two runs.
//!
//! The committed manifest is what readers of the last run's records depend on,
//! so each rule compares the tree against it:
//!
//! - **the additive rule**, per module: a change readers cannot absorb needs
//!   that module's `schema_version` past the committed one, and the version
//!   never falls. A bump in one module admits nothing in another.
//! - **an error's code is its identity**, persisted wherever the error is: a
//!   rename keeps it, a removal retires it for good, and an error that keeps
//!   its name never changes it — those last two at every version, bump or not.
//! - **the set of modules**: a module the manifest holds and the tree lacks is
//!   refused until the root file's `[retired]` list names it, and that list is
//!   permanent.
//!
//! Every rule runs under `--check` too, since a CI that only checks must still
//! refuse a breaking change.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Code, Diagnostics, candidates_for, did_you_mean};
use crate::manifest::{ErrorShape, PreviousManifest, PreviousModule, Retired, Shape, Vocabulary};
use crate::model::{ItemKind, Keyword, Module, Tree};
use crate::root::RetiredList;

/// Holds `tree` to every rule, against `previous` when a manifest is committed,
/// and returns what the next manifest records as retired.
pub(crate) fn check(
    previous: Option<&PreviousManifest>,
    tree: &Tree,
    listed: &RetiredList,
    diagnostics: &mut Diagnostics,
) -> Retired {
    let Some(previous) = previous else {
        // With no manifest the root's list is the only record there is, and it
        // seeds the first one.
        return Retired {
            codes: BTreeMap::new(),
            modules: listed
                .modules
                .iter()
                .map(|name| name.value.clone())
                .collect(),
        };
    };

    check_the_retired_list(previous, tree, listed, diagnostics);
    let mut retired = Retired {
        codes: BTreeMap::new(),
        modules: previous.retired_modules.clone(),
    };
    for (name, before) in &previous.modules {
        let Some(after) = tree.modules.get(name) else {
            if listed.names(name) {
                retired.modules.insert(name.clone());
            } else {
                diagnostics.at(
                    Code::ModuleMissing,
                    listed.anchor,
                    format!(
                        "module `{name}` is in the committed manifest, at `{}`, and no longer in \
                         the tree; restore it, or retire it by naming it in the root file's \
                         `[retired] modules` — a rename is a retirement and a new module",
                        before.directory
                    ),
                );
            }
            continue;
        };
        check_module(name, before, after, diagnostics);
        retired
            .codes
            .insert(name.clone(), retired_codes(before, after));
    }
    retired
}

/// The root's list against the manifest's: it may not drop a name the manifest
/// retired, name one the manifest never held, or leave a retired name declared.
fn check_the_retired_list(
    previous: &PreviousManifest,
    tree: &Tree,
    listed: &RetiredList,
    diagnostics: &mut Diagnostics,
) {
    for name in &previous.retired_modules {
        if listed.names(name) {
            continue;
        }
        if let Some(module) = tree.modules.get(name) {
            diagnostics.at(
                Code::RetiredModuleDeclared,
                module.name_span,
                format!(
                    "module `{name}` carries a retired module's name; a retired name is a retired \
                     error category, and is never declared again"
                ),
            );
        } else {
            diagnostics.at(
                Code::RetirementRemoved,
                listed.anchor,
                format!(
                    "`[retired]` no longer names `{name}`, which the committed manifest retired; \
                     the list is permanent — put the name back"
                ),
            );
        }
    }
    let known = || {
        previous
            .modules
            .keys()
            .chain(&previous.retired_modules)
            .map(String::as_str)
    };
    for name in &listed.modules {
        // A listed name the tree still declares is refused where the tree is
        // validated, manifest or no manifest.
        let was_ever_held = known().any(|held| held == name.value);
        if !was_ever_held && !tree.modules.contains_key(&name.value) {
            diagnostics.at(
                Code::UnknownRetiredModule,
                name.span,
                format!(
                    "`[retired]` names `{}`, which the committed manifest never held{}",
                    name.value,
                    did_you_mean(&candidates_for(&name.value, known()))
                ),
            );
        }
    }
}

fn check_module(
    name: &str,
    before: &PreviousModule,
    after: &Module,
    diagnostics: &mut Diagnostics,
) {
    let version = after.schema_version.value;
    if version < before.schema_version {
        diagnostics.at(
            Code::VersionRegression,
            after.schema_version.span,
            format!(
                "`schema_version` {version} is lower than the committed tree's {} for module \
                 `{name}`; a module's version only grows",
                before.schema_version
            ),
        );
        return;
    }

    // The two rules a bump does not lift.
    for (error_name, error) in &after.items.errors {
        let code = error.code.value;
        if before.retired_codes.contains(&code) {
            diagnostics.at(
                Code::RetiredCodeDeclared,
                error.code.span,
                format!(
                    "error `{error_name}` declares code {code}, which module `{name}` retired; a \
                     retired code is never declared again, at any version"
                ),
            );
        }
        if let Some(previously) = before.vocabulary.errors.get(error_name) {
            if previously.code != code {
                diagnostics.at(
                    Code::ErrorRenumbered,
                    error.code.span,
                    format!(
                        "error `{error_name}` changed its code from {} to {code}; a code is an \
                         error's identity — to give the condition up, remove the error under a \
                         `schema_version` bump, which retires {}",
                        previously.code, previously.code
                    ),
                );
            }
        }
    }

    if version > before.schema_version {
        return;
    }
    let current = Vocabulary::of(&after.items);
    for change in breaking_changes(&before.vocabulary, &current) {
        diagnostics.at(
            Code::BreakingChange,
            after.schema_version.span,
            format!(
                "{change}, which breaks readers of module `{name}` at schema version {version}; \
                 bump its `schema_version` to make the change"
            ),
        );
    }
}

/// The codes `after` leaves spent: those the module had already retired, and
/// the code of every error whose condition is gone — one no error holds any
/// more, under any name.
fn retired_codes(before: &PreviousModule, after: &Module) -> BTreeSet<u32> {
    let held_now: BTreeSet<u32> = after
        .items
        .errors
        .values()
        .map(|error| error.code.value)
        .collect();
    let given_up = before
        .vocabulary
        .errors
        .values()
        .map(|error| error.code)
        .filter(|code| !held_now.contains(code));
    before
        .retired_codes
        .iter()
        .copied()
        .chain(given_up)
        .collect()
}

/// Every change from `previous` to `current` an existing reader cannot absorb:
/// an item removed or renamed, an attribute removed or retyped, an event
/// removed, a metric's type or unit changed, an error removed, renamed or
/// reclassified. Additions are absent from the list by construction — they are
/// the additive rule.
fn breaking_changes(previous: &Vocabulary, current: &Vocabulary) -> Vec<String> {
    let mut changes = Vec::new();
    for ((kind, name), before) in &previous.items {
        let item = format!("{} `{name}`", kind.keyword());
        let Some(after) = current.items.get(&(*kind, name.clone())) else {
            changes.push(format!("{item} was removed or renamed"));
            continue;
        };
        let noun = if *kind == ItemKind::Log {
            "field"
        } else {
            "attribute"
        };
        changes.extend(lost_or_retyped(
            &item,
            noun,
            &before.attributes,
            &after.attributes,
        ));
        for event in before.events.difference(&after.events) {
            changes.push(format!("{item} lost its event `{event}`"));
        }
        changes.extend(metric_changes(&item, before, after));
    }
    for (name, before) in &previous.errors {
        changes.extend(error_changes(name, before, previous, current));
    }
    changes
}

fn lost_or_retyped(
    item: &str,
    noun: &str,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changes = Vec::new();
    for (name, type_before) in before {
        match after.get(name) {
            None => changes.push(format!("{item} lost its {noun} `{name}`")),
            Some(type_after) if type_after != type_before => changes.push(format!(
                "{item}'s {noun} `{name}` changed type from `{type_before}` to `{type_after}`"
            )),
            Some(_) => {}
        }
    }
    changes
}

fn metric_changes(item: &str, before: &Shape, after: &Shape) -> Vec<String> {
    let (Some(before), Some(after)) = (&before.metric, &after.metric) else {
        return Vec::new();
    };
    let mut changes = Vec::new();
    if before.kind != after.kind {
        changes.push(format!(
            "{item} changed type from `{}` to `{}`",
            before.kind, after.kind
        ));
    }
    if before.unit != after.unit {
        changes.push(format!(
            "{item} changed unit from `{}` to `{}`",
            before.unit, after.unit
        ));
    }
    changes
}

/// What became of the error `name` held, read by its code — the identity that
/// survives a rename.
fn error_changes(
    name: &str,
    before: &ErrorShape,
    previous: &Vocabulary,
    current: &Vocabulary,
) -> Vec<String> {
    // An error that kept its name under another code is refused at every
    // version, where the two codes are named; it is not also a removal.
    if current
        .errors
        .get(name)
        .is_some_and(|after| after.code != before.code)
    {
        return Vec::new();
    }
    let holder = current
        .errors
        .iter()
        .find(|(_, after)| after.code == before.code);
    let Some((holder_name, after)) = holder else {
        return vec![format!(
            "error `{name}` was removed, which retires its code {}",
            before.code
        )];
    };
    let mut changes = Vec::new();
    if holder_name != name {
        // The code now belongs to an error that already had a name and another
        // code: that one changed its code, which is refused in its own right.
        if previous.errors.contains_key(holder_name) {
            return Vec::new();
        }
        changes.push(format!("error `{name}` was renamed to `{holder_name}`"));
    }
    let error = format!("error `{holder_name}`");
    if before.class != after.class {
        changes.push(format!(
            "{error} changed class from `{}` to `{}`",
            before.class, after.class
        ));
    }
    changes.extend(lost_or_retyped(
        &error,
        "field",
        &before.fields,
        &after.fields,
    ));
    changes
}
