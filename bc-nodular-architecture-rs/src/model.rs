//! Shared value types used by nodules.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Input, Output};

/// An application's stable identifier.
#[repr(transparent)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AppIdentifier(String);

impl AppIdentifier {
    /// Creates an identifier from its exact source spelling.
    #[must_use]
    pub fn new(identifier: impl Into<String>) -> Self {
        Self(identifier.into())
    }

    /// Borrows the identifier's source spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AppIdentifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for AppIdentifier {
    fn from(identifier: String) -> Self {
        Self::new(identifier)
    }
}

impl From<&str> for AppIdentifier {
    fn from(identifier: &str) -> Self {
        Self::new(identifier)
    }
}

/// Device identity reported to an application nodule.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Device {
    /// Stable device identifier.
    pub identifier: String,
    /// Hardware model name.
    pub model: String,
    /// Human-readable device name.
    pub name: String,
}

/// Monotonic sequence attached to nodule inputs and outputs.
pub type NoduleEventSequence = u64;

/// Thread-safe source of consecutive nodule event sequences.
#[derive(Debug)]
pub struct IncrementingNoduleEventSequence {
    sequence: AtomicU64,
}

impl IncrementingNoduleEventSequence {
    /// Starts a sequence whose first result is `initial + 1`.
    #[must_use]
    pub const fn from_initial(initial: NoduleEventSequence) -> Self {
        Self {
            sequence: AtomicU64::new(initial),
        }
    }

    /// Returns the next sequence value.
    #[must_use]
    pub fn next(&self) -> NoduleEventSequence {
        self.sequence.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
}

impl Default for IncrementingNoduleEventSequence {
    fn default() -> Self {
        Self::from_initial(0)
    }
}

/// Event value for a nodule with no meaningful input or output payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputOutputNone;

impl InputOutputNone {
    /// The sentinel event sequence carried by an empty event.
    #[must_use]
    pub const fn sequence(self) -> NoduleEventSequence {
        0
    }
}

impl Input for InputOutputNone {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

impl Output for InputOutputNone {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

/// Empty value for state or dependency positions with no data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct None;

/// Operating-system identity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperatingSystem {
    /// Operating-system name.
    pub name: String,
    /// Operating-system version.
    pub version: OperatingSystemVersion,
}

/// Semantic operating-system version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct OperatingSystemVersion {
    /// Major version component.
    pub major: u32,
    /// Minor version component.
    pub minor: u32,
    /// Patch version component.
    pub patch: u32,
}

/// Error returned when an operating-system version is not one to three unsigned components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatingSystemVersionParseError {
    #[expect(
        clippy::box_collection,
        reason = "the public error stays one pointer wide without discarding its invalid input"
    )]
    version: Box<String>,
}

impl std::fmt::Display for OperatingSystemVersionParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid operating-system version: {:?}",
            self.version
        )
    }
}

impl std::error::Error for OperatingSystemVersionParseError {}

impl std::str::FromStr for OperatingSystemVersion {
    type Err = OperatingSystemVersionParseError;

    fn from_str(version: &str) -> Result<Self, Self::Err> {
        let invalid = || OperatingSystemVersionParseError {
            version: Box::new(version.to_owned()),
        };
        let components = version.split('.').collect::<Vec<_>>();
        if components.is_empty() || components.len() > 3 {
            return Err(invalid());
        }

        let mut parsed = [0_u32; 3];
        for (index, component) in components.into_iter().enumerate() {
            if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid());
            }
            parsed[index] = component.parse().map_err(|_| invalid())?;
        }

        Ok(Self {
            major: parsed[0],
            minor: parsed[1],
            patch: parsed[2],
        })
    }
}

impl std::fmt::Display for OperatingSystemVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
