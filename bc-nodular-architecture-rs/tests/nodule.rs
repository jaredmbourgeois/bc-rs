use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    Input, NodeResult, Nodule, NoduleEventSequence, Output, SharedDependency,
};

#[derive(PartialEq)]
struct CounterInput(i32);

impl Input for CounterInput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(PartialEq)]
struct CounterOutput(i32);

impl Output for CounterOutput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

struct CounterNodule;

impl Nodule for CounterNodule {
    type Input = CounterInput;
    type Output = CounterOutput;
    type State = i32;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        *state += input.0;
        NodeResult::none()
    }
}

struct CountingNodule;

impl Nodule for CountingNodule {
    type Input = CounterInput;
    type Output = CounterOutput;
    type State = i32;
    type Dependency = SharedDependency;

    fn receive(
        _input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        *state += 1;
        NodeResult::output(CounterOutput(*state))
    }
}

fn dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

#[test]
fn valid_nodule_receive_returns_none() {
    let mut state = 0;

    let result = CounterNodule::receive(&CounterInput(0), &mut state, &dependency());

    assert!(matches!(result, NodeResult::None));
}

#[test]
fn nodule_can_mutate_state() {
    let mut state = 0;

    let first = CountingNodule::receive(&CounterInput(1), &mut state, &dependency());
    assert_eq!(state, 1);
    assert!(matches!(first, NodeResult::Output(CounterOutput(1))));

    let second = CountingNodule::receive(&CounterInput(2), &mut state, &dependency());
    assert_eq!(state, 2);
    assert!(matches!(second, NodeResult::Output(CounterOutput(2))));
}
