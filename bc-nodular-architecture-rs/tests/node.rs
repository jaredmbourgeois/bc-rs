use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Barrier;

use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    Input, Node, NodeReceive, NodeResult, Nodule, NoduleEventSequence, Output, SharedDependency,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CounterInput {
    amount: i32,
    sequence: NoduleEventSequence,
}

impl Input for CounterInput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CounterOutput {
    total: i32,
    sequence: NoduleEventSequence,
}

impl Output for CounterOutput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CounterState {
    count: i32,
}

struct CounterNodule;

impl Nodule for CounterNodule {
    type Input = CounterInput;
    type Output = CounterOutput;
    type State = CounterState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.count += input.amount;
        NodeResult::output(CounterOutput {
            total: state.count,
            sequence: input.sequence,
        })
    }
}

fn dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

fn output(result: &NodeResult<CounterOutput>) -> CounterOutput {
    let NodeResult::Output(output) = result else {
        panic!("expected one immediate output");
    };
    *output
}

#[test]
fn receive_dispatches_to_nodule() {
    let node = Node::<CounterNodule>::new(dependency(), CounterState { count: 0 });

    let result = node.receive(&CounterInput {
        amount: 5,
        sequence: 1,
    });

    assert_eq!(output(&result).total, 5);
}

#[test]
fn state_mutates_across_receives() {
    let node = Node::<CounterNodule>::new(dependency(), CounterState { count: 0 });

    let _first_result = node.receive(&CounterInput {
        amount: 3,
        sequence: 1,
    });
    let result = node.receive(&CounterInput {
        amount: 7,
        sequence: 2,
    });

    assert_eq!(output(&result).total, 10);
}

#[test]
fn with_state_reads_current_state() {
    let node = Node::<CounterNodule>::new(dependency(), CounterState { count: 0 });

    let _result = node.receive(&CounterInput {
        amount: 42,
        sequence: 1,
    });
    let count = node.with_state(|state| state.count);

    assert_eq!(count, 42);
}

#[test]
fn replace_state_resets_without_replacing_dependency() {
    let mut dependency = dependency();
    dependency.app_identifier = "test_app".into();
    let node = Node::<CounterNodule>::new(dependency, CounterState { count: 10 });

    node.replace_state(CounterState { count: 3 });

    assert_eq!(node.with_state(|state| state.count), 3);
    assert_eq!(node.dependency().app_identifier.as_str(), "test_app");
}

#[test]
fn custom_receive_override() {
    let receive = NodeReceive::new(
        |input: &CounterInput, state: &mut CounterState, _dependency: &SharedDependency| {
            state.count += input.amount * 10;
            NodeResult::output(CounterOutput {
                total: state.count,
                sequence: 0,
            })
        },
    );
    let node =
        Node::<CounterNodule>::with_receive(dependency(), CounterState { count: 0 }, receive);

    let result = node.receive(&CounterInput {
        amount: 3,
        sequence: 1,
    });

    assert_eq!(output(&result).total, 30);
}

#[test]
fn dependency_accessor() {
    let mut dependency = dependency();
    dependency.app_identifier = "test_app".into();
    let node = Node::<CounterNodule>::new(dependency, CounterState { count: 0 });

    assert_eq!(node.dependency().app_identifier.as_str(), "test_app");
}

#[test]
fn mutating_panic_preserves_recoverable_state_and_node_operation() {
    let receive = NodeReceive::new(
        |input: &CounterInput, state: &mut CounterState, _dependency: &SharedDependency| {
            state.count += input.amount;
            assert!(input.amount >= 0, "negative input panics after mutation");
            NodeResult::output(CounterOutput {
                total: state.count,
                sequence: input.sequence,
            })
        },
    );
    let node =
        Node::<CounterNodule>::with_receive(dependency(), CounterState { count: 1 }, receive);

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _result = node.receive(&CounterInput {
            amount: -3,
            sequence: 1,
        });
    }));

    assert!(panic.is_err());
    assert_eq!(node.with_state(|state| state.count), -2);
    node.replace_state(CounterState { count: 10 });
    let result = node.receive(&CounterInput {
        amount: 5,
        sequence: 2,
    });
    assert_eq!(output(&result).total, 15);
}

#[test]
fn thread_safety() {
    const ITERATIONS: i32 = 1_000;
    const WORKERS: usize = 2;

    let node = Node::<CounterNodule>::new(dependency(), CounterState { count: 0 });
    let start = Barrier::new(WORKERS + 1);

    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                start.wait();
                for _ in 0..ITERATIONS {
                    let _result = node.receive(&CounterInput {
                        amount: 1,
                        sequence: 0,
                    });
                }
            });
        }
        start.wait();
    });

    assert_eq!(node.with_state(|state| state.count), 2 * ITERATIONS);
}
