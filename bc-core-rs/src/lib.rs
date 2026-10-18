#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod arena;
mod assertion;
mod contiguous_map;
mod hex;
mod percent_format;
mod url_encode;
mod xml_escape;

pub use arena::{Arena, ArenaRef};
pub use assertion::{
    AssertionFailure, AssertionHandler, aborting_assertion_handler, report_assertion_failure,
    report_assertion_failure_at, set_assertion_handler,
};
pub use contiguous_map::ContiguousMap;
pub use hex::{
    SHORT_HEX_WIDTH, abbreviate_hex, decode_hex, decode_hex_into, encode_hex, hex_digit_value,
    is_lowercase_hex,
};
pub use percent_format::basis_points_to_percent_string;
pub use url_encode::{form_urldecode, form_urlencode, percent_decode, percent_encode};
pub use xml_escape::{escape_xml, escape_xml_content, is_xml_char};
