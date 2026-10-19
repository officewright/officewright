//! Builds a paragraph from the input and itemizes and shapes it with the test fonts of
//! tests/fonts, as the layout does: the first bytes describe up to four run styles (fonts,
//! among them missing ones and the symbol fonts Symbol and Wingdings; bold, italic,
//! capitals, small capitals, complex script, right to left, the East Asian hint, kerning;
//! size), the rest are pieces of text, symbols, tabs, soft hyphens, and page and column
//! breaks in those styles. The paragraph must keep what the layout relies on
//! (`check_paragraph`), and break into lines that hold its text in order, in a column, with
//! a line spacing, an alignment, a hanging indent and tab stops the first bytes give too,
//! and a list label when they say so (`check_lines`). Up to four copies of it, kept with
//! the next, kept together, with or without widow control and a page break before as the
//! first bytes say, are then laid out on pages of a height they give, the last of them in a
//! second section of a start the first bytes give when they say so, with every line on
//! exactly one page inside its body (`check_pages`); with the paragraph as their header and
//! the page fields as their footer, in a number format, with a title page and even pages of
//! their own, when the first bytes say so.
#![no_main]

use std::path::Path;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use officewright_core::PageSize;
use officewright_core::Pt;
use officewright_font::{FontBook, FontOrigin};
use officewright_layout::fuzzing::{check_lines, check_pages, check_paragraph};
use officewright_layout::ir::{
    Alignment, Document, DocumentSettings, Field, FieldKind, FontHint, HeaderFooterSet, Indent,
    Inline, Level, LevelAlignment, LevelTextPart, LineSpacing, ListDefinition, ListInstance,
    ListItem, Margins, NumberFormat, PageSetup, Paragraph, ParagraphProps, RunFonts, RunStyle,
    Section, SectionStart, Spacing, Suffix, TabKind, TabStop,
};

const FAMILIES: [&str; 8] = [
    "Calibri",
    "Times New Roman",
    "Courier New",
    "Noto Sans",
    "Symbol",
    "Wingdings",
    "No Such Font",
    "",
];

fn book() -> &'static FontBook {
    static BOOK: OnceLock<FontBook> = OnceLock::new();
    BOOK.get_or_init(|| {
        // In memory, so that loading a face does not read its file again on every run.
        let mut book = FontBook::new();
        let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fonts");
        for family in ["carlito", "liberation", "noto"] {
            let Ok(entries) = std::fs::read_dir(fonts.join(family)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let font = path
                    .extension()
                    .is_some_and(|extension| extension == "ttf" || extension == "otf");
                if let (true, Ok(data)) = (font, std::fs::read(&path)) {
                    let _ = book.add_bytes(data, FontOrigin::User);
                }
            }
        }
        book
    })
}

fn style(bytes: &[u8]) -> RunStyle {
    let byte = |i: usize| bytes.get(i).copied().unwrap_or(0);
    let family = |i: usize| FAMILIES[usize::from(byte(i)) % FAMILIES.len()];
    let fonts = RunFonts::default()
        .with_ascii(family(0))
        .with_high_ansi(family(0))
        .with_east_asia(family(1))
        .with_complex_script(family(1))
        .with_hint(if byte(2) & 1 == 1 {
            FontHint::EastAsia
        } else {
            FontHint::Default
        });
    let flags = byte(2);
    RunStyle::default()
        .with_fonts(fonts)
        .with_bold(flags & 2 != 0)
        .with_italic(flags & 4 != 0)
        .with_caps(flags & 8 != 0)
        .with_small_caps(flags & 16 != 0)
        .with_complex_script(flags & 32 != 0)
        .with_rtl(flags & 64 != 0)
        .with_kern_min((flags & 128 != 0).then_some(Pt(1.0)))
        .with_size(Pt(1.0 + f64::from(byte(3) % 80)))
}

fuzz_target!(|data: &[u8]| {
    // Longer paragraphs find nothing shorter ones do not, and slow the fuzzer down.
    let data = data.get(..2048).unwrap_or(data);
    // Even-page headers in use when bit 5 of byte 12 says so, as the footer below has them.
    let even = data.get(12).is_some_and(|byte| byte & 32 != 0);
    let mut document = Document::new(DocumentSettings::default().with_even_and_odd_headers(even));
    let mut styles = Vec::new();
    for chunk in data.chunks(4).take(4) {
        styles.push(document.add_run_style(style(chunk)));
    }
    let Some(&first) = styles.first() else {
        return;
    };
    let rest = data.get(16..).unwrap_or_default();
    let mut inlines = Vec::new();
    for piece in rest.split(|&byte| byte == 0xFF) {
        let Some((&head, body)) = piece.split_first() else {
            continue;
        };
        let style = styles.get(usize::from(head & 3)).copied().unwrap_or(first);
        let text = String::from_utf8_lossy(body);
        inlines.push(match head >> 2 & 3 {
            0 | 1 => Inline::text(text, style),
            2 => Inline::symbol(
                FAMILIES[usize::from(head >> 4) % FAMILIES.len()],
                u16::from(body.first().copied().unwrap_or(0)) | 0xF000,
                style,
            ),
            _ => match head >> 6 {
                0 => Inline::tab(style),
                1 => Inline::soft_hyphen(style),
                2 => Inline::page_break(style),
                _ => Inline::column_break(style),
            },
        });
    }
    if let Err(problem) = check_paragraph(book(), &document, &inlines) {
        panic!("{problem}");
    }
    let byte = |i: usize| data.get(i).copied().unwrap_or(0);
    let alignment = match byte(4) % 5 {
        0 => Alignment::Start,
        1 => Alignment::Center,
        2 => Alignment::End,
        3 => Alignment::Both,
        _ => Alignment::Distribute,
    };
    let line = match byte(5) % 3 {
        0 => LineSpacing::Multiple(f64::from(byte(6)) / 64.0),
        1 => LineSpacing::Exact(Pt(f64::from(byte(6)) / 4.0)),
        _ => LineSpacing::AtLeast(Pt(f64::from(byte(6)) / 4.0)),
    };
    let kinds = [
        TabKind::Start,
        TabKind::Center,
        TabKind::End,
        TabKind::Decimal,
        TabKind::Bar,
    ];
    let tabs = (0..byte(7) % 4).map(|i| {
        TabStop::default()
            .with_position(Pt(f64::from(byte(8 + usize::from(i))) * 2.0))
            .with_kind(kinds[usize::from(byte(12) >> (2 * i)) % kinds.len()])
    });
    let props = ParagraphProps::default()
        .with_alignment(alignment)
        .with_spacing(Spacing::default().with_line(line))
        .with_indent(
            Indent::default()
                .with_start(Pt(f64::from(byte(9))))
                .with_first_line(Pt(-f64::from(byte(10)))),
        )
        .with_tabs(tabs)
        .with_word_wrap(byte(11) & 1 == 0);
    let paragraph = Paragraph::new(first)
        .with_props(props)
        .with_inlines(inlines);
    // A list item when bit 7 of byte 1 says so (its bits 0 to 2 choose a font). Its label
    // is the number of level 0, from a start and in a format the high bits of byte 0 give,
    // then the text of the first piece, which may be long or hold control characters;
    // aligned and followed as bits 3 to 6 of byte 1 say; in the second style. Its copies
    // count on, and the header's counts afresh.
    let paragraph = if byte(1) & 0x80 != 0 {
        let formats = [
            NumberFormat::Decimal,
            NumberFormat::UpperRoman,
            NumberFormat::LowerLetter,
            NumberFormat::Ordinal,
            NumberFormat::Chicago,
            NumberFormat::DecimalZero,
            NumberFormat::None,
            NumberFormat::Bullet,
            NumberFormat::Other("hebrew1".to_owned()),
        ];
        let high = byte(0) >> 3;
        let literal = rest
            .split(|&byte| byte == 0xFF)
            .next()
            .and_then(|piece| piece.get(1..))
            .map(|text| String::from_utf8_lossy(text).into_owned())
            .unwrap_or_default();
        let level = Level::default()
            .with_start(u32::from(high) * 37)
            .with_format(formats[usize::from(high) % formats.len()].clone())
            .with_text([
                LevelTextPart::Placeholder(0),
                LevelTextPart::Literal(literal),
            ])
            .with_alignment(
                [
                    LevelAlignment::Start,
                    LevelAlignment::Center,
                    LevelAlignment::End,
                ][usize::from(byte(1) >> 3 & 3) % 3],
            )
            .with_suffix(
                [Suffix::Tab, Suffix::Space, Suffix::Nothing][usize::from(byte(1) >> 5 & 3) % 3],
            );
        let list = document.add_list(ListDefinition::default().with_levels([level]));
        let instance = document.add_list_instance(ListInstance::new(list));
        let label_style = styles.get(1).copied().unwrap_or(first);
        paragraph.with_list(Some(ListItem::new(instance, 0, label_style)))
    } else {
        paragraph
    };
    let width = 1.0 + f64::from(byte(13)) * 3.0;
    if let Err(problem) = check_lines(book(), &document, &paragraph, width) {
        panic!("{problem}");
    }
    // The same paragraph on pages: a body `width` wide and up to 530 pt high.
    let flags = byte(14);
    let props = paragraph
        .props()
        .clone()
        .with_keep_next(flags & 1 != 0)
        .with_keep_lines(flags & 2 != 0)
        .with_widow_control(flags & 4 == 0)
        .with_page_break_before(flags & 8 != 0);
    let paragraph = paragraph.with_props(props);
    let height = 20.0 + f64::from(byte(15)) * 2.0;
    let setup = PageSetup::new(PageSize::new(Pt(width + 20.0), Pt(height + 20.0))).with_margins(
        Margins::default()
            .with_top(Pt(10.0))
            .with_bottom(Pt(10.0))
            .with_left(Pt(10.0))
            .with_right(Pt(10.0)),
    );
    let copies = 1 + usize::from(flags >> 4 & 3);
    let mut blocks = (0..copies)
        .map(|_| paragraph.clone().into())
        .collect::<Vec<_>>();
    // The last copies (two of three or four, so that a chain can reach past the start)
    // in a section of their own, which starts on a new page, on the same one, on an odd
    // or even page, or in the next column.
    let second = if flags & 128 != 0 {
        blocks.split_off(blocks.len().saturating_sub(if copies > 2 { 2 } else { 1 }))
    } else {
        Vec::new()
    };
    let mut section = Section::new(setup).with_blocks(blocks);
    if flags & 64 != 0 {
        // A footer of the page fields, in a number format byte 12 gives; a title page and
        // even pages with their own frames when its bits say so.
        let formats = [
            NumberFormat::Decimal,
            NumberFormat::UpperRoman,
            NumberFormat::LowerLetter,
            NumberFormat::Ordinal,
            NumberFormat::Chicago,
            NumberFormat::DecimalZero,
            NumberFormat::None,
            NumberFormat::Other("hebrew1".to_owned()),
        ];
        let format = formats[usize::from(byte(12)) % formats.len()].clone();
        let field = |kind| Inline::Field(Field::new(kind, first).with_format(Some(format.clone())));
        let numbers = Paragraph::new(first).with_inlines([
            field(FieldKind::Page),
            field(FieldKind::NumPages),
            field(FieldKind::SectionPages),
        ]);
        let mut header = HeaderFooterSet::default().with_primary([paragraph.clone().into()]);
        let mut footer = HeaderFooterSet::default().with_primary([numbers.clone().into()]);
        if byte(12) & 16 != 0 {
            header = header.with_first([numbers.clone().into()]);
            footer = footer.with_first([paragraph.clone().into()]);
            section = section.with_title_page(true);
        }
        if byte(12) & 32 != 0 {
            header = header.with_even([numbers.into()]);
            footer = footer.with_even([paragraph.clone().into()]);
        }
        section = section.with_headers(header).with_footers(footer);
    }
    document.add_section(section);
    if !second.is_empty() {
        let starts = [
            SectionStart::NextPage,
            SectionStart::Continuous,
            SectionStart::EvenPage,
            SectionStart::OddPage,
            SectionStart::NextColumn,
        ];
        let start = starts[usize::from(byte(3)) % starts.len()];
        document.add_section(Section::new(setup).with_start(start).with_blocks(second));
    }
    if let Err(problem) = check_pages(book(), &document, 512) {
        panic!("{problem}");
    }
});
