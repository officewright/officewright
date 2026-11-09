//! Writes a three-page PDF with links and named destinations, to click through by hand:
//!
//! ```text
//! cargo run -p officewright-pdf --example links -- OUT.pdf
//! ```
//!
//! - Page 1 has a link to `https://example.com/a b/(Ж)` over the text "Example" at
//!   (72, 700), 120 by 14.5 pt, a link to the destination "Appendix" over the text
//!   "Appendix" at (72, 680), 80 by 14 pt, and a link to "Résumé" over the text "Résumé" at
//!   (72, 660), 80 by 14 pt; and the destinations "Start" at (72, 800), "Résumé" at
//!   (72, 640), "Zeta" at (72, 620) and "_Toc123" at (72, 600): a name outside ASCII
//!   among ASCII names, which qpdf --check finds sorted only when the keys sort the same
//!   as bytes and as text.
//! - Page 2 has the destinations "d000" to "d149", "dNNN" at (36, 800 − 5 × NNN), more
//!   than one node of the name tree holds.
//! - Page 3 has the destinations "Appendix" at (72, 770.25) and "Глава" at (0, 400), a
//!   link to "Глава" at (300, 400), 100 by 20 pt, and a link back to "Start" at (300,
//!   300), 100 by 20 pt.
//!
//! tests/pdf-output/test_links.py checks the file at these positions.

use std::error::Error;

use officewright_pdf::{Document, Font, LinkTarget, PageSize, Pt, StandardFont};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [out] = args.as_slice() else {
        return Err("usage: links OUT.pdf".into());
    };
    let helvetica = Font::standard(StandardFont::Helvetica);
    let destination = |name: &str| LinkTarget::Destination(name.to_owned());
    let mut doc = Document::new();

    let first = doc.add_page(PageSize::A4)?;
    first.add_destination("Start", Pt(72.0), Pt(800.0))?;
    first.add_text(Pt(72.0), Pt(703.0), &helvetica, Pt(12.0), "Example")?;
    let site = LinkTarget::Uri("https://example.com/a b/(Ж)".to_owned());
    first.add_link(Pt(72.0), Pt(700.0), Pt(120.0), Pt(14.5), &site)?;
    first.add_text(Pt(72.0), Pt(683.0), &helvetica, Pt(12.0), "Appendix")?;
    let appendix = destination("Appendix");
    first.add_link(Pt(72.0), Pt(680.0), Pt(80.0), Pt(14.0), &appendix)?;
    first.add_text(Pt(72.0), Pt(663.0), &helvetica, Pt(12.0), "Résumé")?;
    let resume = destination("Résumé");
    first.add_link(Pt(72.0), Pt(660.0), Pt(80.0), Pt(14.0), &resume)?;
    for (name, y) in [("Résumé", 640.0), ("Zeta", 620.0), ("_Toc123", 600.0)] {
        first.add_destination(name, Pt(72.0), Pt(y))?;
    }

    let second = doc.add_page(PageSize::A4)?;
    for i in 0..150_u32 {
        let name = format!("d{i:03}");
        second.add_destination(&name, Pt(36.0), Pt(800.0 - 5.0 * f64::from(i)))?;
    }

    let third = doc.add_page(PageSize::A4)?;
    third.add_destination("Appendix", Pt(72.0), Pt(770.25))?;
    third.add_destination("Глава", Pt(0.0), Pt(400.0))?;
    third.add_text(Pt(300.0), Pt(405.0), &helvetica, Pt(12.0), "To the chapter")?;
    let chapter = destination("Глава");
    third.add_link(Pt(300.0), Pt(400.0), Pt(100.0), Pt(20.0), &chapter)?;
    third.add_text(Pt(300.0), Pt(305.0), &helvetica, Pt(12.0), "To the start")?;
    let start = destination("Start");
    third.add_link(Pt(300.0), Pt(300.0), Pt(100.0), Pt(20.0), &start)?;

    doc.save(out)?;
    Ok(())
}
