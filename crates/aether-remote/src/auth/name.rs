//! What a device may be called. The host reads the name to decide on enrolment, so nothing in it
//! may be invisible or reorder the rest of what is shown.

use super::{AuthError, MAX_DEVICE_NAME_CHARS};

/// Code points that show as nothing or rearrange their neighbours: format, default-ignorable,
/// blank fillers and private-use. Inclusive ranges, ascending.
const UNSEEN: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),     // soft hyphen
    (0x034F, 0x034F),     // combining grapheme joiner
    (0x0600, 0x0605),     // Arabic number signs
    (0x061C, 0x061C),     // Arabic letter mark
    (0x06DD, 0x06DD),     // Arabic end of ayah
    (0x070F, 0x070F),     // Syriac abbreviation mark
    (0x0890, 0x0891),     // Arabic currency marks above
    (0x08E2, 0x08E2),     // Arabic disputed end of ayah
    (0x115F, 0x1160),     // Hangul fillers
    (0x17B4, 0x17B5),     // Khmer inherent vowels
    (0x180B, 0x180F),     // Mongolian variation selectors and vowel separator
    (0x200B, 0x200F),     // zero-width space and joiners, direction marks
    (0x202A, 0x202E),     // bidirectional embeddings and overrides
    (0x2060, 0x206F),     // word joiner, invisible operators, bidirectional isolates
    (0x2800, 0x2800),     // blank braille pattern
    (0x3164, 0x3164),     // Hangul filler
    (0xE000, 0xF8FF),     // private use
    (0xFDD0, 0xFDEF),     // noncharacters
    (0xFE00, 0xFE0F),     // variation selectors
    (0xFEFF, 0xFEFF),     // byte order mark
    (0xFFA0, 0xFFA0),     // halfwidth Hangul filler
    (0xFFF0, 0xFFFC),     // interlinear annotation, object replacement
    (0x110BD, 0x110BD),   // Kaithi number sign
    (0x110CD, 0x110CD),   // Kaithi number sign above
    (0x13430, 0x1343F),   // Egyptian hieroglyph format controls
    (0x1BCA0, 0x1BCA3),   // shorthand format controls
    (0x1D173, 0x1D17A),   // musical format controls
    (0xE0000, 0xE0FFF),   // tags and supplementary variation selectors
    (0xF0000, 0x10_FFFF), // private use planes
];

/// Whether `c` would not show as itself where a name is displayed.
fn is_unseen(c: char) -> bool {
    let point = u32::from(c);
    // The last two code points of every plane are noncharacters.
    let noncharacter = point & 0xFFFE == 0xFFFE;
    c.is_control()
        || noncharacter
        || UNSEEN
            .iter()
            .any(|(first, last)| (*first..=*last).contains(&point))
}

/// A device name as enrolled: bounded, every character visible, and every
/// run of whitespace of any kind a single plain space.
pub fn check_device_name(name: &str) -> Result<String, AuthError> {
    // Before the spaces are folded: a tab or a newline is not a space.
    if name.chars().any(is_unseen) {
        return Err(AuthError::BadName);
    }
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_DEVICE_NAME_CHARS {
        return Err(AuthError::BadName);
    }
    debug_assert!(!name.starts_with(' ') && !name.ends_with(' '));
    debug_assert!(name.chars().all(|c| c == ' ' || !c.is_whitespace()));
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_bounded_and_not_empty() {
        assert_eq!(check_device_name("  phone  "), Ok("phone".into()));
        let longest = "x".repeat(MAX_DEVICE_NAME_CHARS);
        assert_eq!(check_device_name(&longest), Ok(longest.clone()));
        let padded = format!("  {longest}  ");
        assert_eq!(check_device_name(&padded), Ok(longest.clone()));
        for bad in ["", "   ", "\u{a0}\u{3000}", &format!("{longest}x")] {
            assert_eq!(check_device_name(bad), Err(AuthError::BadName), "{bad:?}");
        }
    }

    #[test]
    fn names_in_any_script_and_plain_emoji_are_kept_as_typed() {
        for name in [
            "Robin's iPhone",
            "Zo\u{eb}'s tablet (work)",
            "\u{643}\u{645}\u{628}\u{64a}\u{648}\u{62a}\u{631}",
            "\u{30b1}\u{30f3}\u{306e}\u{96fb}\u{8a71}",
            "phone \u{1f4f1}",
            "e\u{301}cran",
        ] {
            assert_eq!(check_device_name(name), Ok(name.to_owned()), "{name:?}");
        }
    }

    #[test]
    fn every_kind_of_whitespace_becomes_one_plain_space() {
        let spaced = [
            "Kim's  phone",
            "Kim's\u{a0}phone",
            "Kim's\u{2003}\u{2009}phone",
            "Kim's\u{3000}phone",
            "\u{202f}Kim's \u{205f} phone\u{1680}",
            "Kim's\u{2028}phone",
            "Kim's\u{2029}phone",
        ];
        for typed in spaced {
            assert_eq!(
                check_device_name(typed),
                Ok("Kim's phone".into()),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn characters_that_reorder_or_hide_text_are_refused() {
        let hidden = [
            ('\u{202e}', "right-to-left override"),
            ('\u{202a}', "left-to-right embedding"),
            ('\u{2066}', "left-to-right isolate"),
            ('\u{2069}', "pop directional isolate"),
            ('\u{200e}', "left-to-right mark"),
            ('\u{200f}', "right-to-left mark"),
            ('\u{61c}', "Arabic letter mark"),
            ('\u{200b}', "zero-width space"),
            ('\u{200c}', "zero-width non-joiner"),
            ('\u{200d}', "zero-width joiner"),
            ('\u{2060}', "word joiner"),
            ('\u{feff}', "byte order mark"),
            ('\u{ad}', "soft hyphen"),
            ('\u{34f}', "combining grapheme joiner"),
            ('\u{180e}', "Mongolian vowel separator"),
            ('\u{3164}', "Hangul filler"),
            ('\u{2800}', "blank braille pattern"),
            ('\u{fe0f}', "variation selector"),
            ('\u{e0041}', "tag letter"),
            ('\u{e000}', "private use"),
            ('\u{10fffd}', "private use plane"),
            ('\u{fffe}', "noncharacter"),
            ('\u{1ffff}', "noncharacter"),
            ('\u{fffc}', "object replacement"),
            ('\n', "newline"),
            ('\t', "tab"),
            ('\u{1b}', "escape"),
            ('\u{7f}', "delete"),
            ('\u{85}', "next line"),
        ];
        for (c, what) in hidden {
            for typed in [
                format!("Kim's{c}phone"),
                format!("{c}phone"),
                format!("phone{c}"),
            ] {
                let checked = check_device_name(&typed);
                assert_eq!(checked, Err(AuthError::BadName), "{what}: {typed:?}");
            }
        }
        // A name spelled backwards on screen by an override reads as another.
        let spoofed = "phone \u{202e}enohp s'miK";
        assert_eq!(check_device_name(spoofed), Err(AuthError::BadName));
    }

    #[test]
    fn the_table_is_ascending_and_keeps_visible_neighbours() {
        for pair in UNSEEN.windows(2) {
            assert!(
                pair[0].1 < pair[1].0,
                "{pair:x?} overlap or are out of order"
            );
        }
        for (first, last) in UNSEEN {
            assert!(first <= last, "{first:x}..{last:x}");
            assert!(char::from_u32(*first).is_some_and(is_unseen));
            assert!(char::from_u32(*last).is_some_and(is_unseen));
        }
        for visible in [
            'a',
            '\u{e9}',
            '\u{2010}',
            '\u{fffd}',
            '\u{1f4f1}',
            '\u{ac00}',
            ' ',
        ] {
            assert!(!is_unseen(visible), "{visible:?}");
        }
    }
}
