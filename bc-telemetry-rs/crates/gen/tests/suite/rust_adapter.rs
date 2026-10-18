//! The Rust adapter's smoke: an adapter ships with a compile-and-emit run
//! through the real runtime, since generated code that only matches its golden
//! has never been shown to compile. The canonical tree's
//! generated modules are compiled into this suite the way a consumer brings
//! them in, and every kind of item is driven through bc-telemetry-rs into an
//! in-memory exporter. `canonical.rs`'s freshness test is what keeps the files
//! compiled here the generator's current output.

use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use bc_error::is_refusal;
use bc_telemetry::{
    AttributeValue, Attributes, InMemoryExporter, MetricKind, Parent, PipelineBuilder, Severity,
    SpanContext, SpanKind, SpanStatus, StatusCode, current_span_context, ends_ok,
};

#[allow(
    dead_code,
    unreachable_pub,
    reason = "a consumer exports the module and uses what it needs; the smoke drives every kind of item, not every item"
)]
mod store_server {
    include!("../fixtures/canonical/generated/telemetry/rust/store_server_telemetry.rs");
}

#[allow(
    dead_code,
    unreachable_pub,
    reason = "a consumer exports the module and uses what it needs; the smoke drives every kind of item, not every item"
)]
mod payments {
    include!(
        "../fixtures/canonical/services/payments/generated/telemetry/rust/payments_telemetry.rs"
    );
}

#[allow(
    dead_code,
    unreachable_pub,
    reason = "a consumer exports the module and uses what it needs; the smoke drives every kind of item, not every item"
)]
mod ledger {
    include!(
        "../fixtures/canonical/services/payments/ledger/generated/telemetry/rust/ledger_telemetry.rs"
    );
}

/// The pipeline is process-wide; the threaded harness runs these cases at once.
static ONE_PIPELINE: Mutex<()> = Mutex::new(());

fn with_pipeline(body: impl FnOnce(&InMemoryExporter)) {
    let _one: MutexGuard<'_, ()> = ONE_PIPELINE.lock().unwrap_or_else(PoisonError::into_inner);
    let exporter = Arc::new(InMemoryExporter::new());
    let _scope = PipelineBuilder::new(exporter.clone()).install();
    body(&exporter);
}

fn attribute<'a>(attributes: &'a Attributes, key: &str) -> Option<&'a AttributeValue> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

#[test]
fn each_span_verb_records_what_its_declaration_says() {
    with_pipeline(|exporter| {
        // `run`: the body gets the running span, records an event on it, and
        // its result decides the status.
        let answer = store_server::spans::checkout("cart-1", 3).run(
            Parent::none(),
            |result: &i32| {
                if *result == 7 {
                    SpanStatus::ok()
                } else {
                    SpanStatus::error("")
                }
            },
            |span| {
                span.payment_authorized();
                7
            },
        );
        assert_eq!(answer, 7);

        // `start`: the runtime's own guard, current in this scope until it ends.
        {
            let guard =
                store_server::spans::refund(12.5, "order-1", true, 250).start(Parent::none());
            assert_eq!(current_span_context(), Some(guard.span().context()));
            guard.end(SpanStatus::ok());
        }

        // `open`: current nowhere, ended by its holder.
        let held = payments::spans::authorize("acme").open(Parent::none());
        assert_eq!(current_span_context(), None);
        held.processor_called();
        held.into_span().end(SpanStatus::ok());

        let spans = exporter.spans();
        assert_eq!(spans.len(), 3);

        let checkout = &spans[0];
        assert_eq!(checkout.name, "store_server.checkout");
        assert_eq!(checkout.kind, SpanKind::Server);
        assert_eq!(checkout.status, StatusCode::Ok);
        assert_eq!(
            attribute(&checkout.attributes, "cart_id"),
            Some(&AttributeValue::from("cart-1"))
        );
        assert_eq!(
            attribute(&checkout.attributes, "item_count"),
            Some(&AttributeValue::Int64(3))
        );
        assert_eq!(
            attribute(&checkout.attributes, "bc.telemetry.schema_version"),
            Some(&AttributeValue::Int64(store_server::SCHEMA_VERSION))
        );
        let events: Vec<&str> = checkout
            .events
            .iter()
            .map(|event| event.name.as_str())
            .collect();
        assert_eq!(events, ["payment_authorized"]);

        let refund = &spans[1];
        assert_eq!(refund.name, "store_server.refund");
        assert_eq!(refund.kind, SpanKind::Internal, "a span that names no kind");
        assert_eq!(
            attribute(&refund.attributes, "partial"),
            Some(&AttributeValue::Boolean(true))
        );
        assert_eq!(refund.status, StatusCode::Ok);

        let authorize = &spans[2];
        assert_eq!(authorize.name, "payments.authorize");
        assert_eq!(authorize.kind, SpanKind::Client);
        assert_eq!(authorize.events.len(), 1);
        assert_eq!(authorize.events[0].name, "processor_called");
    });
}

// A `start`ed span's events, recorded through the view its guard lends.
#[test]
fn a_started_spans_events_are_recorded_through_its_guard() {
    with_pipeline(|exporter| {
        let guard = payments::spans::authorize("acme").start(Parent::none());
        payments::spans::authorize::Running::of(guard.span()).processor_called();
        guard.end(SpanStatus::ok());

        let spans = exporter.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "payments.authorize");
        assert_eq!(spans[0].events.len(), 1);
        assert_eq!(spans[0].events[0].name, "processor_called");
    });
}

/// The lint's witness: a `start`ed span's guard is
/// the runtime's own `SpanGuard`, so `clippy.toml`'s
/// `await-holding-invalid-types` sees one held across an `.await` — and this
/// expectation goes unfulfilled, and clippy red, the day `start` returns a
/// type that hides the guard, as the `Scope` that wrapped it did.
#[expect(
    clippy::await_holding_invalid_type,
    reason = "the witness: the lint must see a generated span's guard held across an `.await`"
)]
async fn holds_a_started_span_across_an_await() -> SpanContext {
    let guard = payments::spans::authorize("acme").start(Parent::none());
    std::future::ready(()).await;
    let context = guard.span().context();
    guard.end(SpanStatus::ok());
    context
}

#[test]
fn the_lint_witness_runs_what_it_compiles() {
    with_pipeline(|exporter| {
        let mut task = pin!(holds_a_started_span_across_an_await());
        let polled = task.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        let spans = exporter.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(polled, Poll::Ready(spans[0].context));
    });
}

// `run_async`: the span is current for each poll of the body, and ends by its
// output.
#[test]
fn a_span_runs_an_asynchronous_body() {
    with_pipeline(|exporter| {
        let mut task = pin!(payments::spans::authorize("acme").run_async(
            Parent::none(),
            ends_ok,
            async { current_span_context() }
        ));
        let polled = task.as_mut().poll(&mut Context::from_waker(Waker::noop()));

        let spans = exporter.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "payments.authorize");
        assert_eq!(spans[0].status, StatusCode::Ok);
        assert_eq!(polled, Poll::Ready(Some(spans[0].context)));
    });
}

#[test]
fn each_metric_records_its_kind_value_and_attributes() {
    with_pipeline(|exporter| {
        store_server::metrics::request_duration("/cart", 200).record(Parent::none(), 12.5);
        store_server::metrics::orders_placed().record(Parent::none(), 2);
        ledger::metrics::entries_posted().record(Parent::none(), 1);

        let points = exporter.metrics();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].name, "store_server.request_duration");
        assert_eq!(points[0].kind, MetricKind::Histogram);
        assert_eq!(points[0].value, AttributeValue::Float64(12.5));
        assert_eq!(
            attribute(&points[0].attributes, "route"),
            Some(&AttributeValue::from("/cart"))
        );
        assert_eq!(points[1].kind, MetricKind::Counter);
        assert_eq!(points[1].value, AttributeValue::Int64(2));
        assert_eq!(
            points[2].name, "ledger.entries_posted",
            "a nested module's metric"
        );
    });
}

#[test]
fn logs_and_errors_record_what_their_declarations_say() {
    with_pipeline(|exporter| {
        store_server::logs::order_failed("order-2", "declined").emit(Parent::none());

        let declined = payments::errors::authorization_declined("order-3", "51");
        assert_eq!(declined.error().category(), payments::MODULE_NAME);
        assert!(
            declined
                .error()
                .matches(&payments::error_kinds::AUTHORIZATION_DECLINED)
        );
        assert_eq!(
            declined.error().kind(),
            1,
            "the declared code is the error's kind"
        );
        assert!(
            declined.error().location().contains(file!()),
            "located at its caller: {}",
            declined.error().location()
        );
        payments::report::authorization_declined(&declined);
        payments::report_degraded::authorization_declined(&declined);

        let expired = payments::errors::card_expired("order-4");
        assert!(
            is_refusal(expired.error()),
            "a refusal carries the fact by construction"
        );
        payments::record_error(expired.error());

        let logs = exporter.logs();
        assert_eq!(
            logs.len(),
            3,
            "a log and two reports; a refusal is recorded, never logged"
        );

        let log = &logs[0];
        assert_eq!(log.severity, Severity::Error);
        assert_eq!(
            log.body,
            "An order that reached checkout and did not complete"
        );
        assert_eq!(
            attribute(&log.attributes, "bc.telemetry.log"),
            Some(&AttributeValue::from("store_server.order_failed"))
        );
        assert_eq!(
            attribute(&log.attributes, "reason"),
            Some(&AttributeValue::from("declined"))
        );

        let report = &logs[1];
        assert_eq!(report.severity, Severity::Error);
        assert_eq!(report.body, "The processor declined the authorization");
        assert_eq!(
            attribute(&report.attributes, "error.category"),
            Some(&AttributeValue::from("payments"))
        );
        assert_eq!(
            attribute(&report.attributes, "error.kind"),
            Some(&AttributeValue::Int64(1))
        );
        assert_eq!(
            attribute(&report.attributes, "processor_code"),
            Some(&AttributeValue::from("51"))
        );
        assert_eq!(
            attribute(&report.attributes, "bc.telemetry.log"),
            Some(&AttributeValue::from("payments.authorization_declined"))
        );
        assert_eq!(logs[2].severity, Severity::Warn, "a degraded report");
    });
}

/// An error that arrives as an `AnyError` becomes its declared type only when
/// it is one: another kind, or this kind built without a declared field, is
/// handed back unchanged with why — never reported under the wrong name, or
/// with the field silently missing.
#[test]
fn an_error_converts_to_its_declared_type_only_when_it_is_one() {
    use bc_telemetry::NotThisErrorReason;

    let declined = payments::errors::authorization_declined("order-5", "05");
    let converted = payments::errors::authorization_declined::Error::try_from_error(
        declined.clone().into_error(),
    );
    assert_eq!(converted, Ok(declined));

    let expired = payments::errors::card_expired("order-6").into_error();
    let refused = payments::errors::authorization_declined::Error::try_from_error(expired.clone())
        .expect_err("a refusal is not the failure");
    assert_eq!(refused.reason, NotThisErrorReason::OtherKind);
    assert_eq!(refused.error, expired, "handed back unchanged");

    let by_hand = bc_error::AnyError::from_metadata(
        payments::error_kinds::AUTHORIZATION_DECLINED,
        bc_error::MetadataDictionary::from([(
            "order_id".to_owned(),
            bc_error::MetadataValue::String("order-7".to_owned()),
        )]),
    );
    let refused = payments::errors::authorization_declined::Error::try_from_error(by_hand)
        .expect_err("a declared field is missing");
    assert_eq!(
        refused.reason,
        NotThisErrorReason::MissingField("processor_code")
    );
}
