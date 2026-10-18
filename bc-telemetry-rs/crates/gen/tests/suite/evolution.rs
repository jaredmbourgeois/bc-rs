//! How a tree may change between runs: a change existing readers cannot absorb
//! needs its module's `schema_version` bumped past the committed one; an
//! error's code is its identity; and a module leaves the tree only by being
//! retired.

use crate::support::{Scratch, assert_answer, assert_findings, run_in};

const ROOT: &str = "bc-telemetry.toml";
const METRICS: &str = "telemetry/metrics.toml";
const VIEWS: &str = "telemetry/views.toml";
const PAYMENTS: &str = "services/payments/bc-telemetry.toml";
const PAYMENTS_ERRORS: &str = "services/payments/errors.json";
const LEDGER: &str = "services/payments/ledger/bc-telemetry.json";
const MANIFEST: &str = "generated/telemetry/manifest.json";

const WROTE_THE_MANIFEST: &str = "generated/telemetry: wrote 1, removed 0, unchanged 0\n";
const FRESH: &str = "generated/telemetry: fresh (1 file)\n";
const MANIFEST_CHANGED: &str = "error[output-changed]: `generated/telemetry/manifest.json` differs \
                                from what the definitions generate";

/// Where each canonical module writes its `schema_version` value.
const STORE_SERVER_VERSION_AT: &str = "bc-telemetry.toml:10:18";
const PAYMENTS_VERSION_AT: &str = "services/payments/bc-telemetry.toml:7:18";
const LEDGER_VERSION_AT: &str = "services/payments/ledger/bc-telemetry.json:2:51";

fn breaking(at: &str, module: &str, change: &str) -> String {
    format!(
        "{at}: error[breaking-change]: {change}, which breaks readers of module `{module}` at \
         schema version 1; bump its `schema_version` to make the change"
    )
}

fn bump_store_server(scratch: &Scratch) {
    for file in [ROOT, METRICS, VIEWS] {
        scratch.edit(file, "schema_version = 1", "schema_version = 2");
    }
}

fn bump_payments(scratch: &Scratch, from: u32, to: u32) {
    scratch.edit(
        PAYMENTS,
        &format!("schema_version = {from}"),
        &format!("schema_version = {to}"),
    );
    scratch.edit(
        PAYMENTS_ERRORS,
        &format!("\"schema_version\": {from}"),
        &format!("\"schema_version\": {to}"),
    );
}

#[test]
fn a_removed_item_needs_a_bump_and_a_bump_admits_it() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(
        ROOT,
        "[span.refund]\ndescription = \"A refund, from request to settlement\"\n\
         attributes = { order_id = \"digest\", amount = \"float\", partial = \"bool\", settle_time = \"duration\" }\n",
        "",
    );
    let refused = breaking(
        STORE_SERVER_VERSION_AT,
        "store_server",
        "span `refund` was removed or renamed",
    );
    assert_findings(&run_in(scratch.path(), &[]), &[refused.as_str()]);
    // `--check` holds the same rule: a CI that only checks still refuses.
    assert_findings(
        &run_in(scratch.path(), &["--check"]),
        &[MANIFEST_CHANGED, refused.as_str()],
    );

    bump_store_server(&scratch);
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
}

#[test]
fn every_kind_of_breaking_change_is_named() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(
        METRICS,
        "attributes = { route = \"string\", status = \"int\" }",
        "attributes = { route = \"int\" }",
    );
    scratch.edit(METRICS, "type = \"counter\"", "type = \"gauge\"");
    scratch.edit(METRICS, "unit = \"ms\"", "unit = \"s\"");
    scratch.edit(
        ROOT,
        "events = [\"payment_authorized\", \"inventory_reserved\"]",
        "events = [\"payment_authorized\"]",
    );
    scratch.edit(
        VIEWS,
        "fields = { order_id = \"string\", reason = \"string\" }",
        "fields = { order_id = \"digest\" }",
    );
    // A gauge takes no `rate`, so the query that measured the counter goes with
    // it; a query is no part of what readers depend on.
    scratch.edit(
        VIEWS,
        "aggregation = \"rate\"\nwindow = \"30s\"",
        "aggregation = \"avg\"\nwindow = \"30s\"",
    );
    scratch.edit(VIEWS, "group_by = [\"reason\"]\n", "");
    let named = |change: &str| breaking(STORE_SERVER_VERSION_AT, "store_server", change);
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            named("span `checkout` lost its event `inventory_reserved`").as_str(),
            named("metric `orders_placed` changed type from `counter` to `gauge`").as_str(),
            named(
                "metric `request_duration`'s attribute `route` changed type from `string` to `int`",
            )
            .as_str(),
            named("metric `request_duration` lost its attribute `status`").as_str(),
            named("metric `request_duration` changed unit from `ms` to `s`").as_str(),
            named("log `order_failed`'s field `order_id` changed type from `string` to `digest`")
                .as_str(),
            named("log `order_failed` lost its field `reason`").as_str(),
        ],
    );
}

#[test]
fn an_addition_is_the_additive_rule_and_needs_no_bump() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(
        METRICS,
        "attributes = { route = \"string\", status = \"int\" }",
        "attributes = { route = \"string\", status = \"int\", region = \"string\" }",
    );
    scratch.edit(
        ROOT,
        "[span.refund]",
        "[span.restock]\ndescription = \"A restock\"\n\n[span.refund]",
    );
    scratch.edit(
        ROOT,
        "events = [\"payment_authorized\", \"inventory_reserved\"]",
        "events = [\"payment_authorized\", \"inventory_reserved\", \"receipt_sent\"]",
    );
    scratch.edit(
        PAYMENTS_ERRORS,
        "\"fields\": { \"order_id\": \"string\" }",
        "\"fields\": { \"order_id\": \"string\", \"expired_on\": \"string\" }",
    );
    scratch.edit(
        PAYMENTS_ERRORS,
        "  \"error\": {\n",
        "  \"error\": {\n    \"processor_unreachable\": {\n      \"description\": \"The processor \
         could not be reached\",\n      \"class\": \"failure\",\n      \"code\": 3\n    },\n",
    );
    // What only describes — a description, a log's level — changes nothing a
    // reader depends on.
    scratch.edit(VIEWS, "level = \"error\"", "level = \"fatal\"");
    scratch.edit(
        METRICS,
        "description = \"Server-side request latency\"",
        "description = \"Request latency, measured at the server\"",
    );
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
}

#[test]
fn a_falling_version_is_refused() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    // The first module the manifest records is `ledger`.
    scratch.edit(MANIFEST, "\"schema_version\": 1", "\"schema_version\": 3");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[&format!(
            "{LEDGER_VERSION_AT}: error[version-regression]: `schema_version` 1 is lower than the \
             committed tree's 3 for module `ledger`; a module's version only grows"
        )],
    );
}

/// Each module's readers are its own: a bump says its records changed, and
/// says nothing about any other module's.
#[test]
fn a_bump_in_one_module_admits_nothing_in_another() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    bump_payments(&scratch, 1, 2);
    scratch.edit(
        LEDGER,
        "      \"fields\": { \"account\": \"string\" }\n    }\n  },\n  \"error\"",
        "      \"fields\": {}\n    }\n  },\n  \"error\"",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[breaking(
            LEDGER_VERSION_AT,
            "ledger",
            "log `entry_posted` lost its field `account`",
        )
        .as_str()],
    );
}

const DECLINED: &str = "    \"authorization_declined\": {\n      \"description\": \"The processor \
                        declined the authorization\",\n      \"class\": \"failure\",\n      \
                        \"code\": 1,\n      \"fields\": { \"order_id\": \"string\", \
                        \"processor_code\": \"string\" }\n    },\n";

/// Removing the error `authorization_declined` also removes what measured and
/// showed it, in `store_server`.
fn remove_the_declined_error(scratch: &Scratch) {
    scratch.edit(PAYMENTS_ERRORS, DECLINED, "");
    scratch.edit(
        VIEWS,
        "[query.declines-by-processor]\n\
         description = \"Declined authorizations per day, by the processor's code\"\n\
         measure = \"error.payments.authorization_declined\"\n\
         aggregation = \"count\"\n\
         window = \"1d\"\n\
         group_by = [\"processor_code\"]\n",
        "",
    );
    scratch.edit(
        VIEWS,
        "[[dashboard.checkout-health.panel]]\n\
         title = \"Declined authorizations\"\n\
         query = \"error.payments.authorization_declined\"\n\
         render = \"logs\"\n",
        "",
    );
}

/// A code is persisted wherever an error is, so a removed error's code is spent:
/// the manifest carries it from then on, and no later version may declare it.
#[test]
fn a_removed_errors_code_is_retired_for_good() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    remove_the_declined_error(&scratch);
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[breaking(
            PAYMENTS_VERSION_AT,
            "payments",
            "error `authorization_declined` was removed, which retires its code 1",
        )
        .as_str()],
    );

    bump_payments(&scratch, 1, 2);
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert!(
        scratch
            .read(MANIFEST)
            .contains("      \"retired_codes\": [\n        1\n      ]"),
        "the manifest was:\n{}",
        scratch.read(MANIFEST)
    );
    // The retirement is the manifest's own state now: a later run reproduces it.
    assert_answer(&run_in(scratch.path(), &["--check"]), FRESH);

    scratch.edit(
        PAYMENTS_ERRORS,
        "  \"error\": {\n",
        "  \"error\": {\n    \"processor_unreachable\": {\n      \"description\": \"The processor \
         could not be reached\",\n      \"class\": \"failure\",\n      \"code\": 1\n    },\n",
    );
    let retired = "services/payments/errors.json:7:15: error[retired-code]: error \
                   `processor_unreachable` declares code 1, which module `payments` retired; a \
                   retired code is never declared again, at any version";
    assert_findings(&run_in(scratch.path(), &[]), &[retired]);
    // A bump admits a breaking change; it does not give a spent code back.
    bump_payments(&scratch, 2, 3);
    assert_findings(&run_in(scratch.path(), &[]), &[retired]);
}

/// A rename changes what the code is called, not what it means: every persisted
/// pair still names the same condition, so nothing is retired.
#[test]
fn a_renamed_error_keeps_its_code_and_retires_nothing() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(
        PAYMENTS_ERRORS,
        "\"card_expired\"",
        "\"card_past_its_date\"",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[breaking(
            PAYMENTS_VERSION_AT,
            "payments",
            "error `card_expired` was renamed to `card_past_its_date`",
        )
        .as_str()],
    );

    bump_payments(&scratch, 1, 2);
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert!(
        !scratch.read(MANIFEST).contains("\"retired_codes\": [\n"),
        "a rename retired a code:\n{}",
        scratch.read(MANIFEST)
    );
}

/// An error that keeps its name and takes another code is a removal and an
/// addition written as an edit, and is refused whether or not a bump came with
/// it.
#[test]
fn an_error_that_keeps_its_name_never_changes_its_code() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(PAYMENTS_ERRORS, "\"code\": 2", "\"code\": 7");
    let changed = "services/payments/errors.json:13:15: error[code-changed]: error `card_expired` \
                   changed its code from 2 to 7; a code is an error's identity — to give the \
                   condition up, remove the error under a `schema_version` bump, which retires 2";
    assert_findings(&run_in(scratch.path(), &[]), &[changed]);
    bump_payments(&scratch, 1, 2);
    assert_findings(&run_in(scratch.path(), &[]), &[changed]);
}

/// Two errors that trade codes each changed theirs: two findings, and neither
/// is read as the other renamed.
#[test]
fn two_errors_that_swap_codes_are_each_refused() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(PAYMENTS_ERRORS, "\"code\": 1", "\"code\": 9");
    scratch.edit(PAYMENTS_ERRORS, "\"code\": 2", "\"code\": 1");
    scratch.edit(PAYMENTS_ERRORS, "\"code\": 9", "\"code\": 2");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "services/payments/errors.json:7:15: error[code-changed]: error \
             `authorization_declined` changed its code from 1 to 2; a code is an error's identity \
             — to give the condition up, remove the error under a `schema_version` bump, which \
             retires 1",
            "services/payments/errors.json:13:15: error[code-changed]: error `card_expired` \
             changed its code from 2 to 1; a code is an error's identity — to give the condition \
             up, remove the error under a `schema_version` bump, which retires 2",
        ],
    );
}

#[test]
fn an_errors_class_and_fields_are_part_of_what_readers_depend_on() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(
        PAYMENTS_ERRORS,
        "\"class\": \"refusal\"",
        "\"class\": \"failure\"",
    );
    scratch.edit(
        PAYMENTS_ERRORS,
        "\"fields\": { \"order_id\": \"string\", \"processor_code\": \"string\" }",
        "\"fields\": { \"order_id\": \"digest\" }",
    );
    // The query grouped by the field that went.
    scratch.edit(VIEWS, "group_by = [\"processor_code\"]\n", "");
    let named = |change: &str| breaking(PAYMENTS_VERSION_AT, "payments", change);
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            named(
                "error `authorization_declined`'s field `order_id` changed type from `string` to \
                 `digest`",
            )
            .as_str(),
            named("error `authorization_declined` lost its field `processor_code`").as_str(),
            named("error `card_expired` changed class from `refusal` to `failure`").as_str(),
        ],
    );
}

const RETIRE_LEDGER: &str = "[retired]\nmodules = [\"ledger\"]\n\n[generate]";

/// A missing directory is also what an unfetched submodule looks like, so a
/// module leaves the tree only when the root file says it has.
#[test]
fn a_module_that_vanishes_is_refused_until_the_root_retires_it() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.remove_tree("services/payments/ledger");
    let missing = "bc-telemetry.toml:1:1: error[module-missing]: module `ledger` is in the \
                   committed manifest, at `services/payments/ledger`, and no longer in the tree; \
                   restore it, or retire it by naming it in the root file's `[retired] modules` — \
                   a rename is a retirement and a new module";
    assert_findings(&run_in(scratch.path(), &[]), &[missing]);
    assert_findings(
        &run_in(scratch.path(), &["--check"]),
        &[MANIFEST_CHANGED, missing],
    );

    scratch.edit(ROOT, "[generate]", RETIRE_LEDGER);
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    let manifest = scratch.read(MANIFEST);
    assert!(
        manifest.contains("\"retired_modules\": [\n    \"ledger\"\n  ]"),
        "the manifest was:\n{manifest}"
    );
    assert!(!manifest.contains("\"ledger\": {"));
    assert_answer(&run_in(scratch.path(), &["--check"]), FRESH);
}

/// The list is permanent, as the name's retirement is: it may not be declared
/// again, and it may not quietly leave the list.
#[test]
fn a_retired_modules_name_is_never_declared_again_and_never_leaves_the_list() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    let ledger = scratch.read(LEDGER);
    scratch.remove_tree("services/payments/ledger");
    scratch.edit(ROOT, "[generate]", RETIRE_LEDGER);
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);

    // Declared again while still listed.
    scratch.write("accounts/bc-telemetry.json", &ledger);
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:13:12: error[retired-module-declared]: `[retired]` names `ledger`, \
             which this tree still declares; a module is retired once its definition is gone, \
             and its name is never declared again",
        ],
    );

    // Declared again with its entry taken off the list.
    scratch.edit(ROOT, RETIRE_LEDGER, "[generate]");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "accounts/bc-telemetry.json:2:23: error[retired-module-declared]: module `ledger` \
             carries a retired module's name; a retired name is a retired error category, and is \
             never declared again",
        ],
    );

    // Taken off the list with the module still gone.
    scratch.remove_tree("accounts");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:1: error[retirement-removed]: `[retired]` no longer names \
             `ledger`, which the committed manifest retired; the list is permanent — put the name \
             back",
        ],
    );
}

#[test]
fn a_retired_name_the_manifest_never_held_is_refused_as_a_likely_misspelling() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.edit(
        ROOT,
        "[generate]",
        "[retired]\nmodules = [\"leger\", \"warehouse\"]\n\n[generate]",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:13:12: error[unknown-retired-module]: `[retired]` names `leger`, \
             which the committed manifest never held; did you mean `ledger`?",
            "bc-telemetry.toml:13:21: error[unknown-retired-module]: `[retired]` names \
             `warehouse`, which the committed manifest never held",
        ],
    );
}

/// With no manifest committed there is nothing to hold the list to, and nothing
/// else that records what it says: it is the record, and seeds the first one.
#[test]
fn a_retired_list_with_no_manifest_seeds_the_first_one() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.remove(MANIFEST);
    scratch.edit(
        ROOT,
        "[generate]",
        "[retired]\nmodules = [\"warehouse\", \"legacy_billing\"]\n\n[generate]",
    );
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert!(
        scratch
            .read(MANIFEST)
            .contains("\"retired_modules\": [\n    \"legacy_billing\",\n    \"warehouse\"\n  ]"),
        "the manifest was:\n{}",
        scratch.read(MANIFEST)
    );
    assert_answer(&run_in(scratch.path(), &["--check"]), FRESH);
}

/// A module's name is its error category, so a rename is a retirement and a
/// new module; its directory is only where it lives, and may change freely.
#[test]
fn a_renamed_module_is_a_retirement_and_a_moved_one_is_the_same_module() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.rename("services/payments/ledger", "accounting/ledger");
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert!(
        scratch
            .read(MANIFEST)
            .contains("\"ledger\": {\n      \"directory\": \"accounting/ledger\","),
        "the manifest was:\n{}",
        scratch.read(MANIFEST)
    );

    scratch.edit(
        "accounting/ledger/bc-telemetry.json",
        "\"name\": \"ledger\"",
        "\"name\": \"books\"",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:1:1: error[module-missing]: module `ledger` is in the committed \
             manifest, at `accounting/ledger`, and no longer in the tree; restore it, or retire \
             it by naming it in the root file's `[retired] modules` — a rename is a retirement \
             and a new module",
        ],
    );
    scratch.edit(ROOT, "[generate]", RETIRE_LEDGER);
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    let manifest = scratch.read(MANIFEST);
    assert!(manifest.contains("\"books\": {\n      \"directory\": \"accounting/ledger\","));
    assert!(manifest.contains("\"retired_modules\": [\n    \"ledger\"\n  ]"));
}

#[test]
fn a_new_module_joins_the_tree_without_ceremony() {
    let scratch = Scratch::of_the_canonical_vocabulary();
    scratch.write(
        "warehouse/bc-telemetry.toml",
        "[module]\nname = \"warehouse\"\nschema_version = 4\n\n\
         [error.out_of_stock]\ndescription = \"Nothing left to ship\"\nclass = \"refusal\"\ncode = 1\n",
    );
    assert_answer(&run_in(scratch.path(), &[]), WROTE_THE_MANIFEST);
    assert!(scratch.read(MANIFEST).contains(
        "\"warehouse\": {\n      \"directory\": \"warehouse\",\n      \"schema_version\": 4,"
    ));
}
