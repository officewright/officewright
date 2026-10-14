//! Embeds every face of the input in a PDF, as `text-to-pdf --font` would: the face is
//! checked and subset, text in it is measured and drawn, and the document is written.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_font::FontFace;
use officewright_pdf::{Document, Font, PageSize, Pt};

/// Characters of the scripts an embedded font shows, and a ligature.
const SAMPLE: &str = "Hello, Мир! Καλημέρα Բարեւ 0123 ÁÉ € fi 漢字かな";

// libfuzzer-sys installs a panic hook that aborts, so a panic in the subsetter is a
// crash here even though `Font::from_bytes` catches it in other builds.
fuzz_target!(|data: &[u8]| {
    let Ok(count) = FontFace::face_count(data) else {
        return;
    };
    for index in 0..count.min(4) {
        let (Ok(face), Ok(font)) = (
            FontFace::from_bytes(data, index),
            Font::from_bytes(data, index),
        ) else {
            continue;
        };
        let text: String = SAMPLE
            .chars()
            .filter(|&ch| face.glyph_id(ch).is_some())
            .collect();
        let width = font.text_width(&text, Pt(12.0));
        let mut doc = Document::new();
        let Ok(page) = doc.add_page(PageSize::A4) else {
            return;
        };
        let drawn = page.add_text(Pt(72.0), Pt(700.0), &font, Pt(12.0), &text);
        // Text the font can measure it can draw, and the other way round.
        assert_eq!(width.is_ok(), drawn.is_ok(), "{width:?} {drawn:?}");
        if drawn.is_ok() {
            // Subsetting the glyphs the text uses may still find the face damaged.
            let _ = doc.to_bytes();
        }
    }
});
