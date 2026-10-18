//! XML escaping, against `libs/foundation/core/test/xml_escape_test.cpp`.
//!
//! Each test is named for the property it asserts rather than for the C++ case
//! it came from. The tests account for each of the original's 7 cases.
//!
//! The tests below the `escape_xml` block cover `escape_xml_content` and
//! `is_xml_char`, which have no canonical case because the C++ original has no
//! counterpart: they are additions this crate makes for a writer that
//! puts caller text only between tags, and the README says what each does.

use bc_core::{escape_xml, escape_xml_content, is_xml_char};

#[test]
fn escaping_nothing_gives_an_empty_string() {
    assert_eq!(escape_xml(""), "");
}

#[test]
fn text_with_nothing_reserved_in_it_passes_through() {
    assert_eq!(escape_xml("part-42 / rev_3"), "part-42 / rev_3");
}

#[test]
fn each_of_the_five_predefined_entities_is_escaped() {
    assert_eq!(escape_xml("&"), "&amp;");
    assert_eq!(escape_xml("<"), "&lt;");
    assert_eq!(escape_xml(">"), "&gt;");
    assert_eq!(escape_xml("\""), "&quot;");
    assert_eq!(escape_xml("'"), "&apos;");
}

#[test]
fn a_mix_of_reserved_and_plain_characters_keeps_its_order() {
    assert_eq!(escape_xml("a<b>&\"'c"), "a&lt;b&gt;&amp;&quot;&apos;c");
}

#[test]
fn an_already_escaped_ampersand_is_escaped_again() {
    // Escaping is not idempotent by design: the input is text, not markup, so a
    // literal "&amp;" is five characters whose first one needs escaping.
    assert_eq!(escape_xml("&amp;"), "&amp;amp;");
}

#[test]
fn an_apostrophe_takes_the_xml_entity_and_never_the_html_form() {
    // The contract separating this from an HTML escaper: "&apos;" is not a named
    // HTML4 entity, so an HTML escaper must spell it "&#x27;" instead.
    let escaped = escape_xml("it's");
    assert_eq!(escaped, "it&apos;s");
    assert!(!escaped.contains("&#x27;"));
}

#[test]
fn escaping_no_content_gives_an_empty_string() {
    assert_eq!(escape_xml_content(""), "");
}

#[test]
fn element_content_escapes_the_three_that_carry_meaning_between_tags() {
    assert_eq!(escape_xml_content("a<b>&c"), "a&lt;b&gt;&amp;c");
}

#[test]
fn element_content_leaves_the_quotes_alone_and_that_is_the_whole_difference() {
    // The one property this function exists for. Asserted against its sibling
    // rather than spelled twice, so the two cannot drift into agreeing.
    let quoted = "say \"hi\" and 'bye'";
    assert_eq!(escape_xml_content(quoted), quoted);
    assert_ne!(escape_xml_content(quoted), escape_xml(quoted));

    // And they agree wherever no quote is involved.
    let unquoted = "a<b>&c";
    assert_eq!(escape_xml_content(unquoted), escape_xml(unquoted));
}

#[test]
fn an_ampersand_beside_a_bracket_is_escaped_once_each() {
    // The ordering trap a replace-three-times implementation has to get right:
    // handling `<` before `&` renders it `&amp;lt;`, which parses back to the
    // four characters `&lt;` rather than to `<` — a value silently corrupted in
    // a document that still parses.
    let escaped = escape_xml_content("&<");
    assert_eq!(escaped, "&amp;&lt;");
    assert!(!escaped.contains("&amp;lt;"));
}

#[test]
fn an_already_escaped_ampersand_is_escaped_again_in_content_too() {
    assert_eq!(escape_xml_content("&amp;"), "&amp;amp;");
}

#[test]
fn xml_holds_no_c0_control_but_tab_newline_and_return() {
    for code_point in 0u32..0x20 {
        let character = char::from_u32(code_point).expect("every C0 code point is a char");
        let is_allowed_whitespace = matches!(character, '\t' | '\n' | '\r');
        assert_eq!(
            is_xml_char(character),
            is_allowed_whitespace,
            "U+{code_point:04X}"
        );
    }
}

#[test]
fn del_and_the_c1_block_are_legal_because_this_is_xml_1_0() {
    // Only XML 1.1 asks for these escaped. Refusing them here would turn text a
    // conforming 1.0 parser accepts into an error.
    for character in ['\u{7F}', '\u{80}', '\u{85}', '\u{9F}'] {
        assert!(is_xml_char(character), "{character:?}");
    }
}

#[test]
fn the_two_noncharacters_below_the_bmp_boundary_are_refused() {
    // The pair a `< 0x20` control check misses: the production stops at #xFFFD,
    // so U+FFFE and U+FFFF are ordinary `char` values XML cannot carry.
    assert!(!is_xml_char('\u{FFFE}'));
    assert!(!is_xml_char('\u{FFFF}'));

    // Both sides of each boundary, so the range cannot be off by one.
    assert!(is_xml_char('\u{FFFD}'));
    assert!(is_xml_char('\u{10000}'));
    assert!(is_xml_char('\u{10FFFF}'));
}

#[test]
fn ordinary_text_is_representable_end_to_end() {
    // The anti-vacuity half: a predicate that refused everything would pass
    // every assertion above.
    for character in " aZ0~é🙂\u{D7FF}\u{E000}".chars() {
        assert!(is_xml_char(character), "{character:?}");
    }
    assert!(escape_xml_content("plain text").chars().all(is_xml_char));
}

#[test]
fn a_control_character_becomes_the_replacement_character() {
    assert_eq!(escape_xml("a\u{0}b"), "a\u{FFFD}b");
    assert_eq!(escape_xml("x\u{1b}[0m"), "x\u{FFFD}[0m");
    assert_eq!(escape_xml("del\u{7f}"), "del\u{FFFD}");
    // TAB, LF and CR survive in text content.
    assert_eq!(escape_xml("line\r\n\tnext"), "line\r\n\tnext");
}
