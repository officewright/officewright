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
    assert_eq!(font_entry.kind(), ReportKind::FontEmbedding);
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

/// The test font with every one of its names `Officewright Test` replaced by `name`, of
/// the same length: what Word does to a subset it embeds (`___WRD_EMBED_SUB_` and a
/// number).
fn named(program: &[u8], name: &str) -> Vec<u8> {
    renamed_where(program, name, usize::MAX)
}

/// The test font with its first `count` names `Officewright Test` replaced by `name`.
fn renamed_where(program: &[u8], name: &str, count: usize) -> Vec<u8> {
    let utf16 =
        |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_be_bytes).collect() };
    let (from, to) = (utf16("Officewright Test"), utf16(name));
    assert_eq!(from.len(), to.len());
    let mut renamed = program.to_vec();
    let (mut at, mut replaced) = (0, 0);
    while let Some(found) = renamed[at..]
        .windows(from.len())
        .position(|window| window == from)
    {
        if replaced == count {
            break;
        }
        renamed[at + found..at + found + to.len()].copy_from_slice(&to);
        at += found + to.len();
        replaced += 1;
    }
    assert!(replaced > 0);
    renamed
}

#[test]
fn reports_an_embedded_font_whose_names_the_text_cannot_reach() {
    // Word renames a subset it embeds; the program answers to none of the font's names.
    let program = named(&test_font(), "___WRD_EMBED_S_17");
    let document = Document::from_bytes(package_with_body_and_fonts(
        &text_in_test_font("A"),
        &font("Officewright Test", r#"<w:embedRegular r:id="rId1"/>"#),
        &relationship("rId1", "fonts/font1.odttf"),
        &[("word/fonts/font1.odttf", &program)],
    ))
    .unwrap();
    let (_, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
    let listed = entries(&report);
    let entry = report
        .iter()
        .find(|entry| entry.message().contains("holds a font named"))
        .unwrap_or_else(|| panic!("{listed:?}"));
    assert_eq!(entry.kind(), ReportKind::FontSubstituted);
    assert_eq!(
        entry.to_string(),
        "document: the w:embedRegular of the font Officewright Test holds a font named \
         ___WRD_EMBED_S_17, which the text cannot reach by the font's name, so other fonts \
         are used"
    );
    assert!(
        listed.iter().any(|entry| entry == NO_TEST_FONT),
        "{listed:?}"
    );

    // A font's name in another case, or the program's full name alone, is the font's
    // name as the font book compares names: nothing to report.
    let reached = |program: &[u8], name: &str| {
        let document = Document::from_bytes(package_with_body_and_fonts(
            &text_in_test_font("A"),
            &font(name, r#"<w:embedRegular r:id="rId1"/>"#),
            &relationship("rId1", "fonts/font1.odttf"),
            &[("word/fonts/font1.odttf", program)],
        ))
        .unwrap();
        let (_, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
        !report
            .iter()
            .any(|entry| entry.message().contains("holds a font named"))
    };
    assert!(reached(&test_font(), "OFFICEWRIGHT TEST"));
    // The PostScript name too, which the font book finds the font by.
    assert!(reached(&test_font(), "OfficewrightTest"));
    // The family renamed, the full name (name ID 4, after it) kept.
    assert!(reached(
        &renamed_where(&test_font(), "___WRD_EMBED_S_17", 1),
        "Officewright Test"
    ));
    assert!(!reached(&program, "Officewright Test"));
}

#[test]
fn tries_each_program_a_font_embeds() {
    // A font with a regular program and a damaged bold one: the bold is tried too, and
    // reported by its own style.
    let program = test_font();
    let damaged = truetype(64);
    let document = Document::from_bytes(package_with_body_and_fonts(
        &text_in_test_font("A"),
        &font(
            "Officewright Test",
            r#"<w:embedRegular r:id="rId1"/><w:embedBold r:id="rId2"/>"#,
        ),
        &[
            relationship("rId1", "fonts/font1.odttf"),
            relationship("rId2", "fonts/font2.odttf"),
        ]
        .concat(),
        &[
            ("word/fonts/font1.odttf", &program),
            ("word/fonts/font2.odttf", &damaged),
        ],
    ))
    .unwrap();
    let (pdf, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
    assert!(names(&pdf, b"+OfficewrightTest"));
    let fonts: Vec<String> = report
        .iter()
        .filter(|entry| entry.message().contains("the font Officewright Test"))
        .map(ToString::to_string)
        .collect();
    assert_eq!(fonts.len(), 1, "{fonts:?}");
    assert!(
        fonts[0]
            .starts_with("document: the w:embedBold of the font Officewright Test cannot be read"),
        "{fonts:?}"
    );
}

#[test]
fn says_each_message_of_an_error_once() {
    // The errors of rendering are passed through, so walking the causes says nothing
    // twice.
    let document = open("<w:p/>");
    let chain = |err: &Error| {
        let mut messages = vec![err.to_string()];
        let mut cause = std::error::Error::source(err);
        while let Some(next) = cause {
            messages.push(next.to_string());
            cause = next.source();
        }
        messages
    };
    let path = std::env::temp_dir().join("officewright-doc-missing-dir/y/out.pdf");
    let err = document
        .save_pdf_path(&path, &without_system_fonts())
        .unwrap_err();
    let messages = chain(&err);
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].starts_with("cannot write "), "{messages:?}");
    assert_ne!(messages[0], messages[1]);
    let err = document
        .save_pdf_bytes(&without_system_fonts().with_font_data(b"not a font".to_vec()))
        .unwrap_err();
    let messages = chain(&err);
    let mut unique = messages.clone();
    unique.dedup();
    assert_eq!(unique, messages);
    // A document past the page limit: the layout's error, which has no cause.
    let two_pages =
        open(r#"<w:p><w:r><w:t>one</w:t><w:br w:type="page"/><w:t>two</w:t></w:r></w:p>"#);
    let err = two_pages
        .save_pdf_bytes(&without_system_fonts().with_max_pages(1))
        .unwrap_err();
    assert_eq!(chain(&err).len(), 1, "{:?}", chain(&err));
    // The font library's own message.
    let refused = officewright_font::FontBook::new()
        .add_bytes(b"not a font".to_vec(), officewright_font::FontOrigin::User)
        .unwrap_err();
    assert_eq!(messages[0], refused.to_string());
}

/// The test font as a collection (`ttcf`) of `faces` faces, each the same font: its
/// tables placed after the collection's header, their offsets moved to match.
fn collection(program: &[u8], faces: u32) -> Vec<u8> {
    let header = 12 + 4 * faces as usize;
    let mut font = program.to_vec();
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    for table in 0..tables {
        let at = 12 + 16 * table + 8;
        let offset = u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
        font[at..at + 4].copy_from_slice(&(offset + header as u32).to_be_bytes());
    }
    let mut bytes = b"ttcf".to_vec();
    bytes.extend(0x0001_0000u32.to_be_bytes());
    bytes.extend(faces.to_be_bytes());
    for _ in 0..faces {
        bytes.extend((header as u32).to_be_bytes());
    }
    bytes.extend(font);
    bytes
}

/// A document whose text is in "Officewright Test", with the fonts `fonts` (each a name
/// and a program) embedded as regular, in that order.
fn embedding(fonts: &[(&str, &[u8])]) -> Document {
    let declared: String = fonts
        .iter()
        .enumerate()
        .map(|(n, (name, _))| font(name, &format!(r#"<w:embedRegular r:id="rId{n}"/>"#)))
        .collect();
    let relationships: String = (0..fonts.len())
        .map(|n| relationship(&format!("rId{n}"), &format!("fonts/font{n}.odttf")))
        .collect();
    let paths: Vec<String> = (0..fonts.len())
        .map(|n| format!("word/fonts/font{n}.odttf"))
        .collect();
    let files: Vec<(&str, &[u8])> = paths
        .iter()
        .zip(fonts)
        .map(|(path, (_, program))| (path.as_str(), *program))
        .collect();
    Document::from_bytes(package_with_body_and_fonts(
        &text_in_test_font("A"),
        &declared,
        &relationships,
        &files,
    ))
    .unwrap()
}

#[test]
fn uses_at_most_two_hundred_and_fifty_six_embedded_faces() {
    // A collection takes a few kilobytes for a thousand faces; the faces are what costs,
    // so the document's embedded fonts add at most 256.
    let render = |document: &Document| {
        let (pdf, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
        (pdf, report)
    };
    let (pdf, report) = render(&embedding(&[(
        "Officewright Test",
        &collection(&test_font(), 256),
    )]));
    assert!(names(&pdf, b"+OfficewrightTest"));
    assert!(
        !entries(&report)
            .iter()
            .any(|entry| entry.contains("left out")),
        "{:?}",
        entries(&report)
    );
    let (pdf, report) = render(&embedding(&[(
        "Officewright Test",
        &collection(&test_font(), 257),
    )]));
    assert!(!names(&pdf, b"+OfficewrightTest"));
    let entry = report
        .iter()
        .find(|entry| entry.message().contains("left out"))
        .unwrap();
    assert_eq!(entry.kind(), ReportKind::FontEmbedding);
    assert_eq!(
        entry.to_string(),
        "document: 1 embedded font program is left out, past 256 faces and named instances \
         in all, so other fonts are used"
    );
    // The faces of all the programs count together: of two collections of 200 faces,
    // the second is past the limit; a program after it that fits is still used.
    let big = |name: &str| collection(&named(&test_font(), name), 200);
    let (first, second) = (big("Officewright Big1"), big("Officewright Big2"));
    let third = test_font();
    let (pdf, report) = render(&embedding(&[
        ("Officewright Big1", &first),
        ("Officewright Big2", &second),
        ("Officewright Test", &third),
    ]));
    assert!(names(&pdf, b"+OfficewrightTest"));
    let listed = entries(&report);
    assert!(
        listed.iter().any(|entry| entry
            == "document: 1 embedded font program is left out, past 256 faces and named \
                instances in all, so other fonts are used"),
        "{listed:?}"
    );
    // The same within one font: its bold program is past the limit, its italic one,
    // after it, is still used.
    let document = Document::from_bytes(package_with_body_and_fonts(
        &text_in_test_font("A"),
        &font(
            "Mixed",
            r#"<w:embedRegular r:id="rId0"/><w:embedBold r:id="rId1"/><w:embedItalic r:id="rId2"/>"#,
        ),
        &(0..3)
            .map(|n| relationship(&format!("rId{n}"), &format!("fonts/font{n}.odttf")))
            .collect::<String>(),
        &[
            ("word/fonts/font0.odttf", &first),
            ("word/fonts/font1.odttf", &second),
            ("word/fonts/font2.odttf", &third),
        ],
    ))
    .unwrap();
    let (pdf, _) = render(&document);
    assert!(names(&pdf, b"+OfficewrightTest"));
    // Two past it.
    let (_, report) = render(&embedding(&[
        ("Officewright Big1", &first),
        ("Officewright Big2", &second),
        ("Officewright Big3", &big("Officewright Big3")),
    ]));
    assert!(
        entries(&report)
            .iter()
            .any(|entry| entry.starts_with("document: 2 embedded font programs are left out")),
        "{:?}",
        entries(&report)
    );
}

/// The test font with its name table replaced by one at its end with `records`, each
/// (platform, language, name ID, length in bytes) of letters A; and, for each ID of
/// `instances`, a named instance of a variable font whose subfamily name has that ID, in
/// an `fvar` table that takes the place of `post`.
fn with_names(records: &[(u16, u16, u16, u16)], instances: &[u16]) -> Vec<u8> {
    with_names_and_axis_size(records, instances, 20)
}

/// As [`with_names`], with `axis_size` written as the size of the `fvar` table's axis
/// records, which stay of 20 bytes, the instances after them.
fn with_names_and_axis_size(
    records: &[(u16, u16, u16, u16)],
    instances: &[u16],
    axis_size: u16,
) -> Vec<u8> {
    with_axis_records(records, instances, axis_size, 20)
}

/// As [`with_names`], with `axis_size` written as the size of the axis records and the
/// axis record `written` bytes long, the instances after it.
fn with_axis_records(
    records: &[(u16, u16, u16, u16)],
    instances: &[u16],
    axis_size: u16,
    written: usize,
) -> Vec<u8> {
    let mut program = test_font();
    let tables = usize::from(u16::from_be_bytes([program[4], program[5]]));
    let directory = |program: &[u8], tag: &[u8; 4]| {
        (0..tables)
            .map(|table| 12 + 16 * table)
            .find(|&at| &program[at..at + 4] == tag)
            .unwrap()
    };
    let words =
        |values: &[u16]| -> Vec<u8> { values.iter().flat_map(|n| n.to_be_bytes()).collect() };
    let longest = records.iter().map(|record| record.3).max().unwrap_or(0);
    let count = records.len() as u16;
    let mut name = words(&[0, count, 6 + 12 * count]);
    for &(platform, language, id, bytes) in records {
        name.extend(words(&[platform, 1, language, id, bytes, 0]));
    }
    name.extend(std::iter::repeat_n([0u8, b'A'], usize::from(longest / 2)).flatten());
    let place = |program: &mut Vec<u8>, record: usize, table: Vec<u8>| {
        let at = program.len() as u32;
        program[record + 8..record + 12].copy_from_slice(&at.to_be_bytes());
        program[record + 12..record + 16].copy_from_slice(&(table.len() as u32).to_be_bytes());
        program.extend(table);
    };
    let record = directory(&program, b"name");
    place(&mut program, record, name);
    if !instances.is_empty() {
        // One axis, wght, of 20 bytes; instances of 8 bytes: name ID, flags, a coordinate.
        let mut fvar = words(&[1, 0, 16, 2, 1, axis_size, instances.len() as u16, 8]);
        fvar.extend(b"wght");
        fvar.extend(words(&[100, 0, 400, 0, 900, 0, 0, 256]));
        fvar.resize(fvar.len() + written - 20, 0);
        for &id in instances {
            fvar.extend(words(&[id, 0, 400, 0]));
        }
        let record = directory(&program, b"post");
        program[record..record + 4].copy_from_slice(b"fvar");
        place(&mut program, record, fvar);
    }
    program
}

/// The test font with a family name of `bytes` bytes (`bytes / 2` letters A).
fn with_family_of(bytes: u16) -> Vec<u8> {
    with_names(&[(3, 0x409, 1, bytes)], &[])
}

/// A collection of `programs`, laid out in the file in that order, whose faces are
/// `order`: the indexes of the programs.
fn collection_of(programs: &[&[u8]], order: &[usize]) -> Vec<u8> {
    let header = 12 + 4 * order.len();
    let mut placed = Vec::new();
    let mut data = Vec::new();
    for program in programs {
        let at = header + data.len();
        let mut font = program.to_vec();
        let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
        for table in 0..tables {
            let offset = 12 + 16 * table + 8;
            let value = u32::from_be_bytes(font[offset..offset + 4].try_into().unwrap());
            font[offset..offset + 4].copy_from_slice(&(value + at as u32).to_be_bytes());
        }
        placed.push(at as u32);
        data.extend(font);
    }
    let mut bytes = b"ttcf".to_vec();
    bytes.extend(0x0001_0000u32.to_be_bytes());
    bytes.extend((order.len() as u32).to_be_bytes());
    for &face in order {
        bytes.extend(placed[face].to_be_bytes());
    }
    bytes.extend(data);
    bytes
}

#[test]
fn leaves_out_a_program_with_a_name_longer_than_any_font_s() {
    // The font book keeps names however long (#226): a program with a family of 600
    // bytes is left out before it reaches one; one of 500 bytes is not.
    let left_out = |program: &[u8]| {
        let document = embedding(&[("Officewright Test", program)]);
        let (_, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
        report
            .iter()
            .find(|entry| entry.message().contains("longer than"))
            .map(|entry| (entry.kind(), entry.to_string()))
    };
    assert_eq!(
        left_out(&with_family_of(600)),
        Some((
            ReportKind::FontEmbedding,
            "document: the w:embedRegular of the font Officewright Test has a name longer \
             than 512 bytes, which no font has, so it is left out and other fonts are used"
                .to_owned()
        ))
    );
    assert!(left_out(&with_family_of(500)).is_none());
    // A Macintosh family name, of language 0.
    assert!(left_out(&with_names(&[(1, 0, 1, 600)], &[])).is_some());
    // A long name the font book does not keep, such as a license: not left out.
    assert!(left_out(&with_names(&[(3, 0x409, 1, 34), (3, 0x409, 13, 600)], &[])).is_none());
    // A named instance's subfamily name, of any ID, which the font book keeps.
    assert!(
        left_out(&with_names(
            &[(3, 0x409, 1, 34), (3, 0x409, 100, 600)],
            &[100]
        ))
        .is_some()
    );
    assert!(
        left_out(&with_names(
            &[(3, 0x409, 1, 34), (3, 0x409, 100, 500)],
            &[100]
        ))
        .is_none()
    );
    // The second instance's name; and instances where the font book's reader finds them,
    // after axis records of 20 bytes, though the table says 24.
    assert!(
        left_out(&with_names(
            &[(3, 0x409, 1, 34), (3, 0x409, 100, 600)],
            &[5, 100]
        ))
        .is_some()
    );
    assert!(
        left_out(&with_names_and_axis_size(
            &[(3, 0x409, 1, 34), (3, 0x409, 100, 600)],
            &[100],
            24
        ))
        .is_some()
    );
    // And where OpenType puts them for axis records that are 24 bytes, a margin should
    // the reader follow axisSize.
    assert!(
        left_out(&with_axis_records(
            &[(3, 0x409, 1, 34), (3, 0x409, 100, 600)],
            &[100],
            24,
            24
        ))
        .is_some()
    );
    // Each face of a collection: the long name is the second face's; the first, at the
    // end of the file, has a name table whose records run past it.
    let mut cut = test_font();
    let tables = usize::from(u16::from_be_bytes([cut[4], cut[5]]));
    let name = (0..tables)
        .map(|table| 12 + 16 * table)
        .find(|&at| &cut[at..at + 4] == b"name")
        .map(|at| u32::from_be_bytes(cut[at + 8..at + 12].try_into().unwrap()) as usize)
        .unwrap();
    cut[name + 2..name + 4].copy_from_slice(&u16::MAX.to_be_bytes());
    let long = with_family_of(600);
    assert!(left_out(&collection_of(&[&long, &cut], &[1, 0])).is_some());
    assert!(left_out(&collection_of(&[&test_font(), &long], &[0, 1])).is_some());
}

#[test]
fn takes_a_program_found_by_an_alias_or_as_a_metric_compatible_substitute() {
    // Neither is reported as a font the text cannot reach: the font book finds Times New
    // Roman for "Times New Roman CYR", and takes Liberation Sans for Arial. The names are
    // padded with spaces to the test font's length; the font book trims them.
    for (family, name) in [
        ("Times New Roman  ", "Times New Roman CYR"),
        ("Liberation Sans  ", "Arial"),
    ] {
        let document = embedding(&[(name, &named(&test_font(), family))]);
        let (_, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
        let listed = entries(&report);
        assert!(
            !listed.iter().any(|entry| entry.contains("cannot reach")),
            "{name}: {listed:?}"
        );
    }
}

#[test]
fn reports_an_embedded_font_without_a_family_name() {
    // The test font with its family and full names (name IDs 1 and 4) made other names:
    // nothing finds it by "Officewright Test".
    let mut program = test_font();
    let tables = usize::from(u16::from_be_bytes([program[4], program[5]]));
    let name = (0..tables)
        .map(|table| 12 + 16 * table)
        .find(|&at| &program[at..at + 4] == b"name")
        .map(|at| u32::from_be_bytes(program[at + 8..at + 12].try_into().unwrap()) as usize)
        .unwrap();
    let records = usize::from(u16::from_be_bytes([program[name + 2], program[name + 3]]));
    for record in 0..records {
        let id = name + 6 + 12 * record + 6;
        if matches!(u16::from_be_bytes([program[id], program[id + 1]]), 1 | 4) {
            program[id..id + 2].copy_from_slice(&7u16.to_be_bytes());
        }
    }
    let document = Document::from_bytes(package_with_body_and_fonts(
        &text_in_test_font("A"),
        &font("Officewright Test", r#"<w:embedRegular r:id="rId1"/>"#),
        &relationship("rId1", "fonts/font1.odttf"),
        &[("word/fonts/font1.odttf", &program)],
    ))
    .unwrap();
    let (_, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
    let listed = entries(&report);
    assert!(
        listed.iter().any(|entry| entry
            == "document: the w:embedRegular of the font Officewright Test holds a font without \
                a family name, which the text cannot reach by the font's name, so other fonts are \
                used"),
        "{listed:?}"
    );
}

#[test]
fn counts_the_named_instances_of_a_variable_font_in_the_limit() {
    // The font book keeps an entry for each named instance it reads, the first 256 of a
    // face: a face with 255 instances is within the limit of 256, one with 256 is past it.
    let past = |instances: usize| {
        let ids = vec![2u16; instances];
        let program = with_names(&[(3, 0x409, 1, 34), (3, 0x409, 2, 14)], &ids);
        let document = embedding(&[("Officewright Test", &program)]);
        let (_, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
        report
            .iter()
            .any(|entry| entry.message().contains("faces and named instances in all"))
    };
    assert!(!past(255));
    assert!(past(256));
    // The instances of every face of a collection count: two of 200.
    let variable = with_names(&[(3, 0x409, 1, 34), (3, 0x409, 2, 14)], &[2u16; 200]);
    let document = embedding(&[(
        "Officewright Test",
        &collection_of(&[&variable, &variable], &[0, 1]),
    )]);
    let (_, report) = document.save_pdf_bytes(&without_system_fonts()).unwrap();
    assert!(
        report
            .iter()
            .any(|entry| entry.message().contains("faces and named instances in all"))
    );
    assert!(past(1000));
}
