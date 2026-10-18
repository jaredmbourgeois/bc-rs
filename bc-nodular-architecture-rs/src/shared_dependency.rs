//! Shared dependency values available to every nodule.

use std::collections::BTreeMap;
use std::sync::Arc;

use bc_date_time::EpochSource;

use crate::{AppIdentifier, Dependency, IncrementingNoduleEventSequence, NoduleEventSequence};

#[derive(Clone)]
struct EventSequenceSource(Arc<dyn Fn() -> NoduleEventSequence + Send + Sync>);

impl EventSequenceSource {
    fn new(read: impl Fn() -> NoduleEventSequence + Send + Sync + 'static) -> Self {
        Self(Arc::new(read))
    }

    fn next(&self) -> NoduleEventSequence {
        (self.0)()
    }
}

impl std::fmt::Debug for EventSequenceSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EventSequenceSource(..)")
    }
}

/// Optional string encoding callback shared by nodules.
#[derive(Clone)]
pub struct StringifyEncoding(Arc<dyn Fn(&str) -> String + Send + Sync>);

impl StringifyEncoding {
    /// Wraps a string encoding callback.
    #[must_use]
    pub fn new(encode: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        Self(Arc::new(encode))
    }

    /// Encodes one string.
    #[must_use]
    pub fn encode(&self, value: &str) -> String {
        (self.0)(value)
    }
}

impl std::fmt::Debug for StringifyEncoding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StringifyEncoding(..)")
    }
}

/// Optional string ordering callback shared by nodules.
#[derive(Clone)]
#[expect(
    clippy::type_complexity,
    reason = "the erased two-argument callback is the StringifySortingBy contract"
)]
pub struct StringifySortingBy(Arc<dyn Fn(&str, &str) -> bool + Send + Sync>);

impl StringifySortingBy {
    /// Wraps a string ordering callback.
    #[must_use]
    pub fn new(compare: impl Fn(&str, &str) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(compare))
    }

    /// Returns whether `left` sorts before `right`.
    #[must_use]
    pub fn sorts_before(&self, left: &str, right: &str) -> bool {
        (self.0)(left, right)
    }
}

impl std::fmt::Debug for StringifySortingBy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StringifySortingBy(..)")
    }
}

/// Platform values captured or constructed at a composition root and injected into nodules.
#[derive(Debug, Clone)]
pub struct SharedDependency {
    /// Stable application identifier.
    pub app_identifier: AppIdentifier,
    /// Process arguments captured at the composition root.
    pub arguments: Vec<String>,
    /// Environment captured at the composition root.
    pub environment: BTreeMap<String, String>,
    /// Injected wall-clock source.
    pub epoch_source: EpochSource,
    /// Optional string encoding policy.
    pub stringify_encoding: Option<StringifyEncoding>,
    /// Optional string ordering policy.
    pub stringify_sorting_by: Option<StringifySortingBy>,
    event_sequence_source: EventSequenceSource,
}

impl SharedDependency {
    /// Creates shared dependencies with empty process metadata and a fresh event sequence.
    #[must_use]
    pub fn new(epoch_source: EpochSource) -> Self {
        let sequence = Arc::new(IncrementingNoduleEventSequence::default());
        Self {
            app_identifier: AppIdentifier::default(),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            epoch_source,
            stringify_encoding: None,
            stringify_sorting_by: None,
            event_sequence_source: EventSequenceSource::new(move || sequence.next()),
        }
    }

    /// Replaces the event-sequence source.
    #[must_use]
    pub fn with_event_sequence_source(
        mut self,
        source: impl Fn() -> NoduleEventSequence + Send + Sync + 'static,
    ) -> Self {
        self.event_sequence_source = EventSequenceSource::new(source);
        self
    }

    /// Sets the optional string encoding policy.
    #[must_use]
    pub fn with_stringify_encoding(
        mut self,
        encode: impl Fn(&str) -> String + Send + Sync + 'static,
    ) -> Self {
        self.stringify_encoding = Some(StringifyEncoding::new(encode));
        self
    }

    /// Sets the optional string ordering policy.
    #[must_use]
    pub fn with_stringify_sorting_by(
        mut self,
        compare: impl Fn(&str, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.stringify_sorting_by = Some(StringifySortingBy::new(compare));
        self
    }

    /// Returns the next event sequence.
    #[must_use]
    pub fn next_event_sequence(&self) -> NoduleEventSequence {
        self.event_sequence_source.next()
    }

    /// Returns this shared dependency bag.
    #[must_use]
    pub const fn shared(&self) -> &Self {
        self
    }
}

impl Dependency for SharedDependency {
    fn shared(&self) -> &SharedDependency {
        self
    }
}
