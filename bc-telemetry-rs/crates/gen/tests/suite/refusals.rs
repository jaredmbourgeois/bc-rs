//! What a definition is refused for: each finding's code, its `file:line:column`,
//! and its words — and that one run reports every finding, not its first.

use crate::support::{Scratch, assert_answer, assert_findings, run_in};

/// Runs a one-file TOML tree and asserts what it printed.
fn refused(definition: &str, expected: &[&str]) {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", definition);
    assert_findings(&run_in(scratch.path(), &[]), expected);
}

/// Runs a one-file TOML tree and asserts it is valid.
fn accepted(definition: &str) {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", definition);
    assert_answer(
        &run_in(scratch.path(), &[]),
        "bc-telemetry.toml: valid, 1 module; nothing to generate (the root file has no \
         `[generate]` table)\n",
    );
}

const HEADER: &str = "[module]\nname = \"shop\"\nschema_version = 1\n";

#[test]
fn an_unknown_key_is_refused_with_its_nearest_spelling() {
    refused(
        &format!(
            "{HEADER}\n[span.checkout]\ndescription = \"A checkout\"\natributes = {{ cart_id = \"string\" }}\n"
        ),
        &[
            "bc-telemetry.toml:7:1: error[unknown-field]: unknown field `atributes` on span \
             `checkout`; did you mean `attributes`?",
        ],
    );
}

#[test]
fn one_run_reports_every_finding_in_reading_order() {
    refused(
        &format!(
            "{HEADER}
[span.checkout]
attributes = {{ cart_id = \"strng\" }}

[metric.latency]
description = \"Latency\"
type = \"histogrm\"
unit = \"ms\"

[log.failure]
description = \"A failure\"
level = \"critical\"
"
        ),
        &[
            "bc-telemetry.toml:5:7: error[missing-field]: span `checkout` is missing its \
             required `description`",
            "bc-telemetry.toml:6:26: error[invalid-value]: attribute `cart_id` in `attributes` on \
             span `checkout` must be typed one of string | int | float | bool | duration | digest; \
             got `strng`; did you mean `string`?",
            "bc-telemetry.toml:10:8: error[invalid-value]: `type` on metric `latency` must be one \
             of counter | gauge | histogram; got `histogrm`; did you mean `histogram`?",
            "bc-telemetry.toml:15:9: error[invalid-value]: `level` on log `failure` must be one of \
             debug | info | warn | error | fatal; got `critical`",
        ],
    );
}

#[test]
fn a_value_of_the_wrong_type_is_refused_where_it_is_written() {
    refused(
        "[module]\nname = \"shop\"\nschema_version = \"1\"\n\n\
         [span.checkout]\ndescription = \"A checkout\"\nevents = \"paid\"\n",
        &[
            "bc-telemetry.toml:3:18: error[type-mismatch]: `schema_version` on `[module]` must be \
             an integer, got a string",
            "bc-telemetry.toml:7:10: error[type-mismatch]: `events` on span `checkout` must be an \
             array, got a string",
        ],
    );
}

#[test]
fn names_outside_their_grammar_are_refused() {
    refused(
        "[module]\nname = \"store-server\"\nschema_version = 1\n\n\
         [span.Checkout]\ndescription = \"A checkout\"\nattributes = { cartId = \"string\" }\n\n\
         [query.checkout_rate]\ndescription = \"Checkouts\"\nmeasure = \"span.Checkout\"\n\
         aggregation = \"count\"\nwindow = \"1m\"\n",
        &[
            "bc-telemetry.toml:2:8: error[invalid-name]: module name `store-server` is not \
             snake_case: lowercase letters and digits in `_`-separated words, starting with a \
             letter",
            "bc-telemetry.toml:5:7: error[invalid-name]: span name `Checkout` is not snake_case: \
             lowercase letters and digits in `_`-separated words, starting with a letter",
            "bc-telemetry.toml:7:16: error[invalid-name]: attribute name `cartId` in `attributes` \
             on span `Checkout` is not snake_case: lowercase letters and digits in `_`-separated \
             words, starting with a letter",
            "bc-telemetry.toml:9:8: error[invalid-name]: query name `checkout_rate` is not \
             kebab-case: lowercase letters and digits in `-`-separated words, starting with a \
             letter",
        ],
    );
}

#[test]
fn values_outside_what_their_key_accepts_are_refused() {
    refused(
        "[module]\nname = \"shop\"\nschema_version = 0\n\n\
         [span.checkout]\ndescription = \"  \"\nevents = [\"paid\", \"paid\"]\n\n\
         [query.checkouts]\ndescription = \"Checkouts\"\nmeasure = \"checkout\"\n\
         aggregation = \"count\"\nwindow = \"5 minutes\"\n\n\
         [dashboard.empty]\ntitle = \"Nothing\"\npanel = []\n",
        &[
            "bc-telemetry.toml:3:18: error[invalid-value]: `schema_version` must be a whole number \
             from 1 to 2147483647; got 0",
            "bc-telemetry.toml:6:15: error[invalid-value]: `description` on span `checkout` must \
             not be empty",
            "bc-telemetry.toml:7:19: error[duplicate-entry]: event `paid` appears twice in \
             `events` on span `checkout`",
            "bc-telemetry.toml:11:11: error[invalid-value]: `measure` on query `checkouts` must \
             name a declared item as `<kind>.<name>`, or as `<kind>.<module>.<name>` in another \
             module, where the kind is span, metric, log, error; got `checkout`",
            "bc-telemetry.toml:13:10: error[invalid-value]: `window` on query `checkouts` must be a \
             positive whole number of `s`, `m`, `h` or `d`, like `5m`; got `5 minutes`",
            "bc-telemetry.toml:17:9: error[invalid-value]: dashboard `empty` has no panels; a \
             dashboard shows at least one",
        ],
    );
}

/// A metric states its unit, from a closed set: a panel's unit is derived from
/// it, so a free-text one would have nothing to derive.
#[test]
fn a_metric_states_one_of_the_closed_units() {
    refused(
        &format!(
            "{HEADER}
[metric.latency]
description = \"Latency\"
type = \"histogram\"

[metric.size]
description = \"Size\"
type = \"gauge\"
unit = \"msec\"
"
        ),
        &[
            "bc-telemetry.toml:5:9: error[missing-field]: metric `latency` is missing its \
             required `unit`",
            "bc-telemetry.toml:12:8: error[invalid-value]: `unit` on metric `size` must be one of \
             none | ms | s | bytes | percent; got `msec`; did you mean `ms`?",
        ],
    );
}

#[test]
fn an_error_declares_its_class_and_its_code() {
    refused(
        &format!(
            "{HEADER}
[error.declined]

[error.fatal_class]
description = \"A class outside the two\"
class = \"fatal\"
code = 1

[error.negative]
description = \"A code below the range\"
class = \"failure\"
code = -1

[error.too_large]
description = \"A code past a signed 32-bit integer\"
class = \"failure\"
code = 2147483648

[error.quoted]
description = \"A code written as text\"
class = \"refusal\"
code = \"3\"
fields = {{ reason = \"text\" }}
"
        ),
        &[
            "bc-telemetry.toml:5:8: error[missing-field]: error `declined` is missing its \
             required `description`",
            "bc-telemetry.toml:5:8: error[missing-field]: error `declined` is missing its \
             required `class`",
            "bc-telemetry.toml:5:8: error[missing-field]: error `declined` is missing its \
             required `code`",
            "bc-telemetry.toml:9:9: error[invalid-value]: `class` on error `fatal_class` must be \
             one of failure | refusal; got `fatal`",
            "bc-telemetry.toml:15:8: error[invalid-value]: `code` on error `negative` must be a \
             whole number from 0 to 2147483647; got -1",
            "bc-telemetry.toml:20:8: error[invalid-value]: `code` on error `too_large` must be a \
             whole number from 0 to 2147483647; got 2147483648",
            "bc-telemetry.toml:25:8: error[type-mismatch]: `code` on error `quoted` must be an \
             integer, got a string",
            "bc-telemetry.toml:26:21: error[invalid-value]: field `reason` in `fields` on error \
             `quoted` must be typed one of string | int | float | bool | duration | digest; got \
             `text`",
        ],
    );
}

#[test]
fn an_error_code_spans_zero_to_the_largest_signed_32_bit_integer() {
    accepted(&format!(
        "{HEADER}
[error.first]
description = \"The lowest code\"
class = \"failure\"
code = 0

[error.last]
description = \"The highest code\"
class = \"refusal\"
code = 2147483647
"
    ));
}

/// A code names one error: it is what a persisted record keeps of it.
#[test]
fn two_errors_of_one_module_may_not_declare_one_code() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.toml",
        &format!(
            "include = [\"more.toml\"]\n\n{HEADER}
[error.declined]
description = \"Declined\"
class = \"failure\"
code = 4

[error.expired]
description = \"Expired\"
class = \"refusal\"
code = 4
"
        ),
    );
    scratch.write(
        "more.toml",
        &format!(
            "{HEADER}
[error.blocked]
description = \"Blocked\"
class = \"refusal\"
code = 4
"
        ),
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:15:8: error[duplicate-code]: error `expired` declares code 4, \
             which error `declined` declares at bc-telemetry.toml:10:8; a code is one error's \
             identity",
            "more.toml:8:8: error[duplicate-code]: error `blocked` declares code 4, which error \
             `declined` declares at bc-telemetry.toml:10:8; a code is one error's identity",
        ],
    );
}

/// A log and an error are both stored as log records, under one name each.
#[test]
fn an_error_and_a_log_of_one_module_may_not_share_a_name() {
    refused(
        &format!(
            "{HEADER}
[log.declined]
description = \"A declined charge\"
level = \"warn\"

[error.declined]
description = \"The charge was declined\"
class = \"failure\"
code = 1
"
        ),
        &[
            "bc-telemetry.toml:9:8: error[duplicate-item]: error `declined` shares its name with \
             log `declined`, declared at bc-telemetry.toml:5:6; both are stored as log records, \
             so a module gives one name to one of them",
        ],
    );
}

/// PostgreSQL keeps 63 bytes of an identifier and Grafana takes 40 characters
/// of a uid; a derived name one past either is refused, and one at it is not.
#[test]
fn a_derived_name_is_held_to_its_engines_limit() {
    // `metric__shop__` is 14 bytes and `error__shop__` 13, so these item names
    // make view names of exactly 63 and, with one more letter, 64.
    let metric_at_the_limit = "m".repeat(49);
    let error_at_the_limit = "e".repeat(50);
    // `shop-` is 5 characters, so this dashboard name makes a uid of exactly 40.
    let dashboard_at_the_limit = "d".repeat(35);
    let definition = |metric: &str, error: &str, refusal: &str, dashboard: &str| {
        format!(
            "{HEADER}
[metric.{metric}]
description = \"A metric\"
type = \"counter\"
unit = \"none\"

[error.{error}]
description = \"A failure\"
class = \"failure\"
code = 1

[error.{refusal}]
description = \"A refusal, which has no view\"
class = \"refusal\"
code = 2

[query.q]
description = \"A query\"
measure = \"metric.{metric}\"
aggregation = \"count\"
window = \"1m\"

[dashboard.{dashboard}]
title = \"A dashboard\"

[[dashboard.{dashboard}.panel]]
title = \"A panel\"
query = \"query.q\"
render = \"stat\"
"
        )
    };

    // A refusal is never reported, so no view is named after it: its name may
    // be as long as it likes.
    let long_refusal = "r".repeat(80);
    accepted(&definition(
        &metric_at_the_limit,
        &error_at_the_limit,
        &long_refusal,
        &dashboard_at_the_limit,
    ));

    let metric = format!("{metric_at_the_limit}m");
    let error = format!("{error_at_the_limit}e");
    let dashboard = format!("{dashboard_at_the_limit}d");
    refused(
        &definition(&metric, &error, &long_refusal, &dashboard),
        &[
            &format!(
                "bc-telemetry.toml:5:9: error[name-too-long]: metric `{metric}` in module `shop` \
                 has the view name `metric__shop__{metric}`, which is 64 bytes; PostgreSQL keeps \
                 63 and would cut it into another view's name — shorten the module's name or the \
                 item's"
            ),
            &format!(
                "bc-telemetry.toml:10:8: error[name-too-long]: error `{error}` in module `shop` \
                 has the view name `error__shop__{error}`, which is 64 bytes; PostgreSQL keeps 63 \
                 and would cut it into another view's name — shorten the module's name or the \
                 item's"
            ),
            &format!(
                "bc-telemetry.toml:26:12: error[name-too-long]: dashboard `{dashboard}` in module \
                 `shop` has the Grafana uid `shop-{dashboard}`, which is 41 characters; Grafana \
                 takes 40 — shorten the module's name or the dashboard's"
            ),
        ],
    );
}

/// Text no generated file can carry is refused where it is written: a NUL,
/// which no PostgreSQL value holds, and a carriage return no line feed
/// follows — a description is written one comment line per line, and a lone CR
/// would end a `--` or `//` comment early and make the rest of its line code. A
/// line ending written CRLF is a line ending.
#[test]
fn text_no_generated_file_can_carry_is_refused() {
    accepted(&format!(
        "{HEADER}\n[span.checkout]\ndescription = \"Line one\\r\\nline two\"\n"
    ));
    refused(
        &format!(
            "{HEADER}\n[span.checkout]\ndescription = \"A checkout\\u0000\"\n\n\
             [span.refund]\ndescription = \"A refund\\rDROP SCHEMA telemetry CASCADE;\"\n"
        ),
        &[
            "bc-telemetry.toml:6:15: error[invalid-value]: `description` on span `checkout` \
             holds a NUL, which no generated file can carry",
            "bc-telemetry.toml:9:15: error[invalid-value]: `description` on span `refund` \
             holds a carriage return no line feed follows, which would end a generated `--` or \
             `//` comment early",
        ],
    );
}

/// A module's version is stored in each record's `schema_version` column, a
/// PostgreSQL `integer`: a version past its largest value could not be
/// written.
#[test]
fn a_schema_version_fits_its_integer_column() {
    accepted("[module]\nname = \"shop\"\nschema_version = 2147483647\n");
    refused(
        "[module]\nname = \"shop\"\nschema_version = 2147483648\n",
        &[
            "bc-telemetry.toml:3:18: error[invalid-value]: `schema_version` must be a whole \
           number from 1 to 2147483647; got 2147483648",
        ],
    );
}

/// An attribute is a column of its item's view, and PostgreSQL keeps 63 bytes
/// of a column's name: two longer names could become one column.
#[test]
fn an_attribute_name_is_held_to_what_postgresql_keeps() {
    let at_the_limit = "a".repeat(63);
    accepted(&format!(
        "{HEADER}\n[span.checkout]\ndescription = \"A checkout\"\n\
         attributes = {{ {at_the_limit} = \"string\" }}\n"
    ));
    let too_long = "a".repeat(64);
    refused(
        &format!(
            "{HEADER}\n[span.checkout]\ndescription = \"A checkout\"\n\
             attributes = {{ {too_long} = \"string\" }}\n"
        ),
        &[&format!(
            "bc-telemetry.toml:7:16: error[name-too-long]: attribute `{too_long}` in \
             `attributes` on span `checkout` is 64 bytes; PostgreSQL keeps 63 of a column's \
             name and would cut it into another's — shorten it"
        )],
    );
}

/// A tree of two modules: `shop` at the root and `billing` beneath it.
fn two_modules(shop: &str, billing: &str) -> Scratch {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.toml", &format!("{HEADER}{shop}"));
    scratch.write(
        "billing/bc-telemetry.toml",
        &format!("[module]\nname = \"billing\"\nschema_version = 1\n{billing}"),
    );
    scratch
}

const BILLING_ITEMS: &str = "
[metric.charges]
description = \"Charges\"
type = \"counter\"
unit = \"none\"
attributes = { processor = \"string\" }

[error.declined]
description = \"Declined\"
class = \"failure\"
code = 1
fields = { processor_code = \"string\" }

[error.expired]
description = \"Expired\"
class = \"refusal\"
code = 2

[query.charge-rate]
description = \"Charges per second\"
measure = \"metric.charges\"
aggregation = \"rate\"
window = \"1m\"
";

#[test]
fn a_reference_to_nothing_declared_is_refused_with_what_was_meant() {
    refused(
        &format!(
            "{HEADER}
[span.checkout]
description = \"A checkout\"
attributes = {{ route = \"string\" }}

[metric.latency]
description = \"Latency\"
type = \"histogram\"
unit = \"ms\"
attributes = {{ route = \"string\" }}

[query.slow]
description = \"Slow checkouts\"
measure = \"metric.latncy\"
aggregation = \"p95\"
window = \"5m\"

[query.by-kind]
description = \"Checkouts\"
measure = \"metric.checkout\"
aggregation = \"count\"
window = \"5m\"

[query.by-route]
description = \"Latency by route\"
measure = \"metric.latency\"
aggregation = \"avg\"
window = \"5m\"
group_by = [\"rute\"]

[dashboard.overview]
title = \"Overview\"

[[dashboard.overview.panel]]
title = \"Missing\"
query = \"query.slw\"
render = \"timeseries\"
"
        ),
        &[
            "bc-telemetry.toml:17:11: error[dangling-reference]: query `slow` measures \
             `metric.shop.latncy`, which is not declared; did you mean `metric.shop.latency`?",
            "bc-telemetry.toml:23:11: error[dangling-reference]: query `by-kind` measures \
             `metric.shop.checkout`, which is not declared; `checkout` is declared in `shop` as a \
             span",
            "bc-telemetry.toml:32:13: error[dangling-reference]: query `by-route` groups by \
             `rute`, which is not an attribute of `metric.shop.latency`; did you mean `route`?",
            "bc-telemetry.toml:39:9: error[dangling-reference]: panel 1 of dashboard `overview` \
             shows `query.shop.slw`, which is not declared; did you mean `query.shop.slow`?",
        ],
    );
}

/// A reference may name an item in another module; it is held to that module's
/// declarations exactly as a local one is held to its own.
#[test]
fn a_reference_across_modules_is_held_to_the_other_modules_declarations() {
    let scratch = two_modules(
        "
[query.far]
description = \"A module nobody declared\"
measure = \"metric.biling.charges\"
aggregation = \"count\"
window = \"1m\"

[query.missing]
description = \"An item the module does not declare\"
measure = \"metric.billing.charge\"
aggregation = \"count\"
window = \"1m\"

[query.grouped]
description = \"A field the error does not have\"
measure = \"error.billing.declined\"
aggregation = \"count\"
window = \"1m\"
group_by = [\"processor\"]

[dashboard.overview]
title = \"Overview\"

[[dashboard.overview.panel]]
title = \"A query the module does not declare\"
query = \"query.billing.charge-rte\"
render = \"timeseries\"

[[dashboard.overview.panel]]
title = \"A query in a module nobody declared\"
query = \"query.biling.charge-rate\"
render = \"stat\"
",
        BILLING_ITEMS,
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:7:11: error[dangling-reference]: query `far` measures \
             `metric.biling.charges`, but no module `biling` is declared; did you mean `billing`?",
            "bc-telemetry.toml:13:11: error[dangling-reference]: query `missing` measures \
             `metric.billing.charge`, which is not declared; did you mean \
             `metric.billing.charges`?",
            "bc-telemetry.toml:22:13: error[dangling-reference]: query `grouped` groups by \
             `processor`, which is not a field of `error.billing.declined`",
            "bc-telemetry.toml:29:9: error[dangling-reference]: panel 1 of dashboard `overview` \
             shows `query.billing.charge-rte`, which is not declared; did you mean \
             `query.billing.charge-rate`?",
            "bc-telemetry.toml:34:9: error[dangling-reference]: panel 2 of dashboard `overview` \
             shows `query.biling.charge-rate`, but no module `biling` is declared; did you mean \
             `billing`?",
        ],
    );
}

#[test]
fn a_reference_of_no_known_shape_is_refused_where_it_is_written() {
    refused(
        &format!(
            "{HEADER}
[query.too-deep]
description = \"Three dots\"
measure = \"metric.a.b.c\"
aggregation = \"count\"
window = \"1m\"

[query.no-such-kind]
description = \"A metric type is not an item kind\"
measure = \"gauge.cart_size\"
aggregation = \"count\"
window = \"1m\"

[query.no-module]
description = \"An empty module\"
measure = \"metric..cart_size\"
aggregation = \"count\"
window = \"1m\"

[dashboard.overview]
title = \"Overview\"

[[dashboard.overview.panel]]
title = \"A span is aggregated by a query, never shown raw\"
query = \"span.checkout\"
render = \"timeseries\"
"
        ),
        &[
            "bc-telemetry.toml:7:11: error[invalid-value]: `measure` on query `too-deep` must \
             name a declared item as `<kind>.<name>`, or as `<kind>.<module>.<name>` in another \
             module, where the kind is span, metric, log, error; got `metric.a.b.c`",
            "bc-telemetry.toml:13:11: error[invalid-value]: `measure` on query `no-such-kind` \
             must name a declared item as `<kind>.<name>`, or as `<kind>.<module>.<name>` in \
             another module, where the kind is span, metric, log, error; got `gauge.cart_size`",
            "bc-telemetry.toml:19:11: error[invalid-value]: `measure` on query `no-module` must \
             name a declared item as `<kind>.<name>`, or as `<kind>.<module>.<name>` in another \
             module, where the kind is span, metric, log, error; got `metric..cart_size`",
            "bc-telemetry.toml:28:9: error[invalid-value]: `query` on panel 1 of dashboard \
             `overview` must reference `query.<name>`, or `log.<name>` or `error.<name>` for a \
             logs panel — each as `<kind>.<module>.<name>` in another module; got `span.checkout`",
        ],
    );
}

/// The README's aggregation table: what an aggregation measures is fixed per
/// kind, and a pair outside it means nothing.
#[test]
fn an_aggregation_its_measure_does_not_admit_is_refused() {
    let query = |name: &str, measure: &str, aggregation: &str| {
        format!(
            "
[query.{name}]
description = \"A query\"
measure = \"{measure}\"
aggregation = \"{aggregation}\"
window = \"1m\"
"
        )
    };
    let definition = format!(
        "{HEADER}
[span.checkout]
description = \"A checkout\"

[metric.orders]
description = \"Orders\"
type = \"counter\"
unit = \"none\"

[metric.cart_size]
description = \"Cart size\"
type = \"gauge\"
unit = \"none\"

[metric.latency]
description = \"Latency\"
type = \"histogram\"
unit = \"ms\"

[log.failed]
description = \"A failure\"
level = \"error\"

[error.declined]
description = \"Declined\"
class = \"failure\"
code = 1
{}{}{}{}{}{}{}{}{}{}",
        query("counter-p95", "metric.orders", "p95"),
        query("counter-avg", "metric.orders", "avg"),
        query("gauge-sum", "metric.cart_size", "sum"),
        query("gauge-rate", "metric.cart_size", "rate"),
        query("log-sum", "log.failed", "sum"),
        query("error-avg", "error.declined", "avg"),
        // Every aggregation over a histogram and a span, and one from each
        // admitted family over the rest, is accepted.
        query("histogram-max", "metric.latency", "max"),
        query("span-p50", "span.checkout", "p50"),
        query("gauge-min", "metric.cart_size", "min"),
        query("error-rate", "error.declined", "rate"),
    );
    refused(
        &definition,
        &[
            "bc-telemetry.toml:35:15: error[invalid-aggregation]: query `counter-p95` takes \
             `p95` of `metric.shop.orders`, a counter, which admits count | sum | rate",
            "bc-telemetry.toml:41:15: error[invalid-aggregation]: query `counter-avg` takes \
             `avg` of `metric.shop.orders`, a counter, which admits count | sum | rate",
            "bc-telemetry.toml:47:15: error[invalid-aggregation]: query `gauge-sum` takes `sum` \
             of `metric.shop.cart_size`, a gauge, which admits count | avg | min | max | p50 | \
             p90 | p95 | p99",
            "bc-telemetry.toml:53:15: error[invalid-aggregation]: query `gauge-rate` takes \
             `rate` of `metric.shop.cart_size`, a gauge, which admits count | avg | min | max | \
             p50 | p90 | p95 | p99",
            "bc-telemetry.toml:59:15: error[invalid-aggregation]: query `log-sum` takes `sum` of \
             `log.shop.failed`, a log or an error, which admits count | rate",
            "bc-telemetry.toml:65:15: error[invalid-aggregation]: query `error-avg` takes `avg` \
             of `error.shop.declined`, a log or an error, which admits count | rate",
        ],
    );
}

/// Nothing reports a refusal, so it has no record: a query over one
/// could only ever be empty, and a logs panel could only ever show nothing.
#[test]
fn a_refusal_is_neither_measured_nor_shown() {
    let scratch = two_modules(
        "
[query.expiries]
description = \"Expired cards\"
measure = \"error.billing.expired\"
aggregation = \"count\"
window = \"1h\"

[dashboard.overview]
title = \"Overview\"

[[dashboard.overview.panel]]
title = \"Expired cards\"
query = \"error.billing.expired\"
render = \"logs\"

[[dashboard.overview.panel]]
title = \"Declined charges\"
query = \"error.billing.declined\"
render = \"logs\"
",
        BILLING_ITEMS,
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.toml:7:11: error[invalid-value]: query `expiries` measures \
             `error.billing.expired`, a refusal; nothing reports a refusal, so there is no \
             record to measure",
            "bc-telemetry.toml:16:9: error[invalid-value]: panel 1 of dashboard `overview` shows \
             `error.billing.expired`, a refusal; nothing reports a refusal, so there is no \
             record to show",
        ],
    );
}

#[test]
fn a_panel_renders_what_its_source_is() {
    refused(
        &format!(
            "{HEADER}
[metric.orders]
description = \"Orders\"
type = \"counter\"
unit = \"none\"

[log.failure]
description = \"A failure\"
level = \"error\"

[error.declined]
description = \"Declined\"
class = \"failure\"
code = 1

[query.order-count]
description = \"Orders\"
measure = \"metric.orders\"
aggregation = \"count\"
window = \"1h\"

[dashboard.overview]
title = \"Overview\"

[[dashboard.overview.panel]]
title = \"A query rendered as logs\"
query = \"query.order-count\"
render = \"logs\"

[[dashboard.overview.panel]]
title = \"A log rendered as a series\"
query = \"log.failure\"
render = \"timeseries\"

[[dashboard.overview.panel]]
title = \"An error rendered as a table\"
query = \"error.declined\"
render = \"table\"
"
        ),
        &[
            "bc-telemetry.toml:30:9: error[invalid-value]: panel 1 of dashboard `overview` \
             renders `logs`, which shows a log's or an error's records; reference one as \
             `log.<name>` or `error.<name>`",
            "bc-telemetry.toml:35:9: error[invalid-value]: panel 2 of dashboard `overview` \
             renders `timeseries`, which shows a query's result; reference one as \
             `query.<name>`, or render the records with `logs`",
            "bc-telemetry.toml:40:9: error[invalid-value]: panel 3 of dashboard `overview` \
             renders `table`, which shows a query's result; reference one as `query.<name>`, or \
             render the records with `logs`",
        ],
    );
}

/// One mistake, one finding: a declaration refused for its own body is still
/// declared, so what references it — from its own module or another — is not
/// refused a second time as dangling.
#[test]
fn a_reference_to_a_refused_declaration_is_not_reported_again() {
    let scratch = two_modules(
        "
[query.far]
description = \"Across modules\"
measure = \"metric.billing.latency\"
aggregation = \"p95\"
window = \"5m\"
group_by = [\"route\"]
",
        "
[metric.latency]
description = \"Latency\"
type = \"timer\"
unit = \"ms\"

[query.slow]
description = \"Slow\"
measure = \"metric.latency\"
aggregation = \"p95\"
window = \"5m\"
group_by = [\"route\"]
",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "billing/bc-telemetry.toml:7:8: error[invalid-value]: `type` on metric `latency` must \
             be one of counter | gauge | histogram; got `timer`",
        ],
    );
}

#[test]
fn a_generate_key_a_later_release_implements_is_refused_as_early_not_misspelled() {
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\n\n\
             [generate.routing]\nexporter = \"postgresql\"\n"
        ),
        &[&format!(
            "bc-telemetry.toml:8:11: error[not-yet-supported]: `routing` on `[generate]` is not \
             implemented by bc-telemetry-gen {} yet",
            env!("CARGO_PKG_VERSION")
        )],
    );
}

/// `languages` names the adapters every module's code is generated by, from
/// the ones this generator has, each once.
#[test]
fn a_language_is_one_this_generator_has_an_adapter_for_named_once() {
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\n\
             languages = [\"cpp\", \"c++\", \"go\", \"cpp\"]\n"
        ),
        &[
            "bc-telemetry.toml:7:21: error[invalid-value]: `languages` on `[generate]` names \
             `c++`; this generator has an adapter for cpp | rust",
            "bc-telemetry.toml:7:28: error[invalid-value]: `languages` on `[generate]` names \
             `go`; this generator has an adapter for cpp | rust",
            "bc-telemetry.toml:7:34: error[duplicate-entry]: `cpp` appears twice in `languages`",
        ],
    );
}

/// Rust refuses its own keywords, and the methods a span's handles already
/// have; it takes no location parameter, since Rust captures the caller with
/// `#[track_caller]`, so a field may be called `location` there.
#[test]
fn a_name_rust_cannot_use_is_refused_where_it_is_declared() {
    refused(
        &format!(
            "{HEADER}
[generate]
output = \"generated\"
languages = [\"rust\"]

[span.move]
description = \"A move\"
attributes = {{ ref = \"string\" }}
events = [\"of\", \"accepted\"]

[error.declined]
description = \"Declined\"
class = \"failure\"
code = 1
fields = {{ location = \"string\", type = \"string\" }}
"
        ),
        &[
            "bc-telemetry.toml:9:7: error[reserved-name]: span `move` cannot be named so in Rust, \
             which `languages` generates; pick another name",
            "bc-telemetry.toml:11:16: error[reserved-name]: attribute `ref` in `attributes` on span \
             `move` cannot be named so in Rust, which `languages` generates; pick another name",
            "bc-telemetry.toml:12:11: error[reserved-name]: event `of` in `events` on span \
             `move` cannot be named so in Rust, which `languages` generates; pick another name",
            "bc-telemetry.toml:18:33: error[reserved-name]: field `type` in `fields` on error \
             `declined` cannot be named so in Rust, which `languages` generates; pick another name",
        ],
    );
}

/// A name becomes a function, a type, a parameter or a method in each language
/// `languages` names, so one that language cannot use is refused here, at its
/// place in the definition, rather than by the consumer's compiler.
#[test]
fn a_name_a_configured_language_cannot_use_is_refused_where_it_is_declared() {
    refused(
        &format!(
            "{HEADER}
[generate]
output = \"generated\"
languages = [\"cpp\"]

[span.delete]
description = \"A deletion\"
attributes = {{ default = \"string\" }}
events = [\"guard\", \"accepted\"]

[error.declined]
description = \"Declined\"
class = \"failure\"
code = 1
fields = {{ location = \"string\" }}
"
        ),
        &[
            "bc-telemetry.toml:9:7: error[reserved-name]: span `delete` cannot be named so in \
             C++, which `languages` generates; pick another name",
            "bc-telemetry.toml:11:16: error[reserved-name]: attribute `default` in `attributes` on \
             span `delete` cannot be named so in C++, which `languages` generates; pick another \
             name",
            "bc-telemetry.toml:12:11: error[reserved-name]: event `guard` in `events` on span \
             `delete` cannot be named so in C++, which `languages` generates; pick another name",
            "bc-telemetry.toml:18:12: error[reserved-name]: field `location` in `fields` on error \
             `declined` cannot be named so in C++, which `languages` generates; pick another name",
        ],
    );
    // The same names, with no language that cannot use them, are names like any other.
    accepted(&format!(
        "{HEADER}
[span.delete]
description = \"A deletion\"
attributes = {{ default = \"string\" }}
events = [\"guard\", \"accepted\"]
"
    ));
}

/// C++ nests a span's handle classes inside the span's own class, and declares
/// one shared class in each header's `errors` namespace: a span named for a
/// handle, or an error named for that class, would be a class holding its own
/// name, so each is refused where it is declared.
#[test]
fn a_cpp_item_named_for_a_class_it_would_hold_is_refused() {
    refused(
        &format!(
            "{HEADER}
[generate]
output = \"generated\"
languages = [\"cpp\"]

[span.scope]
description = \"A scope\"

[span.running]
description = \"A run\"

[error.not_this_error]
description = \"Not this one\"
class = \"failure\"
code = 1
"
        ),
        &[
            "bc-telemetry.toml:9:7: error[reserved-name]: span `scope` cannot be named so in \
             C++, which `languages` generates; pick another name",
            "bc-telemetry.toml:12:7: error[reserved-name]: span `running` cannot be named so in \
             C++, which `languages` generates; pick another name",
            "bc-telemetry.toml:15:8: error[reserved-name]: error `not_this_error` cannot be named \
             so in C++, which `languages` generates; pick another name",
        ],
    );
}

/// `dialects` chooses the storage engines a layout is generated for, from the
/// ones this generator has a layout for.
#[test]
fn a_dialect_is_one_this_generator_has_a_layout_for_named_once() {
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\n\
             dialects = [\"postgresql\", \"postgres\", \"influxql\", \"postgresql\"]\n"
        ),
        &[
            "bc-telemetry.toml:7:27: error[invalid-value]: `dialects` on `[generate]` names \
             `postgres`; this generator has a layout for postgresql; did you mean `postgresql`?",
            "bc-telemetry.toml:7:39: error[invalid-value]: `dialects` on `[generate]` names \
             `influxql`; this generator has a layout for postgresql",
            "bc-telemetry.toml:7:51: error[duplicate-entry]: `postgresql` appears twice in `dialects`",
        ],
    );
    refused(
        &format!("{HEADER}\n[generate]\noutput = \"generated\"\ndialects = \"postgresql\"\n"),
        &[
            "bc-telemetry.toml:7:12: error[type-mismatch]: `dialects` on `[generate]` must be an \
             array, got a string",
        ],
    );
}

/// `exports` chooses where dashboards go, and an export is configured exactly
/// when it is named: a table for an export nobody named is a forgotten export
/// or a stale table, and either is refused rather than ignored. The Grafana
/// export reads the PostgreSQL layout, so it needs that dialect.
#[test]
fn an_export_is_named_configured_and_given_its_dialect_together() {
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\ndialects = [\"postgresql\"]\n\
             exports = [\"grafana\", \"kibana\", \"grafana\"]\n\n[generate.grafana]\n\
             datasource = \"telemetry\"\n"
        ),
        &[
            "bc-telemetry.toml:8:23: error[invalid-value]: `exports` on `[generate]` names \
             `kibana`; this generator exports to grafana",
            "bc-telemetry.toml:8:33: error[duplicate-entry]: `grafana` appears twice in `exports`",
        ],
    );
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\ndialects = [\"postgresql\"]\n\
             exports = [\"grafana\"]\n"
        ),
        &[
            "bc-telemetry.toml:8:12: error[export-mismatch]: `exports` names `grafana`, so \
           `[generate.grafana]` must say which Grafana data source every panel reads: \
           `datasource = \"<uid>\"`",
        ],
    );
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\n\n[generate.grafana]\n\
             datasource = \"telemetry\"\n"
        ),
        &[
            "bc-telemetry.toml:8:11: error[export-mismatch]: `[generate.grafana]` configures the \
           `grafana` export, which `exports` does not name; add it to `exports` or remove the \
           table",
        ],
    );
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\nexports = [\"grafana\"]\n\n\
             [generate.grafana]\ndatasource = \"telemetry\"\n"
        ),
        &[
            "bc-telemetry.toml:7:12: error[export-mismatch]: the `grafana` export reads the \
           PostgreSQL layout, so `dialects` must name `postgresql`",
        ],
    );
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\ndialects = [\"postgresql\"]\n\
             exports = [\"grafana\"]\n\n[generate.grafana]\ndatasource = \" \"\n\
             timeout = 5\n"
        ),
        &[
            "bc-telemetry.toml:11:14: error[invalid-value]: `datasource` on `[generate.grafana]` \
             must not be empty",
            "bc-telemetry.toml:12:1: error[unknown-field]: unknown field `timeout` on \
             `[generate.grafana]`",
        ],
    );
}

/// An item's view carries a column per attribute beside the columns every view
/// of its kind has, so an attribute may not take one of those names —
/// and which names those are depends on the kind.
#[test]
fn an_attribute_may_not_take_the_name_of_a_column_its_view_already_has() {
    refused(
        &format!(
            "{HEADER}
[span.checkout]
description = \"A checkout\"
attributes = {{ status = \"string\", value = \"int\", customer = \"string\" }}

[metric.requests]
description = \"Requests\"
type = \"counter\"
unit = \"none\"
attributes = {{ status = \"int\", value = \"int\", body = \"string\" }}

[log.failed]
description = \"A failure\"
level = \"error\"
fields = {{ severity = \"string\", status = \"int\" }}

[error.declined]
description = \"Declined\"
class = \"refusal\"
code = 1
fields = {{ body = \"string\", duration_ms = \"int\", time = \"string\" }}
"
        ),
        &[
            "bc-telemetry.toml:7:16: error[reserved-name]: attribute `status` in `attributes` on \
             span `checkout` has the name of a column every span view carries; its own view \
             could not hold both, so pick another name",
            "bc-telemetry.toml:7:50: error[reserved-name]: attribute `customer` in `attributes` on \
             span `checkout` has the name of a column every span view carries; its own view \
             could not hold both, so pick another name",
            "bc-telemetry.toml:13:32: error[reserved-name]: attribute `value` in `attributes` on \
             metric `requests` has the name of a column every metric view carries; its own view \
             could not hold both, so pick another name",
            "bc-telemetry.toml:18:12: error[reserved-name]: field `severity` in `fields` on log \
             `failed` has the name of a column every log view carries; its own view could not \
             hold both, so pick another name",
            "bc-telemetry.toml:24:12: error[reserved-name]: field `body` in `fields` on error \
             `declined` has the name of a column every error view carries; its own view could not \
             hold both, so pick another name",
            "bc-telemetry.toml:24:50: error[reserved-name]: field `time` in `fields` on error \
             `declined` has the name of a column every error view carries; its own view could not \
             hold both, so pick another name",
        ],
    );
}

/// A log's or an error's records statement names the severity `level`, as
/// Grafana's logs panel reads it, so a field of either may not take that
/// name — and a span's or a metric's attribute may, having no such statement.
#[test]
fn a_field_may_not_take_the_name_its_records_statement_gives_the_severity() {
    refused(
        &format!(
            "{HEADER}
[span.checkout]
description = \"A checkout\"
attributes = {{ level = \"string\" }}

[metric.requests]
description = \"Requests\"
type = \"counter\"
unit = \"none\"
attributes = {{ level = \"string\" }}

[log.failed]
description = \"A failure\"
level = \"error\"
fields = {{ level = \"string\" }}

[error.declined]
description = \"Declined\"
class = \"refusal\"
code = 1
fields = {{ level = \"int\" }}
"
        ),
        &[
            "bc-telemetry.toml:18:12: error[reserved-name]: field `level` in `fields` on log \
             `failed` has the name its records statement gives the severity, which Grafana's \
             logs panel reads it by; pick another name",
            "bc-telemetry.toml:24:12: error[reserved-name]: field `level` in `fields` on error \
             `declined` has the name its records statement gives the severity, which Grafana's \
             logs panel reads it by; pick another name",
        ],
    );
}

/// The generator fetches nothing: it reads directories on disk and uses no
/// network. The key that once let it fetch is not one it will ever implement,
/// so it is unknown, not early.
#[test]
fn the_deleted_source_key_is_an_unknown_one() {
    refused(
        &format!(
            "{HEADER}\n[generate]\noutput = \"generated\"\n\n[generate.source]\ngit = \"example\"\n"
        ),
        &["bc-telemetry.toml:8:11: error[unknown-field]: unknown field `source` on `[generate]`"],
    );
}

#[test]
fn an_output_outside_the_definitions_directory_is_refused() {
    refused(
        &format!("{HEADER}\n[generate]\noutput = \"../generated\"\n"),
        &[
            "bc-telemetry.toml:6:10: error[invalid-output-path]: `output` `../generated` must stay \
           inside the directory it is taken from: no `..`, no leading `/` and no empty segment",
        ],
    );
}

/// `output` is recorded in the manifest and compared across runs, so it has one
/// spelling.
#[test]
fn an_output_with_a_dot_segment_is_refused() {
    refused(
        &format!("{HEADER}\n[generate]\noutput = \"./generated\"\n"),
        &[
            "bc-telemetry.toml:6:10: error[invalid-output-path]: `output` `./generated` holds a \
             `.` segment; write the path without it",
        ],
    );
}

#[test]
fn the_retired_list_is_a_list_of_module_names_each_once() {
    refused(
        &format!("{HEADER}\n[retired]\n"),
        &[
            "bc-telemetry.toml:5:2: error[missing-field]: `[retired]` is missing its required \
           `modules`",
        ],
    );
    refused(
        &format!("{HEADER}\n[retired]\nmodules = \"billing\"\n"),
        &[
            "bc-telemetry.toml:6:11: error[type-mismatch]: `modules` on `[retired]` must be an \
           array, got a string",
        ],
    );
    refused(
        &format!(
            "{HEADER}\n[retired]\nmodules = [\"billing\", \"old-billing\", \"billing\", \"shop\"]\n"
        ),
        &[
            "bc-telemetry.toml:6:23: error[invalid-name]: retired module name `old-billing` is \
             not snake_case: lowercase letters and digits in `_`-separated words, starting with a \
             letter",
            "bc-telemetry.toml:6:38: error[duplicate-entry]: module `billing` appears twice in \
             `[retired]`",
            "bc-telemetry.toml:6:49: error[retired-module-declared]: `[retired]` names `shop`, \
             which this tree still declares; a module is retired once its definition is gone, \
             and its name is never declared again",
        ],
    );
}

#[test]
fn a_toml_syntax_error_is_refused_where_the_parser_stopped() {
    refused(
        "[module]\nname = \"shop\nschema_version = 1\n",
        &[
            "bc-telemetry.toml:2:13: error[toml-syntax]: TOML syntax error: invalid basic string, \
           expected `\"`",
        ],
    );
}

#[test]
fn json_is_held_to_the_same_rules_with_its_own_positions() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.json",
        "{\n  \"module\": { \"name\": \"shop\", \"schema_version\": null },\n  \"span\": {\n    \
         \"checkout\": { \"description\": \"A checkout\", \"atributes\": {} }\n  }\n}\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.json:2:49: error[type-mismatch]: `schema_version` on `[module]` must be \
             an integer, got null",
            "bc-telemetry.json:4:48: error[unknown-field]: unknown field `atributes` on span \
             `checkout`; did you mean `attributes`?",
        ],
    );
}

#[test]
fn json_refuses_a_duplicate_key_where_most_parsers_keep_one() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.json",
        "{\"module\": {\"name\": \"shop\", \"name\": \"other\", \"schema_version\": 1}}\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &["bc-telemetry.json:1:29: error[json-syntax]: JSON syntax error: duplicate object key"],
    );
}

#[test]
fn a_json_definition_is_one_object() {
    let scratch = Scratch::new();
    scratch.write("bc-telemetry.json", "[1, 2]\n");
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.json:1:1: error[type-mismatch]: a JSON definition file must be one \
           object, got an array",
        ],
    );
}

#[test]
fn a_definition_with_no_module_or_an_unknown_table_is_refused_at_the_top() {
    refused(
        "[spans.checkout]\ndescription = \"A checkout\"\n",
        &[
            "bc-telemetry.toml:1:1: error[missing-field]: the definition file `bc-telemetry.toml` \
             is missing its required `module`",
            "bc-telemetry.toml:1:2: error[unknown-field]: unknown field `spans` on the definition \
             file `bc-telemetry.toml`; did you mean `span`?",
        ],
    );
}

#[test]
fn a_json_boolean_is_described_as_one() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.json",
        "{\"module\": {\"name\": \"shop\", \"schema_version\": true}}\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.json:1:47: error[type-mismatch]: `schema_version` on `[module]` must be \
           an integer, got a boolean",
        ],
    );
}

/// A float where a definition wants an integer is described as one — and a
/// number past the float range, which JSON's grammar admits, as exactly that.
#[test]
fn a_json_float_and_an_out_of_range_number_are_described_as_what_they_are() {
    let scratch = Scratch::new();
    scratch.write(
        "bc-telemetry.json",
        "{\"module\": {\"name\": \"shop\", \"schema_version\": 1.5},\n \
         \"span\": {\"checkout\": {\"description\": \"A checkout\", \"events\": [1e400]}}}\n",
    );
    assert_findings(
        &run_in(scratch.path(), &[]),
        &[
            "bc-telemetry.json:1:47: error[type-mismatch]: `schema_version` on `[module]` must be \
             an integer, got a float",
            "bc-telemetry.json:2:64: error[type-mismatch]: every element of `events` on span \
             `checkout` must be a string, got a number outside the 64-bit float range",
        ],
    );
}
