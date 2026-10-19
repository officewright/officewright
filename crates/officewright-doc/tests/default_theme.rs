//! A document without a theme part takes the fonts Word's default theme gives scripts
//! (#155): those of the theme Word writes into a document it saves, the copy of
//! `word-rules/E-defaults-empty.docx` that Word re-saved. It reads `tests/corpus` at the
//! root of the repository, so it is left out of the published crate.

use std::path::Path;

use officewright_doc::Document;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

#[test]
fn a_document_without_a_theme_takes_the_script_fonts_of_words_default_theme() {
    let resaved = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/resaved/word-rules/E-defaults-empty.docx");
    let resaved = Document::open(resaved).unwrap();
    let fonts = resaved.theme().unwrap().fonts().unwrap();
    for (collection, theme_font) in [
        (fonts.major(), "majorEastAsia"),
        (fonts.minor(), "minorEastAsia"),
    ] {
        assert_eq!(collection.scripts().len(), 47);
        for (script, typeface) in collection.scripts() {
            // A language of that script, by its script subtag.
            let document = without_theme(&format!("und-{script}"), theme_font).unwrap();
            let run = document
                .sections()
                .next()
                .unwrap()
                .blocks()
                .find_map(|block| match block {
                    officewright_doc::Block::Paragraph(paragraph) => paragraph.runs().next(),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                run.format().fonts().east_asia(),
                typeface,
                "{theme_font} {script}"
            );
        }
    }
}

/// A document without a theme part whose one run takes the theme font `theme_font` for
/// East Asian text, with `language` as the theme font language of East Asian text.
fn without_theme(language: &str, theme_font: &str) -> officewright_doc::Result<Document> {
    let document = format!(
        r#"<w:document xmlns:w="{W}"><w:body><w:p><w:r><w:rPr><w:rFonts w:eastAsiaTheme="{theme_font}"/></w:rPr><w:t>x</w:t></w:r></w:p></w:body></w:document>"#
    );
    let settings = format!(
        r#"<w:settings xmlns:w="{W}"><w:themeFontLang w:val="en-US" w:eastAsia="{language}"/></w:settings>"#
    );
    let entries = [
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/></Types>"#.to_owned(),
        ),
        (
            "_rels/.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_owned(),
        ),
        ("word/document.xml", document),
        (
            "word/_rels/document.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/></Relationships>"#.to_owned(),
        ),
        ("word/settings.xml", settings),
    ];
    Document::from_bytes(zip(&entries))
}

/// A ZIP archive with the entries stored.
fn zip(entries: &[(&str, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let (name, data) = (name.as_bytes(), data.as_bytes());
        let offset = out.len() as u32;
        let crc = crc32(data);
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        for header in [&mut out, &mut central] {
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&[0, 0]);
        }
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
    }
    let directory = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&directory.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
