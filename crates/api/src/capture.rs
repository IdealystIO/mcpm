//! The capture syntax: one want per line, `#tag` anywhere in it.
//!
//! This module is the SINGLE definition of that syntax, and it lives in
//! the wire crate on purpose. The console highlights what you type with
//! [`scan_tags`] and previews what will be written with
//! [`parse_line`]; the server function that actually writes runs the
//! same two functions on the same text. There is no second parser to
//! drift out of agreement with the first, so what the editor colors is
//! exactly what lands in the pool.
//!
//! No dependencies beyond `std` — it compiles into the wasm console as
//! readily as into the host binary.

use serde::{Deserialize, Serialize};

/// One `#tag` found in the text, as a byte range plus its normalized
/// slug. Byte ranges because that is what `code_editor`'s decorations
/// speak, and what `str` indexing wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSpan {
    /// Byte offset of the `#`.
    pub start: usize,
    /// Byte offset one past the last character of the tag.
    pub end: usize,
    /// Normalized slug (lowercase), without the `#`.
    pub name: String,
}

/// One parsed line, ready to become a want.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WantDraftDto {
    /// The idea, with the `#tag` tokens lifted out.
    pub body: String,
    /// Normalized slugs, in the order they appeared, deduplicated.
    pub tags: Vec<String>,
}

/// Find every `#tag` in `text`.
///
/// A tag is `#` followed by a letter, then letters/digits/`-`/`_`. The
/// leading-letter rule is what keeps "issue #42" and "C#" out of the tag
/// vocabulary — a `#` that introduces a number is prose, not filing.
pub fn scan_tags(text: &str) -> Vec<TagSpan> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'#' {
            i += 1;
            continue;
        }
        // A tag opens a word: `a#b` is not a tag.
        if i > 0 && is_tag_body(bytes[i - 1]) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        if end >= bytes.len() || !bytes[end].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        while end < bytes.len() && is_tag_body(bytes[end]) {
            end += 1;
        }
        // Trailing punctuation belongs to the sentence, not the tag.
        while end > start + 1 && matches!(bytes[end - 1], b'-' | b'_') {
            end -= 1;
        }
        spans.push(TagSpan {
            start,
            end,
            name: text[start + 1..end].to_ascii_lowercase(),
        });
        i = end;
    }
    spans
}

fn is_tag_body(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Parse one line into a want draft, or `None` when the line holds no
/// idea (blank, or nothing but tags).
///
/// The `#tag` tokens are lifted OUT of the body: a want's body is what
/// was wanted, and the tags are how it is filed. Leaving the markers in
/// would store the same fact twice and read badly back.
pub fn parse_line(line: &str) -> Option<WantDraftDto> {
    let spans = scan_tags(line);
    let mut body = String::with_capacity(line.len());
    let mut cursor = 0;
    for span in &spans {
        body.push_str(&line[cursor..span.start]);
        cursor = span.end;
    }
    body.push_str(&line[cursor..]);

    let body = collapse_whitespace(&body);
    if body.is_empty() {
        return None;
    }
    let mut tags: Vec<String> = Vec::new();
    for span in spans {
        if !tags.contains(&span.name) {
            tags.push(span.name);
        }
    }
    Some(WantDraftDto { body, tags })
}

/// Parse a whole composer buffer — one want per non-empty line.
pub fn parse_buffer(text: &str) -> Vec<WantDraftDto> {
    text.lines().filter_map(parse_line).collect()
}

/// Squeeze runs of whitespace left behind by lifted tags.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for ch in s.trim().chars() {
        if ch.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
    }
    out
}

/// The `#fragment` being typed immediately before `caret` (a byte
/// offset), if any — what Tab completes against.
///
/// Returns the fragment's byte range and its lowercase text, which is
/// empty for a bare `#`. `None` when the caret is not inside a tag.
pub fn tag_fragment_at(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, String)> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut start = caret;
    while start > 0 && is_tag_body(bytes[start - 1]) {
        start -= 1;
    }
    if start == 0 || bytes[start - 1] != b'#' {
        return None;
    }
    let hash = start - 1;
    if hash > 0 && is_tag_body(bytes[hash - 1]) {
        return None;
    }
    Some((hash..caret, text[start..caret].to_ascii_lowercase()))
}

/// Convert a UTF-16 offset (what every platform's caret API reports)
/// into a byte offset into `text`.
pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    if utf16 == 0 {
        return 0;
    }
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifts_tags_out_of_the_body() {
        let draft = parse_line("exports are painful #ux #Billing").unwrap();
        assert_eq!(draft.body, "exports are painful");
        assert_eq!(draft.tags, vec!["ux", "billing"]);
    }

    #[test]
    fn tags_may_sit_anywhere_in_the_line() {
        let draft = parse_line("the #ux of exports is painful").unwrap();
        assert_eq!(draft.body, "the of exports is painful");
        assert_eq!(draft.tags, vec!["ux"]);
    }

    #[test]
    fn a_hash_before_a_number_is_prose() {
        let draft = parse_line("see issue #42 about exports").unwrap();
        assert_eq!(draft.body, "see issue #42 about exports");
        assert!(draft.tags.is_empty());
    }

    #[test]
    fn mid_word_hashes_are_not_tags() {
        let draft = parse_line("the C#compiler thing").unwrap();
        assert_eq!(draft.body, "the C#compiler thing");
        assert!(draft.tags.is_empty());
    }

    #[test]
    fn trailing_punctuation_stays_in_the_sentence() {
        let spans = scan_tags("about #exports- really");
        assert_eq!(spans[0].name, "exports");
        assert_eq!(&"about #exports- really"[spans[0].start..spans[0].end], "#exports");
    }

    #[test]
    fn duplicate_tags_collapse() {
        let draft = parse_line("#ux this is #ux again").unwrap();
        assert_eq!(draft.tags, vec!["ux"]);
    }

    #[test]
    fn a_line_of_only_tags_is_not_a_want() {
        assert!(parse_line("#ux #ops").is_none());
        assert!(parse_line("   ").is_none());
    }

    #[test]
    fn buffer_splits_on_lines_and_skips_blanks() {
        let drafts = parse_buffer("first idea #ux\n\n  \nsecond idea\n");
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[1].body, "second idea");
    }

    #[test]
    fn fragment_under_the_caret_is_what_tab_completes() {
        let text = "exports are painful #bil";
        let (range, frag) = tag_fragment_at(text, text.len()).unwrap();
        assert_eq!(frag, "bil");
        assert_eq!(&text[range], "#bil");
        // A bare `#` completes against everything.
        assert_eq!(tag_fragment_at("hello #", 7).unwrap().1, "");
        // Not in a tag at all.
        assert!(tag_fragment_at("hello there", 11).is_none());
    }

    #[test]
    fn utf16_offsets_convert_past_astral_characters() {
        let text = "🙂 #ux";
        // The emoji is 2 UTF-16 units, 4 bytes.
        assert_eq!(utf16_to_byte(text, 2), 4);
        let caret = utf16_to_byte(text, 6);
        assert_eq!(tag_fragment_at(text, caret).unwrap().1, "ux");
    }
}
