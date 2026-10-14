# Changelog

Notable changes to Officewright. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): while the version is 0.x,
any release may change the Rust API. The C ABI has its own version (`OW_ABI_VERSION`),
raised on every breaking change.

## [Unreleased]

## [0.1.0] - 2026-10-13

The first pre-release, for trying the C library and the command-line tool. It writes PDF
files with text; it does not convert office documents yet. The Rust crates are not
published on crates.io.

### Added

#### PDF output

- Pages of any size from 3 to 14400 pt, lines of text at given positions, and
  `text_to_pdf`, which lays out plain text on pages with margins, wrapping, tabs and
  form-feed page breaks.
- Helvetica, which every PDF viewer has, for Latin text in WinAnsiEncoding.
- TrueType and OpenType (CFF) fonts, embedded and, unless the font's license forbids
  subsetting, reduced to the glyphs the document uses, with a ToUnicode map so that text
  can be copied and searched (#12). They show Latin, Greek, Cyrillic, Armenian, Georgian,
  Han, Hiragana, Katakana and Hangul syllables, one glyph per character. Text a font
  cannot show, or that needs shaping or right-to-left layout, is refused with the
  character and its position, never altered.
- The font's license (`fsType`) is honoured: a font that forbids embedding, or allows only
  its bitmaps to be embedded, is refused; other restrictions are reported (#12).
- Fonts can be chosen by family name, the way Word names them (#17).
- Page content and embedded fonts are compressed with the Flate filter; compression can be
  turned off (#29).

#### Fonts

- Reading TrueType and OpenType fonts and font collections: metrics, character map, names,
  embedding permissions; subsetting.
- A font book of document, user and system fonts, and a resolver that finds the font for a
  family name. A missing family is replaced by its metric-compatible substitute, measured
  against Word's own rendering: Carlito for Calibri, Liberation Sans, Serif and Mono for
  Arial, Times New Roman and Courier New (#24). Failing that, by a font of similar design
  (Caladea for Cambria, #17, #25) or a common font of the same kind. Every substitution is
  reported (#17).

#### C library

- C ABI version 1, prebuilt for Linux (x86_64 and aarch64 glibc, x86_64 musl), Windows
  (x86_64, aarch64) and macOS (x86_64, aarch64), with `officewright.h`, shared and static
  libraries and a relocatable `officewright.pc`.
- PDF documents: create, lay out text, add pages and text, choose compression, save to a
  file or to memory.
- Fonts: the standard Helvetica, fonts opened from a file or from memory with the
  restrictions of their license, text width, and fonts chosen by family name through a
  font book and a resolver with its substitution report.
- Statuses say what went wrong: a NULL or invalid argument, invalid UTF-8, an I/O error,
  something not supported yet, damaged input (`OW_STATUS_MALFORMED`), an exceeded limit
  (`OW_STATUS_LIMIT_EXCEEDED`, #47), or a caught panic; `ow_last_error_message()` says more.

#### Command line

- `officewright text-to-pdf`: a UTF-8 text file to PDF, with the page size, the font size,
  and a font file or family name (`--font`, `--font-dir`, `--no-system-fonts`).

#### Reading office documents, not yet connected to PDF output

These are in the Rust crates only: neither the C library nor the command-line tool uses
them yet.

- ZIP archives, XML and OPC packages with Markup Compatibility (#2, #3, #4, #27).
- DOCX: the body with sections, paragraphs, runs and tables; content controls,
  hyperlinks, tracked changes and fields; styles; the properties of runs, paragraphs,
  tables, rows, cells and sections; the settings that bear on layout (#31, #32, #38, #39,
  #42, #43, #44).

[Unreleased]: https://github.com/officewright/officewright/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/officewright/officewright/releases/tag/v0.1.0
