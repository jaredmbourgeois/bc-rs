//! Fixtures shared by the deterministic-loop and thread-driver suites.

// Each integration test compiles this module separately, so a fixture only
// one suite needs reads as dead code in the other.
#![allow(dead_code)]

use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    Input, NodeResult, Nodule, NoduleEventSequence, NoduleLoop, Output, SharedDependency,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CountdownInput {
    Tick,
    Run,
    PanicRun,
}

impl Input for CountdownInput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CountdownOutput {
    pub(crate) remaining: usize,
    pub(crate) feeds_back: bool,
}

impl Output for CountdownOutput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CountdownState {
    pub(crate) remaining: usize,
}

pub(crate) struct Countdown;

impl Nodule for Countdown {
    type Input = CountdownInput;
    type Output = CountdownOutput;
    type State = CountdownState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        match input {
            CountdownInput::Tick => {
                state.remaining -= 1;
                NodeResult::run(std::iter::once(CountdownOutput {
                    remaining: state.remaining,
                    feeds_back: true,
                }))
            }
            CountdownInput::Run => NodeResult::run(
                [
                    CountdownOutput {
                        remaining: 2,
                        feeds_back: true,
                    },
                    CountdownOutput {
                        remaining: 9,
                        feeds_back: false,
                    },
                ]
                .into_iter(),
            ),
            CountdownInput::PanicRun => {
                let mut delivered = false;
                NodeResult::run(std::iter::from_fn(move || {
                    assert!(!delivered, "panic after a delivered output");
                    delivered = true;
                    Some(CountdownOutput {
                        remaining: 7,
                        feeds_back: false,
                    })
                }))
            }
        }
    }
}

impl NoduleLoop for Countdown {
    fn input_from_output(
        output: &Self::Output,
        _state: &Self::State,
        _dependency: &Self::Dependency,
    ) -> Option<Self::Input> {
        (output.feeds_back && output.remaining > 0).then_some(CountdownInput::Tick)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OneShotInput(pub(crate) i32);

impl Input for OneShotInput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OneShotOutput(pub(crate) i32);

impl Output for OneShotOutput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

pub(crate) struct OneShot;

impl Nodule for OneShot {
    type Input = OneShotInput;
    type Output = OneShotOutput;
    type State = usize;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        *state += 1;
        NodeResult::output(OneShotOutput(input.0 * 2))
    }
}

impl NoduleLoop for OneShot {
    fn input_from_output(
        _output: &Self::Output,
        _state: &Self::State,
        _dependency: &Self::Dependency,
    ) -> Option<Self::Input> {
        None
    }
}

pub(crate) fn dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

pub(crate) fn assert_failed_loop_panic(panic: Result<(), Box<dyn std::any::Any + Send>>) {
    let Err(payload) = panic else {
        panic!("expected the failed node loop to panic");
    };
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    assert_eq!(
        message,
        Some("the node loop is failed after a previous dispatch panic")
    );
}
