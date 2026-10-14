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
