//! The five XML predefined entities, the element-content subset of them, and
//! what XML can represent at all.

/// Escapes text for inclusion in an XML or SVG document.
///
/// The canonical XML/SVG escaper: reach for this rather than a hand-rolled match
/// over `& < > " '`. Pure and infallible; empty input gives an empty string.
///
/// Escapes all five entities, so the result is safe **anywhere** — element
/// content or an attribute value. Where the text can only ever be element
/// content and the output has to stay comparable byte-for-byte with a tool that
/// leaves quotes alone, [`escape_xml_content`] is the narrower rule.
///
/// Escaping is deliberately **not** idempotent — a literal `&amp;` in the input
/// escapes to `&amp;amp;`, because the input is text, not markup. Escape once,
/// at the point where text becomes a document.
///
/// The apostrophe maps to the XML named entity `&apos;`, never the HTML5 numeric
/// form `&#x27;`. That is the contract separating this from an HTML escaper:
/// `&apos;` is not a named HTML4 entity, so an HTML escaper must avoid it.
///
/// A control character other than TAB, LF or CR — and DEL — becomes U+FFFD.
/// XML 1.0 forbids the C0 controls outright, so a document carrying one is not
/// XML at all and a parser that meets it stops; marked rather than dropped, so
/// the damage is visible where it sat. TAB, LF and CR pass through: they
/// survive in text content, and an attribute's parser turns each into a space.
#[must_use]
pub fn escape_xml(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t' | '\n' | '\r' => escaped.push(character),
            control if control < '\u{20}' || control == '\u{7F}' => escaped.push('\u{FFFD}'),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Escapes text for XML **element content** — `&`, `<` and `>`, and nothing
/// else.
///
/// [`escape_xml`]'s narrower sibling, for a writer that puts caller text only
/// between tags and never inside an attribute. `"` and `'` carry no meaning in
/// element content, so escaping them changes nothing a parser sees — but it
/// does change the bytes, and that is the whole reason this exists: a document
/// this crate renders and one a foreign tool rewrites then differ only where
/// their *content* does, instead of differing on every line that quotes
/// something. Apple's `plutil -convert xml1` is the case that asked for it.
///
/// **Use [`escape_xml`] when in doubt.** A caller who reaches for this and then
/// grows an attribute has written an injection; the five-entity rule is safe
/// everywhere and costs only those bytes.
///
/// `&` is handled by the same single pass as the others, so the ampersand an
/// escape introduces is never itself escaped — the ordering bug a
/// replace-three-times implementation has to get right, and cannot express
/// otherwise.
///
/// Not idempotent, for [`escape_xml`]'s reason: the input is text, not markup.
#[must_use]
pub fn escape_xml_content(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Whether XML 1.0 can represent `character` **at all**.
///
/// The `Char` production, verbatim:
///
/// ```text
/// Char ::= #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]
/// ```
///
/// The companion to the escapers above, and a different kind of rule: escaping
/// answers *how* a character is written, this answers *whether* it can be.
/// There is no escape hatch for the ones it rejects — `&#1;` is refused by a
/// conforming parser exactly as the raw byte is — so a document carrying one is
/// a document nothing reads, and the only honest place to act is before the
/// text reaches a writer. Callers refuse; the writer asserts.
///
/// Two consequences worth stating, because both are the opposite of the guess:
///
/// - **DEL and the C1 block are legal.** Only XML 1.1 asks for them escaped.
///   Rejecting them here would refuse text a 1.0 parser accepts.
/// - **U+FFFE and U+FFFF are not**, despite being ordinary `char` values —
///   the production stops at #xFFFD. They are the pair a `< 0x20` check misses.
///
/// The surrogate gap in the production needs no arm: a Rust `char` cannot hold
/// one.
#[must_use]
pub const fn is_xml_char(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r')
        || matches!(character, '\u{20}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}
