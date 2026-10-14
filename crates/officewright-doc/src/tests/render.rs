//! The document rendered into PDF: `save_pdf`, `save_pdf_path` and `save_pdf_bytes`.

use officewright_layout::{LayoutReport, RenderOptions, ReportKind};

use super::embedded_fonts::{font, package_with_body_and_fonts, relationship, truetype};
use super::*;

/// Only the fonts the document embeds or the test gives, none of the system's, so that
/// what the tests see does not depend on the machine.
fn without_system_fonts() -> RenderOptions {
    RenderOptions::new().with_system_fonts(false)
}

/// The program of the TrueType font `tests/data/embedded-font.docx` embeds, whose only
/// glyph besides `.notdef` is "A", in the family "Officewright Test".
fn test_font() -> Vec<u8> {
    let document = Document::open("tests/data/embedded-font.docx").unwrap();
    document.fonts()[0].embedded()[0]
        .program()
        .unwrap()
        .to_vec()
}

/// A paragraph with `text` in the font "Officewright Test".
fn text_in_test_font(text: &str) -> String {
    format!(
        r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Officewright Test" w:hAnsi="Officewright Test"/></w:rPr><w:t>{text}</w:t></w:r></w:p>"#
    )
}

/// Whether the PDF holds `name`.
fn names(pdf: &[u8], name: &[u8]) -> bool {
    pdf.windows(name.len()).any(|window| window == name)
}

fn entries(report: &LayoutReport) -> Vec<String> {
    report.iter().map(ToString::to_string).collect()
}

/// The layout's entry for a font it finds nowhere.
const NO_TEST_FONT: &str = "document: \"Officewright Test\" → no font available";

#[test]
fn lays_text_out_in_the_font_the_document_embeds() {
    let program = test_font();
    let body = text_in_test_font("AAA");
    let embedding = font("Officewright Test", r#"<w:embedRegular r:id="rId1"/>"#);
    let embedded = Document::from_bytes(package_with_body_and_fonts(
        &body,
        &embedding,
        &relationship("rId1", "fonts/font1.odttf"),
        &[("word/fonts/font1.odttf", &program)],
    ))
    .unwrap();
    let (pdf, report) = embedded.save_pdf_bytes(&without_system_fonts()).unwrap();
    // The font goes into the PDF as a subset, its PostScript name after a tag, and the
    // layout finds it: nothing is said about it.
    assert!(names(&pdf, b"+OfficewrightTest"));
    let listed = entries(&report);
    assert!(
        listed
            .iter()
            .all(|entry| !entry.contains("Officewright Test")),
        "{listed:?}"
    );

    // The same document, declaring the font without embedding it: nothing draws "A".
    let declared = Document::from_bytes(package_with_body_and_fonts(
        &body,
        &font("Officewright Test", ""),
        "",
        &[],
    ))
    .unwrap();
    let (pdf, report) = declared.save_pdf_bytes(&without_system_fonts()).unwrap();
    assert!(!names(&pdf, b"OfficewrightTest"));
    let listed = entries(&report);
    assert!(
        listed.iter().any(|entry| entry == NO_TEST_FONT),
        "{listed:?}"
    );
    // Given by the caller instead, the font draws it again.
    let options = without_system_fonts().with_font_data(program);
    let (pdf, report) = declared.save_pdf_bytes(&options).unwrap();
    assert!(names(&pdf, b"+OfficewrightTest"));
    assert!(!entries(&report).iter().any(|entry| entry == NO_TEST_FONT));
}

/// The test font under another PostScript name, `OfficewrightUser`, in the same family:
/// the font parser does not check the name table's checksum.
fn renamed(program: &[u8]) -> Vec<u8> {
    let utf16 =
        |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_be_bytes).collect() };
    let (from, to) = (utf16("OfficewrightTest"), utf16("OfficewrightUser"));
    let at = program
        .windows(from.len())
        .position(|window| window == from)
        .unwrap();
    let mut renamed = program.to_vec();
    renamed[at..at + to.len()].copy_from_slice(&to);
    renamed
}

#[test]
fn prefers_the_font_the_document_embeds_to_one_of_the_same_family_the_caller_gives() {
    let program = test_font();
    let document = Document::from_bytes(package_with_body_and_fonts(
        &text_in_test_font("A"),
        &font("Officewright Test", r#"<w:embedRegular r:id="rId1"/>"#),
        &relationship("rId1", "fonts/font1.odttf"),
        &[("word/fonts/font1.odttf", &program)],
    ))
    .unwrap();
    let options = without_system_fonts().with_font_data(renamed(&program));
    let (pdf, _) = document.save_pdf_bytes(&options).unwrap();
    assert!(names(&pdf, b"+OfficewrightTest"));
    assert!(!names(&pdf, b"+OfficewrightUser"));
    // Without the embedded font, the caller's is used.
    let declared = Document::from_bytes(package_with_body_and_fonts(
        &text_in_test_font("A"),
        &font("Officewright Test", ""),
        "",
        &[],
    ))
    .unwrap();
    let (pdf, _) = declared.save_pdf_bytes(&options).unwrap();
    assert!(names(&pdf, b"+OfficewrightUser"));
}

#[test]
fn reports_an_embedded_font_it_cannot_read_once_and_renders_without_it() {
    // A part that starts as a TrueType font, so the reader keeps it, but is none: the
    // font book refuses it. Two styles share it; it is tried and reported once.
    let damaged = truetype(64);
    let fonts = font(
        "Broken",
        r#"<w:embedRegular r:id="rId1"/><w:embedBold r:id="rId1"/>"#,
    );
    // A paragraph with an absolute position tab, which the translation leaves out.
    let body = format!(
        r#"<w:p><w:r><w:ptab w:relativeTo="margin" w:alignment="left" w:leader="none"/></w:r></w:p>{}"#,
        text_in_test_font("A")
    );
    let document = Document::from_bytes(package_with_body_and_fonts(
        &body,
        &fonts,
        &relationship("rId1", "fonts/broken.odttf"),
        &[("word/fonts/broken.odttf", &damaged)],
    ))
    .unwrap();
    assert!(document.fonts()[0].embedded()[0].program().is_some());
    let (pdf, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    let listed = entries(&report);
    let broken: Vec<usize> = listed
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.contains("the font Broken"))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(broken.len(), 1, "{listed:?}");
    let font_entry = &report.entries()[broken[0]];
    assert_eq!(font_entry.kind(), ReportKind::FontSubstituted);
    assert!(
        font_entry.to_string().starts_with(
            "document: the w:embedRegular of the font Broken cannot be read, so other fonts are used: "
        ),
        "{listed:?}"
    );
    // The translation's entries come first, then the fonts', then the layout's.
    assert_eq!(
        listed[0],
        "sections[0].blocks[0].inlines: an absolute position tab is left out (w:ptab)"
    );
    assert_eq!(broken[0], 1, "{listed:?}");
    assert!(
        listed[2..].iter().any(|entry| entry == NO_TEST_FONT),
        "{listed:?}"
    );
}

#[test]
fn writes_the_same_pdf_to_a_writer_a_file_and_bytes() {
    let document = Document::open("tests/data/embedded-font.docx").unwrap();
    let options = without_system_fonts();
    let (bytes, report) = document.save_pdf_bytes(&options).unwrap();
    let mut written = Vec::new();
    assert_eq!(document.save_pdf(&mut written, &options).unwrap(), report);
    assert_eq!(written, bytes);
    let dir = std::env::temp_dir().join(format!("officewright-doc-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("embedded-font.pdf");
    assert_eq!(document.save_pdf_path(&path, &options).unwrap(), report);
    let saved = std::fs::read(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(saved, bytes);
}

#[test]
fn turns_what_stops_the_rendering_into_errors() {
    // More pages than the options allow.
    let two_pages =
        open(r#"<w:p><w:r><w:t>one</w:t><w:br w:type="page"/><w:t>two</w:t></w:r></w:p>"#);
    let err = two_pages
        .save_pdf_bytes(&without_system_fonts().with_max_pages(1))
        .unwrap_err();
    assert!(matches!(err, Error::Layout { .. }), "{err:?}");
    assert!(err.to_string().contains("more than 1 pages"), "{err}");
    assert!(matches!(
        officewright_core::Error::from(err),
        officewright_core::Error::LimitExceeded(_)
    ));
    assert!(
        two_pages
            .save_pdf_bytes(&without_system_fonts().with_max_pages(2))
            .is_ok()
    );

    // A font the caller gives that is none.
    let err = two_pages
        .save_pdf_bytes(&without_system_fonts().with_font_data(b"not a font".to_vec()))
        .unwrap_err();
    assert!(matches!(err, Error::Font { .. }), "{err:?}");
    assert!(matches!(
        officewright_core::Error::from(err),
        officewright_core::Error::Malformed(_)
    ));

    // A file that cannot be written, and a writer that fails.
    let path = std::env::temp_dir().join("officewright-doc-missing-dir/x/out.pdf");
    let err = two_pages
        .save_pdf_path(&path, &without_system_fonts())
        .unwrap_err();
    assert!(matches!(err, Error::Pdf { .. }), "{err:?}");
    assert!(err.to_string().starts_with("cannot write "), "{err}");
    assert!(matches!(
        officewright_core::Error::from(err),
        officewright_core::Error::Io { .. }
    ));
    struct Refusing;
    impl std::io::Write for Refusing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("refused"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let err = two_pages
        .save_pdf(Refusing, &without_system_fonts())
        .unwrap_err();
    assert!(matches!(err, Error::Pdf { .. }), "{err:?}");
    assert!(std::error::Error::source(&err).is_some());
}
