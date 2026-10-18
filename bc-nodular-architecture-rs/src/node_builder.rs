//! Binary nodule composition and consuming node construction.

use std::marker::PhantomData;

use crate::{Dependency, Input, Node, NodeReceive, NodeResult, Nodule, Output, SharedDependency};

/// Input routed to one position of a binary [`CombinedNodule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombinedInput<First, Second> {
    /// Input for the first nodule.
    First(First),
    /// Input for the second nodule.
    Second(Second),
}

impl<First, Second> CombinedInput<First, Second> {
    /// Routes an input to the first nodule.
    #[must_use]
    pub const fn first(input: First) -> Self {
        Self::First(input)
    }

    /// Routes an input to the second nodule.
    #[must_use]
    pub const fn second(input: Second) -> Self {
        Self::Second(input)
    }
}

impl<First: Input, Second: Input> Input for CombinedInput<First, Second> {
    fn sequence(&self) -> crate::NoduleEventSequence {
        match self {
            Self::First(input) => input.sequence(),
            Self::Second(input) => input.sequence(),
        }
    }
}

/// Output tagged with the position that produced it in a binary [`CombinedNodule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombinedOutput<First, Second> {
    /// Output from the first nodule.
    First(First),
    /// Output from the second nodule.
    Second(Second),
}

impl<First, Second> CombinedOutput<First, Second> {
    /// Tags an output as coming from the first nodule.
    #[must_use]
    pub const fn first(output: First) -> Self {
        Self::First(output)
    }

    /// Tags an output as coming from the second nodule.
    #[must_use]
    pub const fn second(output: Second) -> Self {
        Self::Second(output)
    }
}

impl<First: Output, Second: Output> Output for CombinedOutput<First, Second> {
    fn sequence(&self) -> crate::NoduleEventSequence {
        match self {
            Self::First(output) => output.sequence(),
            Self::Second(output) => output.sequence(),
        }
    }
}

/// Dependency pair for a binary [`CombinedNodule`].
#[derive(Debug, Clone)]
pub struct CombinedDependency<First, Second> {
    /// Dependencies for the first nodule.
    pub first: First,
    /// Dependencies for the second nodule.
    pub second: Second,
}

impl<First: Dependency, Second: Dependency> Dependency for CombinedDependency<First, Second> {
    fn shared(&self) -> &SharedDependency {
        self.first.shared()
    }
}

/// State pair for a binary [`CombinedNodule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedState<First, Second> {
    /// State for the first nodule.
    pub first: First,
    /// State for the second nodule.
    pub second: Second,
}

/// Binary nodule that independently routes explicitly indexed inputs and outputs.
pub struct CombinedNodule<First, Second>(PhantomData<fn() -> (First, Second)>);

impl<First, Second> std::fmt::Debug for CombinedNodule<First, Second> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CombinedNodule")
    }
}

impl<First, Second> Nodule for CombinedNodule<First, Second>
where
    First: Nodule,
    Second: Nodule,
    First::Output: Send + 'static,
    Second::Output: Send + 'static,
{
    type Input = CombinedInput<First::Input, Second::Input>;
    type Output = CombinedOutput<First::Output, Second::Output>;
    type State = CombinedState<First::State, Second::State>;
    type Dependency = CombinedDependency<First::Dependency, Second::Dependency>;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        match input {
            CombinedInput::First(input) => {
                First::receive(input, &mut state.first, &dependency.first)
                    .map(CombinedOutput::First)
            }
            CombinedInput::Second(input) => {
                Second::receive(input, &mut state.second, &dependency.second)
                    .map(CombinedOutput::Second)
            }
        }
    }
}

/// Mapping contract for a serial [`LinkedNodule`] pipeline.
pub trait NoduleLinking {
    /// Nodule that receives the pipeline input.
    type FirstNodule: Nodule<Output: Send + 'static, Dependency: Clone + Send + 'static>;
    /// Nodule that produces the pipeline output.
    type SecondNodule: Nodule<Output: Send + 'static, Dependency: Clone + Send + 'static>;

    /// Maps an immediate first-nodule output into a possible second-nodule input.
    fn second_input_from_first_output(
        first_output: &<Self::FirstNodule as Nodule>::Output,
        state: &LinkedState<
            <Self::FirstNodule as Nodule>::State,
            <Self::SecondNodule as Nodule>::State,
        >,
        dependency: &LinkedDependency<
            <Self::FirstNodule as Nodule>::Dependency,
            <Self::SecondNodule as Nodule>::Dependency,
        >,
    ) -> Option<<Self::SecondNodule as Nodule>::Input>;

    /// Maps an output delivered by the first nodule's deferred run.
    fn continue_first_run(
        first_output: &<Self::FirstNodule as Nodule>::Output,
        dependency: &LinkedDependency<
            <Self::FirstNodule as Nodule>::Dependency,
            <Self::SecondNodule as Nodule>::Dependency,
        >,
    ) -> Option<<Self::SecondNodule as Nodule>::Output>;
}

/// Dependency pair for a serial [`LinkedNodule`] pipeline.
#[derive(Debug, Clone)]
pub struct LinkedDependency<First, Second> {
    /// Dependencies for the first nodule.
    pub first: First,
    /// Dependencies for the second nodule.
    pub second: Second,
}

impl<First: Dependency, Second: Dependency> Dependency for LinkedDependency<First, Second> {
    fn shared(&self) -> &SharedDependency {
        self.first.shared()
    }
}

/// State pair for a serial [`LinkedNodule`] pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedState<First, Second> {
    /// State for the first nodule.
    pub first: First,
    /// State for the second nodule.
    pub second: Second,
}

/// Serial nodule whose link maps first outputs into the second nodule.
pub struct LinkedNodule<Link>(PhantomData<fn() -> Link>);

impl<Link> std::fmt::Debug for LinkedNodule<Link> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LinkedNodule")
    }
}

impl<Link> Nodule for LinkedNodule<Link>
where
    Link: NoduleLinking,
    <Link::FirstNodule as Nodule>::Output: Send + 'static,
    <Link::SecondNodule as Nodule>::Output: Send + 'static,
    <Link::FirstNodule as Nodule>::Dependency: Clone + 'static,
    <Link::SecondNodule as Nodule>::Dependency: Clone + 'static,
{
    type Input = <Link::FirstNodule as Nodule>::Input;
    type Output = <Link::SecondNodule as Nodule>::Output;
    type State =
        LinkedState<<Link::FirstNodule as Nodule>::State, <Link::SecondNodule as Nodule>::State>;
    type Dependency = LinkedDependency<
        <Link::FirstNodule as Nodule>::Dependency,
        <Link::SecondNodule as Nodule>::Dependency,
    >;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        match Link::FirstNodule::receive(input, &mut state.first, &dependency.first) {
            NodeResult::None => NodeResult::none(),
            NodeResult::Output(first_output) => {
                let Some(second_input) =
                    Link::second_input_from_first_output(&first_output, state, dependency)
                else {
                    return NodeResult::none();
                };
                Link::SecondNodule::receive(&second_input, &mut state.second, &dependency.second)
            }
            NodeResult::Outputs(first_outputs) => {
                let mut result = NodeResult::none();
                for first_output in first_outputs {
                    if let Some(second_input) =
                        Link::second_input_from_first_output(&first_output, state, dependency)
                    {
                        result = result.serial(Link::SecondNodule::receive(
                            &second_input,
                            &mut state.second,
                            &dependency.second,
                        ));
                    }
                }
                result
            }
            NodeResult::Run(run) => {
                let dependency = dependency.clone();
                NodeResult::Run(run.filter_map(move |first_output| {
                    Link::continue_first_run(&first_output, &dependency)
                }))
            }
        }
    }
}

/// Mapping contract for a [`ParentedNodule`] wrapper.
pub trait NoduleParenting {
    /// Nodule whose input and output remain the wrapper's public contract.
    type ParentNodule: Nodule<Output: Send + 'static, Dependency: Clone + Send + 'static>;
    /// Child nodule that also receives mapped parent inputs.
    type ChildNodule: Nodule<Output: Send + 'static, Dependency: Clone + Send + 'static>;

    /// Maps a parent input into a possible child input.
    fn child_input_from_input(
        input: &<Self::ParentNodule as Nodule>::Input,
        state: &ParentedState<
            <Self::ParentNodule as Nodule>::State,
            <Self::ChildNodule as Nodule>::State,
        >,
        dependency: &ParentedDependency<
            <Self::ParentNodule as Nodule>::Dependency,
            <Self::ChildNodule as Nodule>::Dependency,
        >,
    ) -> Option<<Self::ChildNodule as Nodule>::Input>;

    /// Maps one child output back into zero or more parent outputs.
    fn output_from_child_output(
        child_output: &<Self::ChildNodule as Nodule>::Output,
        dependency: &ParentedDependency<
            <Self::ParentNodule as Nodule>::Dependency,
            <Self::ChildNodule as Nodule>::Dependency,
        >,
    ) -> NodeResult<<Self::ParentNodule as Nodule>::Output>;
}

/// Dependency pair for a [`ParentedNodule`] wrapper.
#[derive(Debug, Clone)]
pub struct ParentedDependency<Parent, Child> {
    /// Dependencies for the parent nodule.
    pub parent: Parent,
    /// Dependencies for the child nodule.
    pub child: Child,
}

impl<Parent: Dependency, Child: Dependency> Dependency for ParentedDependency<Parent, Child> {
    fn shared(&self) -> &SharedDependency {
        self.parent.shared()
    }
}

/// State pair for a [`ParentedNodule`] wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentedState<Parent, Child> {
    /// State for the parent nodule.
    pub parent: Parent,
    /// State for the child nodule.
    pub child: Child,
}

/// Wrapper nodule that runs a parent and mapped child for each accepted input.
pub struct ParentedNodule<Parenting>(PhantomData<fn() -> Parenting>);

impl<Parenting> std::fmt::Debug for ParentedNodule<Parenting> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ParentedNodule")
    }
}

impl<Parenting> Nodule for ParentedNodule<Parenting>
where
    Parenting: NoduleParenting,
    <Parenting::ParentNodule as Nodule>::Output: Send + 'static,
    <Parenting::ChildNodule as Nodule>::Output: Send + 'static,
    <Parenting::ParentNodule as Nodule>::Dependency: Clone + 'static,
    <Parenting::ChildNodule as Nodule>::Dependency: Clone + 'static,
{
    type Input = <Parenting::ParentNodule as Nodule>::Input;
    type Output = <Parenting::ParentNodule as Nodule>::Output;
    type State = ParentedState<
        <Parenting::ParentNodule as Nodule>::State,
        <Parenting::ChildNodule as Nodule>::State,
    >;
    type Dependency = ParentedDependency<
        <Parenting::ParentNodule as Nodule>::Dependency,
        <Parenting::ChildNodule as Nodule>::Dependency,
    >;

    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        let mut result =
            Parenting::ParentNodule::receive(input, &mut state.parent, &dependency.parent);

        let Some(child_input) = Parenting::child_input_from_input(input, state, dependency) else {
            return result;
        };

        let child_result =
            Parenting::ChildNodule::receive(&child_input, &mut state.child, &dependency.child);
        let mapped_child_result = match child_result {
            NodeResult::None => NodeResult::none(),
            NodeResult::Output(child_output) => {
                Parenting::output_from_child_output(&child_output, dependency)
            }
            NodeResult::Outputs(child_outputs) => {
                let mut mapped = NodeResult::none();
                for child_output in child_outputs {
                    mapped = mapped.serial(Parenting::output_from_child_output(
                        &child_output,
                        dependency,
                    ));
                }
                mapped
            }
            NodeResult::Run(run) => {
                let dependency = dependency.clone();
                NodeResult::Run(run.flat_map(move |child_output| {
                    Parenting::output_from_child_output(&child_output, &dependency)
                }))
            }
        };
        result = result.serial(mapped_child_result);
        result
    }
}

/// Shape-building phase for a [`Node`].
///
/// Composition is available only in this phase. Adding a custom receive
/// function or a receive decorator moves the builder into
/// [`ConfiguredNodeBuilder`], where further composition is intentionally
/// unavailable.
pub struct NodeBuilder<N>
where
    N: Nodule,
{
    dependency: N::Dependency,
    state: N::State,
}

impl<N> NodeBuilder<N>
where
    N: Nodule,
{
    /// Starts a shape builder with the nodule's own receive function.
    #[must_use]
    pub fn new(dependency: N::Dependency, state: N::State) -> Self {
        Self { dependency, state }
    }

    /// Configures this builder with a custom receive function.
    #[must_use]
    pub fn with_receive(
        self,
        receive: impl Fn(&N::Input, &mut N::State, &N::Dependency) -> NodeResult<N::Output>
        + Send
        + Sync
        + 'static,
    ) -> ConfiguredNodeBuilder<N> {
        ConfiguredNodeBuilder {
            dependency: self.dependency,
            state: self.state,
            receive: NodeReceive::new(receive),
        }
    }

    /// Builds the node and consumes the builder.
    ///
    /// Consuming `self` makes a second build a compile-time error, replacing the
    /// source implementation's runtime assertion.
    ///
    /// ```compile_fail
    /// use bc_date_time::{Epoch, fixed_epoch_source};
    /// use bc_nodular_architecture::{
    ///     Input, NodeBuilder, NodeResult, Nodule, NoduleEventSequence, Output, SharedDependency,
    /// };
    ///
    /// #[derive(PartialEq, Eq)]
    /// struct Event;
    /// impl Input for Event {
    ///     fn sequence(&self) -> NoduleEventSequence { 0 }
    /// }
    /// impl Output for Event {
    ///     fn sequence(&self) -> NoduleEventSequence { 0 }
    /// }
    /// struct Example;
    /// impl Nodule for Example {
    ///     type Input = Event;
    ///     type Output = Event;
    ///     type State = ();
    ///     type Dependency = SharedDependency;
    ///     fn receive(
    ///         _input: &Event,
    ///         _state: &mut (),
    ///         _dependency: &SharedDependency,
    ///     ) -> NodeResult<Event> {
    ///         NodeResult::none()
    ///     }
    /// }
    ///
    /// let dependency = SharedDependency::new(fixed_epoch_source(Epoch::ZERO));
    /// let builder = NodeBuilder::<Example>::new(dependency, ());
    /// let first = builder.build();
    /// let second = builder.build();
    /// # drop((first, second));
    /// ```
    #[must_use]
    pub fn build(self) -> Node<N> {
        Node::new(self.dependency, self.state)
    }

    /// Combines this nodule with an independently routed second nodule.
    #[must_use]
    pub fn combining<Other>(
        self,
        other_dependency: Other::Dependency,
        other_state: Other::State,
    ) -> NodeBuilder<CombinedNodule<N, Other>>
    where
        Other: Nodule,
        N::Output: Send + 'static,
        Other::Output: Send + 'static,
    {
        NodeBuilder::new(
            CombinedDependency {
                first: self.dependency,
                second: other_dependency,
            },
            CombinedState {
                first: self.state,
                second: other_state,
            },
        )
    }

    /// Links this nodule's outputs into a serial second nodule.
    #[must_use]
    pub fn linking<Link>(
        self,
        second_dependency: <Link::SecondNodule as Nodule>::Dependency,
        second_state: <Link::SecondNodule as Nodule>::State,
    ) -> NodeBuilder<LinkedNodule<Link>>
    where
        Link: NoduleLinking<FirstNodule = N>,
        N::Output: Send + 'static,
        N::Dependency: Clone + 'static,
        <Link::SecondNodule as Nodule>::Output: Send + 'static,
        <Link::SecondNodule as Nodule>::Dependency: 'static,
    {
        NodeBuilder::new(
            LinkedDependency {
                first: self.dependency,
                second: second_dependency,
            },
            LinkedState {
                first: self.state,
                second: second_state,
            },
        )
    }

    /// Wraps this parent nodule with a child that receives mapped parent inputs.
    #[must_use]
    pub fn parenting<Parenting>(
        self,
        child_dependency: <Parenting::ChildNodule as Nodule>::Dependency,
        child_state: <Parenting::ChildNodule as Nodule>::State,
    ) -> NodeBuilder<ParentedNodule<Parenting>>
    where
        Parenting: NoduleParenting<ParentNodule = N>,
        N::Output: Send + 'static,
        N::Dependency: Clone + 'static,
        <Parenting::ChildNodule as Nodule>::Output: Send + 'static,
        <Parenting::ChildNodule as Nodule>::Dependency: 'static,
    {
        NodeBuilder::new(
            ParentedDependency {
                parent: self.dependency,
                child: child_dependency,
            },
            ParentedState {
                parent: self.state,
                child: child_state,
            },
        )
    }

    /// Adds a receive function whose result precedes the current receive result.
    #[must_use]
    pub fn receive_before(
        self,
        before: impl Fn(&N::Input, &mut N::State, &N::Dependency) -> NodeResult<N::Output>
        + Send
        + Sync
        + 'static,
    ) -> ConfiguredNodeBuilder<N>
    where
        N: 'static,
        N::Input: 'static,
        N::Output: Send + 'static,
        N::State: 'static,
        N::Dependency: 'static,
    {
        ConfiguredNodeBuilder {
            dependency: self.dependency,
            state: self.state,
            receive: NodeReceive::nodule(),
        }
        .receive_before(before)
    }

    /// Adds a receive function whose result follows the current receive result.
    #[must_use]
    pub fn receive_after(
        self,
        after: impl Fn(&N::Input, &mut N::State, &N::Dependency) -> NodeResult<N::Output>
        + Send
        + Sync
        + 'static,
    ) -> ConfiguredNodeBuilder<N>
    where
        N: 'static,
        N::Input: 'static,
        N::Output: Send + 'static,
        N::State: 'static,
        N::Dependency: 'static,
    {
        ConfiguredNodeBuilder {
            dependency: self.dependency,
            state: self.state,
            receive: NodeReceive::nodule(),
        }
        .receive_after(after)
    }
}

/// Receive-configuration phase for a [`Node`].
///
/// This phase can be decorated and built, but cannot be composed. Build the
/// complete nodule shape with [`NodeBuilder`] before entering this phase.
///
/// ```compile_fail
/// use bc_date_time::{Epoch, fixed_epoch_source};
/// use bc_nodular_architecture::{
///     Input, NodeBuilder, NodeResult, Nodule, NoduleEventSequence, Output, SharedDependency,
/// };
///
/// #[derive(PartialEq, Eq)]
/// struct Event;
/// impl Input for Event {
///     fn sequence(&self) -> NoduleEventSequence { 0 }
/// }
/// impl Output for Event {
///     fn sequence(&self) -> NoduleEventSequence { 0 }
/// }
/// struct Example;
/// impl Nodule for Example {
///     type Input = Event;
///     type Output = Event;
///     type State = ();
///     type Dependency = SharedDependency;
///     fn receive(
///         _input: &Event,
///         _state: &mut (),
///         _dependency: &SharedDependency,
///     ) -> NodeResult<Event> {
///         NodeResult::none()
///     }
/// }
///
/// let dependency = SharedDependency::new(fixed_epoch_source(Epoch::ZERO));
/// let second_dependency = dependency.clone();
/// let configured = NodeBuilder::<Example>::new(dependency, ())
///     .with_receive(|_input, _state, _dependency| NodeResult::none());
/// let invalid = configured.combining::<Example>(second_dependency, ());
/// # drop(invalid);
/// ```
pub struct ConfiguredNodeBuilder<N>
where
    N: Nodule,
{
    dependency: N::Dependency,
    state: N::State,
    receive: NodeReceive<N>,
}

impl<N> ConfiguredNodeBuilder<N>
where
    N: Nodule,
{
    /// Builds the node and consumes the configured builder.
    #[must_use]
    pub fn build(self) -> Node<N> {
        Node::with_receive(self.dependency, self.state, self.receive)
    }

    /// Adds a receive function whose result precedes the current receive result.
    #[must_use]
    pub fn receive_before(
        self,
        before: impl Fn(&N::Input, &mut N::State, &N::Dependency) -> NodeResult<N::Output>
        + Send
        + Sync
        + 'static,
    ) -> Self
    where
        N: 'static,
        N::Input: 'static,
        N::Output: Send + 'static,
        N::State: 'static,
        N::Dependency: 'static,
    {
        let main = self.receive;
        Self {
            dependency: self.dependency,
            state: self.state,
            receive: NodeReceive::new(move |input, state, dependency| {
                let before_result = before(input, state, dependency);
                let main_result = main.call(input, state, dependency);
                before_result.serial(main_result)
            }),
        }
    }

    /// Adds a receive function whose result follows the current receive result.
    #[must_use]
    pub fn receive_after(
        self,
        after: impl Fn(&N::Input, &mut N::State, &N::Dependency) -> NodeResult<N::Output>
        + Send
        + Sync
        + 'static,
    ) -> Self
    where
        N: 'static,
        N::Input: 'static,
        N::Output: Send + 'static,
        N::State: 'static,
        N::Dependency: 'static,
    {
        let main = self.receive;
        Self {
            dependency: self.dependency,
            state: self.state,
            receive: NodeReceive::new(move |input, state, dependency| {
                let main_result = main.call(input, state, dependency);
                let after_result = after(input, state, dependency);
                main_result.serial(after_result)
            }),
        }
    }
}

impl<N> std::fmt::Debug for NodeBuilder<N>
where
    N: Nodule,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NodeBuilder(..)")
    }
}

impl<N> std::fmt::Debug for ConfiguredNodeBuilder<N>
where
    N: Nodule,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConfiguredNodeBuilder(..)")
    }
}
