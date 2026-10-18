//! Adapted from the source `result_test.cpp`.
//!
//! The C++ `Result<T, E>` class ports to Rust's own `Result`, so most of these
//! tests exercise standard-library behavior. That is the point: the assertions
//! prove the source contract still holds, and the spellings show which standard
//! operation each source accessor became. The `expect_present*` family is the
//! only part with no standard equivalent, so it is the only part this crate
//! still defines.

// Unwrapping a literal `Ok`/`Err` is exactly what these tests are for: they pin
// which standard operation each source accessor became, and what the panic
// messages say. clippy::unnecessary_literal_unwrap would have us delete the
// assertions themselves.
#![allow(clippy::unnecessary_literal_unwrap)]

use bc_error::{AnyError, AnyErrorKind, MetadataDictionary, ResultOptionalExt};

#[test]
fn result_ok_state() {
    let result: Result<i32, String> = Ok(42);
    assert_eq!((result.is_ok(), result.is_err()), (true, false));
    assert_eq!(result.as_ref().ok(), Some(&42));
}

#[test]
fn result_err_state() {
    let result: Result<i32, String> = Err(String::from("bad"));
    assert_eq!((result.is_ok(), result.is_err()), (false, true));
    assert_eq!(result.as_ref().err().map(String::as_str), Some("bad"));
}

#[test]
fn result_value_empty_on_err() {
    let result: Result<i32, String> = Err(String::from("bad"));
    assert_eq!(result.as_ref().ok(), None);
}

#[test]
fn result_error_empty_on_ok() {
    let result: Result<i32, String> = Ok(42);
    assert_eq!(result.as_ref().err(), None);
}

#[test]
fn result_value_or() {
    fn result_for(success: bool) -> Result<i32, String> {
        if success {
            Ok(42)
        } else {
            Err(String::from("bad"))
        }
    }

    assert_eq!(result_for(true).unwrap_or(0), 42);
    assert_eq!(result_for(false).unwrap_or(0), 0);
}

#[test]
fn result_map() {
    let result: Result<i32, String> = Ok(21);
    assert_eq!(result.map(|value| value * 2), Ok(42));
}

#[test]
fn result_map_passes_through_error() {
    let result: Result<i32, String> = Err(String::from("bad"));
    assert_eq!(result.map(|value| value * 2), Err(String::from("bad")));
}

#[test]
fn result_map_err() {
    let result: Result<i32, String> = Err(String::from("not found"));
    assert_eq!(result.map_err(|message| message.len()), Err(9));
}

#[test]
fn result_map_err_passes_through_ok() {
    let result: Result<i32, String> = Ok(42);
    assert_eq!(
        result.map_err(|message| message.len()),
        Ok::<i32, usize>(42)
    );
}

#[test]
fn result_take_value() {
    let result: Result<String, i32> = Ok(String::from("hello"));
    assert_eq!(result.ok(), Some(String::from("hello")));
}

#[test]
fn result_take_error() {
    let result: Result<i32, String> = Err(String::from("bad"));
    assert_eq!(result.err(), Some(String::from("bad")));
}

#[test]
fn result_take_value_empty_on_err() {
    let result: Result<i32, String> = Err(String::from("bad"));
    assert_eq!(result.ok(), None);
}

#[test]
fn result_take_error_empty_on_ok() {
    let result: Result<i32, String> = Ok(42);
    assert_eq!(result.err(), None);
}

#[test]
fn result_equality() {
    let first: Result<i32, String> = Ok(42);
    let same: Result<i32, String> = Ok(42);
    let different: Result<i32, String> = Ok(99);
    let error: Result<i32, String> = Err(String::from("bad"));

    assert_eq!(first, same);
    assert_ne!(first, different);
    assert_ne!(first, error);
}

#[test]
fn result_with_any_error() {
    let ok_result: Result<i32, AnyError> = Ok(42);
    assert_eq!(ok_result.as_ref().ok(), Some(&42));

    let error_result: Result<i32, AnyError> = Err(AnyError::at_location(
        AnyErrorKind::new("parse", 1),
        "loc",
        MetadataDictionary::new(),
    ));
    assert_eq!(
        error_result.as_ref().err().map(AnyError::category),
        Some("parse")
    );
}

#[test]
fn result_move_only_value() {
    let result: Result<Box<i32>, String> = Ok(Box::new(42));
    let taken = result.ok().unwrap();
    assert_eq!(*taken, 42);
}

#[test]
fn result_expect_value_moves_value_out() {
    let result: Result<Box<i32>, String> = Ok(Box::new(42));
    let owned = result.unwrap();
    assert_eq!(*owned, 42);
}

#[test]
fn result_void_ok_state() {
    let result: Result<(), String> = Ok(());
    assert_eq!((result.is_ok(), result.is_err()), (true, false));
}

#[test]
fn result_void_err_state() {
    let result: Result<(), String> = Err(String::from("failed"));
    assert!(result.is_err());
    assert_eq!(result.as_ref().err().map(String::as_str), Some("failed"));
}

#[test]
fn result_void_error_empty_on_ok() {
    let result: Result<(), String> = Ok(());
    assert_eq!(result.as_ref().err(), None);
}

#[test]
fn result_void_equality() {
    let first: Result<(), String> = Ok(());
    let same: Result<(), String> = Ok(());
    let error: Result<(), String> = Err(String::from("x"));
    assert_eq!(first, same);
    assert_ne!(first, error);
}

#[test]
fn result_void_with_any_error() {
    let result: Result<(), AnyError> = Err(AnyError::at_location(
        AnyErrorKind::new("io", 5),
        "loc",
        MetadataDictionary::new(),
    ));
    assert_eq!(result.as_ref().err().map(AnyError::category), Some("io"));
}

#[test]
fn result_expect_present_moves_inner_value_out() {
    let result: Result<Option<String>, i32> = Ok(Some(String::from("row")));
    assert_eq!(result.expect_present(), "row");
}

#[test]
fn result_expect_present_is_move_eligible() {
    let result: Result<Option<Box<i32>>, i32> = Ok(Some(Box::new(7)));
    let owned = result.expect_present();
    assert_eq!(*owned, 7);
}

#[test]
fn result_expect_present_ref_borrows_without_consuming() {
    let result: Result<Option<String>, i32> = Ok(Some(String::from("row")));
    assert_eq!(result.expect_present_ref(), "row");
    assert!(result.is_ok());
    assert_eq!(result.expect_present(), "row");
}

#[test]
fn result_expect_present_ref_mutates_in_place() {
    let mut result: Result<Option<String>, i32> = Ok(Some(String::from("row")));
    *result.expect_present_mut() = String::from("edited");
    assert_eq!(result.expect_present(), "edited");
}

#[test]
#[should_panic(expected = "expect_present called on an Err Result")]
fn result_expect_present_on_err_panics() {
    let result: Result<Option<i32>, String> = Err(String::from("bad"));
    let _ = result.expect_present();
}

#[test]
#[should_panic(expected = "expect_present called on an empty Option")]
fn result_expect_present_on_empty_optional_panics() {
    let result: Result<Option<i32>, String> = Ok(None);
    let _ = result.expect_present();
}

#[test]
#[should_panic(expected = "expect_present_ref called on an Err Result")]
fn result_expect_present_ref_on_err_panics() {
    let result: Result<Option<i32>, String> = Err(String::from("bad"));
    let _ = result.expect_present_ref();
}

// The source's `expect_value` / `expect_error` port to `unwrap` / `unwrap_err`.
// Standard `unwrap` renders the error into the panic message, which the removed
// `ErrorResult::expect_value` discarded — these two assertions pin that gain.
#[test]
#[should_panic(expected = "called `Result::unwrap()` on an `Err` value: \"bad\"")]
fn result_unwrap_on_err_panics_and_reports_the_error() {
    let result: Result<i32, String> = Err(String::from("bad"));
    let _ = result.unwrap();
}

#[test]
#[should_panic(expected = "called `Result::unwrap_err()` on an `Ok` value: 1")]
fn result_unwrap_err_on_ok_panics_and_reports_the_value() {
    let result: Result<i32, String> = Ok(1);
    let _ = result.unwrap_err();
}

#[test]
fn result_expect_error_consumes_error() {
    let result: Result<i32, String> = Err(String::from("bad"));
    assert_eq!(result.unwrap_err(), "bad");
}

#[test]
#[should_panic(expected = "expect_present_ref called on an empty Option")]
fn result_expect_present_ref_on_empty_optional_panics() {
    let result: Result<Option<i32>, String> = Ok(None);
    let _ = result.expect_present_ref();
}

#[test]
#[should_panic(expected = "expect_present_mut called on an Err Result")]
fn result_expect_present_mut_on_err_panics() {
    let mut result: Result<Option<i32>, String> = Err(String::from("bad"));
    let _ = result.expect_present_mut();
}

#[test]
#[should_panic(expected = "expect_present_mut called on an empty Option")]
fn result_expect_present_mut_on_empty_optional_panics() {
    let mut result: Result<Option<i32>, String> = Ok(None);
    let _ = result.expect_present_mut();
}

// The void `map_err` the source added late: re-homing an error needs no
// longhand, and an ok result passes through untouched.
#[test]
fn result_void_map_err() {
    let result: Result<(), i32> = Err(7);
    let mapped = result.map_err(|code| format!("code {code}"));
    assert_eq!(mapped, Err(String::from("code 7")));
}

#[test]
fn result_void_map_err_passes_through_ok() {
    let result: Result<(), i32> = Ok(());
    let mapped = result.map_err(|code| format!("code {code}"));
    assert_eq!(mapped, Ok(()));
}

#[test]
fn result_void_map_err_moves_the_error() {
    let result: Result<(), String> = Err(String::from("moved"));
    let mapped = result.map_err(|message| message + "!");
    assert_eq!(mapped, Err(String::from("moved!")));
}

// fidelity-compile-proof: `ResultTest.ExpectPresentRefRefusesATemporaryTheBorrowWouldOutlive`.
// The borrow is tied to the result it reads, so the compiler refuses one that
// would outlive a temporary — the dangling reference the source deletes its
// rvalue overloads to prevent. This item pins that signature.
const _: for<'result> fn(&'result Result<Option<i32>, String>) -> &'result i32 =
    <Result<Option<i32>, String> as ResultOptionalExt<i32, String>>::expect_present_ref;
