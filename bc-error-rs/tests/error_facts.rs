//! Ported from `libs/foundation/error/test/error_facts_test.cpp`.

use std::collections::BTreeMap;
use std::panic::Location;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bc_core::{AssertionFailure, AssertionHandler, set_assertion_handler};
use bc_error::{
    AnyError, AnyErrorKind, DomainError, DomainErrorBase, ErrorFacts, MetadataValue, RefusalKind,
    caused_by, error_detail, error_facts, format_error_message, is_commit_outcome_unknown,
    is_corrupt_row, is_invariant_breach, is_permanent_failure, is_refusal, make_any_error,
    prefixed_error_detail, sqlstate_of, with_commit_unknown_fact, with_corrupt_row_fact,
    with_facts_of, with_invariant_breach_fact, with_refusal_fact, with_sqlstate_fact,
};

/// A store's domain error, as a store raises one.
#[derive(Clone, Debug)]
struct StoreSideError {
    base: DomainErrorBase,
}

impl StoreSideError {
    #[track_caller]
    fn store_error(detail: impl Into<String>) -> Self {
        Self {
            base: DomainErrorBase::with_detail(1, "store_error", detail),
        }
    }
}

impl DomainError for StoreSideError {
    const CATEGORY: &'static str = "store_side";
    fn base(&self) -> &DomainErrorBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut DomainErrorBase {
        &mut self.base
    }
}

fn plain(detail: &str) -> StoreSideError {
    StoreSideError::store_error(detail)
}

fn connection_lost() -> StoreSideError {
    plain("connection lost")
}

const HANDLER_KIND: AnyErrorKind = AnyErrorKind::new("handler", 3);
const REFUSING_KIND: RefusalKind = RefusalKind::new("handler", 4);

/// Attaching an invariant breach reports to the process-wide assertion
/// handler, so every case that attaches one holds this: a case capturing
/// reports must not see another case's.
static ONE_HANDLER_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_handler_at_a_time() -> MutexGuard<'static, ()> {
    ONE_HANDLER_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn an_error_built_by_hand_carries_no_facts() {
    assert!(!is_corrupt_row(&connection_lost()));
    assert!(!is_commit_outcome_unknown(&connection_lost()));
    assert_eq!(sqlstate_of(&connection_lost()), None);
}

// The facts are metadata, never text. A detail that merely looks like the
// retired bracket tokens states nothing — that spelling was the defect.
#[test]
fn a_detail_that_looks_like_a_retired_token_states_nothing() {
    let lookalike = plain("bad row [corrupt_row] [commit_outcome_unknown] [sqlstate=40P01]");
    assert!(!is_corrupt_row(&lookalike));
    assert!(!is_commit_outcome_unknown(&lookalike));
    assert_eq!(sqlstate_of(&lookalike), None);
}

#[test]
fn the_writers_set_exactly_their_own_fact_and_leave_the_text_alone() {
    let corrupt = with_corrupt_row_fact(plain("bad row"));
    assert!(is_corrupt_row(&corrupt));
    assert!(!is_commit_outcome_unknown(&corrupt));
    assert_eq!(error_detail(&corrupt, ""), "bad row");

    let unknown = with_commit_unknown_fact(connection_lost());
    assert!(is_commit_outcome_unknown(&unknown));
    assert!(!is_corrupt_row(&unknown));
}

#[test]
fn the_writers_work_on_an_any_error() {
    let base = AnyError::from_detail(HANDLER_KIND, "bad row");
    let corrupt = with_corrupt_row_fact(base.clone());
    assert!(is_corrupt_row(&corrupt));
    assert!(!is_corrupt_row(&base), "a writer returns a new error");
    assert!(is_commit_outcome_unknown(&with_commit_unknown_fact(base)));
}

// `make_any_error` copies metadata verbatim, so the facts cross it with no help
// — and survive a context being stamped on afterwards.
#[test]
fn the_facts_cross_the_any_error_wrap_and_a_context_stamp() {
    let mut error = with_commit_unknown_fact(connection_lost());
    error
        .base_mut()
        .set_metadata("sqlstate", MetadataValue::from("57P01"));
    let any = make_any_error(&error).with_context("checkout_finalize");
    assert!(is_commit_outcome_unknown(&any));
    assert_eq!(sqlstate_of(&any), Some("57P01"));
}

// The site a fact used to be lost at: a new error built from an upstream one's
// text.
#[test]
fn with_facts_of_carries_every_fact_onto_a_new_domain_error() {
    let mut upstream = with_corrupt_row_fact(plain("bad price row"));
    upstream
        .base_mut()
        .set_metadata("sqlstate", MetadataValue::from("22P02"));

    let rebuilt = with_facts_of(&upstream, plain("component price lookup failed"));
    assert!(is_corrupt_row(&rebuilt));
    assert_eq!(sqlstate_of(&rebuilt), Some("22P02"));
    assert!(!is_commit_outcome_unknown(&rebuilt));
    assert_eq!(error_detail(&rebuilt, ""), "component price lookup failed");
}

#[test]
fn with_facts_of_carries_onto_an_any_error_and_from_one() {
    let upstream = make_any_error(&with_commit_unknown_fact(connection_lost()));
    let onto_any = with_facts_of(
        &upstream,
        AnyError::from_detail(HANDLER_KIND, "commit failed"),
    );
    assert!(is_commit_outcome_unknown(&onto_any));
    assert!(onto_any.matches(&HANDLER_KIND));

    let onto_domain = with_facts_of(&upstream, plain("commit failed"));
    assert!(is_commit_outcome_unknown(&onto_domain));
}

// A staging refusal leaves its transaction wrapped in the store's own error; the
// fact that makes it permanent has to ride that wrap.
#[test]
fn an_invariant_breach_rides_onto_the_stores_error() {
    let _one = one_handler_at_a_time();
    let codec_refusal = with_invariant_breach_fact(AnyError::from_detail(
        HANDLER_KIND,
        "the OrderPlaced codec refuses what its producer wrote",
    ));
    assert!(is_invariant_breach(&codec_refusal));
    assert!(!is_corrupt_row(&codec_refusal));

    let folded = with_facts_of(&codec_refusal, plain("order staging failed"));
    assert!(is_invariant_breach(&folded));
    assert!(is_invariant_breach(&make_any_error(&folded)));
    assert!(!is_invariant_breach(&connection_lost()));
}

/// One report, copied out of what the handler only borrows.
#[derive(Clone, Debug)]
struct Reported {
    expectation: String,
    detail: String,
    location: &'static Location<'static>,
}

// The breach is also a violated expectation, reported to the process's
// assertion handler at the line that attached the fact — once. Carrying the fact
// onto the store's error is not a second breach.
#[test]
fn attaching_an_invariant_breach_reports_it_once_where_it_was_noticed() {
    let _one = one_handler_at_a_time();
    let reported = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&reported);
    let previous = set_assertion_handler(Some(AssertionHandler::new(
        move |failure: &AssertionFailure<'_>| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Reported {
                    expectation: failure.expectation.to_owned(),
                    detail: failure.detail.to_owned(),
                    location: failure.location,
                });
        },
    )));
    let reports = || {
        reported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    };

    let noticed_at = Location::caller();
    let codec_refusal = with_invariant_breach_fact(AnyError::from_detail(
        HANDLER_KIND,
        "the OrderPlaced codec refuses what its producer wrote",
    ));

    let failures = reports();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].expectation,
        "our own code keeps the rule it states"
    );
    assert!(
        failures[0]
            .detail
            .contains("the OrderPlaced codec refuses what its producer wrote"),
        "{}",
        failures[0].detail
    );
    assert_eq!(failures[0].location.line(), noticed_at.line() + 1);

    let folded = with_facts_of(&codec_refusal, plain("order staging failed"));
    assert!(is_invariant_breach(&folded));
    assert_eq!(reports().len(), 1);
    drop(set_assertion_handler(previous));
}

#[test]
fn with_facts_of_leaves_an_error_alone_when_upstream_states_nothing() {
    let kept = with_facts_of(&connection_lost(), with_corrupt_row_fact(plain("bad row")));
    assert!(is_corrupt_row(&kept));
}

// Any error can carry metadata, and the SQLSTATE is logged as a
// fixed-vocabulary attribute — so a value that is not SQLSTATE-shaped is not one.
#[test]
fn only_a_well_formed_sqlstate_is_read() {
    for bogus in ["", "57p01", "57P0", "57P011", "DROP TABLE", "57 01"] {
        let mut error = connection_lost();
        error
            .base_mut()
            .set_metadata("sqlstate", MetadataValue::from(bogus));
        assert_eq!(sqlstate_of(&error), None, "{bogus}");
    }
    let mut not_a_string = connection_lost();
    not_a_string
        .base_mut()
        .set_metadata("sqlstate", MetadataValue::from(57_001_i64));
    assert_eq!(sqlstate_of(&not_a_string), None);
}

#[test]
fn a_flag_that_is_not_the_boolean_true_is_not_set() {
    let mut stringly = connection_lost();
    stringly
        .base_mut()
        .set_metadata("corrupt_row", MetadataValue::from("true"));
    assert!(!is_corrupt_row(&stringly));
    let mut cleared = connection_lost();
    cleared
        .base_mut()
        .set_metadata("corrupt_row", MetadataValue::from(false));
    assert!(!is_corrupt_row(&cleared));
}

// A refusal is declared at its condition: every error raised as a
// `RefusalKind` carries the fact, whichever factory built it, and a plain kind's
// error never does.
#[test]
fn every_factory_of_a_refusal_kind_stamps_the_refusal() {
    assert!(is_refusal(&AnyError::from_kind(REFUSING_KIND)));
    let with_detail = AnyError::from_detail(REFUSING_KIND, "no printer is registered");
    assert!(is_refusal(&with_detail));
    assert_eq!(error_detail(&with_detail, ""), "no printer is registered");
    assert!(is_refusal(&AnyError::from_metadata(
        REFUSING_KIND,
        BTreeMap::from([(String::from("detail"), MetadataValue::from("absent"))]),
    )));

    assert!(!is_refusal(&AnyError::from_detail(
        HANDLER_KIND,
        "store down"
    )));
}

// `matches` answers for a refusal exactly as for a plain kind: by category and
// value.
#[test]
fn a_refusal_matches_its_own_kind_and_no_other() {
    let refused = AnyError::from_kind(REFUSING_KIND);
    assert!(refused.matches(&REFUSING_KIND));
    assert!(!refused.matches(&HANDLER_KIND));
    assert!(!AnyError::from_kind(HANDLER_KIND).matches(&REFUSING_KIND));
    assert_eq!(refused.category(), "handler");
    assert_eq!(refused.kind(), 4);
}

// The fact is metadata, so it rides every wrap an error passes through: a
// context stamp, and a new error built out of it.
#[test]
fn the_refusal_crosses_a_context_stamp_and_with_facts_of() {
    let refused = AnyError::from_detail(REFUSING_KIND, "no printer");
    assert!(is_refusal(
        &refused.clone().with_context("label_printer_lookup")
    ));
    assert!(is_refusal(&with_facts_of(
        &refused,
        plain("could not queue the label")
    )));
    assert!(is_refusal(&with_refusal_fact(connection_lost())));
    assert!(!is_refusal(&connection_lost()));
}

#[test]
fn the_refusal_is_named_in_the_diagnostic() {
    let text = format_error_message(&AnyError::from_detail(REFUSING_KIND, "no printer"));
    assert!(text.contains(" refused"), "{text}");
    let plain_text = format_error_message(&AnyError::from_detail(HANDLER_KIND, "down"));
    assert!(!plain_text.contains(" refused"), "{plain_text}");
}

// The one reading of permanence: the three facts that say the same attempt
// fails the same way, and none of those that say it might not.
#[test]
fn a_permanent_failure_is_a_corrupt_row_a_breach_or_a_refusal_and_nothing_else() {
    let _one = one_handler_at_a_time();
    assert!(is_permanent_failure(&with_corrupt_row_fact(
        connection_lost()
    )));
    assert!(is_permanent_failure(&with_invariant_breach_fact(
        AnyError::from_kind(HANDLER_KIND)
    )));
    assert!(is_permanent_failure(&AnyError::from_kind(REFUSING_KIND)));

    assert!(!is_permanent_failure(&connection_lost()));
    assert!(!is_permanent_failure(&with_commit_unknown_fact(
        connection_lost()
    )));
    assert!(!is_permanent_failure(&with_sqlstate_fact(
        connection_lost(),
        "57P01"
    )));
}

// Every fact, once: the list `with_facts_of` carries and telemetry names under
// its own attribute.
#[test]
fn the_fact_list_names_each_fact_once() {
    assert_eq!(error_facts::FACT_KEYS.len(), 5);
    for key in error_facts::FACT_KEYS {
        assert!(error_facts::is_fact_key(key), "{key}");
        let others = error_facts::FACT_KEYS
            .iter()
            .filter(|other| **other == key)
            .count();
        assert_eq!(others, 1, "{key}");
    }
    assert!(!error_facts::is_fact_key("detail"));
}

// An error caused by a failed read carries every fact the read carried, under
// the caller's kind, and says what failed in front of the read's own reason.
#[test]
fn caused_by_carries_every_fact_and_leads_with_what_failed() {
    let cause = with_sqlstate_fact(with_corrupt_row_fact(plain("bad row")), "XX001");
    let built = caused_by(&cause, HANDLER_KIND, "Failed to look up order o-1");
    assert!(built.matches(&HANDLER_KIND));
    assert!(is_corrupt_row(&built));
    assert_eq!(sqlstate_of(&built), Some("XX001"));
    assert_eq!(
        error_detail(&built, ""),
        "Failed to look up order o-1: bad row"
    );
}

#[test]
fn caused_by_a_cause_that_states_no_reason_is_the_sentence_alone() {
    let cause = with_commit_unknown_fact(plain(""));
    let built = caused_by(&cause, HANDLER_KIND, "commit failed");
    assert!(is_commit_outcome_unknown(&built));
    assert_eq!(error_detail(&built, ""), "commit failed");
}

// A refusal condition is born refused; the cause's facts ride beside that.
#[test]
fn caused_by_a_refusal_condition_is_refused_and_keeps_the_causes_facts() {
    let built = caused_by(
        &with_sqlstate_fact(connection_lost(), "23514"),
        REFUSING_KIND,
        "refused",
    );
    assert!(is_refusal(&built));
    assert_eq!(sqlstate_of(&built), Some("23514"));
}

// The domain form: the domain type's own constructor, called here with the
// prefixed detail, and the cause's facts carried onto what it built.
#[test]
fn a_domain_error_caused_by_another_carries_its_facts_and_leads_with_what_failed() {
    let cause = make_any_error(&with_corrupt_row_fact(plain("bad row")));
    let built = with_facts_of(
        &cause,
        StoreSideError::store_error(prefixed_error_detail(&cause, "read failed")),
    );
    assert!(is_corrupt_row(&built));
    assert_eq!(error_detail(&built, ""), "read failed: bad row");
    // The constructor is called here, so the error is located here.
    assert!(built.location().contains(file!()), "{}", built.location());
}

/// A domain error whose constructor takes its kind under another name — the
/// source's `CustomerErrorCode` shape. The domain form calls whatever builds
/// the error, so the spelling of its kind does not matter.
#[derive(Clone, Debug)]
struct CodedError {
    base: DomainErrorBase,
}

#[derive(Clone, Copy)]
enum Code {
    StoreError = 1,
}

impl CodedError {
    #[track_caller]
    fn new(code: Code, message: String) -> Self {
        Self {
            base: DomainErrorBase::with_detail(code as i32, "store_error", message),
        }
    }
}

impl DomainError for CodedError {
    const CATEGORY: &'static str = "coded";
    fn base(&self) -> &DomainErrorBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut DomainErrorBase {
        &mut self.base
    }
}

#[test]
fn a_domain_error_whose_kind_is_spelled_code_is_built_the_same_way() {
    let cause = with_sqlstate_fact(connection_lost(), "57P01");
    let built = with_facts_of(
        &cause,
        CodedError::new(
            Code::StoreError,
            prefixed_error_detail(&cause, "session create failed"),
        ),
    );
    assert_eq!(sqlstate_of(&built), Some("57P01"));
    assert_eq!(
        error_detail(&built, ""),
        "session create failed: connection lost"
    );
}

// The error is located where `caused_by` was called, as a factory's is, not
// inside the helper.
#[test]
fn caused_by_locates_the_error_at_its_caller() {
    let built = caused_by(&connection_lost(), HANDLER_KIND, "read failed");
    assert!(built.location().contains(file!()), "{}", built.location());
}

// A refusal condition reads back what it was declared with, as a plain kind
// does: what a registry or a generated module shows of it.
#[test]
fn a_refusal_kind_reads_back_its_category_and_value() {
    assert_eq!(REFUSING_KIND.category(), "handler");
    assert_eq!(REFUSING_KIND.value(), 4);
}
