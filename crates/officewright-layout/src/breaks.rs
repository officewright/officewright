//! Where a line may end: the Unicode line breaking algorithm (UAX #14) over a paragraph's
//! text, with Word's own rules on top.
//!
//! The text is the paragraph's text as it is drawn, with one character standing for
//! each piece that is not text (see [`PLACEHOLDER_OBJECT`] and the others), so that UAX
//! #14 sees what is around each break.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use unicode_linebreak::{BreakOpportunity, linebreaks};

/// Stands for an inline object: UAX #14 class Contingent Break (CB), so a line may end
/// before and after it.
pub(crate) const PLACEHOLDER_OBJECT: char = '\u{FFFC}';
/// Stands for a line, page or column break: class Mandatory Break (BK), so the line ends
/// after it.
pub(crate) const PLACEHOLDER_BREAK: char = '\u{2028}';
/// Stands for a tab: class Break After, so a line may end after it but not before it.
pub(crate) const PLACEHOLDER_TAB: char = '\t';
/// Stands for a note reference or a note's own mark: class Word Joiner (WJ), so the mark
/// stays with the word before it.
pub(crate) const PLACEHOLDER_MARK: char = '\u{2060}';

/// How a line may end at a place in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opportunity {
    /// The line may end here.
    Allowed,
    /// The line ends here.
    Mandatory,
}

/// The places where a line may end, by the byte offset in the text at which the next
/// line would start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BreakMap {
    breaks: Vec<(usize, Opportunity)>,
}

impl BreakMap {
    /// The places in `text` where a line may end. With `word_wrap` off (`w:wordWrap`),
    /// a line may also end between any two characters of a word, in the scripts that
    /// separate words by spaces only (MS-OI29500 2.1.67 b).
    pub(crate) fn new(text: &str, word_wrap: bool) -> Self {
        let mut breaks: Vec<(usize, Opportunity)> = linebreaks(text)
            .map(|(at, opportunity)| {
                let opportunity = match opportunity {
                    BreakOpportunity::Mandatory => Opportunity::Mandatory,
                    BreakOpportunity::Allowed => Opportunity::Allowed,
                };
                (at, opportunity)
            })
            .collect();
        if !word_wrap {
            let mut extra = Vec::new();
            // The last character that is not a mark or joiner of the one before it.
            let mut base: Option<char> = None;
            for (at, ch) in text.char_indices() {
                // A word joiner (a note mark's placeholder among them) keeps the
                // characters on both sides together.
                if matches!(ch, '\u{2060}' | '\u{FEFF}') {
                    base = None;
                    continue;
                }
                if crate::itemize::attaches(ch) {
                    continue;
                }
                if base.is_some_and(|before| wordy(before) && wordy(ch)) {
                    extra.push((at, Opportunity::Allowed));
                }
                base = Some(ch);
            }
            breaks.extend(extra);
            breaks.sort_by_key(|&(at, _)| at);
            // A break the algorithm found wins over one added here.
            breaks.dedup_by(|later, earlier| {
                if later.0 == earlier.0 {
                    if later.1 == Opportunity::Mandatory {
                        earlier.1 = Opportunity::Mandatory;
                    }
                    true
                } else {
                    false
                }
            });
        }
        Self { breaks }
    }

    /// How a line may end before byte `at`; `None` when it may not.
    pub(crate) fn at(&self, at: usize) -> Option<Opportunity> {
        self.breaks
            .binary_search_by_key(&at, |&(offset, _)| offset)
            .ok()
            .and_then(|index| self.breaks.get(index))
            .map(|&(_, opportunity)| opportunity)
    }

    /// The places, in order.
    #[cfg(test)]
    pub(crate) fn all(&self) -> &[(usize, Opportunity)] {
        &self.breaks
    }
}

/// Whether `ch` is a letter or digit of a script that separates words by spaces.
fn wordy(ch: char) -> bool {
    ch.is_alphanumeric() && spaced(ch)
}

/// Whether `ch` is of a script that separates words by spaces, unlike Chinese, Japanese,
/// Thai, Lao, Khmer and Myanmar, which a line may break inside anyway or which need a
/// dictionary.
fn spaced(ch: char) -> bool {
    use unicode_script::{Script, UnicodeScript};
    !matches!(
        ch.script(),
        Script::Han
            | Script::Hiragana
            | Script::Katakana
            | Script::Bopomofo
            | Script::Thai
            | Script::Lao
            | Script::Khmer
            | Script::Myanmar
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(map: &BreakMap) -> Vec<usize> {
        map.all()
            .iter()
            .filter(|(_, opportunity)| *opportunity == Opportunity::Allowed)
            .map(|&(at, _)| at)
            .collect()
    }

    #[test]
    fn a_line_ends_after_spaces_and_at_the_end() {
        let map = BreakMap::new("one two  three", true);
        assert_eq!(allowed(&map), [4, 9]);
        assert_eq!(map.at(14), Some(Opportunity::Mandatory));
        assert_eq!(map.at(3), None);
        assert_eq!(map.at(4), Some(Opportunity::Allowed));
    }

    #[test]
    fn placeholders_break_as_the_pieces_they_stand_for() {
        // A line may end before and after an object.
        let text = format!("a{PLACEHOLDER_OBJECT}b");
        let map = BreakMap::new(&text, true);
        assert_eq!(allowed(&map), [1, 1 + PLACEHOLDER_OBJECT.len_utf8()]);
        // A line ends after a break.
        let text = format!("a{PLACEHOLDER_BREAK}b");
        let map = BreakMap::new(&text, true);
        assert_eq!(
            map.at(1 + PLACEHOLDER_BREAK.len_utf8()),
            Some(Opportunity::Mandatory)
        );
        // After a tab, not before it.
        let map = BreakMap::new("a\tb", true);
        assert_eq!(allowed(&map), [2]);
        // A note mark stays with the word before it and the word after it.
        let text = format!("word{PLACEHOLDER_MARK}more");
        assert!(allowed(&BreakMap::new(&text, true)).is_empty());
        let text = format!("word{PLACEHOLDER_MARK} more");
        assert_eq!(
            allowed(&BreakMap::new(&text, true)),
            [4 + PLACEHOLDER_MARK.len_utf8() + 1]
        );
    }

    #[test]
    fn soft_hyphens_and_no_break_spaces() {
        // After a soft hyphen, not at a no-break space or a no-break hyphen.
        let map = BreakMap::new("ab\u{00AD}cd e\u{00A0}f g\u{2011}h", true);
        assert_eq!(allowed(&map), [4, 7, 12]);
    }

    #[test]
    fn without_word_wrap_words_break_between_any_two_characters() {
        let map = BreakMap::new("ab cd", false);
        assert_eq!(allowed(&map), [1, 3, 4]);
        // Not before a mark, and not inside Chinese, which UAX #14 breaks anyway.
        let map = BreakMap::new("a\u{0301}b", false);
        assert_eq!(allowed(&map), [3]);
        let map = BreakMap::new("中文字", false);
        assert_eq!(allowed(&map), [3, 6]);
        // Thai needs a dictionary: no breaks inside its words either way.
        assert!(allowed(&BreakMap::new("ภาษาไทย", false)).is_empty());
        // A note mark stays with the words on both sides.
        let text = format!("word{PLACEHOLDER_MARK}more");
        assert_eq!(
            allowed(&BreakMap::new(&text, false)),
            [
                1,
                2,
                3,
                5 + PLACEHOLDER_MARK.len_utf8(),
                6 + PLACEHOLDER_MARK.len_utf8(),
                7 + PLACEHOLDER_MARK.len_utf8()
            ]
        );
    }
}
