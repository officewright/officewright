//! Builds a paragraph from the input and itemizes and shapes it with the test fonts of
//! tests/fonts, as the layout does: the first bytes describe up to four run styles
//! (fonts, among them missing ones and the symbol fonts Symbol and Wingdings; bold,
//! italic, capitals, small capitals, complex script, right to left, the East Asian hint,
//! kerning; size), the rest are pieces of text, symbols, tabs and soft hyphens in those styles.
//! The paragraph must keep what the layout relies on (`check_paragraph`), and break into
//! lines that hold its text in order, in a column, with a line spacing, an alignment, a
//! hanging indent and tab stops the first bytes give too (`check_lines`). Up to four
//! copies of it, kept with the next, kept together, with or without widow control and
//! a page break before as the first bytes say, are then laid out on pages of a height
//! they give, the last of them in a second section of a start the first bytes give when
//! they say so, with every line on exactly one page inside its body (`check_pages`).
#![no_main]

use std::path::Path;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use officewright_core::PageSize;
use officewright_core::Pt;
use officewright_font::{FontBook, FontOrigin};
use officewright_layout::fuzzing::{check_lines, check_pages, check_paragraph};
use officewright_layout::ir::{
    Alignment, Document, FontHint, Indent, Inline, LineSpacing, Margins, PageSetup, Paragraph,
    ParagraphProps, RunFonts, RunStyle, Section, SectionStart, Spacing, TabKind, TabStop,
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
    let mut document = Document::default();
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
            _ => {
                if head & 0x40 == 0 {
                    Inline::tab(style)
                } else {
                    Inline::soft_hyphen(style)
                }
            }
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
    // The last copy in a section of its own, which starts on a new page, on the same one,
    // on an odd or even page, or in the next column.
    let second = if flags & 128 != 0 { blocks.pop() } else { None };
    document.add_section(Section::new(setup).with_blocks(blocks));
    if let Some(last) = second {
        let starts = [
            SectionStart::NextPage,
            SectionStart::Continuous,
            SectionStart::EvenPage,
            SectionStart::OddPage,
            SectionStart::NextColumn,
        ];
        let start = starts[usize::from(byte(3)) % starts.len()];
        document.add_section(Section::new(setup).with_start(start).with_blocks([last]));
    }
    if let Err(problem) = check_pages(book(), &document, 512) {
        panic!("{problem}");
    }
});
