//! Draws JPEG and PNG files on a page, to look at by hand:
//!
//! ```text
//! cargo run -p officewright-pdf --example images -- OUT.pdf IMAGE...
//! ```
//!
//! Each image is drawn 100 pt square over a green square, so that its transparent parts
//! show green, in a grid of four columns: image `i` (from 0) has its lower-left corner at
//! x = 40 + 130 × (i mod 4), y = 680 − 130 × (i div 4). An image that cannot be read is
//! reported and left out, its square stays green. tests/pdf-output/test_images.py checks
//! the page at these positions.

use std::error::Error;

use officewright_pdf::{Color, Document, GraphicsPath, Image, PageSize, Pt};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let out = args.next().ok_or("usage: images OUT.pdf IMAGE...")?;
    let mut doc = Document::new();
    let page = doc.add_page(PageSize::A4)?;
    for (i, path) in args.enumerate() {
        let x = 40.0 + 130.0 * (i % 4) as f64;
        let y = 680.0 - 130.0 * (i / 4) as f64;
        page.fill_path(
            &GraphicsPath::rect(Pt(x), Pt(y), Pt(100.0), Pt(100.0)),
            Color::rgb(0, 128, 0),
        )?;
        match Image::from_file(&path) {
            Ok(image) => page.draw_image(&image, Pt(x), Pt(y), Pt(100.0), Pt(100.0))?,
            Err(err) => eprintln!("error: {err}"),
        }
    }
    doc.save(out)?;
    Ok(())
}
