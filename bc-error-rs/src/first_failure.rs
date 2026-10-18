use crate::{AnyError, MetadataValue};

/// The failures of a batch whose items are independent: every item is still
/// attempted, the **first** failure is what the batch reports, and how many
/// items failed rides beside it.
///
/// A batch that stopped at its first failure left every later item
/// unattempted; one that logged each failure and carried on reported success.
/// This keeps going and says so. It holds only faults — a failure the next
/// attempt may not repeat; what the batch refused is the caller's to count.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FirstFailure {
    first: Option<AnyError>,
    count: i64,
}

impl FirstFailure {
    /// A batch with no failure yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts `error`, and keeps it if it is the first.
    pub fn keep(&mut self, error: AnyError) {
        self.count = self.count.saturating_add(1);
        if self.first.is_none() {
            self.first = Some(error);
        }
    }

    /// How many items failed.
    #[must_use]
    pub const fn count(&self) -> i64 {
        self.count
    }

    /// The first failure, carrying the batch's failure count as
    /// `batch.failures`; `None` when no item failed.
    #[must_use]
    pub fn take(self) -> Option<AnyError> {
        let count = self.count;
        self.first
            .map(|first| first.with_metadata("batch.failures", MetadataValue::Int64(count)))
    }
}
