//! Writes a one-page PDF of text in one family resolved in each style from the fonts of a
//! folder, to look at by hand:
//!
//! ```text
//! cargo run -p officewright-pdf --example synthetic -- FONT_DIR FAMILY OUT.pdf
//! ```
//!
//! Line `i` (from 0) is regular, italic, bold and bold italic in that order: "HHHHHHHH"
//! at 24 pt, its baseline at y = 700 − 60 × i, from x = 72. A style the family's faces
//! lack is drawn on top of the face it takes: italic slanted, bold filled and stroked.
//! The resolver's report is printed. tests/pdf-output/test_synthetic.py checks the page
//! at these positions.

use std::error::Error;

use officewright_font::{FontBook, FontRequest, FontResolver};
use officewright_pdf::{Document, Font, PageSize, Pt};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, family, out] = args.as_slice() else {
        return Err("usage: synthetic FONT_DIR FAMILY OUT.pdf".into());
    };
    let mut book = FontBook::new();
    book.add_dir(dir)?;
    let mut resolver = FontResolver::new(&book);
    let mut doc = Document::new();
    let page = doc.add_page(PageSize::A4)?;
    let styles = [(false, false), (false, true), (true, false), (true, true)];
    for (line, (bold, italic)) in (0u32..).zip(styles) {
        let request = FontRequest::new(family).bold(bold).italic(italic);
        let font = Font::resolve(&mut resolver, &request)?;
        let y = 700.0 - 60.0 * f64::from(line);
        page.add_text(Pt(72.0), Pt(y), &font, Pt(24.0), "HHHHHHHH")?;
    }
    for substitution in resolver.substitutions() {
        println!("{substitution}");
    }
    doc.save(out)?;
    Ok(())
}
