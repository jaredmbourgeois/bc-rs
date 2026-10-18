//! W3C Trace Context propagation: the `traceparent` header.
//!
//! One format, one length, no options — which is why this is the smallest and
//! most self-contained part of the crate.

use bc_core::{decode_hex_into, encode_hex, is_lowercase_hex};

use crate::types::SpanContext;

/// The exact length of a version-`00` `traceparent` header.
///
/// `00-{32 hex}-{16 hex}-{2 hex}`, with three separators.
const TRACEPARENT_LENGTH: usize = 55;

/// Parses a W3C `traceparent` header.
///
/// Returns `None` for anything that is not a well-formed header, including an
/// all-zero trace or span id, which the spec calls invalid.
///
/// Forward-compatible, as [Trace Context §3.2.2.2] requires: a version above
/// `00` is parsed from its first 55 characters rather than rejected, so this
/// process keeps propagating a trace when a peer upgrades. Only version `ff` —
/// which the spec forbids — and malformed input are refused.
///
/// [Trace Context §3.2.2.2]: https://www.w3.org/TR/trace-context/#versioning-of-traceparent
#[must_use]
pub fn parse_traceparent(header: &str) -> Option<SpanContext> {
    // Byte indexing below is only sound on ASCII, and every valid header is
    // ASCII hex and dashes. A multi-byte character also changes the byte length,
    // so the length check very nearly covers this on its own — but only very
    // nearly: 55 bytes of UTF-8 can hold fewer than 55 characters.
    // A future version may be *longer* than 55, and the spec says to read its
    // first 55 characters rather than give up; version 00 must be exactly 55.
    if header.len() < TRACEPARENT_LENGTH || !header.is_ascii() {
        return None;
    }
    let bytes = header.as_bytes();

    if bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
        return None;
    }
    let version = &header[0..2];
    if !is_lowercase_hex(version, 2) {
        return None;
    }
    if version == "00" {
        // The defined version is exact: trailing bytes mean a malformed header,
        // not a newer one.
        if header.len() != TRACEPARENT_LENGTH {
            return None;
        }
    } else {
        // `ff` is reserved and invalid. Any other higher version is read
        // optimistically from the first 55 characters — but only when the field
        // genuinely ends there, which for a longer header means a separator.
        if version == "ff" {
            return None;
        }
        if header.len() > TRACEPARENT_LENGTH && bytes[TRACEPARENT_LENGTH] != b'-' {
            return None;
        }
    }

    // The spec's ABNF is `32HEXDIGLC` / `16HEXDIGLC` — lowercase hex. Accepting
    // uppercase would make this process treat as valid a header a conformant
    // peer rejects, so the two disagree about whether a trace continues.
    if !is_lowercase_hex(&header[3..35], 32)
        || !is_lowercase_hex(&header[36..52], 16)
        || !is_lowercase_hex(&header[53..55], 2)
    {
        return None;
    }

    let mut context = SpanContext::default();
    if !decode_hex_into(&header[3..35], &mut context.trace_id.bytes) {
        return None;
    }
    if !decode_hex_into(&header[36..52], &mut context.span_id.bytes) {
        return None;
    }
    let mut flags = [0_u8; 1];
    if !decode_hex_into(&header[53..55], &mut flags) {
        return None;
    }
    context.trace_flags = flags[0];

    if !context.trace_id.is_valid() || !context.span_id.is_valid() {
        return None;
    }
    Some(context)
}

/// Renders a span context as a W3C `traceparent` header.
#[must_use]
pub fn format_traceparent(context: &SpanContext) -> String {
    let mut rendered = String::with_capacity(TRACEPARENT_LENGTH);
    rendered.push_str("00-");
    rendered.push_str(&context.trace_id.to_hex());
    rendered.push('-');
    rendered.push_str(&context.span_id.to_hex());
    rendered.push('-');
    rendered.push_str(&encode_hex(&[context.trace_flags]));
    rendered
}

/// The identity of [`TraceId`] and [`SpanId`] rendered back, for a reader who
/// wonders whether the two functions above agree.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SpanId, TraceId};

    #[test]
    fn formatting_then_parsing_returns_the_same_context() {
        let context = SpanContext {
            trace_id: TraceId {
                bytes: [
                    0x4b, 0xf9, 0x2f, 0x35, 0x77, 0xb3, 0x4d, 0xa6, 0xa3, 0xce, 0x92, 0x9d, 0x0e,
                    0x0e, 0x47, 0x36,
                ],
            },
            span_id: SpanId {
                bytes: [0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7],
            },
            trace_flags: 1,
        };

        let header = format_traceparent(&context);
        assert_eq!(header.len(), TRACEPARENT_LENGTH);
        assert_eq!(parse_traceparent(&header), Some(context));
    }
}
