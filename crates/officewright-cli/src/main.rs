//! The `officewright` command-line tool.
#![forbid(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use officewright_font::{FontBook, FontRequest, FontResolver};
use officewright_pdf::{Error, Font, PageSize, Pt, StandardFont, text_to_pdf};

fn cli() -> Command {
    Command::new("officewright")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Converts office documents without Microsoft Office or LibreOffice")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("text-to-pdf")
                .about("Converts a UTF-8 text file to PDF")
                .long_about(
                    "Converts a UTF-8 text file to PDF, with 72 pt margins. Long lines wrap at \
                     spaces and a form feed starts a new page.\n\n\
                     Without --font the text is set in Helvetica, which every PDF viewer has, \
                     and only characters in WinAnsiEncoding (Latin text) are supported. With \
                     --font the font is embedded, reduced to the glyphs the text uses \
                     unless its license forbids subsetting. It shows the characters it has of the Latin, Greek, Cyrillic, Armenian, \
                     Georgian, Han, Hiragana and Katakana scripts and Hangul syllables, one \
                     glyph per character; scripts that need shaping or right-to-left layout \
                     are not supported yet.\n\n\
                     --font takes a font file, or a family name looked up among the system \
                     fonts and the fonts in each --font-dir, as Word looks up fonts. When the \
                     family is not there, its metric-compatible substitute (Carlito for \
                     Calibri, Liberation Sans for Arial, ...) is used, else a font of similar \
                     design (Caladea for Cambria, which lays out differently) or a common \
                     font of the same kind. Substitutions and what the font's license \
                     restricts are printed as warnings.",
                )
                .arg(
                    Arg::new("input")
                        .value_name("INPUT")
                        .required(true)
                        .value_parser(value_parser!(PathBuf))
                        .help("Text file to convert"),
                )
                .arg(
                    Arg::new("output")
                        .value_name("OUTPUT")
                        .required(true)
                        .value_parser(value_parser!(PathBuf))
                        .help("PDF file to write; an existing file is replaced"),
                )
                .arg(
                    Arg::new("page-size")
                        .long("page-size")
                        .value_name("SIZE")
                        .value_parser(["a4", "letter"])
                        .default_value("a4")
                        .help("Page size"),
                )
                .arg(
                    Arg::new("font")
                        .long("font")
                        .value_name("NAME|FILE")
                        .value_parser(value_parser!(OsString))
                        .help(
                            "Font to embed: a family name, or a TrueType or OpenType font \
                             file (the first font of a collection); Helvetica if not given",
                        ),
                )
                .arg(
                    Arg::new("font-dir")
                        .long("font-dir")
                        .value_name("DIR")
                        .value_parser(value_parser!(PathBuf))
                        .action(ArgAction::Append)
                        .requires("font")
                        .help("Folder of fonts to look up a --font family name in; repeatable"),
                )
                .arg(
                    Arg::new("no-system-fonts")
                        .long("no-system-fonts")
                        .action(ArgAction::SetTrue)
                        .requires("font")
                        .help("Look up a --font family name only in the --font-dir folders"),
                )
                .arg(
                    Arg::new("font-size")
                        .long("font-size")
                        .value_name("PT")
                        .value_parser(font_size)
                        .allow_negative_numbers(true)
                        .default_value("11")
                        .help("Font size in points"),
                ),
        )
}

/// Parses `--font-size`: a number of points within the range the PDF writer accepts.
/// Anything else is a usage error, like any other invalid argument.
fn font_size(value: &str) -> Result<f64, String> {
    let (min, max) = (Font::MIN_SIZE.0, Font::MAX_SIZE.0);
    match value.parse::<f64>() {
        Ok(size) if (min..=max).contains(&size) => Ok(size),
        _ => Err(format!("expected a number of points from {min} to {max}")),
    }
}

fn main() -> ExitCode {
    let result = match cli().get_matches().subcommand() {
        Some(("text-to-pdf", args)) => run_text_to_pdf(args),
        _ => Err("unknown command".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_text_to_pdf(args: &ArgMatches) -> Result<(), String> {
    // clap guarantees these: they are required or have defaults.
    let (Some(input), Some(output), Some(page_size), Some(&font_size)) = (
        args.get_one::<PathBuf>("input"),
        args.get_one::<PathBuf>("output"),
        args.get_one::<String>("page-size"),
        args.get_one::<f64>("font-size"),
    ) else {
        return Err("missing arguments".to_owned());
    };
    let page_size = match page_size.as_str() {
        "letter" => PageSize::LETTER,
        _ => PageSize::A4,
    };

    let bytes = fs::read(input).map_err(|err| format!("cannot read {}: {err}", input.display()))?;
    let text = String::from_utf8(bytes).map_err(|err| {
        format!(
            "{} is not valid UTF-8 at byte {}",
            input.display(),
            err.utf8_error().valid_up_to()
        )
    })?;
    let font = match args.get_one::<OsString>("font") {
        Some(font) => font_arg(font, args)?,
        None => Font::standard(StandardFont::Helvetica),
    };
    for note in font.embedding_notes() {
        eprintln!("warning: font {}: {note}", font.name());
    }
    let doc = text_to_pdf(&text, page_size, &font, Pt(font_size)).map_err(|err| match err {
        Error::Unencodable { offset, .. }
        | Error::UnsupportedCharacter { offset, .. }
        | Error::MissingGlyph { offset, .. } => {
            let (line, column) = position(&text, offset);
            format!("{}:{line}:{column}: {err}", input.display())
        }
        err => describe(&err),
    })?;
    doc.save(output).map_err(|err| describe(&err))
}

/// The font `--font` names: a file if `value` names one, or looks like a path or a font
/// file name; otherwise a family name, resolved among the system fonts and the
/// `--font-dir` folders, with every substitution printed as a warning.
fn font_arg(value: &OsStr, args: &ArgMatches) -> Result<Font, String> {
    let dirs: Vec<&PathBuf> = args
        .get_many::<PathBuf>("font-dir")
        .into_iter()
        .flatten()
        .collect();
    let system = !args.get_flag("no-system-fonts");
    let path = Path::new(value);
    let is_file_name = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        });
    // A file path need not be UTF-8; a family name must be.
    let looks_like_path = value.to_string_lossy().contains(['/', '\\']);
    if path.is_file() || is_file_name || looks_like_path {
        if !dirs.is_empty() || !system {
            return Err(
                "--font-dir and --no-system-fonts apply to a font family name, not a font file"
                    .to_owned(),
            );
        }
        return Font::from_file(path, 0).map_err(|err| describe(&err));
    }
    let Some(family) = value.to_str() else {
        return Err(format!(
            "font family name {} is not valid UTF-8",
            value.to_string_lossy()
        ));
    };
    let mut book = FontBook::new();
    if system {
        book.add_system_fonts();
    }
    for dir in dirs {
        book.add_dir(dir).map_err(|err| describe(&err))?;
    }
    let mut resolver = FontResolver::new(&book);
    let font =
        Font::resolve(&mut resolver, &FontRequest::new(family)).map_err(|err| match err {
            Error::FontNotFound { .. } => {
                let place = if system {
                    "the system font folders and --font-dir"
                } else {
                    "--font-dir"
                };
                match book.len() {
                    0 => format!("{err}: no fonts were found in {place}"),
                    faces => {
                        format!("{err}: none of the {faces} fonts in {place} can stand in for it")
                    }
                }
            }
            err => describe(&err),
        })?;
    for substitution in resolver.substitutions() {
        eprintln!("warning: font {substitution}");
    }
    Ok(font)
}

/// The 1-based line and column (in characters) of a byte offset, as an editor shows
/// it: lines end at `\n`, `\r\n` or a lone `\r`, as in `text_to_pdf`, and a leading
/// byte order mark takes no column.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let before = before.strip_prefix('\u{FEFF}').unwrap_or(before);
    let (mut line, mut column) = (1, 1);
    let mut chars = before.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // Counted at the `\n` that follows.
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                line += 1;
                column = 1;
            }
            _ => column += 1,
        }
    }
    (line, column)
}

/// An error message followed by its causes, such as the operating system's I/O error.
fn describe(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The position of the first 'Ж' in `text`.
    fn position_of_zhe(text: &str) -> (usize, usize) {
        position(text, text.find('Ж').unwrap())
    }

    #[test]
    fn counts_lines_like_an_editor() {
        assert_eq!(position_of_zhe("abЖ"), (1, 3));
        assert_eq!(position_of_zhe("ok\nab Ж"), (2, 4));
        assert_eq!(position_of_zhe("ok\r\nab Ж"), (2, 4));
        assert_eq!(position_of_zhe("ok\rab Ж"), (2, 4));
        assert_eq!(position_of_zhe("a\r\rЖ"), (3, 1));
        assert_eq!(position_of_zhe("a\n\r\nЖ"), (3, 1));
    }

    #[test]
    fn counts_columns_in_characters_without_the_byte_order_mark() {
        assert_eq!(position_of_zhe("\u{FEFF}abЖ"), (1, 3));
        assert_eq!(position_of_zhe("\u{FEFF}a\nüüЖ"), (2, 3));
        assert_eq!(position_of_zhe("€\tЖ"), (1, 3));
    }

    #[test]
    fn accepts_only_positive_finite_font_sizes() {
        assert_eq!(font_size("9.5"), Ok(9.5));
        assert_eq!(font_size("0.01"), Ok(0.01));
        assert_eq!(font_size("32767"), Ok(32767.0));
        for bad in [
            "abc", "0", "0.009", "-1", "32768", "nan", "inf", "1e400", "",
        ] {
            assert!(font_size(bad).is_err(), "{bad}");
        }
    }
}
