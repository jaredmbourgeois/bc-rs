#![doc = include_str!("../README.md")]
#![deny(missing_docs)]
// A discard or a lint exception in this crate says why, and stops compiling
// quietly the day it stops being true: `#[expect]` is fulfilled-or-warn, and
// these three make `let _ =` and `#[allow]` say so at the site.
#![warn(
    clippy::let_underscore_must_use,
    clippy::allow_attributes,
    clippy::allow_attributes_without_reason
)]

mod adapters;
mod cli;
mod diagnostics;
mod discover;
mod document;
mod evolution;
mod front;
mod grafana;
mod layout;
mod load;
mod lower;
mod manifest;
mod model;
mod names;
mod output;
mod paths;
mod root;
mod source;
mod validate;
mod walker;

pub use cli::{ExitStatus, run};
