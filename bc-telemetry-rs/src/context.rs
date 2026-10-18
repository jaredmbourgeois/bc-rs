//! Spans: who holds them, which one is current, and what each belongs to.
//!
//! The port of the C++ library's `context.hpp` and `context.cpp`. A span
//! is one shared state behind a leaf lock. The handle that owns it is an
//! [`OpenSpan`], which belongs to whoever holds it and to no thread; a
//! [`SpanGuard`] is an open span that is also current on this thread for as
//! long as the guard lives. Which span a new span, a log record or a metric
//! point belongs to is a [`Parent`]: a resolved value, taken when it is asked
//! for and the same wherever it is carried.
//!
//! # The rules
//!
//! - **Current** is the newest activation on this thread. `start_span` adds one
//!   its guard gives back; `with_span` adds one for the length of its body. An
//!   activation is taken back by a process-wide token, never by position, so
//!   ending spans out of order disturbs nothing.
//! - **Mutating the current span with none current is a misuse**: counted as a
//!   `misuse` drop and reported to the process's assertion handler
//!   (`bc_core::report_assertion_failure_at`), naming the caller's line. It
//!   never fails the call. The `*_if_any` forms are for a caller whose span is
//!   optional, and do neither.
//! - **Caller-supplied code never fails the caller**: a sampler, a clock, an
//!   exporter or an end strategy that panics costs a record and a `failed`
//!   count. Where C++ catches a throw, this catches the unwind.
//! - **A span follows its task.** The context is per thread, so asynchronous
//!   code makes a span current for exactly the polls of the future it runs —
//!   [`with_span_async`], [`running_span_async`] — on whichever thread polls it,
//!   and takes it back before the poll returns. What the thread runs next, such
//!   as another task, never inherits it. This is `Future` and nothing more: no
//!   runtime, no feature, no second store. A guard held across an `.await` would
//!   defeat it, so `clippy.toml` refuses one (`await-holding-invalid-types`).
//! - **Nothing caller-supplied runs under a lock or a borrow**: a span's lock is
//!   a leaf — under it there are only moves, pushes and the test of `ended` —
//!   and this thread's activations are never borrowed across a call out. The
//!   clock, the exporter and the assertion handler may all use telemetry
//!   themselves.
//!
//! # What ownership settles that C++ checks at run time
//!
//! The C++ reports a family of misuses this port makes unwritable, so it
//! carries no code for them:
//!
//! - **Ending consumes the handle** — [`OpenSpan::end`] and [`SpanGuard::end`]
//!   take `self`. A span cannot be used after it ends, lent after it ends, or
//!   ended under a loan or under a racing mutation, because each needs the
//!   handle the end took. A span whose handle is alive is open.
//! - **A guard cannot reach another thread.** `SpanGuard` is not `Send`, so it
//!   is consumed on the thread that started it, and its activation is always
//!   there to take back.
//! - **A moved-from span does not exist**, so nothing asks what one does.
//!
//! # What else differs
//!
//! - **Every choice is an argument.** C++ defaults a span's parent to the
//!   current one, its kind to internal and its end to the result type's
//!   default; here the caller says each, because this family has no default
//!   arguments.
//! - **Ids come from the installed pipeline's [`IdSource`](crate::IdSource).**
//!   C++ reads the operating system's randomness directly; with no pipeline
//!   installed, a span here has the nil context, which nothing exports.

use std::cell::RefCell;
use std::future::Future;
use std::marker::PhantomData;
use std::panic::{self, AssertUnwindSafe, Location};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context as TaskContext, Poll};

use bc_core::report_assertion_failure_at;
use bc_date_time::Epoch;

use crate::exporter::SamplingDecision;
use crate::pipeline::{DropReason, Pipeline, installed_pipeline};
use crate::types::{
    AttributeValue, Attributes, Span, SpanContext, SpanEvent, SpanId, SpanKind, StatusCode,
    TRACE_FLAG_SAMPLED, TraceId,
};

// --- How a span ends ---

/// The status a span ends with, or is given while it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanStatus {
    /// Unset, ok or error.
    pub code: StatusCode,
    /// A message for an error. It reaches every exporter, so it carries only
    /// text the caller chose as safe to export.
    pub message: String,
}

impl SpanStatus {
    /// No status: ending with it keeps the status the span has.
    #[must_use]
    pub fn unset() -> Self {
        Self {
            code: StatusCode::Unset,
            message: String::new(),
        }
    }

    /// Success.
    #[must_use]
    pub fn ok() -> Self {
        Self {
            code: StatusCode::Ok,
            message: String::new(),
        }
    }

    /// Failure, with a message that is safe to export — often none.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            code: StatusCode::Error,
            message: message.into(),
        }
    }
}

/// The end strategy for a body whose result says nothing about success: the
/// span ends ok. What C++'s `SpanOutcome<void>` is.
#[must_use]
pub fn ends_ok<R>(_result: &R) -> SpanStatus {
    SpanStatus::ok()
}

/// The end strategy for a `Result`: ok for `Ok`, an error for `Err` — with no
/// message, because an error's text is free text that may not be safe to
/// export.
#[must_use]
pub fn ends_by_result<T, E>(result: &Result<T, E>) -> SpanStatus {
    match result {
        Ok(_) => SpanStatus::ok(),
        Err(_) => SpanStatus::error(""),
    }
}

// --- One span's state ---

/// Whether a span is exported when it ends and, when it is not, why. Decided
/// once, when the span starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recording {
    /// Exported when it ends.
    Recorded,
    /// The sampler dropped it, or its parent is not exported.
    Unsampled,
    /// The sampler or the clock panicked while it started.
    Failed,
    /// No pipeline was installed when it started: nothing to time it or count
    /// it on.
    Unobserved,
}

/// What changes while a span runs, behind its leaf lock.
#[derive(Debug)]
struct Changing {
    ended: bool,
    name: String,
    status: StatusCode,
    status_message: String,
    attributes: Attributes,
    events: Vec<SpanEvent>,
}

/// A started span: one shared state, held by the handle that owns the span and
/// by every activation of it.
#[derive(Debug)]
struct SpanState {
    // Fixed when the span starts, so read on any thread without the lock.
    context: SpanContext,
    parent_span_id: Option<SpanId>,
    kind: SpanKind,
    /// Meaningful only for a `Recorded` span.
    start_time: Epoch,
    recording: Recording,
    changing: Mutex<Changing>,
}

impl SpanState {
    /// The lock, whatever a panic elsewhere left it: under it there are only
    /// moves and pushes, so it is whole after any panic.
    fn lock(&self) -> MutexGuard<'_, Changing> {
        self.changing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether anything said about this span can ever be exported. A span that
    /// will not be exported is still tracked — current, a parent, ended — but
    /// nothing is stored on it and no clock is read for it.
    fn stores_changes(&self) -> bool {
        self.recording == Recording::Recorded
    }
}

// --- This thread's context ---

/// One "this span is current on this thread" entry.
#[derive(Debug)]
struct Activation {
    token: u64,
    state: Arc<SpanState>,
}

thread_local! {
    /// This thread's context: the newest entry is the current span.
    static ACTIVATIONS: RefCell<Vec<Activation>> = const { RefCell::new(Vec::new()) };
}

/// Tokens start at 1, so 0 can mean "no activation", and are process-wide: a
/// token names one activation anywhere, so taking one back can never take
/// another's.
static NEXT_ACTIVATION_TOKEN: AtomicU64 = AtomicU64::new(1);

// The three below reach this thread's context with `try_with`, which fails only
// while the thread is being torn down — a guard held in another thread-local
// may be dropped after this one is gone. There is no context left then, so
// there is nothing to push, take back or read, and telemetry must not panic.

/// Makes `state` current on this thread; the token takes it back, and is 0
/// when the thread has no context left to push onto.
fn push_activation(state: &Arc<SpanState>) -> u64 {
    let token = NEXT_ACTIVATION_TOKEN.fetch_add(1, Ordering::Relaxed);
    let pushed = ACTIVATIONS.try_with(|activations| {
        activations.borrow_mut().push(Activation {
            token,
            state: Arc::clone(state),
        });
    });
    if pushed.is_ok() { token } else { 0 }
}

/// Takes one activation back, by token. Called only by the scope that pushed
/// it, on its thread, so the token is there to take.
fn take_back_activation(token: u64) {
    if token == 0 {
        return;
    }
    let removed = ACTIVATIONS.try_with(|activations| {
        let mut activations = activations.borrow_mut();
        let index = activations
            .iter()
            .rposition(|activation| activation.token == token);
        index.map(|index| activations.remove(index))
    });
    // A guard is not `Send` and a loan cannot outlive its scope, so a pushed
    // activation is still on its thread when its scope ends.
    debug_assert!(
        !matches!(removed, Ok(None)),
        "activation {token} was not on the thread that pushed it"
    );
}

/// The span current on this thread: the newest activation.
fn current_state() -> Option<Arc<SpanState>> {
    ACTIVATIONS
        .try_with(|activations| {
            activations
                .borrow()
                .last()
                .map(|activation| Arc::clone(&activation.state))
        })
        .ok()
        .flatten()
}

/// The context of the span current on this thread, if any.
#[must_use]
pub fn current_span_context() -> Option<SpanContext> {
    current_state().map(|state| state.context)
}

// --- Counting and reporting ---

fn count_drop(reason: DropReason) {
    if let Some(pipeline) = installed_pipeline() {
        pipeline.record_drop(reason);
    }
}

/// The expectation a current-span mutator reports when it finds none.
const A_SPAN_IS_ACTIVE: &str = "a span is active";

/// A misuse of the span API: one count and one report. Never called with a
/// span's lock or this thread's activations held — the handler is the
/// composition root's code.
fn report_misuse(detail: &str, location: &'static Location<'static>) {
    count_drop(DropReason::Misuse);
    report_assertion_failure_at(A_SPAN_IS_ACTIVE, detail, location);
}

/// `operation("subject")`, for a report: enough to find the call, and only a
/// name the caller's code chose — never a value.
fn described(operation: &str, subject: &str) -> String {
    format!("{operation}(\"{subject}\")")
}

/// Calls caller-supplied code, containing a panic: `None` when it panicked.
pub(crate) fn contained<R>(call: impl FnOnce() -> R) -> Option<R> {
    panic::catch_unwind(AssertUnwindSafe(call)).ok()
}

/// The time of something happening now, or `None` when the clock panicked —
/// which is counted, since a record with no time is not recorded.
fn now_from(pipeline: &Pipeline) -> Option<Epoch> {
    let now = contained(|| pipeline.now());
    if now.is_none() {
        pipeline.record_drop(DropReason::Failed);
    }
    now
}

// --- Changing a span ---

/// Whether a change reached its span. One that did not found it ended: through
/// the current span, that is a span that ended after it was found current —
/// which only code the clock runs can do — and through a handle, never.
#[derive(Clone, Copy)]
#[must_use]
enum Mutation {
    Applied,
    SpanHasEnded,
}

fn mutate_if_open(state: &SpanState, change: impl FnOnce(&mut Changing)) -> Mutation {
    let mut changing = state.lock();
    if changing.ended {
        return Mutation::SpanHasEnded;
    }
    change(&mut changing);
    Mutation::Applied
}

fn add_event_to(state: &SpanState, name: String, attributes: Attributes) -> Mutation {
    // The clock is read before the lock is taken: it is caller-supplied, and
    // may itself say something about this span.
    let timestamp = if state.stores_changes() {
        installed_pipeline().and_then(|pipeline| now_from(&pipeline))
    } else {
        None
    };
    mutate_if_open(state, |open| {
        if let Some(timestamp) = timestamp {
            open.events.push(SpanEvent {
                name,
                timestamp,
                attributes,
            });
        }
    })
}

fn set_status_of(state: &SpanState, status: SpanStatus) -> Mutation {
    let stored = state.stores_changes();
    mutate_if_open(state, |open| {
        if stored {
            open.status = status.code;
            open.status_message = status.message;
        }
    })
}

fn set_attribute_of(state: &SpanState, key: String, value: AttributeValue) -> Mutation {
    let stored = state.stores_changes();
    mutate_if_open(state, |open| {
        if stored {
            open.attributes.push((key, value));
        }
    })
}

/// A change made through a handle. Its span is open while the handle lives, so
/// one that finds it ended is this crate's defect — counted, never lost
/// without a trace.
fn through_handle(mutation: Mutation) {
    let applied = matches!(mutation, Mutation::Applied);
    debug_assert!(applied, "a span is open while its handle lives");
    if !applied {
        count_drop(DropReason::Misuse);
    }
}

/// Hands a completed span to the pipeline, after it has ended and its data has
/// left its state. A span that was never going to be exported is counted by
/// its reason; one that started with no pipeline is counted by nothing, since
/// no pipeline saw it start.
fn export_completed(recording: Recording, mut completed: Span) {
    let Some(pipeline) = installed_pipeline() else {
        return;
    };
    match recording {
        Recording::Recorded => {}
        Recording::Unsampled => {
            pipeline.record_drop(DropReason::Unsampled);
            return;
        }
        Recording::Failed => {
            pipeline.record_drop(DropReason::Failed);
            return;
        }
        Recording::Unobserved => return,
    }
    let Some(end_time) = now_from(&pipeline) else {
        return;
    };
    completed.end_time = end_time;
    if contained(|| pipeline.exporter().export_spans(&[completed])).is_none() {
        pipeline.record_drop(DropReason::Failed);
    }
}

/// Ends a span: marks it ended and moves its data out under the lock, then
/// exports it with the lock released. An unset final status keeps the status
/// the span has.
fn end_span(state: &SpanState, final_status: SpanStatus) {
    let stores_changes = state.stores_changes();
    let completed = {
        let mut changing = state.lock();
        debug_assert!(!changing.ended, "a span is ended once, by its owner");
        if changing.ended {
            return;
        }
        changing.ended = true;
        if final_status.code != StatusCode::Unset && stores_changes {
            changing.status = final_status.code;
            changing.status_message = final_status.message;
        }
        Span {
            name: std::mem::take(&mut changing.name),
            context: state.context,
            parent_span_id: state.parent_span_id,
            kind: state.kind,
            status: changing.status,
            status_message: std::mem::take(&mut changing.status_message),
            start_time: state.start_time,
            end_time: Epoch::ZERO,
            attributes: std::mem::take(&mut changing.attributes),
            events: std::mem::take(&mut changing.events),
        }
    };
    export_completed(state.recording, completed);
}

// --- Which span something belongs to ---

/// The parent of a new span, or the span a log record or a metric point is
/// recorded under.
///
/// Always a resolved value: it names the same span wherever it is carried, so a
/// parent taken where work is handed over is still that parent on the thread
/// that does the work — and a parent taken from a span is still that span's
/// after the span ends, since a span's ids are facts about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent {
    /// The parent's ids, and whether the parent itself is exported — which a
    /// `RecordOnly` parent is and does not advertise in its sampled flag.
    resolved: Option<(SpanContext, bool)>,
}

impl Parent {
    /// The span current on this thread, now. None current means no parent,
    /// which is never an error: a span becomes the root of a new trace, a log or
    /// a metric is unparented.
    #[must_use]
    pub fn current() -> Self {
        current_state().map_or(Self { resolved: None }, |state| Self::of(&state))
    }

    /// This span, whatever is current.
    #[must_use]
    pub fn span(parent: &OpenSpan) -> Self {
        parent.state().map_or(Self { resolved: None }, Self::of)
    }

    /// A span in another process: a parsed `traceparent`. A context with no
    /// valid ids is no parent: parenting to it would export a span whose trace
    /// id is all zeros, which no collector accepts. The sampled flag is all the
    /// wire carries, so it stands for both whether the parent is exported and
    /// whether it is advertised.
    #[must_use]
    pub fn remote(parent: SpanContext) -> Self {
        if !parent.trace_id.is_valid() || !parent.span_id.is_valid() {
            return Self { resolved: None };
        }
        Self {
            resolved: Some((parent, parent.is_sampled())),
        }
    }

    /// No parent, whatever is current.
    #[must_use]
    pub const fn none() -> Self {
        Self { resolved: None }
    }

    /// The parent's ids, when there is one.
    #[must_use]
    pub fn context(&self) -> Option<SpanContext> {
        self.resolved.map(|(context, _)| context)
    }

    fn of(state: &SpanState) -> Self {
        Self {
            resolved: Some((state.context, state.recording == Recording::Recorded)),
        }
    }
}

// --- Starting a span ---

/// Starts a span under `parent`, current nowhere yet. Every caller-supplied
/// callback is called here, before the span exists anywhere another thread
/// could see it.
fn begin_span(parent: Parent, name: String, kind: SpanKind, attributes: Attributes) -> SpanState {
    let pipeline = installed_pipeline();
    // An id source that panicked has given the span no id. It is never exported
    // with the nil one in its place: its recording fails, and its end counts it
    // as `DropReason::Failed`, as a sampler or a clock that could not answer is.
    let mut id_source_failed = false;
    let (trace_id, parent_span_id) = if let Some((context, _)) = parent.resolved {
        (context.trace_id, Some(context.span_id))
    } else if let Some(pipeline) = &pipeline {
        let minted = contained(|| pipeline.id_source().trace_id());
        id_source_failed |= minted.is_none();
        (minted.unwrap_or_default(), None)
    } else {
        (TraceId::default(), None)
    };

    // Sampling follows the parent. The parent's decision is authoritative for
    // continuing a trace: the sampler may narrow — drop a span its parent kept
    // — and never widen. Whether to record follows whether the parent is
    // exported; whether to mark sampled follows its sampled flag, so a child of
    // a `RecordOnly` span is recorded and not advertised.
    let (parent_recorded, parent_sampled) = match parent.resolved {
        Some((context, recorded)) => (recorded, context.is_sampled()),
        None => (true, true),
    };

    let mut recording = Recording::Unobserved;
    let mut decision = SamplingDecision::RecordAndSample;
    let mut start_time = Epoch::ZERO;
    if let Some(pipeline) = &pipeline {
        match contained(|| pipeline.sampler().decide(&trace_id, &name)) {
            // A sampler that cannot answer has not said "record".
            None => recording = Recording::Failed,
            Some(decided) => {
                decision = decided;
                recording = if parent_recorded && decided != SamplingDecision::Drop {
                    Recording::Recorded
                } else {
                    Recording::Unsampled
                };
            }
        }
        // Read only for a span that will be exported, and before its flags are
        // decided: a span with no start is not exported, so it must not tell a
        // downstream service to sample its trace.
        if recording == Recording::Recorded {
            match contained(|| pipeline.now()) {
                Some(now) => start_time = now,
                None => recording = Recording::Failed,
            }
        }
    }

    let span_id = if let Some(pipeline) = &pipeline {
        let minted = contained(|| pipeline.id_source().span_id());
        id_source_failed |= minted.is_none();
        minted.unwrap_or_default()
    } else {
        SpanId::default()
    };
    if id_source_failed {
        recording = Recording::Failed;
    }
    let sampled = recording == Recording::Recorded
        && parent_sampled
        && decision == SamplingDecision::RecordAndSample;
    SpanState {
        context: SpanContext {
            trace_id,
            span_id,
            trace_flags: if sampled { TRACE_FLAG_SAMPLED } else { 0 },
        },
        parent_span_id,
        kind,
        start_time,
        recording,
        changing: Mutex::new(Changing {
            ended: false,
            name,
            status: StatusCode::Unset,
            status_message: String::new(),
            attributes,
            events: Vec::new(),
        }),
    }
}

// --- A span you hold ---

/// An open span: it belongs to whoever holds this handle and to no thread. It
/// may be moved to, mutated on and ended on any thread, and several threads may
/// mutate it at once. It is current nowhere until [`with_span`] makes it so.
/// Dropping it ends it, keeping whatever status it has.
#[derive(Debug)]
#[must_use = "a span ends when its handle drops, so discarding one ends it immediately"]
pub struct OpenSpan {
    /// Always `Some` while the handle can be named: it is taken only by the
    /// methods that consume the handle, and by its drop.
    state: Option<Arc<SpanState>>,
}

impl OpenSpan {
    /// The span's ids. They are facts about it, so a copy taken now stays good
    /// after the span ends.
    #[must_use]
    pub fn context(&self) -> SpanContext {
        self.state()
            .map_or_else(SpanContext::default, |state| state.context)
    }

    /// Records an event on this span.
    pub fn add_event(&self, name: impl Into<String>, attributes: Attributes) {
        if let Some(state) = self.state() {
            through_handle(add_event_to(state, name.into(), attributes));
        }
    }

    /// Sets an attribute on this span. Attributes append: the same key twice is
    /// two attributes.
    pub fn set_attribute(&self, key: impl Into<String>, value: impl Into<AttributeValue>) {
        if let Some(state) = self.state() {
            through_handle(set_attribute_of(state, key.into(), value.into()));
        }
    }

    /// Sets this span's status while it runs.
    pub fn set_status(&self, status: SpanStatus) {
        if let Some(state) = self.state() {
            through_handle(set_status_of(state, status));
        }
    }

    /// Ends the span now with `status` — an unset one keeps the status it has.
    pub fn end(mut self, status: SpanStatus) {
        if let Some(state) = self.state.take() {
            end_span(&state, status);
        }
    }

    fn state(&self) -> Option<&SpanState> {
        debug_assert!(
            self.state.is_some(),
            "a handle names its span while it lives"
        );
        self.state.as_deref()
    }
}

impl Drop for OpenSpan {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            end_span(&state, SpanStatus::unset());
        }
    }
}

// --- A span that is current in a scope ---

/// An open span that is also current on this thread for as long as the guard
/// lives: what [`start_span`] returns, and what scoped, synchronous code holds.
///
/// A guard is a promise about one thread's scope, so it is not `Send`: to keep
/// the span past the scope or send it elsewhere, [`release`](Self::release) it
/// into an [`OpenSpan`].
#[derive(Debug)]
#[must_use = "a span ends when its guard drops, so discarding one ends it immediately"]
pub struct SpanGuard {
    span: OpenSpan,
    /// This guard's entry in its thread's context; 0 once given back.
    activation: u64,
    _not_send: PhantomData<*const ()>,
}

impl SpanGuard {
    /// The span itself: to read its ids, mutate it whatever is current, name it
    /// as a parent, or lend it.
    #[must_use = "this only lends the span; it does nothing on its own"]
    pub const fn span(&self) -> &OpenSpan {
        &self.span
    }

    /// Ends the span now with `status` — an unset one keeps what it has — and
    /// stops it being current.
    pub fn end(mut self, status: SpanStatus) {
        // The activation goes back FIRST: ending exports, an exporter may start
        // a span of its own, and that must find this one already not current.
        take_back_activation(std::mem::replace(&mut self.activation, 0));
        if let Some(state) = self.span.state.take() {
            end_span(&state, status);
        }
    }

    /// Stops the span being current here and hands it on, still open.
    #[must_use = "the released span ends when its handle drops"]
    pub fn release(mut self) -> OpenSpan {
        take_back_activation(std::mem::replace(&mut self.activation, 0));
        OpenSpan {
            state: self.span.state.take(),
        }
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        // Given back before the span's own handle drops after this, and ends it.
        take_back_activation(std::mem::replace(&mut self.activation, 0));
    }
}

/// Starts a span under `parent`, current on this thread until its guard ends.
pub fn start_span(
    parent: Parent,
    name: impl Into<String>,
    kind: SpanKind,
    attributes: Attributes,
) -> SpanGuard {
    let state = Arc::new(begin_span(parent, name.into(), kind, attributes));
    // Current only now, after every callback `begin_span` makes has returned.
    let activation = push_activation(&state);
    SpanGuard {
        span: OpenSpan { state: Some(state) },
        activation,
        _not_send: PhantomData,
    }
}

/// Opens a span under `parent`, current nowhere until [`with_span`] makes it so.
pub fn open_span(
    parent: Parent,
    name: impl Into<String>,
    kind: SpanKind,
    attributes: Attributes,
) -> OpenSpan {
    OpenSpan {
        state: Some(Arc::new(begin_span(parent, name.into(), kind, attributes))),
    }
}

// --- Mutating the current span ---

/// Records an event on the current span. With none current, a misuse.
#[track_caller]
pub fn add_span_event(name: impl Into<String>, attributes: Attributes) {
    let name = name.into();
    let location = Location::caller();
    let detail = described("add_span_event", &name);
    let applied = current_state()
        .is_some_and(|state| matches!(add_event_to(&state, name, attributes), Mutation::Applied));
    if !applied {
        report_misuse(&detail, location);
    }
}

/// Sets the current span's status. With none current, a misuse.
#[track_caller]
pub fn set_span_status(status: SpanStatus) {
    let location = Location::caller();
    let applied = current_state()
        .is_some_and(|state| matches!(set_status_of(&state, status), Mutation::Applied));
    if !applied {
        report_misuse("set_span_status", location);
    }
}

/// Sets an attribute on the current span. With none current, a misuse. The
/// report names the key; the value never leaves the caller.
#[track_caller]
pub fn set_span_attribute(key: impl Into<String>, value: impl Into<AttributeValue>) {
    let key = key.into();
    let location = Location::caller();
    let detail = described("set_span_attribute", &key);
    let applied = current_state().is_some_and(|state| {
        matches!(
            set_attribute_of(&state, key, value.into()),
            Mutation::Applied
        )
    });
    if !applied {
        report_misuse(&detail, location);
    }
}

// The `*_if_any` forms: with no span current — including one that ended after
// it was found current — nothing happens and nothing is counted.

/// As [`add_span_event`], for a caller that does not know whether a span is
/// current and is right not to.
pub fn add_span_event_if_any(name: impl Into<String>, attributes: Attributes) {
    if let Some(state) = current_state() {
        let _mutation = add_event_to(&state, name.into(), attributes);
    }
}

/// As [`set_span_status`], with none current doing nothing.
pub fn set_span_status_if_any(status: SpanStatus) {
    if let Some(state) = current_state() {
        let _mutation = set_status_of(&state, status);
    }
}

/// As [`set_span_attribute`], with none current doing nothing.
pub fn set_span_attribute_if_any(key: impl Into<String>, value: impl Into<AttributeValue>) {
    if let Some(state) = current_state() {
        let _mutation = set_attribute_of(&state, key.into(), value.into());
    }
}

// --- Running code in a span ---

/// A span made current on this thread for a scope by code that does not own
/// it. The activation is taken back when the scope ends, however it ends.
struct Lent {
    activation: u64,
}

impl Lent {
    fn activate(span: &OpenSpan) -> Self {
        let activation = span.state.as_ref().map_or(0, push_activation);
        Self { activation }
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        take_back_activation(self.activation);
    }
}

/// Runs `body` with `span` current on this thread. The span is controlled
/// elsewhere — some other code holds it and will end it — so nothing here ends
/// it, whatever `body` returns or however it unwinds. Several threads may each
/// have one span current at once.
pub fn with_span<R>(span: &OpenSpan, body: impl FnOnce(&OpenSpan) -> R) -> R {
    let _lent = Lent::activate(span);
    body(span)
}

/// Takes `span` over: runs `body` with it current, then ends it with the status
/// `end` gives the result. A guard is handed over as `guard.release()`.
pub fn with_span_ending<R>(
    span: OpenSpan,
    end: impl FnOnce(&R) -> SpanStatus,
    body: impl FnOnce(&OpenSpan) -> R,
) -> R {
    // The span is current for the body only: it stops being current before it
    // ends, as every activation must.
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| with_span(&span, body)));
    finish(outcome, end, |status| span.end(status))
}

/// Starts a span under `parent`, runs `body` in it, and ends it with the status
/// `end` gives `body`'s result.
pub fn running_span<R>(
    parent: Parent,
    name: impl Into<String>,
    kind: SpanKind,
    attributes: Attributes,
    end: impl FnOnce(&R) -> SpanStatus,
    body: impl FnOnce(&OpenSpan) -> R,
) -> R {
    let guard = start_span(parent, name, kind, attributes);
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| body(guard.span())));
    finish(outcome, end, |status| guard.end(status))
}

/// Ends a span by how its body went, and hands the body's result back — or its
/// panic on, after ending the span as an error with no message, so no panic
/// text reaches an exporter. A strategy that panics costs the span its status
/// and a `failed` count, never the caller its result.
fn finish<R>(
    outcome: std::thread::Result<R>,
    end: impl FnOnce(&R) -> SpanStatus,
    ending: impl FnOnce(SpanStatus),
) -> R {
    match outcome {
        Ok(result) => {
            let status = contained(|| end(&result)).unwrap_or_else(|| {
                count_drop(DropReason::Failed);
                SpanStatus::unset()
            });
            ending(status);
            result
        }
        Err(unwinding) => {
            ending(SpanStatus::error(""));
            panic::resume_unwind(unwinding)
        }
    }
}

// --- Running a future in a span ---

/// A future that has a span current on this thread for each of its polls, and
/// nowhere between them: what [`with_span_async`] returns.
#[must_use = "a future does nothing until it is awaited"]
pub struct InSpan<'span, F> {
    span: &'span OpenSpan,
    /// Boxed so the wrapper can poll it without `unsafe` pinning: a `Box` is
    /// `Unpin` whatever it holds, so this wrapper is too.
    future: Pin<Box<F>>,
}

impl<F> std::fmt::Debug for InSpan<'_, F> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InSpan")
            .field("span", &self.span.context())
            .finish_non_exhaustive()
    }
}

impl<F: Future> Future for InSpan<'_, F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<Self::Output> {
        // Current for this poll only, and taken back however the poll ends.
        let _lent = Lent::activate(self.span);
        self.future.as_mut().poll(context)
    }
}

/// Runs `future` with `span` current for each of its polls. The span is
/// controlled elsewhere, as with [`with_span`], so nothing here ends it.
pub fn with_span_async<F: Future>(span: &OpenSpan, future: F) -> InSpan<'_, F> {
    InSpan {
        span,
        future: Box::pin(future),
    }
}

/// Starts a span under `parent` when first polled, runs `future` with it
/// current for each poll, and ends it with the status `end` gives the output.
///
/// The parent is a value, so it is the one named where this is called, wherever
/// the future is polled. A future that panics ends its span as an error and
/// resumes the panic, as [`running_span`] does; one dropped before it completes
/// ends its span with the status it has.
pub async fn running_span_async<F: Future>(
    parent: Parent,
    name: impl Into<String>,
    kind: SpanKind,
    attributes: Attributes,
    end: impl FnOnce(&F::Output) -> SpanStatus,
    future: F,
) -> F::Output {
    let span = open_span(parent, name, kind, attributes);
    let outcome = CatchingUnwind {
        future: with_span_async(&span, future),
    }
    .await;
    finish(outcome, end, |status| span.end(status))
}

/// Polls a future, turning a panic from any poll into its output — what
/// `catch_unwind` is for a closure. It is never polled again after one.
struct CatchingUnwind<F> {
    future: F,
}

impl<F: Future + Unpin> Future for CatchingUnwind<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let polled = panic::catch_unwind(AssertUnwindSafe(|| {
            Pin::new(&mut self.future).poll(context)
        }));
        match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Err(unwinding) => Poll::Ready(Err(unwinding)),
        }
    }
}
