//! Documents of the layout IR rendered into PDF: pages, glyph runs in the test fonts,
//! shading, lines, pictures and the report. With the test fonts of tests/fonts/ and the
//! PNG seeds of fuzz/seeds/ at the repository root, which are not part of this crate, so
//! this test is not in the published crate.
#![cfg(all(feature = "fuzzing", feature = "pdf"))]

mod common;

use common::{TestResult, book, random_paragraph, repository};
use officewright_core::{Color, PageSize, Pt};
use officewright_layout::ir::{
    ColorValue, Document, ImageData, ImageFormat, Inline, InlineObject, ObjectContent, PageSetup,
    Paragraph, ParagraphProps, Picture, RunFonts, RunStyle, Section, Shading, Size, TabLeader,
    TabStop, Underline, UnderlineKind,
};
use officewright_layout::{Error, RenderOptions, ReportKind, render_pdf};
use officewright_pdf::Compression;

/// Options that leave the PDF's streams as they are, so that a test can read them.
fn plain() -> RenderOptions {
    RenderOptions::new()
        .with_system_fonts(false)
        .with_compression(Compression::None)
}

/// A document of one section of A4 pages with `make`'s paragraphs, in Liberation Sans
/// 11 pt.
fn document(
    make: impl Fn(&mut Document, officewright_layout::ir::RunStyleId) -> Vec<Paragraph>,
) -> Document {
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(11.0)),
    );
    let paragraphs = make(&mut document, style);
    document.add_section(
        Section::new(PageSetup::new(PageSize::A4))
            .with_blocks(paragraphs.into_iter().map(Into::into).collect::<Vec<_>>()),
    );
    document
}

/// The PDF as text, its binary streams included as they are.
fn text(pdf: &officewright_pdf::Document) -> TestResult<String> {
    Ok(String::from_utf8_lossy(&pdf.to_bytes()?).into_owned())
}

#[test]
fn text_is_drawn_in_its_embedded_font_on_pages_of_its_size() -> TestResult {
    let book = book()?;
    let document = document(|_, style| {
        (0..80)
            .map(|n| {
                Paragraph::new(style).with_inlines([Inline::text(
                    format!("Paragraph {n} of a document long enough for two pages."),
                    style,
                )])
            })
            .collect()
    });
    let rendered = render_pdf(&document, &book, &plain())?;
    assert_eq!(rendered.pdf().page_count(), 2);
    let pdf = text(rendered.pdf())?;
    assert!(
        pdf.contains("LiberationSans"),
        "no embedded Liberation Sans"
    );
    // A4, in points.
    assert!(pdf.contains("595.2"), "no A4 media box");
    // Glyphs shown with positions.
    assert!(pdf.contains("TJ") || pdf.contains("Tj"));
    assert!(rendered.report().is_empty(), "{:?}", rendered.report());
    Ok(())
}

#[test]
fn shading_underlines_leaders_and_pictures_are_painted() -> TestResult {
    let book = book()?;
    let png = std::fs::read(repository().join("fuzz/seeds/png/rgb.png"))?;
    let document = document(|document, style| {
        let wavy = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Liberation Sans"))
                .with_size(Pt(11.0))
                .with_underline(Some(Underline::default().with_kind(UnderlineKind::Wave))),
        );
        let image = document.add_image(ImageData::new(ImageFormat::Png, png.clone()));
        let emf = document.add_image(ImageData::new(ImageFormat::Other("emf".into()), vec![1, 2]));
        let picture = |image| {
            Inline::Object(InlineObject::new(
                ObjectContent::Picture(Picture::new(image)),
                Size::new(Pt(40.0), Pt(30.0)),
            ))
        };
        let shaded = ParagraphProps::default()
            .with_shading(Some(
                Shading::default().with_fill(ColorValue::Rgb(Color::rgb(200, 200, 200))),
            ))
            .with_tabs([TabStop::default()
                .with_position(Pt(300.0))
                .with_leader(TabLeader::Dot)]);
        vec![
            Paragraph::new(style).with_props(shaded).with_inlines([
                Inline::text("Wavy", wavy),
                Inline::tab(style),
                Inline::text("12", style),
            ]),
            Paragraph::new(style).with_inlines([picture(image), picture(emf)]),
        ]
    });
    let rendered = render_pdf(&document, &book, &plain())?;
    let pdf = text(rendered.pdf())?;
    // The paragraph's shading and the placeholder of the EMF picture: filled rectangles.
    assert!(
        pdf.matches(" re\nf").count() + pdf.matches(" re f").count() >= 2,
        "no filled box"
    );
    // The wavy underline: one path of curves.
    assert!(pdf.contains(" c\n") || pdf.contains(" c "), "no curve");
    // The PNG picture: an image XObject drawn.
    assert!(pdf.contains("/Subtype /Image") || pdf.contains("/Subtype/Image"));
    assert!(pdf.contains(" Do"), "no image drawn");
    // The dot leader stands for its periods through the ToUnicode map, with no
    // replacement text.
    assert!(
        !pdf.contains("/ActualText"),
        "a leader needs no replacement text"
    );
    assert!(
        rendered
            .report()
            .entries()
            .iter()
            .any(|entry| entry.kind() == ReportKind::FeatureUnsupported
                && entry.message().contains("emf")),
        "{:?}",
        rendered.report()
    );
    Ok(())
}

#[test]
fn rendering_stops_at_the_page_limit_and_refuses_a_broken_document() -> TestResult {
    let book = book()?;
    let long = document(|_, style| {
        (0..200)
            .map(|_| Paragraph::new(style).with_inlines([Inline::text("A line.", style)]))
            .collect()
    });
    let limited = render_pdf(&long, &book, &plain().with_max_pages(2));
    assert!(
        matches!(limited, Err(Error::TooManyPages { limit: 2, .. })),
        "{limited:?}"
    );
    assert!(render_pdf(&long, &book, &plain().with_max_pages(10)).is_ok());
    // A run style of another document is something no correct producer makes.
    let mut other = Document::default();
    let foreign = other.add_run_style(RunStyle::default());
    let broken = document(|_, _| vec![Paragraph::new(foreign)]);
    assert!(matches!(
        render_pdf(&broken, &book, &plain()),
        Err(Error::InvalidDocument { .. })
    ));
    Ok(())
}

#[test]
fn values_beyond_word_s_limits_are_clamped_and_reported() -> TestResult {
    // A character scale of 900 %: Word's limit is 600 %.
    let book = book()?;
    let document = document(|document, _| {
        let wide = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Liberation Sans"))
                .with_scale(900),
        );
        vec![Paragraph::new(wide).with_inlines([Inline::text("Wide", wide)])]
    });
    let rendered = render_pdf(&document, &book, &plain())?;
    assert!(
        rendered
            .report()
            .entries()
            .iter()
            .any(|entry| entry.kind() == ReportKind::ValueClamped),
        "{:?}",
        rendered.report()
    );
    Ok(())
}

#[test]
fn the_first_line_is_drawn_from_the_top_and_synthetic_bold_is_stroked() -> TestResult {
    let book = book()?;
    let document = document(|document, style| {
        // Noto Sans of tests/fonts has no bold face: bold is drawn by stroking.
        let bold = document.add_run_style(
            RunStyle::default()
                .with_fonts(RunFonts::new("Noto Sans"))
                .with_size(Pt(11.0))
                .with_bold(true),
        );
        vec![
            Paragraph::new(style).with_inlines([Inline::text("Top", style)]),
            Paragraph::new(bold).with_inlines([Inline::text("bold", bold)]),
        ]
    });
    let rendered = render_pdf(&document, &book, &plain())?;
    let pdf = text(rendered.pdf())?;
    // "1 0 0 1 X Y Tm": the first run starts at the left margin, its baseline 72 pt and
    // Liberation Sans' 10.32 pt below the top of the A4 page, 841.89 pt high.
    let first = pdf
        .lines()
        .find(|line| line.ends_with(" Tm"))
        .ok_or("no text matrix")?;
    let numbers: Vec<f64> = first
        .split(' ')
        .filter_map(|word| word.parse().ok())
        .collect();
    let (x, y) = (numbers.get(4).copied(), numbers.get(5).copied());
    assert_eq!(x, Some(72.0), "{first}");
    let y = y.ok_or("no y")?;
    assert!((y - (841.89 - 72.0 - 10.32)).abs() < 0.05, "{first}");
    // Fill and stroke for the synthetic bold.
    assert!(pdf.contains("2 Tr"), "no stroked text");
    Ok(())
}

#[test]
fn damaged_and_turned_pictures_are_drawn_as_they_can_be_and_reported() -> TestResult {
    let book = book()?;
    let png = std::fs::read(repository().join("fuzz/seeds/png/rgb.png"))?;
    let document = document(|document, style| {
        let turned = document.add_image(ImageData::new(ImageFormat::Png, png.clone()));
        let damaged = document.add_image(ImageData::new(ImageFormat::Png, vec![0x89, b'P', 1]));
        let picture = |picture: Picture| {
            Inline::Object(InlineObject::new(
                ObjectContent::Picture(picture),
                Size::new(Pt(40.0), Pt(30.0)),
            ))
        };
        vec![Paragraph::new(style).with_inlines([
            picture(Picture::new(turned).with_rotation(90.0)),
            picture(Picture::new(damaged)),
        ])]
    });
    let rendered = render_pdf(&document, &book, &plain())?;
    let messages: Vec<&str> = rendered
        .report()
        .entries()
        .iter()
        .map(|entry| entry.message())
        .collect();
    assert!(
        messages.iter().any(|message| message.contains("turned")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|message| message.contains("grey box")),
        "{messages:?}"
    );
    let pdf = text(rendered.pdf())?;
    // The turned picture is drawn upright; the damaged one is a grey box.
    assert!(pdf.contains(" Do"), "no image drawn");
    assert!(
        pdf.contains("0.753 0.753 0.753") || pdf.contains(".7529"),
        "no grey box"
    );
    Ok(())
}

#[test]
fn every_random_document_renders() -> TestResult {
    // Paragraphs of every ingredient (`random_paragraph`), several to a document, on small
    // pages: each renders into a PDF with a page, whatever the fonts, tabs and breaks.
    let book = book()?;
    for seed in 0..200 {
        let mut document = Document::default();
        let mut blocks = Vec::new();
        let mut width = 100.0;
        for index in 0..4 {
            let (paragraph, column) = random_paragraph(seed * 4 + index, &mut document);
            width = column;
            blocks.push(paragraph.into());
        }
        let setup = PageSetup::new(PageSize::new(Pt(width + 144.0), Pt(300.0)));
        document.add_section(Section::new(setup).with_blocks(blocks));
        let rendered =
            render_pdf(&document, &book, &plain()).map_err(|err| format!("seed {seed}: {err}"))?;
        assert!(rendered.pdf().page_count() >= 1, "seed {seed}");
        let bytes = rendered.pdf().to_bytes()?;
        assert!(bytes.starts_with(b"%PDF-"), "seed {seed}");
    }
    Ok(())
}

#[test]
fn content_beyond_what_a_pdf_holds_still_renders() -> TestResult {
    use officewright_layout::ir::{BreakClear, HeaderFooterSet};
    let book = book()?;
    let png = std::fs::read(repository().join("fuzz/seeds/png/rgb.png"))?;
    let picture = |image, width: f64, height: f64| {
        Inline::Object(InlineObject::new(
            ObjectContent::Picture(Picture::new(image)),
            Size::new(Pt(width), Pt(height)),
        ))
    };
    // A paragraph of 3,000 lines, each "A".
    let tall = |style| {
        let mut inlines = Vec::new();
        for line in 0..3_000 {
            if line > 0 {
                inlines.push(Inline::line_break(BreakClear::None, style));
            }
            inlines.push(Inline::text("A", style));
        }
        Paragraph::new(style).with_inlines(inlines)
    };
    let cases: Vec<(&str, Document)> = vec![
        (
            "pictures of no size",
            document(|document, style| {
                let image = document.add_image(ImageData::new(ImageFormat::Png, png.clone()));
                vec![
                    Paragraph::new(style)
                        .with_inlines([picture(image, 0.0, 0.0), picture(image, 0.0, 20.0)]),
                ]
            }),
        ),
        (
            "pictures of 40,000 pt",
            document(|document, style| {
                let image = document.add_image(ImageData::new(ImageFormat::Png, png.clone()));
                let emf =
                    document.add_image(ImageData::new(ImageFormat::Other("emf".into()), vec![1]));
                vec![Paragraph::new(style).with_inlines([
                    picture(image, 40_000.0, 40_000.0),
                    picture(emf, 40_000.0, 40_000.0),
                ])]
            }),
        ),
        ("a header and a footer of 3,000 lines", {
            let mut framed = Document::default();
            let style = framed.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Liberation Sans"))
                    .with_size(Pt(11.0)),
            );
            let frame = HeaderFooterSet::default().with_primary([tall(style).into()]);
            framed.add_section(
                Section::new(PageSetup::new(PageSize::A4))
                    .with_headers(frame.clone())
                    .with_footers(frame)
                    .with_blocks([Paragraph::new(style).into()]),
            );
            framed
        }),
        (
            "a picture 33,000 pt tall",
            document(|document, style| {
                let image = document.add_image(ImageData::new(ImageFormat::Png, png.clone()));
                vec![Paragraph::new(style).with_inlines([picture(image, 40.0, 33_000.0)])]
            }),
        ),
        ("a page 2 pt square", {
            let mut tiny = Document::default();
            let style = tiny.add_run_style(
                RunStyle::default()
                    .with_fonts(RunFonts::new("Liberation Sans"))
                    .with_size(Pt(11.0)),
            );
            let setup = PageSetup::new(PageSize::new(Pt(2.0), Pt(2.0))).with_margins(
                officewright_layout::ir::Margins::default()
                    .with_top(Pt(0.0))
                    .with_bottom(Pt(0.0))
                    .with_left(Pt(0.0))
                    .with_right(Pt(0.0)),
            );
            tiny.add_section(
                Section::new(setup).with_blocks([Paragraph::new(style)
                    .with_inlines([Inline::text("A", style)])
                    .into()]),
            );
            tiny
        }),
    ];
    for (case, document) in cases {
        let rendered =
            render_pdf(&document, &book, &plain()).map_err(|err| format!("{case}: {err}"))?;
        let bytes = rendered.pdf().to_bytes()?;
        assert!(bytes.starts_with(b"%PDF-"), "{case}");
        let reported = |text: &str| {
            rendered
                .report()
                .entries()
                .iter()
                .any(|entry| entry.message().contains(text))
        };
        match case {
            "pictures of 40,000 pt" | "a picture 33,000 pt tall" => {
                assert!(
                    reported("more than a PDF holds"),
                    "{case}: {:?}",
                    rendered.report()
                );
            }
            "a page 2 pt square" => {
                assert!(
                    reported("pages narrower or shorter"),
                    "{case}: {:?}",
                    rendered.report()
                );
            }
            _ => {}
        }
    }
    Ok(())
}

#[test]
fn the_options_make_a_book_of_their_fonts() -> TestResult {
    let fonts = repository().join("tests/fonts/carlito");
    let families = |book: &officewright_font::FontBook| -> Vec<String> {
        book.faces()
            .filter_map(|(_, face)| face.names().family().map(str::to_owned))
            .collect()
    };
    let from_dir = RenderOptions::new()
        .with_system_fonts(false)
        .with_font_dir(&fonts)
        .font_book()?;
    assert_eq!(from_dir.len(), 4);
    assert!(families(&from_dir).iter().all(|family| family == "Carlito"));
    let data = std::fs::read(repository().join("tests/fonts/liberation/LiberationMono-Bold.ttf"))?;
    let in_memory = RenderOptions::new()
        .with_system_fonts(false)
        .with_font_data(data)
        .font_book()?;
    assert_eq!(families(&in_memory), ["Liberation Mono"]);
    Ok(())
}

/// officewright-font's test font (glyphs 0 `.notdef`, 1 space, 2 `A`, 3 acute, 4 `Á`)
/// with glyph `glyph` damaged as [`damage_glyph`] does.
fn with_damaged_glyph(glyph: usize) -> TestResult<Vec<u8>> {
    let mut font =
        std::fs::read(repository().join("crates/officewright-font/tests/data/minimal.ttf"))?;
    damage_glyph(&mut font, glyph)?;
    Ok(font)
}

/// Damages glyph `glyph` of `font`, a TrueType font with long `loca` offsets: its `loca`
/// entry sends it past the end of `glyf`, so that it cannot be subset.
fn damage_glyph(font: &mut [u8], glyph: usize) -> TestResult {
    let be32 = |font: &[u8], at: usize| -> TestResult<u32> {
        Ok(u32::from_be_bytes(
            font.get(at..at + 4).ok_or("truncated")?.try_into()?,
        ))
    };
    let tables = usize::try_from(be32(font, 4)? >> 16)?;
    let loca = (0..tables)
        .map(|i| 12 + 16 * i)
        .find(|&at| font.get(at..at + 4) == Some(b"loca"))
        .ok_or("no loca")?;
    let loca = usize::try_from(be32(font, loca + 8)?)?;
    // Long offsets: a glyph ends at the entry after its own.
    let end = loca + 4 * (glyph + 1);
    font.get_mut(end..end + 4)
        .ok_or("no loca entry")?
        .copy_from_slice(&0x0100_0000_u32.to_be_bytes());
    Ok(())
}

/// A document of one paragraph of `text` in the test font at 11 pt.
fn in_test_font(text: &str) -> Document {
    let mut document = Document::default();
    let test = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Officewright Test"))
            .with_size(Pt(11.0)),
    );
    document.add_section(Section::new(PageSetup::new(PageSize::A4)).with_blocks(vec![
        Paragraph::new(test).with_inlines([Inline::text(text, test)]).into(),
    ]));
    document
}

/// The messages of the report's entries of `kind`.
fn messages(report: &officewright_layout::LayoutReport, kind: ReportKind) -> Vec<String> {
    report
        .entries()
        .iter()
        .filter(|entry| entry.kind() == kind)
        .map(|entry| entry.message().to_owned())
        .collect()
}

#[test]
fn a_soft_hyphen_takes_the_hyphen_minus_where_the_hyphen_cannot_be_embedded() -> TestResult {
    use officewright_font::{FontFace, FontOrigin};
    use officewright_layout::fuzzing;
    // Liberation Sans with its glyph of U+2010 damaged (long `loca` offsets): the hyphen
    // drawn where a line breaks at a soft hyphen is the hyphen-minus, which it can embed.
    let mut font =
        std::fs::read(repository().join("tests/fonts/liberation/LiberationSans-Regular.ttf"))?;
    let face = FontFace::from_bytes(font.clone(), 0)?;
    let (hyphen, minus) = (
        face.glyph_id('\u{2010}').ok_or("no U+2010")?,
        face.glyph_id('-').ok_or("no hyphen-minus")?,
    );
    damage_glyph(&mut font, usize::from(hyphen.to_u16()))?;
    let mut book = officewright_font::FontBook::new();
    book.add_bytes(font, FontOrigin::Document)?;
    let mut document = Document::default();
    let style = document.add_run_style(
        RunStyle::default()
            .with_fonts(RunFonts::new("Liberation Sans"))
            .with_size(Pt(11.0)),
    );
    let paragraph = Paragraph::new(style)
        .with_inlines([Inline::text("Hyphen\u{AD}ation hyphen\u{AD}ation", style)]);
    let painted = fuzzing::paint(&book, &document, &[paragraph], 60.0);
    let dump = painted.text();
    assert!(
        dump.contains(&format!(" {}@", minus.to_u16())),
        "no hyphen-minus: {dump}"
    );
    assert!(
        !dump.contains(&format!(" {}@", hyphen.to_u16())),
        "the damaged hyphen is drawn: {dump}"
    );
    Ok(())
}

#[test]
fn text_of_a_font_with_a_damaged_glyph_is_drawn_in_another_font() -> TestResult {
    // Issue #217: an embedded font whose glyph of `A` cannot be subset would make the whole
    // PDF fail. Word shows such text in another font: here Carlito, the first face of the
    // test fonts with `A`, and the face is reported.
    let mut book = book()?;
    book.add_bytes(
        with_damaged_glyph(2)?,
        officewright_font::FontOrigin::Document,
    )?;
    let rendered = render_pdf(&in_test_font("A"), &book, &plain())?;
    let pdf = text(rendered.pdf())?;
    assert!(pdf.contains("Carlito"), "A is not in Carlito");
    assert!(
        !pdf.contains("OfficewrightTest"),
        "the damaged face is embedded"
    );
    assert_eq!(
        messages(rendered.report(), ReportKind::FontEmbedding),
        [
            "some glyphs of Officewright Test cannot be embedded (cannot subset font face 0: \
             glyph 2: its loca entries do not give a range of the glyf table); their text is \
             drawn in other fonts, or as the missing-glyph shape where no other font has it"
        ]
    );
    Ok(())
}

#[test]
fn a_glyph_shaping_chose_that_cannot_be_embedded_is_drawn_as_the_missing_glyph() -> TestResult {
    // `Á` damaged, and no other font: shaping composes `A` and a combining acute, which the
    // font has whole, into `Á`, which the character map did not give; and `Á` typed has no
    // other font to fall back to. Both are drawn as `.notdef`, and the PDF is made.
    let mut book = officewright_font::FontBook::new();
    book.add_bytes(
        with_damaged_glyph(4)?,
        officewright_font::FontOrigin::Document,
    )?;
    for (typed, code) in [("A\u{301}", "0301"), ("\u{C1}", "00C1")] {
        let rendered = render_pdf(&in_test_font(typed), &book, &plain())?;
        let pdf = text(rendered.pdf())?;
        assert!(pdf.contains("OfficewrightTest"), "{typed:?}");
        // The missing-glyph shape stands for the text, which the PDF still gives.
        assert!(pdf.contains(code), "{typed:?}: no {code} in the text");
        assert_eq!(
            messages(rendered.report(), ReportKind::FontEmbedding).len(),
            1,
            "{typed:?}: {:?}",
            rendered.report()
        );
    }
    Ok(())
}
