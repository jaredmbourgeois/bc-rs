//! Flat N-way side-by-side nodule composition.
//!
//! [`combined_nodules!`] generates ordinary enums and structs rather than a
//! nested binary composition. Each leaf has its own input/output variant and
//! state/dependency field, so routing remains unambiguous when different leaves
//! use identical value types.

/// One leaf of a flat combination, and the projections that reach it.
///
/// [`combined_nodules!`](crate::combined_nodules) implements this for every leaf
/// it is given, which is
/// what lets the generated input, output, and state types be keyed by leaf type
/// rather than by position. `Combined` is the generated nodule the leaf belongs
/// to, so one nodule can be a leaf of several combinations without ambiguity.
///
/// Implementing this by hand is not supported. It cannot be sealed — a
/// `macro_rules!` macro expands in the caller's crate and so can only implement
/// a trait that crate can name — but the generated variants and fields are the
/// macro's to define, and a hand-written implementation routing a foreign leaf
/// into one of them will misattribute inputs rather than fail to compile.
pub trait CombinedLeaf<Combined>: crate::Nodule
where
    Combined: crate::Nodule,
{
    /// Wraps this leaf's input as the combined input.
    fn wrap_input(input: Self::Input) -> Combined::Input;

    /// Wraps this leaf's output as the combined output.
    fn wrap_output(output: Self::Output) -> Combined::Output;

    /// Borrows this leaf's slot of the combined state.
    fn state(state: &Combined::State) -> &Self::State;

    /// Mutably borrows this leaf's slot of the combined state.
    fn state_mut(state: &mut Combined::State) -> &mut Self::State;
}

/// Generates one flat nodule and its input, output, state, and dependency types.
///
/// Each entry supplies an enum variant, a field name, and a distinct nodule
/// type. Every leaf gets a [`CombinedLeaf`] implementation, which is what makes
/// construction and state access type-keyed:
///
/// ```ignore
/// combined_nodules! {
///     pub AppNodule, AppInput, AppOutput, AppState, AppDependency {
///         Orders(orders): OrdersNodule,
///         Inventory(inventory): InventoryNodule,
///     }
/// }
///
/// let input = AppInput::of::<OrdersNodule>(orders_input);
/// let orders_state = state.of::<OrdersNodule>();
/// let dependency = AppDependency::uniform(shared_dependency);
/// ```
///
/// The C++ combinator also has `of_input`, which deduces a leaf from a unique
/// input value type. A declarative Rust macro cannot provide that deduction and
/// still support repeated input types: the corresponding `From` implementations
/// would overlap under Rust's coherence rules. Use `Input::of::<Leaf>(value)` or
/// construct the generated variant directly; both make leaf identity explicit.
#[macro_export]
macro_rules! combined_nodules {
    (
        $(#[$combined_attribute:meta])*
        $visibility:vis $combined:ident,
        $input:ident,
        $output:ident,
        $state:ident,
        $dependency:ident {
            $first_variant:ident($first_field:ident): $first_leaf:ty
            $(, $variant:ident($field:ident): $leaf:ty)*
            $(,)?
        }
    ) => {
        $(#[$combined_attribute])*
        #[doc = concat!(
            "Flat nodule over ",
            stringify!($first_leaf),
            $(", ", stringify!($leaf),)*
            "."
        )]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        $visibility struct $combined;

        impl $combined {
            /// Number of leaves in this flat nodule.
            pub const SIZE: usize = 1 + [$(stringify!($leaf)),*].len();
        }

        #[doc = concat!("Input routed to one leaf of [`", stringify!($combined), "`].")]
        #[derive(PartialEq)]
        $visibility enum $input {
            #[doc = concat!("Input for [`", stringify!($first_leaf), "`].")]
            $first_variant(<$first_leaf as $crate::Nodule>::Input),
            $(
                #[doc = concat!("Input for [`", stringify!($leaf), "`].")]
                $variant(<$leaf as $crate::Nodule>::Input),
            )*
        }

        impl $input {
            /// Constructs input for an explicitly selected leaf.
            #[must_use]
            pub fn of<Leaf>(input: <Leaf as $crate::Nodule>::Input) -> Self
            where
                Leaf: $crate::CombinedLeaf<$combined>,
            {
                <Leaf as $crate::CombinedLeaf<$combined>>::wrap_input(input)
            }
        }

        impl $crate::Input for $input {
            fn sequence(&self) -> $crate::NoduleEventSequence {
                match self {
                    Self::$first_variant(value) => $crate::Input::sequence(value),
                    $(Self::$variant(value) => $crate::Input::sequence(value),)*
                }
            }
        }

        impl ::std::fmt::Debug for $input {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    Self::$first_variant(_) => formatter.write_str(concat!(
                        stringify!($input),
                        "::",
                        stringify!($first_variant),
                        "(..)",
                    )),
                    $(
                        Self::$variant(_) => formatter.write_str(concat!(
                            stringify!($input),
                            "::",
                            stringify!($variant),
                            "(..)",
                        )),
                    )*
                }
            }
        }

        #[doc = concat!("Output produced by one leaf of [`", stringify!($combined), "`].")]
        #[derive(PartialEq)]
        $visibility enum $output {
            #[doc = concat!("Output from [`", stringify!($first_leaf), "`].")]
            $first_variant(<$first_leaf as $crate::Nodule>::Output),
            $(
                #[doc = concat!("Output from [`", stringify!($leaf), "`].")]
                $variant(<$leaf as $crate::Nodule>::Output),
            )*
        }

        impl $output {
            /// Constructs output for an explicitly selected leaf.
            #[must_use]
            pub fn of<Leaf>(output: <Leaf as $crate::Nodule>::Output) -> Self
            where
                Leaf: $crate::CombinedLeaf<$combined>,
            {
                <Leaf as $crate::CombinedLeaf<$combined>>::wrap_output(output)
            }
        }

        impl $crate::Output for $output {
            fn sequence(&self) -> $crate::NoduleEventSequence {
                match self {
                    Self::$first_variant(value) => $crate::Output::sequence(value),
                    $(Self::$variant(value) => $crate::Output::sequence(value),)*
                }
            }
        }

        impl ::std::fmt::Debug for $output {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    Self::$first_variant(_) => formatter.write_str(concat!(
                        stringify!($output),
                        "::",
                        stringify!($first_variant),
                        "(..)",
                    )),
                    $(
                        Self::$variant(_) => formatter.write_str(concat!(
                            stringify!($output),
                            "::",
                            stringify!($variant),
                            "(..)",
                        )),
                    )*
                }
            }
        }

        #[doc = concat!("State of every leaf in [`", stringify!($combined), "`].")]
        #[derive(PartialEq)]
        $visibility struct $state {
            #[doc = concat!("State of [`", stringify!($first_leaf), "`].")]
            pub $first_field: <$first_leaf as $crate::Nodule>::State,
            $(
                #[doc = concat!("State of [`", stringify!($leaf), "`].")]
                pub $field: <$leaf as $crate::Nodule>::State,
            )*
        }

        impl $state {
            /// Borrows the state belonging to `Leaf`.
            #[must_use]
            pub fn of<Leaf>(&self) -> &<Leaf as $crate::Nodule>::State
            where
                Leaf: $crate::CombinedLeaf<$combined>,
            {
                <Leaf as $crate::CombinedLeaf<$combined>>::state(self)
            }

            /// Mutably borrows the state belonging to `Leaf`.
            #[must_use]
            pub fn of_mut<Leaf>(&mut self) -> &mut <Leaf as $crate::Nodule>::State
            where
                Leaf: $crate::CombinedLeaf<$combined>,
            {
                <Leaf as $crate::CombinedLeaf<$combined>>::state_mut(self)
            }
        }

        impl ::std::default::Default for $state
        where
            <$first_leaf as $crate::Nodule>::State: ::std::default::Default,
            $(<$leaf as $crate::Nodule>::State: ::std::default::Default,)*
        {
            fn default() -> Self {
                Self {
                    $first_field: ::std::default::Default::default(),
                    $($field: ::std::default::Default::default(),)*
                }
            }
        }

        impl ::std::fmt::Debug for $state {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str(concat!(stringify!($state), "(..)"))
            }
        }

        #[doc = concat!("Dependencies of every leaf in [`", stringify!($combined), "`].")]
        $visibility struct $dependency {
            #[doc = concat!("Dependency of [`", stringify!($first_leaf), "`].")]
            pub $first_field: <$first_leaf as $crate::Nodule>::Dependency,
            $(
                #[doc = concat!("Dependency of [`", stringify!($leaf), "`].")]
                pub $field: <$leaf as $crate::Nodule>::Dependency,
            )*
        }

        impl $dependency {
            /// Clones one dependency into every leaf slot.
            #[must_use]
            pub fn uniform<Uniform>(dependency: Uniform) -> Self
            where
                Uniform: ::std::clone::Clone,
                $first_leaf: $crate::Nodule<Dependency = Uniform>,
                $($leaf: $crate::Nodule<Dependency = Uniform>,)*
            {
                Self {
                    $first_field: dependency.clone(),
                    $($field: dependency.clone(),)*
                }
            }
        }

        impl $crate::Dependency for $dependency {
            fn shared(&self) -> &$crate::SharedDependency {
                $crate::Dependency::shared(&self.$first_field)
            }
        }

        impl ::std::fmt::Debug for $dependency {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str(concat!(stringify!($dependency), "(..)"))
            }
        }

        impl $crate::CombinedLeaf<$combined> for $first_leaf {
            fn wrap_input(input: Self::Input) -> $input {
                $input::$first_variant(input)
            }

            fn wrap_output(output: Self::Output) -> $output {
                $output::$first_variant(output)
            }

            fn state(state: &$state) -> &Self::State {
                &state.$first_field
            }

            fn state_mut(state: &mut $state) -> &mut Self::State {
                &mut state.$first_field
            }
        }

        $(
            impl $crate::CombinedLeaf<$combined> for $leaf {
                fn wrap_input(input: Self::Input) -> $input {
                    $input::$variant(input)
                }

                fn wrap_output(output: Self::Output) -> $output {
                    $output::$variant(output)
                }

                fn state(state: &$state) -> &Self::State {
                    &state.$field
                }

                fn state_mut(state: &mut $state) -> &mut Self::State {
                    &mut state.$field
                }
            }
        )*

        impl $crate::Nodule for $combined {
            type Input = $input;
            type Output = $output;
            type State = $state;
            type Dependency = $dependency;

            fn receive(
                input: &Self::Input,
                state: &mut Self::State,
                dependency: &Self::Dependency,
            ) -> $crate::NodeResult<Self::Output> {
                match input {
                    $input::$first_variant(input) => {
                        match <$first_leaf as $crate::Nodule>::receive(
                            input,
                            &mut state.$first_field,
                            &dependency.$first_field,
                        ) {
                            $crate::NodeResult::None => $crate::NodeResult::None,
                            $crate::NodeResult::Output(output) => {
                                $crate::NodeResult::Output($output::$first_variant(output))
                            }
                            $crate::NodeResult::Outputs(outputs) => $crate::NodeResult::Outputs(
                                outputs
                                    .into_iter()
                                    .map($output::$first_variant)
                                    .collect(),
                            ),
                            $crate::NodeResult::Run(run) => {
                                $crate::NodeResult::Run(run)
                                    .map($output::$first_variant)
                            }
                        }
                    }
                    $(
                        $input::$variant(input) => {
                            match <$leaf as $crate::Nodule>::receive(
                                input,
                                &mut state.$field,
                                &dependency.$field,
                            ) {
                                $crate::NodeResult::None => $crate::NodeResult::None,
                                $crate::NodeResult::Output(output) => {
                                    $crate::NodeResult::Output($output::$variant(output))
                                }
                                $crate::NodeResult::Outputs(outputs) => {
                                    $crate::NodeResult::Outputs(
                                        outputs
                                            .into_iter()
                                            .map($output::$variant)
                                            .collect(),
                                    )
                                }
                                $crate::NodeResult::Run(run) => {
                                    $crate::NodeResult::Run(run)
                                        .map($output::$variant)
                                }
                            }
                        }
                    )*
                }
            }
        }
    };
}
