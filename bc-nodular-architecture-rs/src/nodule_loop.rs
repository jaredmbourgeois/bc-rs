//! Feedback-capable nodule contract.

use crate::Nodule;

/// Nodule whose outputs may produce immediate feedback inputs.
pub trait NoduleLoop: Nodule {
    /// Maps an observed output to the next input, or ends that feedback path.
    fn input_from_output(
        output: &Self::Output,
        state: &Self::State,
        dependency: &Self::Dependency,
    ) -> Option<Self::Input>;
}
