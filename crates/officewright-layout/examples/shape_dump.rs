//! Shapes a text with a font as the layout does and prints the glyphs.
//!
//! ```text
//! cargo run -p officewright-layout --features fuzzing --example shape_dump -- \
//!     tests/fonts/carlito/Carlito-Regular.ttf "office AVATAR" [--kern] [--liga] [--lang tr]
//! ```
//!
//! Each line is one glyph: its id, the byte offset of its characters in the text, its
//! advance and its offsets, in the font's design units. The last line sums the advances.
//! `hb-shape --no-glyph-names --show-extents=0 --features=-liga,-kern font text` prints
//! the same numbers when HarfBuzz is installed.

use std::process::ExitCode;

use officewright_font::FontFace;
use officewright_layout::fuzzing::shape;
use officewright_layout::ir::Ligatures;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(text)) = (args.next(), args.next()) else {
        eprintln!("usage: shape_dump FONT TEXT [--kern] [--liga] [--lang TAG]");
        return ExitCode::FAILURE;
    };
    let mut kerning = false;
    let mut ligatures = Ligatures::default();
    let mut language = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kern" => kerning = true,
            "--liga" => ligatures = ligatures.with_standard(true),
            "--lang" => language = args.next(),
            other => {
                eprintln!("shape_dump: unknown option {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let face = match std::fs::read(&path)
        .map_err(|err| err.to_string())
        .and_then(|data| FontFace::from_bytes(data, 0).map_err(|err| err.to_string()))
    {
        Ok(face) => face,
        Err(err) => {
            eprintln!("shape_dump: {path}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let shaped = shape(&face, &text, kerning, ligatures, language.as_deref());
    println!(
        "{} units per em{}",
        face.units_per_em(),
        if shaped.is_nominal() {
            ", shaped from the character map alone"
        } else {
            ""
        }
    );
    println!("glyph\tcluster\tadvance\tx offset\ty offset");
    let mut total = 0i64;
    for glyph in shaped.glyphs() {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            glyph.id(),
            glyph.cluster(),
            glyph.x_advance(),
            glyph.x_offset(),
            glyph.y_offset()
        );
        total += i64::from(glyph.x_advance());
    }
    println!("total advance {total}");
    ExitCode::SUCCESS
}
