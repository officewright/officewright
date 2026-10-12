//! The `officewright` command-line tool.
#![forbid(unsafe_code)]

use std::error::Error as _;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command, value_parser};
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
                    "Converts a UTF-8 text file to PDF in Helvetica, with 72 pt margins. Long \
                     lines wrap at spaces and a form feed starts a new page. Only characters \
                     in WinAnsiEncoding (Latin text) are supported.",
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
    let helvetica = Font::standard(StandardFont::Helvetica);
    let doc =
        text_to_pdf(&text, page_size, &helvetica, Pt(font_size)).map_err(|err| match err {
            Error::Unencodable { offset, .. } => {
                let (line, column) = position(&text, offset);
                format!("{}:{line}:{column}: {err}", input.display())
            }
            err => describe(&err),
        })?;
    doc.save(output).map_err(|err| describe(&err))
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
fn describe(err: &Error) -> String {
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
