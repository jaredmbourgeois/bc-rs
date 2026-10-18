//! Application lifecycle values and their default state transition.

use crate::{
    AppIdentifier, Device, Input, InputOutputNone, NodeResult, NoduleEventSequence, OperatingSystem,
};

/// Application lifecycle event kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppInputType {
    /// The application started.
    #[default]
    Started,
    /// The application stopped and will terminate.
    Stopped,
    /// The application terminated.
    Terminated,
}

/// Application lifecycle input.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppInput {
    /// Identifier of the application that emitted the event.
    pub app_identifier: AppIdentifier,
    /// Monotonic event sequence.
    pub seq: NoduleEventSequence,
    /// Lifecycle event kind.
    pub input_type: AppInputType,
    /// Process exit code associated with the event.
    pub exit_code: i32,
}

impl AppInput {
    /// Returns the event's monotonic sequence.
    #[must_use]
    pub const fn sequence(&self) -> NoduleEventSequence {
        self.seq
    }
}

impl Input for AppInput {
    fn sequence(&self) -> NoduleEventSequence {
        self.seq
    }
}

/// Current application run state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppRunState {
    /// The application has not started.
    #[default]
    NotStarted,
    /// The application is running.
    Running,
    /// The application stopped and will terminate.
    WillTerminate,
    /// The application terminated.
    Terminated,
}

/// Application identity, host details, and lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppState {
    /// Device on which the application runs.
    pub device: Device,
    /// Stable application identifier.
    pub identifier: AppIdentifier,
    /// Human-readable application name.
    pub name: String,
    /// Operating system on which the application runs.
    pub operating_system: OperatingSystem,
    /// Current lifecycle state.
    pub run_state: AppRunState,
}

/// Applies an application lifecycle input to state.
///
/// Each input kind maps directly to its state regardless of the previous state.
pub fn receive_app_input(input: &AppInput, state: &mut AppState) -> NodeResult<InputOutputNone> {
    state.run_state = match input.input_type {
        AppInputType::Started => AppRunState::Running,
        AppInputType::Stopped => AppRunState::WillTerminate,
        AppInputType::Terminated => AppRunState::Terminated,
    };
    NodeResult::none()
}
