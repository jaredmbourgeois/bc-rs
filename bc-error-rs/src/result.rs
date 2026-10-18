//! Extensions to [`std::result::Result`].
//!
//! The C++ source's `Result<T, E>` class ports to Rust's own
//! [`std::result::Result`]; its accessors port to their standard spellings
//! (`is_ok`, `ok`, `err`, `unwrap`, `unwrap_err`, `map`, `map_err`,
//! `unwrap_or`). Only the nested `Result<Option<T>, E>` unwrap has no standard
//! equivalent, so only that one is defined here.

/// Checked access to the inner value of a `Result<Option<T>, E>`.
///
/// The shape is the source's detail-read idiom: the outer `Result` answers "did
/// the read succeed", the inner `Option` answers "did the row exist".
pub trait ResultOptionalExt<T, E>: Sized {
    /// Consumes the result and returns its present inner value.
    ///
    /// # Panics
    ///
    /// Panics if the result is an error or contains `None`.
    #[must_use]
    #[track_caller]
    fn expect_present(self) -> T;

    /// Borrows the present inner value without consuming the result.
    ///
    /// # Panics
    ///
    /// Panics if the result is an error or contains `None`.
    #[must_use]
    #[track_caller]
    fn expect_present_ref(&self) -> &T;

    /// Mutably borrows the present inner value without consuming the result.
    ///
    /// # Panics
    ///
    /// Panics if the result is an error or contains `None`.
    #[must_use]
    #[track_caller]
    fn expect_present_mut(&mut self) -> &mut T;
}

// clippy::match_wild_err_arm wants the error rendered into the panic message.
// It cannot be: `E` carries no `Debug` bound, and adding one would narrow the
// trait to error types that have it. The source's `assert(is_ok())` is equally
// blind, and the messages below are pinned by #[should_panic] tests.
#[allow(clippy::match_wild_err_arm)]
impl<T, E> ResultOptionalExt<T, E> for std::result::Result<Option<T>, E> {
    #[track_caller]
    fn expect_present(self) -> T {
        match self {
            Ok(Some(value)) => value,
            Ok(None) => panic!("ResultOptionalExt::expect_present called on an empty Option"),
            Err(_) => panic!("ResultOptionalExt::expect_present called on an Err Result"),
        }
    }

    #[track_caller]
    fn expect_present_ref(&self) -> &T {
        match self {
            Ok(Some(value)) => value,
            Ok(None) => {
                panic!("ResultOptionalExt::expect_present_ref called on an empty Option")
            }
            Err(_) => panic!("ResultOptionalExt::expect_present_ref called on an Err Result"),
        }
    }

    #[track_caller]
    fn expect_present_mut(&mut self) -> &mut T {
        match self {
            Ok(Some(value)) => value,
            Ok(None) => {
                panic!("ResultOptionalExt::expect_present_mut called on an empty Option")
            }
            Err(_) => panic!("ResultOptionalExt::expect_present_mut called on an Err Result"),
        }
    }
}
