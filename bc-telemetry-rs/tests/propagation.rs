//! Ported from `libs/platform/telemetry/test/propagation_test.cpp`.

use bc_telemetry::{
    SpanContext, SpanId, TRACE_FLAG_SAMPLED, TraceId, format_traceparent, parse_traceparent,
};

/// The W3C specification's own example header.
const SAMPLE: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

fn sample_context() -> SpanContext {
    SpanContext {
        trace_id: TraceId {
            bytes: [
                0x4b, 0xf9, 0x2f, 0x35, 0x77, 0xb3, 0x4d, 0xa6, 0xa3, 0xce, 0x92, 0x9d, 0x0e, 0x0e,
                0x47, 0x36,
            ],
        },
        span_id: SpanId {
            bytes: [0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7],
        },
        trace_flags: TRACE_FLAG_SAMPLED,
    }
}

#[test]
fn format_traceparent_renders_the_spec_example() {
    assert_eq!(format_traceparent(&sample_context()), SAMPLE);
}

#[test]
fn parse_valid_traceparent() {
    assert_eq!(parse_traceparent(SAMPLE), Some(sample_context()));
}

#[test]
fn round_trip() {
    let parsed = parse_traceparent(SAMPLE).expect("the spec example parses");
    assert_eq!(format_traceparent(&parsed), SAMPLE);
}

#[test]
fn uppercase_hex_is_rejected_as_the_spec_requires() {
    // The ABNF is `32HEXDIGLC` / `16HEXDIGLC` — lowercase only. This case used
    // to assert the opposite, under the name `parse_is_case_insensitive`, which
    // locked in a disagreement with every conformant peer: they start a new
    // trace where this process continued the old one.
    let upper = "00-4BF92F3577B34DA6A3CE929D0E0E4736-00F067AA0BA902B7-01";
    assert_eq!(parse_traceparent(upper), None);
    // Mixed case in any single field is equally invalid.
    assert_eq!(
        parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00F067AA0BA902B7-01"),
        None
    );
    // Formatting has exactly one spelling, and it is the valid one.
    assert_eq!(format_traceparent(&sample_context()), SAMPLE);
}

#[test]
fn reject_wrong_length() {
    assert_eq!(parse_traceparent(""), None);
    assert_eq!(parse_traceparent(&SAMPLE[..54]), None);
    // Version 00 is exactly 55: a trailing byte is malformed, not a newer
    // version, because the version field still says 00.
    assert_eq!(parse_traceparent(&format!("{SAMPLE}0")), None);
}

#[test]
fn a_future_version_is_parsed_from_its_first_55_characters() {
    // Trace Context §3.2.2.2: a higher version SHOULD be parsed rather than
    // refused, or every trace crossing this process breaks the day a peer
    // upgrades. This case used to assert the refusal, as `reject_non_version_00`.
    assert_eq!(
        parse_traceparent(&format!("01{}", &SAMPLE[2..])),
        Some(sample_context())
    );
    // A longer future header is read from its first 55 characters, and the
    // field must genuinely end there.
    assert_eq!(
        parse_traceparent(&format!("01{}-extra", &SAMPLE[2..])),
        Some(sample_context())
    );
    assert_eq!(
        parse_traceparent(&format!("01{}xextra", &SAMPLE[2..])),
        None
    );
}

#[test]
fn version_ff_is_reserved_and_refused() {
    assert_eq!(parse_traceparent(&format!("ff{}", &SAMPLE[2..])), None);
    // The version field is itself lowercase hex.
    assert_eq!(parse_traceparent(&format!("0G{}", &SAMPLE[2..])), None);
}

#[test]
fn reject_bad_delimiters() {
    for position in [2_usize, 35, 52] {
        let mut broken: Vec<u8> = SAMPLE.bytes().collect();
        broken[position] = b'x';
        let broken = String::from_utf8(broken).expect("ascii stays utf-8");
        assert_eq!(parse_traceparent(&broken), None, "position {position}");
    }
}

#[test]
fn reject_invalid_hex() {
    let mut broken: Vec<u8> = SAMPLE.bytes().collect();
    broken[3] = b'g';
    let broken = String::from_utf8(broken).expect("ascii stays utf-8");
    assert_eq!(parse_traceparent(&broken), None);
}

#[test]
fn reject_all_zero_trace_id() {
    let header = "00-00000000000000000000000000000000-00f067aa0ba902b7-01";
    assert_eq!(parse_traceparent(header), None);
}

#[test]
fn reject_all_zero_span_id() {
    let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01";
    assert_eq!(parse_traceparent(header), None);
}

#[test]
fn not_sampled_flag_parses() {
    let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";
    let parsed = parse_traceparent(header).expect("a zero flag is still a valid header");
    assert!(!parsed.is_sampled());
    assert_eq!(parsed.trace_flags, 0);
}

#[test]
fn format_not_sampled() {
    let context = SpanContext {
        trace_flags: 0,
        ..sample_context()
    };
    assert!(format_traceparent(&context).ends_with("-00"));
}

#[test]
fn a_multibyte_header_of_the_right_byte_length_is_refused() {
    // Rust-added. The length check counts bytes; a 55-byte UTF-8 string can hold
    // fewer than 55 characters, and byte-indexing it would slice a character.
    // Two ASCII bytes replaced by one two-byte character: 55 bytes, 54 chars.
    let header = format!("00-\u{e9}{}", &SAMPLE[5..]);
    assert_eq!(header.len(), 55);
    assert_eq!(header.chars().count(), 54);
    assert_eq!(parse_traceparent(&header), None);
}
