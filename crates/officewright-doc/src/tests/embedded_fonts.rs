//! Fonts embedded in the package: their programs, read and deobfuscated.

use super::*;

const FONT_TABLE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable";
const FONT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/font";

/// The key of `{00112233-4455-6677-8899-AABBCCDDEEFF}`: its bytes in the reverse order of
/// the hexadecimal digits as written (ECMA-376 Part 1, §17.8.1).
const KEY: &str = "{00112233-4455-6677-8899-AABBCCDDEEFF}";
const KEY_BYTES: [u8; 16] = [
    0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00,
];

/// The start of a TrueType font (`00 01 00 00`), then `size - 4` bytes counting up.
fn truetype(size: usize) -> Vec<u8> {
    let mut font = vec![0, 1, 0, 0];
    font.extend((0..size.saturating_sub(4)).map(|n| (n % 251) as u8));
    font
}

/// A font obfuscated with [`KEY_BYTES`]: its first 32 bytes XORed with them twice over.
fn obfuscated(font: &[u8]) -> Vec<u8> {
    let mut bytes = font.to_vec();
    for (n, byte) in bytes.iter_mut().take(32).enumerate() {
        *byte ^= KEY_BYTES[n % 16];
    }
    bytes
}

/// A package whose font table declares `fonts` (the XML of each `w:font`), with the
/// font table's relationships and the files of the font parts.
fn package_with_fonts(fonts: &str, relationships: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let table = format!(
        r#"<w:fonts xmlns:w="{W}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{fonts}</w:fonts>"#
    );
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{relationships}</Relationships>"#
    );
    let mut all: Vec<(&str, &[u8])> = vec![
        ("word/fontTable.xml", table.as_bytes()),
        ("word/_rels/fontTable.xml.rels", rels.as_bytes()),
    ];
    all.extend_from_slice(files);
    files_package(
        "<w:p/>",
        &format!(r#"<Relationship Id="rIdFonts" Type="{FONT_TABLE}" Target="fontTable.xml"/>"#),
        &all,
        r#"<Default Extension="odttf" ContentType="application/vnd.openxmlformats-officedocument.obfuscatedFont"/>"#,
    )
}

fn relationship(id: &str, target: &str) -> String {
    format!(r#"<Relationship Id="{id}" Type="{FONT}" Target="{target}"/>"#)
}

/// A `w:font` named `name` with these embedding elements.
fn font(name: &str, embedded: &str) -> String {
    format!(r#"<w:font w:name="{name}">{embedded}</w:font>"#)
}

/// The program of each embedded font, by the font's name and the style.
fn programs(document: &Document) -> Vec<(String, EmbeddedFontStyle, Option<&[u8]>)> {
    document
        .fonts()
        .iter()
        .flat_map(|font| {
            font.embedded()
                .iter()
                .map(|embedded| (font.name().to_owned(), embedded.style(), embedded.program()))
        })
        .collect()
}

#[test]
fn reads_and_deobfuscates_embedded_fonts() {
    let alpha = truetype(100);
    let beta = [b"OTTO".as_slice(), &[7; 60]].concat();
    let gamma = truetype(80);
    let fonts = [
        // Two styles of one part with one key: read once.
        font(
            "Alpha",
            &format!(
                r#"<w:embedRegular r:id="rId1" w:fontKey="{KEY}"/><w:embedBold r:id="rId1" w:fontKey="{KEY}" w:subsetted="1"/>"#
            ),
        ),
        // No key: the part as it is.
        font("Beta", r#"<w:embedRegular r:id="rId2"/>"#),
        // A key, but the part is not obfuscated.
        font("Gamma", &format!(r#"<w:embedItalic r:id="rId3" w:fontKey="{KEY}"/>"#)),
        // A font outside the package is never loaded, and not reported.
        font("Theta", r#"<w:embedRegular r:id="rId7"/>"#),
    ]
    .concat();
    let relationships = [
        relationship("rId1", "fonts/font1.odttf"),
        relationship("rId2", "fonts/font2.ttf"),
        relationship("rId3", "fonts/font3.odttf"),
        format!(
            r#"<Relationship Id="rId7" Type="{FONT}" Target="https://example.com/font.ttf" TargetMode="External"/>"#
        ),
    ]
    .concat();
    let hidden = obfuscated(&alpha);
    assert_ne!(hidden, alpha);
    let package = package_with_fonts(
        &fonts,
        &relationships,
        &[
            ("word/fonts/font1.odttf", &hidden),
            ("word/fonts/font2.ttf", &beta),
            ("word/fonts/font3.odttf", &gamma),
        ],
    );
    let document = Document::from_bytes(package).unwrap();
    let programs = programs(&document);
    assert_eq!(
        programs,
        [
            (
                "Alpha".to_owned(),
                EmbeddedFontStyle::Regular,
                Some(&*alpha)
            ),
            ("Alpha".to_owned(), EmbeddedFontStyle::Bold, Some(&*alpha)),
            ("Beta".to_owned(), EmbeddedFontStyle::Regular, Some(&*beta)),
            ("Gamma".to_owned(), EmbeddedFontStyle::Italic, Some(&*gamma)),
            ("Theta".to_owned(), EmbeddedFontStyle::Regular, None),
        ]
    );
    // One program for both styles of Alpha.
    assert!(std::ptr::eq(programs[0].2.unwrap(), programs[1].2.unwrap()));
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
    // The bytes stay out of debug output.
    let debug = format!("{:?}", document.fonts()[0].embedded()[0]);
    assert!(debug.contains("Program(100 bytes)"), "{debug}");
}

#[test]
fn reports_embedded_fonts_it_cannot_read() {
    let fonts = [
        // Neither with the key nor without it a font.
        font(
            "Delta",
            &format!(r#"<w:embedRegular r:id="rId4" w:fontKey="{KEY}"/>"#),
        ),
        font(
            "Epsilon",
            r#"<w:embedRegular r:id="rId4" w:fontKey="{0011-22}"/>"#,
        ),
        // Without a key, not a font either.
        font("Kappa", r#"<w:embedRegular r:id="rId4"/>"#),
        font("Zeta", r#"<w:embedRegular r:id="rId9"/>"#),
        font("Eta", r#"<w:embedBoldItalic r:id="rId6"/>"#),
        // A damaged entry of the package.
        font(
            "Iota",
            &format!(r#"<w:embedRegular r:id="rId8" w:fontKey="{KEY}"/>"#),
        ),
    ]
    .concat();
    let relationships = [
        relationship("rId4", "fonts/font4.odttf"),
        relationship("rId6", "fonts/gone.odttf"),
        relationship("rId8", "fonts/font8.odttf"),
    ]
    .concat();
    let damaged = obfuscated(&truetype(64));
    let mut package = package_with_fonts(
        &fonts,
        &relationships,
        &[
            (
                "word/fonts/font4.odttf",
                b"not a font at all, with or without a key",
            ),
            ("word/fonts/font8.odttf", &damaged),
        ],
    );
    let at = package
        .windows(damaged.len())
        .position(|window| window == damaged)
        .unwrap();
    package[at + 40] ^= 0xFF;
    let document = Document::from_bytes(package).unwrap();
    assert!(
        programs(&document)
            .iter()
            .all(|(_, _, program)| program.is_none())
    );
    let messages: Vec<String> = document
        .diagnostics()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(messages.len(), 6, "{messages:?}");
    let place = "/word/fontTable.xml: ";
    assert_eq!(
        messages[..5],
        [
            format!(
                "{place}the w:embedRegular of the font Delta is not a TrueType or OpenType font"
            ),
            format!(
                "{place}the w:embedRegular of the font Epsilon has a w:fontKey that is not a GUID"
            ),
            format!(
                "{place}the w:embedRegular of the font Kappa is not a TrueType or OpenType font"
            ),
            format!(
                "{place}the w:embedRegular of the font Zeta names the relationship rId9, which the font table does not have"
            ),
            format!(
                "{place}the w:embedBoldItalic of the font Eta is in /word/fonts/gone.odttf, which the package does not have"
            ),
        ]
    );
    assert!(
        messages[5].starts_with(&format!(
            "{place}the w:embedRegular of the font Iota cannot be read: "
        )),
        "{messages:?}"
    );
}

#[test]
fn stops_at_a_limit_of_the_package_while_reading_embedded_fonts() {
    // Below the least total size of the parts read that opens the document, the limit
    // stops the reading rather than being reported as a font that cannot be read.
    let fonts = font(
        "Alpha",
        &format!(r#"<w:embedRegular r:id="rId1" w:fontKey="{KEY}"/>"#),
    );
    let program = truetype(20_000);
    let hidden = obfuscated(&program);
    let package = package_with_fonts(
        &fonts,
        &relationship("rId1", "fonts/font1.odttf"),
        &[("word/fonts/font1.odttf", &hidden)],
    );
    let opened = |total| {
        LoadOptions::new()
            .with_limits(Limits::new().with_max_total_size(total))
            .open_bytes(package.clone())
    };
    let (mut low, mut high) = (0, 1 << 20);
    while low + 1 < high {
        let middle = (low + high) / 2;
        if opened(middle).is_ok() {
            high = middle;
        } else {
            low = middle;
        }
    }
    let err = opened(high - 1).unwrap_err();
    assert!(matches!(err, Error::LimitExceeded { .. }), "{err}");
    let document = opened(high).unwrap();
    assert_eq!(document.fonts()[0].embedded()[0].program(), Some(&*program));
}
