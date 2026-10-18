use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    Dependency, Input, Node, NodeResult, Nodule, NoduleEventSequence, Output, RunStep,
    SharedDependency, combined_nodules,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct CommonInput {
    value: i32,
    sequence: NoduleEventSequence,
}

impl Input for CommonInput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CommonOutput {
    value: i32,
    sequence: NoduleEventSequence,
}

impl Output for CommonOutput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Default, PartialEq, Eq)]
struct CommonState {
    value: i32,
}

struct MultiplyNodule;

impl Nodule for MultiplyNodule {
    type Input = CommonInput;
    type Output = CommonOutput;
    type State = CommonState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.value = input.value * 2;
        NodeResult::output(CommonOutput {
            value: state.value,
            sequence: input.sequence,
        })
    }
}

struct AddNodule;

impl Nodule for AddNodule {
    type Input = CommonInput;
    type Output = CommonOutput;
    type State = CommonState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.value += input.value;
        NodeResult::output(CommonOutput {
            value: state.value,
            sequence: input.sequence,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FlagInput {
    enabled: bool,
    sequence: NoduleEventSequence,
}

impl Input for FlagInput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FlagOutput {
    enabled: bool,
    sequence: NoduleEventSequence,
}

impl Output for FlagOutput {
    fn sequence(&self) -> NoduleEventSequence {
        self.sequence
    }
}

#[derive(Default, PartialEq, Eq)]
struct FlagState {
    enabled: bool,
}

struct FlagNodule;

impl Nodule for FlagNodule {
    type Input = FlagInput;
    type Output = FlagOutput;
    type State = FlagState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.enabled = input.enabled;
        NodeResult::output(FlagOutput {
            enabled: input.enabled,
            sequence: input.sequence,
        })
    }
}

struct MultipleOutputNodule;

impl Nodule for MultipleOutputNodule {
    type Input = CommonInput;
    type Output = CommonOutput;
    type State = CommonState;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        if input.value == i32::MAX {
            let sequence = input.sequence;
            return NodeResult::run(
                [
                    CommonOutput { value: 3, sequence },
                    CommonOutput { value: 4, sequence },
                ]
                .into_iter(),
            );
        }
        if input.value == 0 {
            return NodeResult::none();
        }
        if input.value < 0 {
            return NodeResult::outputs(vec![
                CommonOutput {
                    value: 1,
                    sequence: input.sequence,
                },
                CommonOutput {
                    value: 2,
                    sequence: input.sequence,
                },
            ]);
        }

        state.value = input.value;
        NodeResult::output(CommonOutput {
            value: input.value,
            sequence: input.sequence,
        })
    }
}

combined_nodules! {
    Trio, TrioInput, TrioOutput, TrioState, TrioDependency {
        Multiply(multiply): MultiplyNodule,
        Add(add): AddNodule,
        Flag(flag): FlagNodule,
    }
}

combined_nodules! {
    Pair, PairInput, PairOutput, PairState, PairDependency {
        Multiply(multiply): MultiplyNodule,
        Add(add): AddNodule,
    }
}

combined_nodules! {
    MultipleAndFlag,
    MultipleAndFlagInput,
    MultipleAndFlagOutput,
    MultipleAndFlagState,
    MultipleAndFlagDependency {
        Multiple(multiple): MultipleOutputNodule,
        Flag(flag): FlagNodule,
    }
}

fn shared_dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

#[test]
fn routes_inputs_and_outputs_by_explicit_leaf_type() {
    assert_eq!(Trio::SIZE, 3);
    let dependency = TrioDependency::uniform(shared_dependency());
    let mut state = TrioState::default();

    let multiply = Trio::receive(
        &TrioInput::of::<MultiplyNodule>(CommonInput {
            value: 5,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );
    assert!(matches!(
        multiply,
        NodeResult::Output(TrioOutput::Multiply(CommonOutput { value: 10, .. }))
    ));

    let flag = Trio::receive(
        &TrioInput::of::<FlagNodule>(FlagInput {
            enabled: true,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );
    assert!(matches!(
        flag,
        NodeResult::Output(TrioOutput::Flag(FlagOutput { enabled: true, .. }))
    ));
}

#[test]
fn duplicate_input_output_and_state_types_keep_leaf_identity() {
    let dependency = PairDependency::uniform(shared_dependency());
    let mut state = PairState::default();

    let add = Pair::receive(
        &PairInput::of::<AddNodule>(CommonInput {
            value: 5,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );
    assert!(matches!(
        add,
        NodeResult::Output(PairOutput::Add(CommonOutput { value: 5, .. }))
    ));
    assert_eq!(state.of::<AddNodule>().value, 5);
    assert_eq!(state.of::<MultiplyNodule>().value, 0);

    let multiply = Pair::receive(
        &PairInput::of::<MultiplyNodule>(CommonInput {
            value: 5,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );
    assert!(matches!(
        multiply,
        NodeResult::Output(PairOutput::Multiply(CommonOutput { value: 10, .. }))
    ));
    assert_eq!(state.of::<MultiplyNodule>().value, 10);
    assert_eq!(state.of::<AddNodule>().value, 5);
}

#[test]
fn state_access_is_keyed_by_leaf_type_for_mutable_and_shared_borrows() {
    let mut state = TrioState::default();
    state.of_mut::<MultiplyNodule>().value = 7;
    state.of_mut::<FlagNodule>().enabled = true;

    assert_eq!(state.of::<MultiplyNodule>().value, 7);
    assert_eq!(state.of::<AddNodule>().value, 0);
    assert!(state.of::<FlagNodule>().enabled);
}

#[test]
fn uniform_dependency_clones_into_every_leaf_slot_and_exposes_shared_values() {
    let mut shared = shared_dependency();
    shared.app_identifier = "test-app".into();

    let dependency = TrioDependency::uniform(shared);

    assert_eq!(Trio::SIZE, 3);
    assert_eq!(dependency.shared().app_identifier.as_str(), "test-app");
    assert_eq!(
        dependency.multiply.shared().app_identifier.as_str(),
        "test-app"
    );
    assert_eq!(dependency.add.shared().app_identifier.as_str(), "test-app");
    assert_eq!(dependency.flag.shared().app_identifier.as_str(), "test-app");
}

#[test]
fn state_equality_compares_every_leaf_slot() {
    let first = TrioState::default();
    let mut second = TrioState::default();
    assert_eq!(first, second);

    second.of_mut::<AddNodule>().value = 1;
    assert_ne!(first, second);
}

#[test]
fn sequence_delegates_to_the_active_leaf_value() {
    let input = TrioInput::of::<AddNodule>(CommonInput {
        value: 0,
        sequence: 42,
    });

    assert_eq!(input.sequence(), 42);
}

#[test]
fn receive_preserves_node_result_shapes_leaf_identity_and_source_order() {
    let dependency = MultipleAndFlagDependency::uniform(shared_dependency());
    let mut state = MultipleAndFlagState::default();

    let none = MultipleAndFlag::receive(
        &MultipleAndFlagInput::of::<MultipleOutputNodule>(CommonInput {
            value: 0,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );
    assert!(matches!(none, NodeResult::None));

    let many = MultipleAndFlag::receive(
        &MultipleAndFlagInput::of::<MultipleOutputNodule>(CommonInput {
            value: -1,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );
    let NodeResult::Outputs(outputs) = many else {
        panic!("expected multiple outputs");
    };
    assert!(matches!(
        outputs.as_slice(),
        [
            MultipleAndFlagOutput::Multiple(CommonOutput { value: 1, .. }),
            MultipleAndFlagOutput::Multiple(CommonOutput { value: 2, .. }),
        ]
    ));

    let deferred = MultipleAndFlag::receive(
        &MultipleAndFlagInput::of::<MultipleOutputNodule>(CommonInput {
            value: i32::MAX,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );
    let NodeResult::Run(mut run) = deferred else {
        panic!("expected deferred outputs");
    };
    let mut deferred_outputs = Vec::new();
    loop {
        match run.advance() {
            RunStep::Output(output) => deferred_outputs.push(output),
            RunStep::Continue => {}
            RunStep::Complete => break,
        }
    }
    assert!(matches!(
        deferred_outputs.as_slice(),
        [
            MultipleAndFlagOutput::Multiple(CommonOutput { value: 3, .. }),
            MultipleAndFlagOutput::Multiple(CommonOutput { value: 4, .. }),
        ]
    ));
}

#[test]
fn explicit_leaf_construction_replaces_cpp_of_input_deduction() {
    let input = MultipleAndFlagInput::of::<FlagNodule>(FlagInput {
        enabled: true,
        sequence: 1,
    });
    assert_eq!(
        input,
        MultipleAndFlagInput::Flag(FlagInput {
            enabled: true,
            sequence: 1,
        })
    );

    let dependency = MultipleAndFlagDependency::uniform(shared_dependency());
    let mut state = MultipleAndFlagState::default();
    let output = MultipleAndFlag::receive(
        &MultipleAndFlagInput::of::<MultipleOutputNodule>(CommonInput {
            value: 7,
            sequence: 0,
        }),
        &mut state,
        &dependency,
    );

    assert!(matches!(
        output,
        NodeResult::Output(MultipleAndFlagOutput::Multiple(CommonOutput {
            value: 7,
            ..
        }))
    ));
    assert_eq!(state.of::<MultipleOutputNodule>().value, 7);
}

#[test]
fn combined_nodule_runs_inside_node() {
    let node = Node::<MultipleAndFlag>::new(
        MultipleAndFlagDependency::uniform(shared_dependency()),
        MultipleAndFlagState::default(),
    );

    let result = node.receive(&MultipleAndFlagInput::of::<MultipleOutputNodule>(
        CommonInput {
            value: 9,
            sequence: 0,
        },
    ));
    assert!(matches!(
        result,
        NodeResult::Output(MultipleAndFlagOutput::Multiple(CommonOutput {
            value: 9,
            ..
        }))
    ));

    node.with_state(|state| {
        assert_eq!(state.of::<MultipleOutputNodule>().value, 9);
        assert!(!state.of::<FlagNodule>().enabled);
    });
}
