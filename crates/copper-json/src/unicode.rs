//! Python's Unicode string predicates and `repr(str)`.
//!
//! Character classes come from the Unicode general category. Python's
//! `isdigit` also accepts a few `No` characters with a digit value (such as
//! superscripts); those are treated as non-digits here, a difference no
//! CopperScript source exercises.

use unicode_general_category::{GeneralCategory, get_general_category};

/// `str.isspace()`: Unicode whitespace plus the separators U+001C..U+001F.
pub fn python_isspace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{1c}'..='\u{1f}' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// `str.isalpha()`: general category Lu, Ll, Lt, Lm or Lo.
pub fn python_isalpha(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_alphabetic();
    }
    matches!(
        get_general_category(character),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

/// `str.isdigit()` for decimal digits (general category Nd).
pub fn python_isdigit(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_digit();
    }
    get_general_category(character) == GeneralCategory::DecimalNumber
}

/// `str.isalnum()`: letters and numbers (L* and N* categories).
pub fn python_isalnum(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_alphanumeric();
    }
    python_isalpha(character)
        || matches!(
            get_general_category(character),
            GeneralCategory::DecimalNumber | GeneralCategory::LetterNumber | GeneralCategory::OtherNumber
        )
}

/// `str.isprintable()` for one character: not a control, format, surrogate,
/// private-use, unassigned or separator character, except the space.
pub fn python_isprintable(character: char) -> bool {
    if character == ' ' {
        return true;
    }
    !matches!(
        get_general_category(character),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
            | GeneralCategory::SpaceSeparator
    )
}

/// Python's `repr(text)`.
pub fn python_str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if character == quote => {
                out.push('\\');
                out.push(quote);
            }
            _ if (character as u32) < 0x20 || character == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", character as u32));
            }
            _ if character.is_ascii() || python_isprintable(character) => out.push(character),
            _ => {
                let code = character as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_matches_python() {
        assert_eq!(python_str_repr("abc"), "'abc'");
        assert_eq!(python_str_repr("it's"), "\"it's\"");
        assert_eq!(python_str_repr("a'\""), "'a\\'\"'");
        assert_eq!(python_str_repr("\u{0}\n\u{7f}"), "'\\x00\\n\\x7f'");
        assert_eq!(python_str_repr("\u{e9}\u{a0}\u{2028}\u{1f600}\u{e000}"), "'\u{e9}\\xa0\\u2028\u{1f600}\\ue000'");
    }
}
