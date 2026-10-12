//! Prints what officewright-font reads from fonts, for checking it by hand.
//!
//! ```text
//! cargo run -p officewright-font --example font_report -- info FONT_FILE...
//! cargo run -p officewright-font --example font_report -- subset FONT_FILE TEXT OUT_FILE
//! cargo run -p officewright-font --example font_report -- resolve [--dir DIR]... FAMILY...
//! ```
//!
//! `info` prints the names, style, metrics and embedding permissions of every face.
//! `subset` writes the subset a PDF would embed for TEXT. `resolve` indexes the
//! system fonts and the fonts in each DIR, and prints which face each family resolves
//! to, regular and bold, and the substitution report.

use std::error::Error;
use std::process::ExitCode;

use officewright_font::{FontBook, FontFace, FontRequest, FontResolver};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((command, rest)) if command == "info" && !rest.is_empty() => info(rest),
        Some((command, [font, text, out])) if command == "subset" => subset(font, text, out),
        Some((command, rest)) if command == "resolve" && !rest.is_empty() => resolve(rest),
        _ => {
            eprintln!(
                "usage: font_report info FONT_FILE...\n       \
                 font_report subset FONT_FILE TEXT OUT_FILE\n       \
                 font_report resolve [--dir DIR]... FAMILY..."
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let mut message = err.to_string();
            let mut source = err.source();
            while let Some(cause) = source {
                message.push_str(&format!(": {cause}"));
                source = cause.source();
            }
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn info(paths: &[String]) -> Result<(), Box<dyn Error>> {
    for path in paths {
        let data = std::fs::read(path)?;
        for index in 0..FontFace::face_count(&data)? {
            let font = FontFace::from_bytes(data.clone(), index)?;
            let names = font.names();
            let metrics = font.metrics();
            let hhea = metrics.hhea();
            println!("{path}, face {index}");
            println!("  family:          {}", names.family().unwrap_or("-"));
            println!(
                "  all families:    {}",
                names.families().collect::<Vec<_>>().join(" | ")
            );
            println!("  subfamily:       {}", names.subfamily().unwrap_or("-"));
            println!(
                "  typographic:     {} / {}",
                names.typographic_family().unwrap_or("-"),
                names.typographic_subfamily().unwrap_or("-")
            );
            println!("  full name:       {}", names.full_name().unwrap_or("-"));
            println!(
                "  PostScript name: {}",
                names.postscript_name().unwrap_or("-")
            );
            println!("  weight, italic:  {}, {}", font.weight(), font.is_italic());
            println!(
                "  outlines:        {:?}{}",
                font.outline_format(),
                if font.is_variable() {
                    " (variable)"
                } else {
                    ""
                }
            );
            println!("  glyphs:          {}", font.glyph_count());
            println!("  units per em:    {}", font.units_per_em());
            println!(
                "  hhea:            ascender {}, descender {}, line gap {}",
                hhea.ascender(),
                hhea.descender(),
                hhea.line_gap()
            );
            if let Some(typo) = metrics.typo() {
                println!(
                    "  OS/2 typo:       ascender {}, descender {}, line gap {}{}",
                    typo.ascender(),
                    typo.descender(),
                    typo.line_gap(),
                    if metrics.use_typo_metrics() {
                        " (USE_TYPO_METRICS)"
                    } else {
                        ""
                    }
                );
            }
            if let Some(win) = metrics.win() {
                println!(
                    "  OS/2 win:        ascent {}, descent {}",
                    win.ascent(),
                    win.descent()
                );
            }
            let embedding = font.embedding();
            println!(
                "  embedding:       {:?}{}{}",
                embedding.license(),
                if embedding.allows_subsetting() {
                    ""
                } else {
                    ", no subsetting"
                },
                if embedding.bitmap_only() {
                    ", bitmap only"
                } else {
                    ""
                }
            );
            let map = font.char_map();
            let sample: Vec<String> = "AЖ€"
                .chars()
                .map(|ch| match map.map(ch) {
                    Some(glyph) => format!(
                        "{ch} = glyph {glyph}, advance {}",
                        font.advance_width(glyph).unwrap_or(0)
                    ),
                    None => format!("{ch} = not in font"),
                })
                .collect();
            println!("  characters:      {}", sample.join("; "));
        }
    }
    Ok(())
}

fn subset(path: &str, text: &str, out: &str) -> Result<(), Box<dyn Error>> {
    let font = FontFace::from_bytes(std::fs::read(path)?, 0)?;
    let mut missing = String::new();
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        match font.glyph_id(ch) {
            Some(glyph) => glyphs.push(glyph),
            None => missing.push(ch),
        }
    }
    let subset = font.subset(glyphs.iter().copied())?;
    std::fs::write(out, subset.data())?;
    println!(
        "{out}: {} bytes, {} glyphs (the font has {} bytes, {} glyphs)",
        subset.data().len(),
        subset.glyph_count(),
        font.data().len(),
        font.glyph_count()
    );
    for (ch, glyph) in text
        .chars()
        .filter_map(|ch| font.glyph_id(ch).map(|g| (ch, g)))
    {
        if let Some(new) = subset.glyph_id(glyph) {
            println!("  {ch:?}: glyph {glyph} -> {new}");
        }
    }
    if !missing.is_empty() {
        println!("  not in the font: {missing:?}");
    }
    Ok(())
}

fn resolve(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut book = FontBook::new();
    let mut families = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--dir" {
            let dir = args.next().ok_or("--dir needs a directory")?;
            println!("{dir}: {} font faces", book.add_dir(dir)?);
        } else {
            families.push(arg);
        }
    }
    println!("system: {} font faces", book.add_system_fonts());
    for skipped in book.skipped() {
        println!("skipped {skipped}");
    }
    let mut resolver = FontResolver::new(&book);
    for family in families {
        for bold in [false, true] {
            let resolution = resolver
                .resolve(&FontRequest::new(family.as_str()).bold(bold))
                .clone();
            let file = resolution
                .face()
                .and_then(|id| book.face(id))
                .and_then(|face| face.path())
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            println!("{resolution}  [{:?}] {file}", resolution.kind());
        }
    }
    println!("substitution report:");
    for substitution in resolver.substitutions() {
        println!("  {substitution}");
    }
    Ok(())
}
