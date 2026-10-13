//! Shapes a text with a font as the layout does and prints the glyphs.
//!
//! ```text
//! cargo run -p officewright-layout --features fuzzing --example shape_dump -- \
//!     tests/fonts/carlito/Carlito-Regular.ttf "office AVATAR" [--kern] [--liga] [--dlig] [--calt] \
//!     [--script Latn] [--lang ro]
//! ```
//!
//! Each line is one glyph: its id, the byte offset of its characters in the text, its
//! advance and its offsets, in the font's design units, and whether breaking the text
//! there would need it shaped again. The last line sums the advances.
//! `hb-shape --no-glyph-names --show-extents=0 --features=-liga,-kern font text` prints
//! the same numbers when HarfBuzz is installed.

use std::process::ExitCode;

use officewright_font::FontFace;
use officewright_layout::fuzzing::{Settings, shape};
use officewright_layout::ir::Ligatures;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(text)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: shape_dump FONT TEXT [--kern] [--liga] [--dlig] [--calt] [--script CODE] \
             [--lang TAG]"
        );
        return ExitCode::FAILURE;
    };
    let mut settings = Settings::new();
    let mut ligatures = Ligatures::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kern" => settings = settings.with_kerning(true),
            "--liga" => ligatures = ligatures.with_standard(true),
            "--dlig" => ligatures = ligatures.with_discretional(true),
            "--calt" => settings = settings.with_contextual_alternates(true),
            "--script" => {
                if let Some(code) = args.next() {
                    settings = settings.with_script(&code);
                }
            }
            "--lang" => {
                if let Some(tag) = args.next() {
                    settings = settings.with_language(&tag);
                }
            }
            other => {
                eprintln!("shape_dump: unknown option {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let settings = settings.with_ligatures(ligatures);
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
    let shaped = shape(&face, &text, &settings);
    println!(
        "{} units per em{}",
        face.units_per_em(),
        if shaped.is_nominal() {
            ", shaped from the character map alone"
        } else {
            ""
        }
    );
    println!("glyph\tcluster\tadvance\tx offset\ty offset\tunsafe to break");
    let mut total = 0i64;
    for glyph in shaped.glyphs() {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            glyph.id(),
            glyph.cluster(),
            glyph.x_advance(),
            glyph.x_offset(),
            glyph.y_offset(),
            glyph.unsafe_to_break()
        );
        total += i64::from(glyph.x_advance());
    }
    println!("total advance {total}");
    ExitCode::SUCCESS
}
