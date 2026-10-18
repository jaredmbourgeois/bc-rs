use bc_nodular_architecture::{
    IncrementingNoduleEventSequence, Input, InputFromOutput, InputOutputNone, None, Output, State,
};

#[derive(PartialEq)]
struct PartialEvent(u8);

impl Input for PartialEvent {
    fn sequence(&self) -> u64 {
        u64::from(self.0)
    }
}

impl Output for PartialEvent {
    fn sequence(&self) -> u64 {
        u64::from(self.0)
    }
}

#[derive(PartialEq)]
struct FeedbackInput(u8);

impl Input for FeedbackInput {
    fn sequence(&self) -> u64 {
        u64::from(self.0)
    }
}

#[derive(PartialEq)]
struct FeedbackOutput(u8);

impl Output for FeedbackOutput {
    fn sequence(&self) -> u64 {
        u64::from(self.0)
    }
}

impl InputFromOutput<FeedbackOutput> for FeedbackInput {
    fn from_output(output: FeedbackOutput) -> Option<Self> {
        Some(Self(output.0))
    }
}

#[test]
fn incrementing_nodule_event_sequence_starts_at_initial_value_plus_one() {
    let sequence = IncrementingNoduleEventSequence::from_initial(0);

    assert_eq!(sequence.next(), 1);
}

#[test]
fn incrementing_nodule_event_sequence_starts_at_custom_initial_value_plus_one() {
    let sequence = IncrementingNoduleEventSequence::from_initial(41);

    assert_eq!(sequence.next(), 42);
}

#[test]
fn incrementing_nodule_event_sequence_is_monotonically_increasing() {
    let sequence = IncrementingNoduleEventSequence::default();
    let first = sequence.next();
    let second = sequence.next();
    let third = sequence.next();

    assert!(first < second);
    assert!(second < third);
}

#[test]
fn incrementing_nodule_event_sequence_produces_consecutive_values() {
    let sequence = IncrementingNoduleEventSequence::default();

    assert_eq!(
        [sequence.next(), sequence.next(), sequence.next()],
        [1, 2, 3]
    );
}

#[test]
fn input_output_none_sequence_is_zero() {
    assert_eq!(InputOutputNone.sequence(), 0);
}

#[test]
fn input_output_none_satisfies_both_event_contracts() {
    fn sequences(input: &impl Input, output: &impl Output) -> (u64, u64) {
        (input.sequence(), output.sequence())
    }

    assert_eq!(sequences(&InputOutputNone, &InputOutputNone), (0, 0));
}

#[test]
fn input_output_none_all_instances_equal() {
    assert_eq!(InputOutputNone, InputOutputNone);
}

#[test]
fn none_satisfies_the_equatable_state_contract() {
    // fidelity-compile-proof: the `impl State` bound is the assertion; a runtime
    // check could not distinguish satisfying the contract from failing to.
    fn accepts_state(_state: impl State) {}

    accepts_state(None);
}

#[test]
fn none_all_instances_equal() {
    assert_eq!(None, None);
}

#[test]
fn contracts_require_partial_equality_without_total_equality() {
    fn accepts_input(_input: impl Input) {}
    fn accepts_output(_output: impl Output) {}
    fn accepts_state(_state: impl State) {}

    accepts_input(PartialEvent(1));
    accepts_output(PartialEvent(2));
    accepts_state(PartialEvent(3));
}

#[test]
fn input_from_output_is_an_explicit_usable_contract() {
    fn convert<I, O>(output: O) -> Option<I>
    where
        I: InputFromOutput<O>,
        O: Output,
    {
        I::from_output(output)
    }

    let converted = convert::<FeedbackInput, _>(FeedbackOutput(7));

    assert!(matches!(converted, Some(FeedbackInput(7))));
}
