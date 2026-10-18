//! Conformance against the `JSONTestSuite` corpus (`tests/fixtures/JSONTestSuite`,
//! MIT, revision in `REVISION`).
//!
//! The corpus names each case by what a parser should do with it: `y_` must be
//! accepted, `n_` must be rejected, and `i_` is left to the implementation. That
//! third group is where a strict validator has to state a policy, so every one
//! of those cases is listed below with the policy that decides it.
//!
//! This is also where the README's "stricter than `serde_json`" claim is
//! measured rather than asserted.

use std::fs;

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/JSONTestSuite/test_parsing"
);

/// The four `y_` cases this crate deliberately rejects.
///
/// RFC 8259 permits duplicate object keys and leaves the outcome undefined, so
/// the corpus marks them as "must accept". Rejecting them is the whole reason
/// the lazy reader is sound: it returns the first member matching a key, which
/// is only unambiguous once no key appears twice.
///
/// The `\u0000` escape is likewise valid by the grammar and decodes to a NUL byte —
/// which every C-string boundary downstream truncates at rather than failing —
/// so the decoder refuses it (`JsonValidationErrorKind::NulCharacter`) and the
/// two corpus cases that carry it are rejected on purpose.
const DELIBERATELY_REJECTED_Y_CASES: &[&str] = &[
    "y_object_duplicated_key.json",
    "y_object_duplicated_key_and_value.json",
    "y_object_escaped_null_in_key.json",
    "y_string_null_escape.json",
];

/// Implementation-defined cases this crate **accepts**: numbers that are valid
/// by the grammar but whose magnitude no `f64` can hold.
///
/// The validator's job is syntax. Range belongs to whoever reads the value, and
/// [`JsonReader::floating`] refuses to hand back an infinity — so accepting the
/// document costs nothing and rejecting it would mean the validator second-
/// guessing a type it does not know the caller wants.
const ACCEPTED_IMPLEMENTATION_DEFINED: &[&str] = &[
    "i_number_double_huge_neg_exp.json",
    "i_number_huge_exp.json",
    "i_number_neg_int_huge_exp.json",
    "i_number_pos_double_huge_exp.json",
    "i_number_real_neg_overflow.json",
    "i_number_real_pos_overflow.json",
    "i_number_real_underflow.json",
    "i_number_too_big_neg_int.json",
    "i_number_too_big_pos_int.json",
    "i_number_very_big_negative_int.json",
];

/// Implementation-defined cases this crate **rejects**: text that is not UTF-8,
/// surrogates that do not pair, a byte-order mark, and nesting past the limit.
///
/// Every one of these is a case where a permissive parser substitutes a
/// replacement character or silently truncates. At an untrusted boundary that
/// turns malformed input into plausible data, which is exactly what this gate
/// exists to prevent.
const REJECTED_IMPLEMENTATION_DEFINED: &[&str] = &[
    "i_object_key_lone_2nd_surrogate.json",
    "i_string_1st_surrogate_but_2nd_missing.json",
    "i_string_1st_valid_surrogate_2nd_invalid.json",
    "i_string_UTF-16LE_with_BOM.json",
    "i_string_UTF-8_invalid_sequence.json",
    "i_string_UTF8_surrogate_U+D800.json",
    "i_string_incomplete_surrogate_and_escape_valid.json",
    "i_string_incomplete_surrogate_pair.json",
    "i_string_incomplete_surrogates_escape_valid.json",
    "i_string_invalid_lonely_surrogate.json",
    "i_string_invalid_surrogate.json",
    "i_string_invalid_utf-8.json",
    "i_string_inverted_surrogates_U+1D11E.json",
    "i_string_iso_latin_1.json",
    "i_string_lone_second_surrogate.json",
    "i_string_lone_utf8_continuation_byte.json",
    "i_string_not_in_unicode_range.json",
    "i_string_overlong_sequence_2_bytes.json",
    "i_string_overlong_sequence_6_bytes.json",
    "i_string_overlong_sequence_6_bytes_null.json",
    "i_string_truncated-utf-8.json",
    "i_string_utf16BE_no_BOM.json",
    "i_string_utf16LE_no_BOM.json",
    "i_structure_500_nested_arrays.json",
    "i_structure_UTF-8_BOM_empty_object.json",
];

/// Every case in the corpus, as (file name, bytes).
fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut cases: Vec<_> = fs::read_dir(CORPUS)
        .expect("the vendored corpus must be present")
        .map(|entry| {
            let path = entry.expect("readable directory entry").path();
            let name = path
                .file_name()
                .expect("a file")
                .to_string_lossy()
                .into_owned();
            (name, fs::read(&path).expect("readable case"))
        })
        .collect();
    cases.sort_by(|left, right| left.0.cmp(&right.0));
    cases
}

#[test]
fn every_y_case_is_accepted_except_the_documented_four() {
    let mut checked = 0;
    for (name, bytes) in corpus() {
        if !name.starts_with("y_") {
            continue;
        }
        checked += 1;
        let accepted = bc_json::validate_json_bytes(&bytes).is_ok();
        if DELIBERATELY_REJECTED_Y_CASES.contains(&name.as_str()) {
            assert!(!accepted, "{name} is meant to be rejected");
        } else {
            assert!(accepted, "{name} must be accepted");
        }
    }
    assert_eq!(checked, 95, "the corpus should hold 95 y_ cases");
}

#[test]
fn every_n_case_is_rejected() {
    let mut checked = 0;
    for (name, bytes) in corpus() {
        if !name.starts_with("n_") {
            continue;
        }
        checked += 1;
        assert!(
            bc_json::validate_json_bytes(&bytes).is_err(),
            "{name} must be rejected"
        );
    }
    assert_eq!(checked, 188, "the corpus should hold 188 n_ cases");
}

#[test]
fn every_i_case_has_a_recorded_disposition() {
    let present: Vec<String> = corpus()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with("i_"))
        .collect();

    let mut recorded: Vec<&str> = ACCEPTED_IMPLEMENTATION_DEFINED
        .iter()
        .chain(REJECTED_IMPLEMENTATION_DEFINED)
        .copied()
        .collect();
    recorded.sort_unstable();

    // Neither list may drift from the corpus: an unlisted case is an undecided
    // policy, and a listed case that no longer exists is a stale claim.
    assert_eq!(
        present, recorded,
        "every i_ case needs exactly one disposition"
    );

    for (name, bytes) in corpus() {
        let accepted = bc_json::validate_json_bytes(&bytes).is_ok();
        if ACCEPTED_IMPLEMENTATION_DEFINED.contains(&name.as_str()) {
            assert!(accepted, "{name} is recorded as accepted");
        } else if REJECTED_IMPLEMENTATION_DEFINED.contains(&name.as_str()) {
            assert!(!accepted, "{name} is recorded as rejected");
        }
    }
}

#[test]
fn the_strictness_delta_against_serde_json_is_exactly_what_the_readme_claims() {
    let mut stricter = Vec::new();
    let mut looser = Vec::new();

    for (name, bytes) in corpus() {
        let ours = bc_json::validate_json_bytes(&bytes).is_ok();
        let theirs = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
        if theirs && !ours {
            stricter.push(name);
        } else if ours && !theirs {
            looser.push(name);
        }
    }

    // Stricter, and only here: duplicate keys and the `\u0000` escape.
    assert_eq!(stricter, DELIBERATELY_REJECTED_Y_CASES);

    // Looser, and only here: `serde_json` rejects a number whose exponent
    // overflows `f64` at parse time, because it parses to a value. This crate
    // validates the grammar and leaves range to the reader, which is the
    // source's behavior too — its own suite asserts that `1e400` validates.
    assert_eq!(
        looser,
        [
            "i_number_huge_exp.json",
            "i_number_neg_int_huge_exp.json",
            "i_number_pos_double_huge_exp.json",
            "i_number_real_neg_overflow.json",
            "i_number_real_pos_overflow.json",
        ]
    );
}

#[test]
fn pathological_nesting_is_bounded_rather_than_fatal() {
    // The corpus carries 100,000 opening brackets and a 250 KB open array of
    // objects. A recursive validator without a depth limit dies on the stack
    // here; this is the test that the limit is what stands between the two.
    for name in [
        "n_structure_100000_opening_arrays.json",
        "n_structure_open_array_object.json",
    ] {
        let bytes = fs::read(format!("{CORPUS}/{name}")).expect("case present");
        let error = bc_json::validate_json_bytes(&bytes).expect_err("must be rejected");
        assert_eq!(
            error.kind,
            bc_json::JsonValidationErrorKind::NestingTooDeep,
            "{name}"
        );
    }
}
