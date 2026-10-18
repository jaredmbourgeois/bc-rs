//! Ported from `libs/platform/error_telemetry/test/error_telemetry_test.cpp`.

mod support;

use bc_error::{
    AnyError, AnyErrorKind, error_facts, with_corrupt_row_fact, with_refusal_fact,
    with_sqlstate_fact,
};
use bc_telemetry::{
    AttributeValue, Attributes, Parent, Severity, SpanKind, SpanStatus, StatusCode, dropped_count,
    ends_by_result, error_identity_attributes, record_error_telemetry, report_degraded,
    report_error, running_span, start_span,
};

use support::{AssertionCapture, with_in_memory};

const STORE_FAULT: AnyErrorKind = AnyErrorKind::new("store", 3);

fn attribute<'a>(attributes: &'a Attributes, key: &str) -> Option<&'a AttributeValue> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

fn text_attribute(attributes: &Attributes, key: &str) -> String {
    match attribute(attributes, key) {
        Some(AttributeValue::String(text)) => text.clone(),
        Some(_) => "<not a string>".to_owned(),
        None => "<absent>".to_owned(),
    }
}

/// Records `error` inside a span of its own and hands back that span's
/// attributes on its one event.
fn recorded_event_attributes(
    exporter: &bc_telemetry::InMemoryExporter,
    error: &AnyError,
) -> Attributes {
    {
        let _guard = start_span(
            Parent::current(),
            "test",
            SpanKind::Internal,
            Attributes::new(),
        );
        record_error_telemetry(error);
    }
    let spans = exporter.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].events.len(), 1);
    assert_eq!(spans[0].events[0].name, "error");
    spans[0].events[0].attributes.clone()
}

#[test]
fn records_a_span_event_with_category_kind_and_location() {
    with_in_memory(|exporter| {
        let error = AnyError::from_detail(STORE_FAULT, "Reader::method");
        let attributes = recorded_event_attributes(exporter, &error);

        assert_eq!(text_attribute(&attributes, "error.category"), "store");
        assert_eq!(
            attribute(&attributes, "error.kind"),
            Some(&AttributeValue::Int64(3))
        );
        assert!(
            text_attribute(&attributes, "error.location").contains(file!()),
            "{attributes:?}"
        );
        assert_eq!(
            text_attribute(&attributes, "error.meta.detail"),
            "\"Reader::method\""
        );
    });
}

// Each fact is named once, under its own attribute — never again as
// `error.meta.<key>`.
#[test]
fn each_fact_is_recorded_once_under_its_own_name() {
    with_in_memory(|exporter| {
        let error = with_refusal_fact(with_corrupt_row_fact(with_sqlstate_fact(
            AnyError::from_detail(STORE_FAULT, "x"),
            "57P01",
        )));
        let attributes = recorded_event_attributes(exporter, &error);

        assert_eq!(
            attribute(&attributes, "error.refused"),
            Some(&AttributeValue::Boolean(true))
        );
        assert_eq!(
            attribute(&attributes, "error.corrupt_row"),
            Some(&AttributeValue::Boolean(true))
        );
        assert_eq!(
            text_attribute(&attributes, "db.response.status_code"),
            "57P01"
        );
        for key in error_facts::FACT_KEYS {
            assert_eq!(
                attribute(&attributes, &format!("error.meta.{key}")),
                None,
                "{key}"
            );
        }
    });
}

#[test]
fn includes_the_context_chain() {
    with_in_memory(|exporter| {
        let error = AnyError::from_kind(AnyErrorKind::new("store", 1))
            .with_context("Inner::call")
            .with_context("Outer::call");
        let attributes = recorded_event_attributes(exporter, &error);
        assert_eq!(
            text_attribute(&attributes, "error.context_chain"),
            "Outer::call > Inner::call"
        );
    });
}

// A 5xx must be explainable from the log alone, and the log must not carry the
// detail, which is free text some store composed — the fixture carries a
// customer's email the way a database's message once did.
#[test]
fn report_error_logs_what_failed_and_its_sqlstate_but_never_the_detail() {
    with_in_memory(|exporter| {
        let detail = "duplicate key value violates unique constraint \"customers_email_key\"\n\
                      DETAIL:  Key (email)=(shopper@example.test) already exists.";
        let error = with_sqlstate_fact(AnyError::from_detail(STORE_FAULT, detail), "23505")
            .with_context("customers.create");
        report_error("storefront store fault", &error, Attributes::new());

        let logs = exporter.logs();
        assert_eq!(logs.len(), 1);
        let log = &logs[0];
        assert_eq!(log.severity, Severity::Error);
        assert_eq!(log.body, "storefront store fault");
        assert_eq!(text_attribute(&log.attributes, "error.category"), "store");
        assert_eq!(
            text_attribute(&log.attributes, "error.context_chain"),
            "customers.create"
        );
        assert_eq!(
            text_attribute(&log.attributes, "db.response.status_code"),
            "23505"
        );
        assert!(attribute(&log.attributes, "error.location").is_some());
        for (key, value) in &log.attributes {
            assert!(!key.starts_with("error.meta."), "{key}");
            if let AttributeValue::String(text) = value {
                assert!(!text.contains("shopper@example.test"), "{key}");
            }
        }
    });
}

// Reporting adds the log; it does not trade the span event away for it.
#[test]
fn report_error_still_records_the_span_event() {
    with_in_memory(|exporter| {
        {
            let _guard = start_span(
                Parent::current(),
                "test",
                SpanKind::Internal,
                Attributes::new(),
            );
            report_error(
                "storefront store fault",
                &AnyError::from_detail(STORE_FAULT, "Reader::method"),
                Attributes::new(),
            );
        }

        let spans = exporter.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].events.len(), 1);
        assert_eq!(spans[0].events[0].name, "error");
        assert_eq!(spans[0].status, StatusCode::Error);
        assert_eq!(spans[0].status_message, "store:3");
        assert_eq!(exporter.logs().len(), 1);
        // The log goes under the span the error was met in.
        assert_eq!(exporter.logs()[0].span_context, Some(spans[0].context));
    });
}

// An error is reported wherever it is met, and most paths open no span: a
// missing span is the ordinary case, so nothing is counted as dropped and
// nothing reaches the assertion handler.
#[test]
fn recording_an_error_with_no_span_open_is_not_a_misuse() {
    with_in_memory(|exporter| {
        let assertions = AssertionCapture::install();
        let error = AnyError::from_detail(STORE_FAULT, "Reader::method");
        record_error_telemetry(&error);
        report_error("storefront store fault", &error, Attributes::new());
        report_degraded("rates unavailable", &error, Attributes::new());

        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
        assert_eq!(dropped_count(), 0);
        // The logs are the half that reaches somewhere without a span.
        assert_eq!(exporter.logs().len(), 2);
        assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
    });
}

// A body that returns a `Result` ends its span ok or as an error with no
// message: the error's text is free text, and a status message reaches every
// exporter. The C++ makes this the result type's default; here it is the end
// strategy `ends_by_result`, passed.
#[test]
fn a_result_ends_its_span_ok_or_as_an_error_with_no_message() {
    with_in_memory(|exporter| {
        let loaded = running_span(
            Parent::current(),
            "load",
            SpanKind::Internal,
            Attributes::new(),
            ends_by_result,
            |_span| Ok::<i32, AnyError>(7),
        );
        assert_eq!(loaded, Ok(7));

        let refused = running_span(
            Parent::current(),
            "save",
            SpanKind::Internal,
            Attributes::new(),
            ends_by_result,
            |_span| {
                Err::<(), AnyError>(AnyError::from_detail(
                    STORE_FAULT,
                    "Key (email)=(shopper@example.test) already exists.",
                ))
            },
        );
        assert!(refused.is_err());

        let spans = exporter.spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].name, "load");
        assert_eq!(spans[0].status, StatusCode::Ok);
        assert_eq!(spans[1].name, "save");
        assert_eq!(spans[1].status, StatusCode::Error);
        assert_eq!(spans[1].status_message, "");
    });
}

// A span that should say more names its own status: the strategy chooses text
// that is safe to export, here the error's category rather than its detail.
#[test]
fn an_end_strategy_names_a_status_of_its_own_for_a_result() {
    with_in_memory(|exporter| {
        let refused = running_span(
            Parent::current(),
            "save",
            SpanKind::Internal,
            Attributes::new(),
            |result: &Result<(), AnyError>| match result {
                Ok(()) => SpanStatus::ok(),
                Err(error) => SpanStatus::error(error.category()),
            },
            |_span| Err(AnyError::from_detail(STORE_FAULT, "free text")),
        );
        assert!(refused.is_err());

        let spans = exporter.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].status, StatusCode::Error);
        assert_eq!(spans[0].status_message, "store");
    });
}

// A degrade the code answered past is logged too — at warn, because the request
// still got an answer — with the same identity and nothing from the metadata.
// The call site's context rides after the identity.
#[test]
fn report_degraded_logs_at_warn_with_the_identity_but_never_the_detail() {
    with_in_memory(|exporter| {
        let error = with_sqlstate_fact(
            AnyError::from_detail(AnyErrorKind::new("store", 4), "sentinel-detail-text"),
            "40P01",
        )
        .with_context("shares.list");
        report_degraded(
            "streak share list degraded",
            &error,
            vec![("share.surface".to_owned(), AttributeValue::from("streak"))],
        );

        let logs = exporter.logs();
        assert_eq!(logs.len(), 1);
        let log = &logs[0];
        assert_eq!(log.severity, Severity::Warn);
        assert_eq!(log.body, "streak share list degraded");
        assert_eq!(text_attribute(&log.attributes, "error.category"), "store");
        assert_eq!(
            text_attribute(&log.attributes, "error.context_chain"),
            "shares.list"
        );
        assert_eq!(
            text_attribute(&log.attributes, "db.response.status_code"),
            "40P01"
        );
        assert_eq!(
            log.attributes.last(),
            Some(&("share.surface".to_owned(), AttributeValue::from("streak")))
        );
        for (key, value) in &log.attributes {
            assert!(!key.starts_with("error.meta."), "{key}");
            if let AttributeValue::String(text) = value {
                assert!(!text.contains("sentinel-detail-text"), "{key}");
            }
        }
    });
}

#[test]
fn an_error_with_no_sqlstate_carries_no_status_code() {
    let error = AnyError::from_detail(AnyErrorKind::new("store", 1), "connection refused");
    let attributes = error_identity_attributes(&error);
    assert_eq!(attribute(&attributes, "db.response.status_code"), None);
    // And no flag it does not carry.
    for flag in [
        "error.corrupt_row",
        "error.commit_outcome_unknown",
        "error.invariant_breach",
        "error.refused",
    ] {
        assert_eq!(attribute(&attributes, flag), None, "{flag}");
    }
}
