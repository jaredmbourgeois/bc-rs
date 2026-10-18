//! Runtime-neutral nodular unidirectional data flow.
//!
//! Nodules are pure state transitions: an input and injected dependencies enter,
//! state changes, and a [`NodeResult`] describes zero or more outputs. [`NodeLoop`]
//! owns immediate feedback scheduling and drains it iteratively.

#![deny(missing_docs)]

mod model;
mod node;
mod node_builder;
mod node_loop;
mod node_result;
mod nodule;
mod nodule_loop;
mod shared_dependency;

mod app_nodule;
mod combined_nodules;

pub use combined_nodules::CombinedLeaf;

pub use app_nodule::{AppInput, AppInputType, AppRunState, AppState, receive_app_input};
pub use model::{
    AppIdentifier, Device, IncrementingNoduleEventSequence, InputOutputNone, NoduleEventSequence,
    None, OperatingSystem, OperatingSystemVersion, OperatingSystemVersionParseError,
};
pub use node::{Node, NodeReceive};
pub use node_builder::{
    CombinedDependency, CombinedInput, CombinedNodule, CombinedOutput, CombinedState,
    ConfiguredNodeBuilder, LinkedDependency, LinkedNodule, LinkedState, NodeBuilder, NoduleLinking,
    NoduleParenting, ParentedDependency, ParentedNodule, ParentedState,
};
pub use node_loop::{
    Acceptance, NodeLoop, Progress, Submission, ThreadNodeLoopClosed, ThreadNodeLoopHandle,
    ThreadNodeLoopRunner,
};
pub use node_result::{NodeResult, Run, RunStep};
pub use nodule::{Dependency, Input, InputFromOutput, Nodule, Output, State};
pub use nodule_loop::NoduleLoop;
pub use shared_dependency::{SharedDependency, StringifyEncoding, StringifySortingBy};

/// Compiles every Rust block in the README as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExamples;
