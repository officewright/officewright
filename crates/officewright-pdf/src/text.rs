use std::{iter, mem};

use officewright_core::{PageSize, Pt};

use crate::font::{Lookup, check_font_size};
use crate::{Document, Error, Font, Result};

/// Margin on every side of the page.
const MARGIN: Pt = Pt(72.0);

/// Distance between baselines as a multiple of the font size.
const LINE_SPACING: f64 = 1.2;

/// Tabs advance to the next multiple of this many characters.
const TAB_STOP: usize = 8;

/// Lays out plain text on pages of `page_size`, in `font` at `font_size`.
///
/// Margins are 72 pt on every side and baselines are 1.2 × `font_size` apart. Lines are
/// measured with the font's advance widths, as [`Font::text_width`] measures them.
///
/// - `\n`, `\r\n` and `\r` end a line; a form feed (`\f`) ends a line and starts a new
///   page. A line break at the very end of the text adds no empty line or page.
/// - A tab advances to the next multiple of 8 characters.
/// - A byte order mark at the start is dropped.
/// - A line wider than the page wraps at spaces; a word wider than a whole line breaks
///   between characters. Spaces at a wrap are dropped.
///
/// Empty text gives one blank page.
///
/// ```
/// use officewright_pdf::{Font, PageSize, Pt, StandardFont, text_to_pdf};
///
/// let helvetica = Font::standard(StandardFont::Helvetica);
/// let doc = text_to_pdf("Hello,\nworld", PageSize::A4, &helvetica, Pt(11.0))?;
/// assert_eq!(doc.page_count(), 1);
/// # Ok::<(), officewright_pdf::Error>(())
/// ```
///
/// # Errors
///
/// For any other character the font cannot show, the errors of
/// [`Page::add_text`](crate::Page::add_text) with the character's byte offset in `text`;
/// [`Error::InvalidFontSize`], [`Error::InvalidPageSize`], and [`Error::NoRoomForText`]
/// if the page is not larger than its margins.
pub fn text_to_pdf(
    text: &str,
    page_size: PageSize,
    font: &Font,
    font_size: Pt,
) -> Result<Document> {
    check_font_size(font_size)?;
    let mut doc = Document::new();
    let mut page = doc.add_page(page_size)?;
    let line_width = page_size.width().0 - 2.0 * MARGIN.0;
    if line_width <= 0.0 || page_size.height().0 - 2.0 * MARGIN.0 <= 0.0 {
        return Err(Error::NoRoomForText {
            width: page_size.width(),
            height: page_size.height(),
            margin: MARGIN,
        });
    }

    let top = page_size.height().0 - MARGIN.0 - font_size.0;
    let max_units = line_width * 1000.0 / font_size.0;
    let mut y = top;
    let mut page_has_lines = false;
    let lookup = font.lookup();
    for paragraph in paragraphs(text, &lookup)? {
        for (i, line) in wrap(&paragraph.text, &lookup, max_units)
            .into_iter()
            .enumerate()
        {
            // A page takes at least one line, so even a huge font makes progress.
            let page_full = page_has_lines && y < MARGIN.0;
            if page_full || (i == 0 && paragraph.new_page) {
                page = doc.add_page(page_size)?;
                y = top;
            }
            if !line.is_empty() {
                page.add_text(MARGIN, Pt(y), font, font_size, line)?;
            }
            y -= font_size.0 * LINE_SPACING;
            page_has_lines = true;
        }
    }
    Ok(doc)
}

/// A line of the input, before wrapping.
#[derive(Debug, Default, PartialEq)]
struct Paragraph {
    text: String,
    /// Follows a form feed.
    new_page: bool,
}

/// Splits `text` into lines, expands tabs and checks every character.
fn paragraphs(text: &str, lookup: &Lookup<'_>) -> Result<Vec<Paragraph>> {
    let (body, base) = match text.strip_prefix('\u{FEFF}') {
        Some(body) => (body, '\u{FEFF}'.len_utf8()),
        None => (text, 0),
    };
    let mut paragraphs = Vec::new();
    let mut current = Paragraph::default();
    let mut column = 0;
    let mut chars = body.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        match ch {
            '\n' | '\r' | '\u{C}' => {
                if ch == '\r' {
                    chars.next_if(|&(_, next)| next == '\n');
                }
                let next = Paragraph {
                    text: String::new(),
                    new_page: ch == '\u{C}',
                };
                paragraphs.push(mem::replace(&mut current, next));
                column = 0;
            }
            '\t' => {
                let spaces = TAB_STOP - column % TAB_STOP;
                current.text.extend(iter::repeat_n(' ', spaces));
                column += spaces;
            }
            _ => {
                lookup.glyph(ch, base + offset)?;
                current.text.push(ch);
                column += 1;
            }
        }
    }
    // Text that ends with a line break has no further line.
    if !current.text.is_empty() {
        paragraphs.push(current);
    }
    Ok(paragraphs)
}

/// Breaks `line` into pieces no wider than `max_units` (1/1000 em), preferring spaces.
/// Returns at least one piece.
fn wrap<'a>(line: &'a str, lookup: &Lookup<'_>, max_units: f64) -> Vec<&'a str> {
    let mut pieces = Vec::new();
    let mut rest = line;
    loop {
        let mut width = 0.0;
        let mut overflow = None;
        let mut last_space = None;
        let mut has_text = false;
        for (i, ch) in rest.char_indices() {
            let advance = units(lookup, ch);
            // The first character always fits, so every piece makes progress.
            if i > 0 && width + advance > max_units {
                overflow = Some(i);
                break;
            }
            if ch != ' ' {
                has_text = true;
            } else if has_text {
                last_space = Some(i);
            }
            width += advance;
        }
        let Some(overflow) = overflow else {
            pieces.push(rest);
            return pieces;
        };
        let (head, tail) = rest
            .split_at_checked(last_space.unwrap_or(overflow))
            .unwrap_or((rest, ""));
        pieces.push(head.trim_end_matches(' '));
        rest = tail.trim_start_matches(' ');
        if rest.is_empty() {
            return pieces;
        }
    }
}

/// The advance width of `ch` in 1/1000 em; 0 for characters the font cannot show.
fn units(lookup: &Lookup<'_>, ch: char) -> f64 {
    lookup.glyph(ch, 0).map_or(0.0, |glyph| glyph.advance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StandardFont;

    fn helvetica() -> Font {
        Font::standard(StandardFont::Helvetica)
    }

    fn width(piece: &str) -> f64 {
        piece
            .chars()
            .map(|ch| units(&helvetica().lookup(), ch))
            .sum()
    }

    fn paragraph(text: &str, new_page: bool) -> Paragraph {
        Paragraph {
            text: text.into(),
            new_page,
        }
    }

    #[test]
    fn splits_lines_on_every_kind_of_break() {
        assert_eq!(
            paragraphs("a\nb\r\nc\rd\u{C}e\n", &helvetica().lookup()).unwrap(),
            [
                paragraph("a", false),
                paragraph("b", false),
                paragraph("c", false),
                paragraph("d", false),
                paragraph("e", true),
            ]
        );
    }

    #[test]
    fn keeps_empty_lines_but_not_a_final_break() {
        assert_eq!(
            paragraphs("a\n\nb\n", &helvetica().lookup()).unwrap(),
            [
                paragraph("a", false),
                paragraph("", false),
                paragraph("b", false)
            ]
        );
        assert_eq!(paragraphs("", &helvetica().lookup()).unwrap(), []);
        assert_eq!(
            paragraphs("a\u{C}", &helvetica().lookup()).unwrap(),
            [paragraph("a", false)]
        );
    }

    #[test]
    fn expands_tabs_to_multiples_of_eight() {
        assert_eq!(
            paragraphs("ab\tc\t\td", &helvetica().lookup()).unwrap(),
            [paragraph(
                &format!("ab{}c{}d", " ".repeat(6), " ".repeat(15)),
                false
            )]
        );
    }

    #[test]
    fn drops_a_leading_byte_order_mark_and_reports_offsets_in_the_input() {
        assert_eq!(
            paragraphs("\u{FEFF}a", &helvetica().lookup()).unwrap(),
            [paragraph("a", false)]
        );
        let err = paragraphs("\u{FEFF}ab\nЖ", &helvetica().lookup()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::Unencodable {
                    ch: 'Ж', offset: 6
                }
            ),
            "{err:?}"
        );
        assert!(matches!(
            paragraphs("a\u{1}", &helvetica().lookup()).unwrap_err(),
            Error::Unencodable {
                ch: '\u{1}',
                offset: 1
            }
        ));
    }

    #[test]
    fn wraps_at_spaces() {
        let line = "The quick brown fox jumps over the lazy dog. ".repeat(5);
        let line = line.trim_end();
        let max = 10_000.0;
        let pieces = wrap(line, &helvetica().lookup(), max);
        assert!(pieces.len() > 1);
        for piece in &pieces {
            assert!(width(piece) <= max, "{piece:?}");
            assert!(
                !piece.starts_with(' ') && !piece.ends_with(' '),
                "{piece:?}"
            );
        }
        assert_eq!(pieces.join(" "), line);
    }

    #[test]
    fn breaks_a_word_longer_than_a_line() {
        let pieces = wrap("WWWWWWWWWW", &helvetica().lookup(), 3000.0);
        assert_eq!(pieces, ["WWW", "WWW", "WWW", "W"]);
    }

    #[test]
    fn puts_at_least_one_character_on_each_line() {
        assert_eq!(wrap("WW", &helvetica().lookup(), 1.0), ["W", "W"]);
        assert_eq!(wrap("", &helvetica().lookup(), 1.0), [""]);
    }

    #[test]
    fn keeps_indentation_and_drops_spaces_at_a_wrap() {
        assert_eq!(
            wrap("    aaa bbb", &helvetica().lookup(), 3000.0),
            ["    aaa", "bbb"]
        );
        assert_eq!(wrap("aaa      ", &helvetica().lookup(), 1700.0), ["aaa"]);
    }

    #[test]
    fn starts_a_new_page_when_the_page_is_full() {
        // A4 at 11 pt: the first baseline is at 841.89 - 72 - 11 = 758.89 pt and the
        // last one must stay at or above 72 pt, so (758.89 - 72) / 13.2 + 1 = 53 lines fit.
        let lines = |n: usize| "x\n".repeat(n);
        let pages = |text: &str| {
            text_to_pdf(text, PageSize::A4, &helvetica(), Pt(11.0))
                .unwrap()
                .page_count()
        };
        assert_eq!(pages(&lines(53)), 1);
        assert_eq!(pages(&lines(54)), 2);
        assert_eq!(pages(&lines(106)), 2);
        assert_eq!(pages(&lines(107)), 3);
    }

    #[test]
    fn starts_a_new_page_after_a_form_feed() {
        let pages = |text: &str| {
            text_to_pdf(text, PageSize::A4, &helvetica(), Pt(11.0))
                .unwrap()
                .page_count()
        };
        assert_eq!(pages(""), 1);
        assert_eq!(pages("a\u{C}b"), 2);
        assert_eq!(pages("a\u{C}"), 1);
        assert_eq!(pages("a\u{C}\u{C}b"), 3);
    }

    #[test]
    fn makes_progress_with_a_font_larger_than_the_page() {
        let doc = text_to_pdf("WW", PageSize::A4, &helvetica(), Pt(1000.0)).unwrap();
        assert_eq!(doc.page_count(), 2);
    }

    #[test]
    fn places_lines_inside_the_margins() {
        let pdf = text_to_pdf("first\nsecond", PageSize::LETTER, &helvetica(), Pt(10.0))
            .unwrap()
            .to_bytes()
            .unwrap();
        let pdf = String::from_utf8_lossy(&pdf);
        assert!(pdf.contains("72 710 Td\n(first) Tj"), "{pdf}");
        assert!(pdf.contains("72 698 Td\n(second) Tj"), "{pdf}");
    }

    #[test]
    fn rejects_pages_without_room_for_text() {
        let small = PageSize::new(Pt(144.0), Pt(500.0));
        let err = text_to_pdf("a", small, &helvetica(), Pt(10.0)).unwrap_err();
        assert!(matches!(err, Error::NoRoomForText { .. }), "{err:?}");
        assert_eq!(
            err.to_string(),
            "page size 144 x 500 pt leaves no room for text inside 72 pt margins"
        );
        let err = text_to_pdf("a", PageSize::A4, &helvetica(), Pt(0.0)).unwrap_err();
        assert!(matches!(err, Error::InvalidFontSize(_)), "{err:?}");
        let err =
            text_to_pdf("a", PageSize::new(Pt(1.0), Pt(1.0)), &helvetica(), Pt(10.0)).unwrap_err();
        assert!(matches!(err, Error::InvalidPageSize { .. }), "{err:?}");
    }
}
