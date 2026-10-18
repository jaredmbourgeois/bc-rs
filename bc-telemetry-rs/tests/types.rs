//! Ported from `libs/platform/telemetry/test/types_test.cpp`.

use bc_date_time::Epoch;
use bc_telemetry::{
    AttributeValue, Attributes, Span, SpanContext, SpanEvent, SpanId, SpanKind, StatusCode,
    TRACE_FLAG_SAMPLED, TraceId, fixed_id_source, system_id_source,
};

#[test]
fn trace_id_default_is_invalid() {
    assert!(!TraceId::default().is_valid());
}

#[test]
fn trace_id_non_zero_is_valid() {
    let mut id = TraceId::default();
    id.bytes[15] = 1;
    assert!(id.is_valid());
}

#[test]
fn trace_id_to_hex() {
    let id = TraceId {
        bytes: [
            0x4b, 0xf9, 0x2f, 0x35, 0x77, 0xb3, 0x4d, 0xa6, 0xa3, 0xce, 0x92, 0x9d, 0x0e, 0x0e,
            0x47, 0x36,
        ],
    };
    assert_eq!(id.to_hex(), "4bf92f3577b34da6a3ce929d0e0e4736");
}

#[test]
fn trace_id_to_hex_length() {
    assert_eq!(TraceId::default().to_hex().len(), 32);
}

#[test]
fn trace_id_equality() {
    let mut left = TraceId::default();
    let right = TraceId::default();
    assert_eq!(left, right);
    left.bytes[0] = 9;
    assert_ne!(left, right);
}

#[test]
fn span_id_default_is_invalid() {
    assert!(!SpanId::default().is_valid());
}

#[test]
fn span_id_non_zero_is_valid() {
    let mut id = SpanId::default();
    id.bytes[7] = 1;
    assert!(id.is_valid());
}

#[test]
fn span_id_to_hex() {
    let id = SpanId {
        bytes: [0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7],
    };
    assert_eq!(id.to_hex(), "00f067aa0ba902b7");
}

#[test]
fn span_id_to_hex_length() {
    assert_eq!(SpanId::default().to_hex().len(), 16);
}

#[test]
fn span_id_equality() {
    let mut left = SpanId::default();
    let right = SpanId::default();
    assert_eq!(left, right);
    left.bytes[0] = 9;
    assert_ne!(left, right);
}

#[test]
fn span_context_default_is_not_sampled() {
    assert!(!SpanContext::default().is_sampled());
}

#[test]
fn span_context_sampled_flag() {
    let context = SpanContext {
        trace_flags: TRACE_FLAG_SAMPLED,
        ..SpanContext::default()
    };
    assert!(context.is_sampled());
}

#[test]
fn span_context_sampled_flag_with_other_bits() {
    // Only the low bit means sampled; a future flag beside it must not change
    // the answer.
    let context = SpanContext {
        trace_flags: TRACE_FLAG_SAMPLED | 0b1111_0000,
        ..SpanContext::default()
    };
    assert!(context.is_sampled());

    let other_bits_only = SpanContext {
        trace_flags: 0b1111_0000,
        ..SpanContext::default()
    };
    assert!(!other_bits_only.is_sampled());
}

#[test]
fn span_context_equality() {
    let left = SpanContext::default();
    let mut right = SpanContext::default();
    assert_eq!(left, right);
    right.trace_flags = 1;
    assert_ne!(left, right);
}

#[test]
fn span_event_construction() {
    let event = SpanEvent {
        name: "cache.miss".to_owned(),
        timestamp: Epoch::from_millis(7),
        attributes: vec![("key".to_owned(), AttributeValue::Int64(1))],
    };
    assert_eq!(event.name, "cache.miss");
    assert_eq!(event.timestamp, Epoch::from_millis(7));
    assert_eq!(event.attributes.len(), 1);
}

#[test]
fn span_default_values() {
    let span = Span::default();
    assert!(span.name.is_empty(), "{:?}", span.name);
    assert_eq!(span.kind, SpanKind::Internal);
    assert_eq!(span.status, StatusCode::Unset);
    assert!(span.status_message.is_empty(), "{:?}", span.status_message);
    assert_eq!(span.start_time, Epoch::ZERO);
    assert_eq!(span.end_time, Epoch::ZERO);
    assert!(span.parent_span_id.is_none());
    assert!(span.attributes.is_empty(), "{:?}", span.attributes);
    assert!(span.events.is_empty(), "{:?}", span.events);
}

// The four `GenerateTraceId` / `GenerateSpanId` cases, rewritten against the
// injectable `IdSource` the port routes generation through.

#[test]
fn the_system_id_source_produces_a_valid_trace_id() {
    assert!(system_id_source().trace_id().is_valid());
}

#[test]
fn the_system_id_source_produces_unique_trace_ids() {
    let source = system_id_source();
    let first = source.trace_id();
    let second = source.trace_id();
    assert_ne!(first, second);
}

#[test]
fn the_system_id_source_produces_a_valid_span_id() {
    assert!(system_id_source().span_id().is_valid());
}

#[test]
fn the_system_id_source_produces_unique_span_ids() {
    let source = system_id_source();
    assert_ne!(source.span_id(), source.span_id());
}

#[test]
fn a_fixed_id_source_is_deterministic() {
    // The seam the C++ has no analogue for, and the reason every span-producing
    // case in this suite can assert an exact id.
    let source = fixed_id_source(0xAB);
    assert_eq!(source.trace_id().bytes, [0xAB; 16]);
    assert_eq!(source.span_id().bytes, [0xAB; 8]);
}

#[test]
fn span_kind_values() {
    // Discriminants are part of the contract: they cross a wire.
    assert_eq!(SpanKind::Internal as u8, 0);
    assert_eq!(SpanKind::Server as u8, 1);
    assert_eq!(SpanKind::Client as u8, 2);
    assert_eq!(SpanKind::Producer as u8, 3);
    assert_eq!(SpanKind::Consumer as u8, 4);
}

#[test]
fn status_code_values() {
    assert_eq!(StatusCode::Unset as u8, 0);
    assert_eq!(StatusCode::Ok as u8, 1);
    assert_eq!(StatusCode::Error as u8, 2);
}

#[test]
fn attribute_value_bool_variant() {
    assert_eq!(AttributeValue::from(true), AttributeValue::Boolean(true));
}

#[test]
fn attribute_value_int64_variant() {
    assert_eq!(AttributeValue::from(-5_i64), AttributeValue::Int64(-5));
}

#[test]
fn attribute_value_float64_variant() {
    assert_eq!(AttributeValue::from(1.5_f64), AttributeValue::Float64(1.5));
}

#[test]
fn attribute_value_string_variant() {
    assert_eq!(
        AttributeValue::from("text"),
        AttributeValue::String("text".to_owned())
    );
    let attributes: Attributes = vec![("k".to_owned(), AttributeValue::from("v"))];
    assert_eq!(attributes.len(), 1);
}
