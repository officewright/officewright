//! Writes a one-page PDF to look at by hand:
//! `cargo run -p officewright-pdf --example hello -- hello.pdf`
//!
//! It shows text at several sizes, the Latin characters outside ASCII that Helvetica
//! covers, characters PDF strings must escape, and a line right-aligned by measuring it.

use std::error::Error;

use officewright_pdf::{Document, Font, PageSize, Pt, StandardFont};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: hello <output.pdf>")?;
    let font = Font::standard(StandardFont::Helvetica);
    let size = PageSize::A4;
    let mut doc = Document::new();
    let page = doc.add_page(size)?;

    page.add_text(Pt(72.0), Pt(760.0), &font, Pt(24.0), "Hello, world")?;
    let lines = [
        "Grüße aus Köln, ¿qué tal?, Œuvres complètes, Ærøskøbing",
        "Quotes “double” and ‘single’, dashes – and —, 5 €, 3 £, 99 ¢, 50 ‰",
        "Escaped in PDF strings: (parentheses) and a backslash \\",
    ];
    for (i, line) in lines.iter().enumerate() {
        page.add_text(Pt(72.0), Pt(720.0 - 18.0 * i as f64), &font, Pt(12.0), line)?;
    }

    let right = "This line ends at the right margin";
    let width = font.text_width(right, Pt(12.0))?;
    page.add_text(
        Pt(size.width().0 - 72.0 - width.0),
        Pt(640.0),
        &font,
        Pt(12.0),
        right,
    )?;

    doc.save(path)?;
    Ok(())
}
