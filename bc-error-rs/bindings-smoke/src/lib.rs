//! Re-exports the bound crate so its uniffi scaffolding lands in a `cdylib`.
//!
//! The scaffolding's exported symbols live in `bc_error`; a `cdylib` that
//! depends on it carries them, which is all a binding generator and a foreign
//! caller need.
pub use bc_error::*;
