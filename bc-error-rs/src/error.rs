use std::borrow::Cow;
use std::fmt;

use crate::error_facts::{
    self, is_commit_outcome_unknown, is_corrupt_row, is_invariant_breach, is_refusal, sqlstate_of,
};
use crate::{AnyErrorKind, ErrorCondition, MetadataDictionary, MetadataValue, format_location};

/// Where an error's human text lives, best key first. Every reader of "the text
/// this error carries" walks this list rather than naming one key.
pub const MESSAGE_METADATA_KEYS: [&str; 3] = ["detail", "reason", "message"];

/// An owned, structured error value with context chaining.
///
/// The payload is boxed so the type is one pointer wide, keeping
/// `Result<T, AnyError>` cheap on every fallible call in every dependent crate.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct AnyError(Box<AnyErrorPayload>);

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct AnyErrorPayload {
    category: Cow<'static, str>,
    kind: i32,
    location: Box<str>,
    metadata: MetadataDictionary,
    context: Vec<String>,
}

// The boxing above is load-bearing for every dependent crate's calling
// convention, so pin it rather than trusting it to survive a future field.
const _: () = assert!(size_of::<AnyError>() == size_of::<usize>());
const _: () = assert!(size_of::<Result<(), AnyError>>() <= 2 * size_of::<usize>());

impl AnyError {
    /// Assembles the payload. Private, so every error is raised as a named
    /// [`ErrorCondition`] through one of the factories below rather than from a
    /// category and an integer that happened to be passed in that order. A
    /// refusal condition's error gains the `refused` fact here, so no factory
    /// can forget it.
    fn raise(
        condition: impl ErrorCondition,
        location: String,
        mut metadata: MetadataDictionary,
    ) -> Self {
        if condition.refuses() {
            metadata.insert(
                error_facts::REFUSED.to_owned(),
                MetadataValue::Boolean(true),
            );
        }
        let value = condition.value();
        Self(Box::new(AnyErrorPayload {
            category: condition.into_category(),
            kind: value,
            location: location.into_boxed_str(),
            metadata,
            context: Vec::new(),
        }))
    }

    /// Creates an error of `kind` and captures the caller's source location.
    #[must_use]
    #[track_caller]
    pub fn from_kind(kind: impl ErrorCondition) -> Self {
        Self::raise(kind, format_location(), MetadataDictionary::new())
    }

    /// Creates an error of `kind` carrying `detail` as its stated reason,
    /// capturing the caller's source location.
    ///
    /// `detail` lands under the metadata key of the same name — the one
    /// [`error_detail`] reads — so the reason reaches the rendered diagnostic
    /// *and* the location survives.
    #[must_use]
    #[track_caller]
    pub fn from_detail(kind: impl ErrorCondition, detail: impl Into<String>) -> Self {
        let mut metadata = MetadataDictionary::new();
        metadata.insert(String::from("detail"), MetadataValue::from(detail.into()));
        Self::raise(kind, format_location(), metadata)
    }

    /// Creates an error of `kind` with structured metadata, capturing the
    /// caller's source location.
    #[must_use]
    #[track_caller]
    pub fn from_metadata(kind: impl ErrorCondition, metadata: MetadataDictionary) -> Self {
        Self::raise(kind, format_location(), metadata)
    }

    /// Creates an error of `kind` at an origin that already exists.
    ///
    /// The one factory that does *not* capture its caller: it is for erasing a
    /// typed error whose provenance was recorded somewhere else, so overwriting
    /// that location with this call's would lose the only interesting fact.
    /// [`make_any_error`] is its caller.
    #[must_use]
    pub fn at_location(
        kind: AnyErrorKind,
        location: impl Into<String>,
        metadata: MetadataDictionary,
    ) -> Self {
        Self::raise(kind, location.into(), metadata)
    }

    /// Reports whether this error is of `kind` — both halves, not just the
    /// number. A refusal condition matches exactly as a plain kind does.
    ///
    /// The same numeric value under a different category is a different error,
    /// and comparing [`kind`](Self::kind) alone silently says otherwise.
    #[must_use]
    pub fn matches(&self, kind: &impl ErrorCondition) -> bool {
        self.category() == kind.category() && self.kind() == kind.value()
    }

    /// Returns this error's category.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.0.category
    }

    /// Returns this error's numeric kind within its category.
    #[must_use]
    pub fn kind(&self) -> i32 {
        self.0.kind
    }

    /// Returns the source location where this error originated.
    #[must_use]
    pub fn location(&self) -> &str {
        &self.0.location
    }

    /// Returns this error's structured metadata.
    #[must_use]
    pub fn metadata(&self) -> &MetadataDictionary {
        &self.0.metadata
    }

    /// Returns the context chain in outermost-first order.
    #[must_use]
    pub fn context(&self) -> &[String] {
        &self.0.context
    }

    /// Consumes this error and prepends `context` to the context chain.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.0.context.insert(0, context.into());
        self
    }

    /// Consumes this error and adds or replaces a metadata value.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: MetadataValue) -> Self {
        self.0.metadata.insert(key.into(), value);
        self
    }
}

/// The facts every error exposes, so any error type can be copied into an
/// [`AnyError`].
///
/// Named `ErrorFacts` rather than `Error` so it never shadows
/// [`std::error::Error`], which [`AnyError`] also implements.
pub trait ErrorFacts {
    /// Returns the error category.
    fn category(&self) -> &str;

    /// Returns the numeric error kind within the category.
    fn kind(&self) -> i32;

    /// Returns the source location where the error originated.
    fn location(&self) -> &str;

    /// Returns structured metadata attached to the error.
    fn metadata(&self) -> &MetadataDictionary;

    /// Returns the context chain in outermost-first order.
    fn context(&self) -> &[String] {
        &[]
    }
}

impl ErrorFacts for AnyError {
    fn category(&self) -> &str {
        self.category()
    }

    fn kind(&self) -> i32 {
        self.kind()
    }

    fn location(&self) -> &str {
        self.location()
    }

    fn metadata(&self) -> &MetadataDictionary {
        self.metadata()
    }

    fn context(&self) -> &[String] {
        self.context()
    }
}

impl fmt::Display for AnyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_error_message(self))
    }
}

// `source` stays unimplemented: the model has a context chain of strings and
// optional nested errors under caller-chosen metadata keys, but no designated
// cause. Nominating one would invent a relation the source library does not
// have. The full chain is carried by `Display` instead.
impl std::error::Error for AnyError {}

/// Copies any [`ErrorFacts`] implementation into an owned [`AnyError`].
#[must_use]
pub fn make_any_error(error: &(impl ErrorFacts + ?Sized)) -> AnyError {
    // Through `at_location`, so an erased error keeps the origin it recorded
    // rather than gaining this call's.
    let mut erased = AnyError::at_location(
        AnyErrorKind::owned(error.category(), error.kind()),
        error.location(),
        error.metadata().clone(),
    );
    // The source library stops there, because its `Error` concept has no
    // `context()` to read. This port's `ErrorFacts` does, and carrying the
    // chain across is where the port goes further than its source.
    erased.0.context = error.context().to_vec();
    erased
}

/// The one spelling of "this operation failed, here is where": `error`, as an
/// [`AnyError`], with `context` naming the operation in hand.
///
/// `context` is a stable `snake_case` token — `"bpm_order_shipped"` — never an
/// interpolated sentence: contexts are grouped and searched, and a token that
/// carries an order id groups with nothing. Variable data goes in
/// [`AnyError::with_metadata`] on the returned error.
///
/// One function where the source has two: an [`AnyError`] is itself an
/// [`ErrorFacts`] that carries its context chain here, so the context it arrived
/// with is kept beneath the new one.
///
/// # Errors
///
/// Always: it is the failure, built.
pub fn fail<T>(
    error: &(impl ErrorFacts + ?Sized),
    context: impl Into<String>,
) -> Result<T, AnyError> {
    Err(make_any_error(error).with_context(context))
}

/// The reason an error states, borrowed from its own metadata: a non-empty
/// string under `detail`, or the empty string when it states none.
///
/// Only a string is a stated reason. A non-string value is not prose a person
/// can read — for a nested error, stringifying it would hand them its source
/// location — so it states nothing here. The borrow is the error's, so the
/// compiler refuses a view that would outlive it.
#[must_use]
pub fn error_detail_view<E: ErrorFacts + ?Sized>(error: &E) -> &str {
    match error.metadata().get("detail") {
        Some(MetadataValue::String(detail)) => detail,
        _ => "",
    }
}

/// The reason an error states, or `fallback` — the caller's own sentence — when
/// it states none.
#[must_use]
pub fn error_detail(error: &(impl ErrorFacts + ?Sized), fallback: &str) -> String {
    let detail = error_detail_view(error);
    if detail.is_empty() { fallback } else { detail }.to_owned()
}

/// The sentence naming what failed, then the stated reason when there is one:
/// `"<prefix>: <detail>"`, or `prefix` alone — never a dangling `": "`.
#[must_use]
pub fn prefixed_error_detail(error: &(impl ErrorFacts + ?Sized), prefix: &str) -> String {
    let detail = error_detail_view(error);
    if detail.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}: {detail}")
    }
}

/// The one-line diagnostic: the context chain outermost-first,
/// `category kind=<n>`, the structured facts, then the stated reason when there
/// is one. **It never emits a source position**, which is what makes it safe to
/// hand a requester; an operator's sink that wants one uses
/// [`format_error_message_with_location`].
#[must_use]
pub fn format_error_message(error: &AnyError) -> String {
    let body = format_error_body(error);
    let message = match selected_metadata_message(error) {
        Some(message) => format!("{body}: {message}"),
        None => body,
    };
    sanitize_one_line(&message)
}

/// The same diagnostic, plus ` @ <location>` when — and only when — the error
/// states no reason: a source position is the last resort for an error that
/// says nothing, never an addition to one that does. For an operator's sink — a
/// log line, stderr — and never a response body.
#[must_use]
pub fn format_error_message_with_location(error: &AnyError) -> String {
    let body = format_error_body(error);
    let message = match selected_metadata_message(error) {
        Some(message) => format!("{body}: {message}"),
        None => format!("{body} @ {}", error.location()),
    };
    sanitize_one_line(&message)
}

/// What both formatters share: the context chain, the identity, and the facts
/// spelled beside the kind so the line an operator greps still names them —
/// `sqlstate=40P01` reads as it did when the code was a token in the text.
fn format_error_body(error: &AnyError) -> String {
    let mut body = String::new();
    for context in error.context() {
        body.push_str(context);
        body.push_str(": ");
    }
    body.push_str(error.category());
    body.push_str(" kind=");
    body.push_str(&error.kind().to_string());

    if let Some(sqlstate) = sqlstate_of(error) {
        body.push_str(" sqlstate=");
        body.push_str(sqlstate);
    }
    let flags = [
        (is_corrupt_row(error), " corrupt_row"),
        (is_commit_outcome_unknown(error), " commit_outcome_unknown"),
        (is_invariant_breach(error), " invariant_breach"),
        (is_refusal(error), " refused"),
    ];
    for (set, spelling) in flags {
        if set {
            body.push_str(spelling);
        }
    }
    body
}

/// The text an error carries, from [`MESSAGE_METADATA_KEYS`] in order. `detail`
/// is read through [`error_detail_view`], so the diagnostic and a user's
/// message agree about what counts as a stated reason; `reason` and `message`
/// are free-form keys, so a non-null value that is not a string still prints.
fn selected_metadata_message(error: &AnyError) -> Option<String> {
    for key in MESSAGE_METADATA_KEYS {
        if key == "detail" {
            let detail = error_detail_view(error);
            if !detail.is_empty() {
                return Some(detail.to_owned());
            }
            continue;
        }
        let Some(value) = error.metadata().get(key) else {
            continue;
        };

        if let Some(message) = value.as_string() {
            if !message.is_empty() {
                return Some(message.to_owned());
            }
        } else if !value.is_null() {
            return Some(value.stringify());
        }
    }

    None
}

fn sanitize_one_line(message: &str) -> String {
    let mut sanitized = String::with_capacity(message.len());

    for character in message.chars() {
        match character {
            '\\' => sanitized.push_str("\\\\"),
            '\n' => sanitized.push_str("\\n"),
            '\r' => sanitized.push_str("\\r"),
            '\t' => sanitized.push_str("\\t"),
            '\u{2028}' | '\u{2029}' => sanitized.extend(character.escape_unicode()),
            character if character.is_control() => sanitized.extend(character.escape_unicode()),
            character => sanitized.push(character),
        }
    }

    sanitized
}

#[cfg(test)]
mod tests {
    use super::{sanitize_one_line, selected_metadata_message};
    use crate::{AnyError, AnyErrorKind, MetadataDictionary, MetadataValue};

    #[test]
    fn sanitize_escapes_backslash_before_the_escapes_it_introduces() {
        assert_eq!(sanitize_one_line("a\\b\nc"), "a\\\\b\\nc");
    }

    #[test]
    fn sanitize_passes_ordinary_text_through_unchanged() {
        assert_eq!(sanitize_one_line("café ok"), "café ok");
    }

    #[test]
    fn selected_message_prefers_detail_then_reason_then_message() {
        let error =
            AnyError::at_location(AnyErrorKind::new("c", 1), "loc", MetadataDictionary::new())
                .with_metadata("message", MetadataValue::from("third"))
                .with_metadata("reason", MetadataValue::from("second"))
                .with_metadata("detail", MetadataValue::from("first"));

        assert_eq!(selected_metadata_message(&error).as_deref(), Some("first"));
    }

    #[test]
    fn selected_message_skips_empty_strings_and_nulls() {
        let error =
            AnyError::at_location(AnyErrorKind::new("c", 1), "loc", MetadataDictionary::new())
                .with_metadata("detail", MetadataValue::from(""))
                .with_metadata("reason", MetadataValue::Null)
                .with_metadata("message", MetadataValue::from("used"));

        assert_eq!(selected_metadata_message(&error).as_deref(), Some("used"));
    }
}
