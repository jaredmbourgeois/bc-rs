mod support;

use bc_nodular_architecture::{
    Dependency, Input, NodeLoop, NodeResult, Nodule, NoduleEventSequence, NoduleLoop, Output,
    SharedDependency,
};
use support::{
    Countdown, CountdownInput, CountdownOutput, CountdownState, OneShot, OneShotInput,
    assert_failed_loop_panic, dependency,
};

#[test]
fn deterministic_countdown_drains_feedback() {
    let mut observed = Vec::new();
    let mut node_loop = NodeLoop::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 5 },
        |output| observed.push(output.remaining),
    );

    node_loop.receive(CountdownInput::Tick);
    let state = node_loop.into_state();

    assert_eq!(observed, [4, 3, 2, 1, 0]);
    assert_eq!(state, CountdownState { remaining: 0 });
}

#[test]
fn feedback_terminates_immediately_at_zero() {
    let mut observed = Vec::new();
    let mut node_loop = NodeLoop::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 1 },
        |output| observed.push(output.remaining),
    );

    node_loop.receive(CountdownInput::Tick);
    drop(node_loop);

    assert_eq!(observed, [0]);
}

#[test]
fn state_accessor_reports_state_after_feedback_drains() {
    let mut node_loop = NodeLoop::<Countdown>::new(dependency(), CountdownState { remaining: 3 });

    node_loop.receive(CountdownInput::Tick);

    assert_eq!(node_loop.state(), &CountdownState { remaining: 0 });
}

#[test]
fn no_feedback_delivers_one_output_exactly_once() {
    let mut observed = Vec::new();
    let mut node_loop =
        NodeLoop::<OneShot>::with_output(dependency(), 0, |output| observed.push(output.0));

    node_loop.receive(OneShotInput(21));
    drop(node_loop);

    assert_eq!(observed, [42]);
}

#[test]
fn deterministic_run_delivers_multiple_outputs_depth_first() {
    let mut observed = Vec::new();
    let mut node_loop = NodeLoop::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 2 },
        |output| observed.push(output.remaining),
    );

    node_loop.receive(CountdownInput::Run);
    drop(node_loop);

    assert_eq!(observed, [2, 1, 0, 9]);
}

#[test]
fn one_hundred_thousand_run_feedback_transitions_use_bounded_stack() {
    const DEPTH: usize = 100_000;

    let mut output_count = 0;
    let mut last_output = usize::MAX;
    let mut node_loop = NodeLoop::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: DEPTH },
        |output| {
            output_count += 1;
            last_output = output.remaining;
        },
    );

    node_loop.receive(CountdownInput::Tick);
    let state = node_loop.into_state();

    assert_eq!(output_count, DEPTH);
    assert_eq!(last_output, 0);
    assert_eq!(state, CountdownState { remaining: 0 });
}

#[test]
fn injected_output_is_observed_before_its_feedback() {
    let mut observed = Vec::new();
    let mut node_loop = NodeLoop::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 2 },
        |output| observed.push(output.remaining),
    );

    node_loop.send(CountdownOutput {
        remaining: 1,
        feeds_back: true,
    });
    drop(node_loop);

    assert_eq!(observed, [1, 1, 0]);
}

#[test]
fn run_output_is_delivered_before_a_later_panic() {
    let mut observed = Vec::new();
    let mut node_loop = NodeLoop::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 0 },
        |output| observed.push(output.remaining),
    );

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        node_loop.receive(CountdownInput::PanicRun);
    }));
    let reuse = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        node_loop.receive(CountdownInput::Run);
    }));
    drop(node_loop);

    assert!(panic.is_err());
    assert_failed_loop_panic(reuse);
    assert_eq!(observed, [7]);
}

#[test]
fn observer_panic_clears_causal_work_and_poison_reuse_fails_stop() {
    let observation_count = std::cell::Cell::new(0);
    let mut node_loop = NodeLoop::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 0 },
        |_output| {
            observation_count.set(observation_count.get() + 1);
            panic!("observer panic");
        },
    );

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        node_loop.receive(CountdownInput::Run);
    }));
    let reuse = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        node_loop.receive(CountdownInput::Run);
    }));
    drop(node_loop);

    assert!(panic.is_err());
    assert_failed_loop_panic(reuse);
    assert_eq!(observation_count.get(), 1);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanicFeedbackInput {
    Start,
    Feedback,
}

impl Input for PanicFeedbackInput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PanicFeedbackOutput(&'static str);

impl Output for PanicFeedbackOutput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

struct PanicFeedbackNodule;

impl Nodule for PanicFeedbackNodule {
    type Input = PanicFeedbackInput;
    type Output = PanicFeedbackOutput;
    type State = ();
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        _state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        match input {
            PanicFeedbackInput::Start => NodeResult::run(
                [PanicFeedbackOutput("first"), PanicFeedbackOutput("stale")].into_iter(),
            ),
            PanicFeedbackInput::Feedback => panic!("feedback panic"),
        }
    }
}

impl NoduleLoop for PanicFeedbackNodule {
    fn input_from_output(
        output: &Self::Output,
        _state: &Self::State,
        _dependency: &Self::Dependency,
    ) -> Option<Self::Input> {
        (output.0 == "first").then_some(PanicFeedbackInput::Feedback)
    }
}

#[test]
fn feedback_panic_clears_causal_work_and_poison_reuse_fails_stop() {
    let mut observed = Vec::new();
    let mut node_loop = NodeLoop::<PanicFeedbackNodule>::with_output(dependency(), (), |output| {
        observed.push(output.0);
    });

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        node_loop.receive(PanicFeedbackInput::Start);
    }));
    let reuse = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        node_loop.receive(PanicFeedbackInput::Start);
    }));
    drop(node_loop);

    assert!(panic.is_err());
    assert_failed_loop_panic(reuse);
    assert_eq!(observed, ["first"]);
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OrderingInput {
    Run,
    Feedback,
}

impl Input for OrderingInput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OrderingOutput {
    name: &'static str,
    feeds_back: bool,
}

impl Output for OrderingOutput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Clone)]
struct OrderingDependency {
    shared: SharedDependency,
    events: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl Dependency for OrderingDependency {
    fn shared(&self) -> &SharedDependency {
        &self.shared
    }
}

struct OrderingNodule;

impl Nodule for OrderingNodule {
    type Input = OrderingInput;
    type Output = OrderingOutput;
    type State = ();
    type Dependency = OrderingDependency;

    fn receive(
        input: &Self::Input,
        _state: &mut Self::State,
        dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        match input {
            OrderingInput::Run => {
                let events = std::sync::Arc::clone(&dependency.events);
                let mut step = 0;
                NodeResult::run(std::iter::from_fn(move || {
                    let output = match step {
                        0 => OrderingOutput {
                            name: "first",
                            feeds_back: true,
                        },
                        1 => {
                            events.lock().unwrap().push("between");
                            OrderingOutput {
                                name: "second",
                                feeds_back: false,
                            }
                        }
                        _ => return None,
                    };
                    step += 1;
                    Some(output)
                }))
            }
            OrderingInput::Feedback => {
                dependency.events.lock().unwrap().push("feedback");
                NodeResult::none()
            }
        }
    }
}

impl NoduleLoop for OrderingNodule {
    fn input_from_output(
        output: &Self::Output,
        _state: &Self::State,
        _dependency: &Self::Dependency,
    ) -> Option<Self::Input> {
        output.feeds_back.then_some(OrderingInput::Feedback)
    }
}

#[test]
fn run_delivery_and_feedback_precede_later_run_side_effects() {
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observer_events = std::sync::Arc::clone(&events);
    let mut node_loop = NodeLoop::<OrderingNodule>::with_output(
        OrderingDependency {
            shared: dependency(),
            events: std::sync::Arc::clone(&events),
        },
        (),
        move |output| observer_events.lock().unwrap().push(output.name),
    );

    node_loop.receive(OrderingInput::Run);

    assert_eq!(
        *events.lock().unwrap(),
        ["first", "feedback", "between", "second"]
    );
}
