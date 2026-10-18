use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    CombinedDependency, CombinedInput, CombinedNodule, CombinedOutput, CombinedState, Dependency,
    Input, LinkedDependency, LinkedNodule, LinkedState, NodeBuilder, NodeResult, Nodule,
    NoduleEventSequence, NoduleLinking, NoduleParenting, Output, ParentedDependency,
    ParentedNodule, ParentedState, RunStep, SharedDependency,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestInput {
    value: i32,
    sequence: NoduleEventSequence,
}

impl Input for TestInput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestOutput {
    value: i32,
    sequence: NoduleEventSequence,
}

impl Output for TestOutput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestState {
    value: i32,
}

struct DoublerNodule;

impl Nodule for DoublerNodule {
    type Input = TestInput;
    type Output = TestOutput;
    type State = TestState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.value = input.value * 2;
        NodeResult::output(TestOutput {
            value: state.value,
            sequence: 0,
        })
    }
}

struct AdderNodule;

impl Nodule for AdderNodule {
    type Input = TestInput;
    type Output = TestOutput;
    type State = TestState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.value += input.value;
        NodeResult::output(TestOutput {
            value: state.value,
            sequence: 0,
        })
    }
}

fn dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

fn input(value: i32) -> TestInput {
    TestInput { value, sequence: 0 }
}

fn output_values(result: NodeResult<TestOutput>) -> Vec<i32> {
    match result {
        NodeResult::None => Vec::new(),
        NodeResult::Output(output) => vec![output.value],
        NodeResult::Outputs(outputs) => outputs.into_iter().map(|output| output.value).collect(),
        NodeResult::Run(mut run) => {
            let mut outputs = Vec::new();
            loop {
                match run.advance() {
                    RunStep::Output(output) => outputs.push(output.value),
                    RunStep::Continue => {}
                    RunStep::Complete => return outputs,
                }
            }
        }
    }
}

// Ports NodeBuilder.BuildCreatesWorkingNode.
#[test]
fn node_builder_build_creates_working_node() {
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 }).build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [10]);
}

#[test]
fn node_builder_custom_receive_can_be_wrapped_before_build() {
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 })
        .with_receive(|input, state, _dependency| {
            state.value = input.value + 100;
            NodeResult::output(TestOutput {
                value: state.value,
                sequence: 0,
            })
        })
        .receive_before(|_input, _state, _dependency| {
            NodeResult::output(TestOutput {
                value: -1,
                sequence: 0,
            })
        })
        .build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [-1, 105]);
}

#[derive(PartialEq)]
struct BorrowedInput<'value>(&'value str);

impl Input for BorrowedInput<'_> {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(PartialEq)]
struct BorrowedOutput<'value>(&'value str);

impl Output for BorrowedOutput<'_> {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(PartialEq)]
struct BorrowedState<'value>(&'value str);

struct BorrowedDependency<'value>(&'value SharedDependency);

impl Dependency for BorrowedDependency<'_> {
    fn shared(&self) -> &SharedDependency {
        self.0
    }
}

struct BorrowedNodule<'value>(PhantomData<&'value ()>);

impl<'value> Nodule for BorrowedNodule<'value> {
    type Input = BorrowedInput<'value>;
    type Output = BorrowedOutput<'value>;
    type State = BorrowedState<'value>;
    type Dependency = BorrowedDependency<'value>;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.0 = input.0;
        NodeResult::output(BorrowedOutput(input.0))
    }
}

#[test]
fn node_builder_default_receive_accepts_borrowed_associated_types() {
    let shared = dependency();
    let initial = String::from("initial");
    let updated = String::from("updated");
    let node = NodeBuilder::<BorrowedNodule<'_>>::new(
        BorrowedDependency(&shared),
        BorrowedState(&initial),
    )
    .build();

    let result = node.receive(&BorrowedInput(&updated));

    assert!(matches!(
        result,
        NodeResult::Output(BorrowedOutput("updated"))
    ));
    assert_eq!(node.with_state(|state| state.0), "updated");
}

// NodeBuilder.BuildDoubleCallAsserts is ported by NodeBuilder::build's compile_fail example.

// Ports NodeBuilder.ReceiveBeforeRunsBeforeMain.
#[test]
fn node_builder_receive_before_runs_before_main() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let before_order = Arc::clone(&order);
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 })
        .receive_before(move |_input, _state, _dependency| {
            before_order.lock().unwrap().push("before");
            NodeResult::none()
        })
        .build();

    let result = node.receive(&input(5));

    assert_eq!(*order.lock().unwrap(), ["before"]);
    assert_eq!(output_values(result), [10]);
}

// Ports NodeBuilder.ReceiveAfterRunsAfterMain.
#[test]
fn node_builder_receive_after_runs_after_main() {
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 })
        .receive_after(|_input, _state, _dependency| {
            NodeResult::output(TestOutput {
                value: 999,
                sequence: 0,
            })
        })
        .build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [10, 999]);
}

type Combined = CombinedNodule<DoublerNodule, AdderNodule>;

fn combined_node(first: i32, second: i32) -> bc_nodular_architecture::Node<Combined> {
    NodeBuilder::<Combined>::new(
        CombinedDependency {
            first: dependency(),
            second: dependency(),
        },
        CombinedState {
            first: TestState { value: first },
            second: TestState { value: second },
        },
    )
    .build()
}

// Ports CombinedNodule.RoutesFirstInput.
#[test]
fn combined_nodule_routes_first_input() {
    let node = combined_node(0, 0);

    let result = node.receive(&CombinedInput::first(input(5)));

    match result {
        NodeResult::Output(CombinedOutput::First(output)) => assert_eq!(output.value, 10),
        _ => panic!("first input was not routed to the first output position"),
    }
}

// Ports CombinedNodule.RoutesSecondInput.
#[test]
fn combined_nodule_routes_second_input() {
    let node = combined_node(0, 100);

    let result = node.receive(&CombinedInput::second(input(5)));

    match result {
        NodeResult::Output(CombinedOutput::Second(output)) => assert_eq!(output.value, 105),
        _ => panic!("second input was not routed to the second output position"),
    }
}

// Ports CombinedNodule.CombiningBuilder.
#[test]
fn combined_nodule_combining_builder() {
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 })
        .combining::<AdderNodule>(dependency(), TestState { value: 0 })
        .build();

    let result = node.receive(&CombinedInput::first(input(7)));

    match result {
        NodeResult::Output(CombinedOutput::First(output)) => assert_eq!(output.value, 14),
        _ => panic!("combining did not retain the first route index"),
    }
}

#[test]
fn combined_nodule_can_be_decorated_after_composition() {
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 })
        .combining::<AdderNodule>(dependency(), TestState { value: 0 })
        .receive_after(|_input, _state, _dependency| {
            NodeResult::output(CombinedOutput::Second(TestOutput {
                value: 999,
                sequence: 0,
            }))
        })
        .build();

    let result = node.receive(&CombinedInput::first(input(7)));

    assert!(matches!(
        result,
        NodeResult::Outputs(outputs)
            if matches!(
                outputs.as_slice(),
                [
                    CombinedOutput::First(TestOutput { value: 14, .. }),
                    CombinedOutput::Second(TestOutput { value: 999, .. }),
                ]
            )
    ));
}

struct DoubleToAddLink;

impl NoduleLinking for DoubleToAddLink {
    type FirstNodule = DoublerNodule;
    type SecondNodule = AdderNodule;

    fn second_input_from_first_output(
        first_output: &TestOutput,
        _state: &LinkedState<TestState, TestState>,
        _dependency: &LinkedDependency<SharedDependency, SharedDependency>,
    ) -> Option<TestInput> {
        Some(input(first_output.value))
    }

    fn continue_first_run(
        first_output: &TestOutput,
        _dependency: &LinkedDependency<SharedDependency, SharedDependency>,
    ) -> Option<TestOutput> {
        Some(TestOutput {
            value: first_output.value + 1000,
            sequence: 0,
        })
    }
}

type Linked = LinkedNodule<DoubleToAddLink>;

// Ports LinkedNodule.PipelinesFirstToSecond.
#[test]
fn linked_nodule_pipelines_first_to_second() {
    let node = NodeBuilder::<Linked>::new(
        LinkedDependency {
            first: dependency(),
            second: dependency(),
        },
        LinkedState {
            first: TestState { value: 0 },
            second: TestState { value: 100 },
        },
    )
    .build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [110]);
}

// Ports LinkedNodule.LinkingBuilder.
#[test]
fn linked_nodule_linking_builder() {
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 })
        .linking::<DoubleToAddLink>(dependency(), TestState { value: 100 })
        .build();

    let result = node.receive(&input(3));

    assert_eq!(output_values(result), [106]);
}

struct ParentChildRelation;

impl NoduleParenting for ParentChildRelation {
    type ParentNodule = DoublerNodule;
    type ChildNodule = AdderNodule;

    fn child_input_from_input(
        input: &TestInput,
        _state: &ParentedState<TestState, TestState>,
        _dependency: &ParentedDependency<SharedDependency, SharedDependency>,
    ) -> Option<TestInput> {
        Some(*input)
    }

    fn output_from_child_output(
        child_output: &TestOutput,
        _dependency: &ParentedDependency<SharedDependency, SharedDependency>,
    ) -> NodeResult<TestOutput> {
        NodeResult::output(TestOutput {
            value: child_output.value + 1000,
            sequence: 0,
        })
    }
}

type Parented = ParentedNodule<ParentChildRelation>;

// Ports ParentedNodule.BothParentAndChildReceive.
#[test]
fn parented_nodule_both_parent_and_child_receive() {
    let node = NodeBuilder::<Parented>::new(
        ParentedDependency {
            parent: dependency(),
            child: dependency(),
        },
        ParentedState {
            parent: TestState { value: 0 },
            child: TestState { value: 0 },
        },
    )
    .build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [10, 1005]);
}

// Ports ParentedNodule.ParentingBuilder.
#[test]
fn parented_nodule_parenting_builder() {
    let node = NodeBuilder::<DoublerNodule>::new(dependency(), TestState { value: 0 })
        .parenting::<ParentChildRelation>(dependency(), TestState { value: 0 })
        .build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [10, 1005]);
}

struct RunDoublerNodule;

impl Nodule for RunDoublerNodule {
    type Input = TestInput;
    type Output = TestOutput;
    type State = TestState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        _state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        let value = input.value * 2;
        NodeResult::run(
            [
                TestOutput { value, sequence: 0 },
                TestOutput {
                    value: value + 1,
                    sequence: 0,
                },
            ]
            .into_iter(),
        )
    }
}

struct RunDoubleToAddLink;

impl NoduleLinking for RunDoubleToAddLink {
    type FirstNodule = RunDoublerNodule;
    type SecondNodule = AdderNodule;

    fn second_input_from_first_output(
        first_output: &TestOutput,
        _state: &LinkedState<TestState, TestState>,
        _dependency: &LinkedDependency<SharedDependency, SharedDependency>,
    ) -> Option<TestInput> {
        Some(input(first_output.value))
    }

    fn continue_first_run(
        first_output: &TestOutput,
        _dependency: &LinkedDependency<SharedDependency, SharedDependency>,
    ) -> Option<TestOutput> {
        Some(TestOutput {
            value: first_output.value + 1000,
            sequence: 0,
        })
    }
}

#[test]
fn linked_nodule_run_continues_without_invoking_second_receive() {
    type RunLinked = LinkedNodule<RunDoubleToAddLink>;
    let node = NodeBuilder::<RunLinked>::new(
        LinkedDependency {
            first: dependency(),
            second: dependency(),
        },
        LinkedState {
            first: TestState { value: 0 },
            second: TestState { value: 100 },
        },
    )
    .build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [1010, 1011]);
}

struct RunChildNodule;

impl Nodule for RunChildNodule {
    type Input = TestInput;
    type Output = TestOutput;
    type State = TestState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        _state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        let value = input.value;
        NodeResult::run(
            [
                TestOutput { value, sequence: 0 },
                TestOutput {
                    value: value + 1,
                    sequence: 0,
                },
            ]
            .into_iter(),
        )
    }
}

struct RunParentChildRelation;

impl NoduleParenting for RunParentChildRelation {
    type ParentNodule = DoublerNodule;
    type ChildNodule = RunChildNodule;

    fn child_input_from_input(
        input: &TestInput,
        _state: &ParentedState<TestState, TestState>,
        _dependency: &ParentedDependency<SharedDependency, SharedDependency>,
    ) -> Option<TestInput> {
        Some(*input)
    }

    fn output_from_child_output(
        child_output: &TestOutput,
        _dependency: &ParentedDependency<SharedDependency, SharedDependency>,
    ) -> NodeResult<TestOutput> {
        let value = child_output.value + 1000;
        NodeResult::run(std::iter::once(TestOutput { value, sequence: 0 }))
    }
}

#[test]
fn parented_nodule_run_wraps_child_and_nested_parent_runs_in_order() {
    type RunParented = ParentedNodule<RunParentChildRelation>;
    let node = NodeBuilder::<RunParented>::new(
        ParentedDependency {
            parent: dependency(),
            child: dependency(),
        },
        ParentedState {
            parent: TestState { value: 0 },
            child: TestState { value: 0 },
        },
    )
    .build();

    let result = node.receive(&input(5));

    assert_eq!(output_values(result), [10, 1005, 1006]);
}
