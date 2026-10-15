# Officewright

Read, write and convert office documents — DOCX/DOC, XLSX/XLS, PPTX/PPT and PDF —
without Microsoft Office or LibreOffice.

The core is written in Rust. Every other language uses it through a stable C ABI,
shipped as prebuilt libraries with a C header, so any language with an FFI can call
it without a Rust toolchain.

> **Status: pre-release 0.1.0.** It writes PDF files with text from Rust, C and the
> command line; see [what 0.1.0 does](#what-010-does). The main branch also converts
> DOCX to PDF, in an early version that is not in a release yet; see
> [DOCX to PDF](#docx-to-pdf-early-version).

Officewright is a source-available alternative to Spire.Office and Aspose. It is not
affiliated with, endorsed by or connected to the makers of either product; the
names are trademarks of their respective owners and are used here only to describe
what this project is an alternative to.

## What 0.1.0 does

0.1.0 is a pre-release for trying the C library and the command-line tool. It is not ready
for production use.

It can:

- write PDF files with text: Helvetica for Latin text, or a TrueType or OpenType font for
  Latin, Greek, Cyrillic, Armenian, Georgian, Han, Hiragana, Katakana and Hangul text,
  embedded and, unless its license forbids subsetting, reduced to the glyphs the text uses;
- choose a font by family name, with substitutes measured against Word's own rendering and
  a report of every substitution;
- do this from Rust, from any language through the C library, prebuilt for Linux, Windows
  and macOS on the [releases page](https://github.com/officewright/officewright/releases),
  and with `officewright text-to-pdf`.

It cannot yet:

- convert DOCX, DOC, XLSX or PPTX to PDF. The main branch converts DOCX in an early
  version, [described below](#docx-to-pdf-early-version);
- shape text (Arabic, Hebrew, the Indic scripts), or draw images, tables and vector
  graphics;
- read PDF files;
- be installed from crates.io or a package manager.

[CHANGELOG.md](CHANGELOG.md) lists everything in the release.

## DOCX to PDF (early version)

The main branch converts Word documents (DOCX, and DOCM, DOTX and DOTM) to PDF. This is
an early version that is not in a release yet: build it from source, and expect a
document to look different from Word wherever it uses something from the list below.

It lays out text as Word does: with the fonts the document embeds, then those you give
and the system's, and metric-compatible substitutes for missing ones; shaped with
HarfRust, or without kerning and ligatures from a font whose layout tables it cannot
use; with Word's line breaking, spacing, and keep and widow rules; on the pages of
each section, with their sizes and margins, headers, footers and page numbers. It draws
inline PNG and JPEG pictures, underlines, strikethrough, highlight and shading. What it
knows it shows otherwise than Word would, such as a substituted font or something not
laid out yet, it reports: the command line prints it as notes, and the C library and
Rust return it as a report.

Not laid out yet:

- tables: the paragraphs of their cells follow one another at the full width;
- the bullets and numbers of lists;
- footnotes and endnotes;
- floating (anchored) pictures and shapes, text boxes, charts, SmartArt, VML drawings,
  embedded objects and equations;
- text in several columns (laid out in one column), paragraph and page borders, and line
  numbers;
- right-to-left text (Arabic, Hebrew) in its reading order, and vertical text;
- links, which are plain text, and automatic hyphenation;
- cropped, rotated or mirrored pictures, which are drawn whole and upright, and pictures
  in formats other than PNG and JPEG, which are drawn as grey boxes;
- fields other than page numbers and page counts, which show the result stored in the
  document.

Comments are left out and tracked changes are shown as accepted. Lists, footnotes,
floating objects, right-to-left and vertical text, links, hyphenation, page borders and
line numbers are not in the report yet
([#303](https://github.com/officewright/officewright/issues/303)). DOC and RTF files
cannot be opened yet.

From the command line:

```sh
cargo build --release -p officewright-cli
target/release/officewright docx-to-pdf letter.docx letter.pdf
target/release/officewright docx-to-pdf --font-dir ./fonts --report notes.json cv.docx cv.pdf
```

```text
note: sections[0].blocks[18]: tables are not laid out yet: the paragraphs of their cells follow one another at the full width
note: document: "Calibri" → "Carlito" (metric-compatible substitute)
```

`--font FILE` adds a font file, `--no-system-fonts` leaves out the system's fonts, and
`--max-pages N` makes a document that takes more pages fail (100,000 by default).

From C, with the library that `scripts/install-capi.sh PREFIX` builds (it needs
[cargo-c](https://github.com/lu-zero/cargo-c)) and installs in the layout of a release
archive:

```c
#include <stdio.h>
#include <officewright.h>

int main(void) {
    OwDocDocument *doc = NULL;
    OwRenderOptions *options = NULL;
    OwLayoutReport *report = NULL;
    size_t len = 0;
    int ok = ow_doc_document_open_path("report.docx", &doc) == OW_STATUS_OK
          && ow_render_options_new(&options) == OW_STATUS_OK
          && ow_render_options_add_font_dir(options, "fonts") == OW_STATUS_OK
          && ow_doc_document_save_pdf_path(doc, "report.pdf", options, &report) == OW_STATUS_OK
          && ow_layout_report_len(report, &len) == OW_STATUS_OK;
    if (!ok) fprintf(stderr, "error: %s\n", ow_last_error_message());
    for (size_t i = 0; i < len; i++) {
        int32_t kind;
        const char *location, *message;
        if (ow_layout_report_entry(report, i, &kind, &location, &message) == OW_STATUS_OK)
            printf("note: %s: %s\n", location, message);
    }
    ow_layout_report_free(report);
    ow_render_options_free(options);
    ow_doc_document_free(doc);
    return ok ? 0 : 1;
}
```

`ow_doc_document_open_bytes` and `ow_doc_document_save_pdf_bytes` do the same in
memory. From Rust, `officewright_doc::Document::open` and `save_pdf_path` do it, with the
crate's feature `pdf`.

## Command line

```sh
officewright text-to-pdf notes.txt notes.pdf
officewright text-to-pdf --page-size letter --font-size 10 notes.txt notes.pdf
officewright text-to-pdf --font DejaVuSans.ttf notes.txt notes.pdf
officewright text-to-pdf --font "Times New Roman" --font-dir ./fonts notes.txt notes.pdf
```

The text file must be UTF-8. Without `--font` it is set in Helvetica, which shows Latin
text only (WinAnsiEncoding). With `--font` the font is embedded, reduced to the glyphs the
text uses unless its license forbids subsetting, and shows the characters it has of the
Latin, Greek, Cyrillic, Armenian, Georgian, Han, Hiragana and Katakana scripts and Hangul
syllables. Text is not shaped yet, so scripts that need shaping or right-to-left layout
(Arabic, Hebrew, the Indic scripts…) are refused. `--font` also takes a family name,
looked up among the system fonts and the folders given with `--font-dir`
(`--no-system-fonts` leaves the system out); when the family is missing, its
metric-compatible substitute (Carlito for Calibri, Liberation for Arial, Times New Roman
and Courier New) is used, else a font of similar design (Caladea for Cambria, which lays
out differently than Cambria in Word) or a common font of the same kind. Characters a font
cannot show are reported with their line and column; substitutions and restrictions in the
font's license are printed as warnings. Page content and embedded fonts are compressed
with the Flate filter.

## C library

Each release publishes `officewright-<version>-<target>.tar.gz` (`.zip` on Windows) for
Linux (x86_64 and aarch64 glibc, x86_64 musl), Windows (x86_64, aarch64) and macOS
(x86_64, aarch64), with `include/officewright.h`, the shared and static library and
`lib/pkgconfig/officewright.pc`, which works wherever the archive is unpacked. The Linux
libraries need glibc 2.28 or newer; the musl libraries need musl and libgcc at run time
(`apk add libgcc` on Alpine), also when linked statically.

```sh
export PKG_CONFIG_PATH=/path/to/officewright/lib/pkgconfig
cc hello.c $(pkg-config --cflags --libs officewright) -o hello
```

```c
#include <stdio.h>
#include <officewright.h>

int main(void) {
    OwPdfFont *font = NULL;
    OwPdfDocument *doc = NULL;
    int ok = ow_pdf_font_new_standard(OW_STANDARD_FONT_HELVETICA, &font) == OW_STATUS_OK
          && ow_pdf_document_from_text("Hello, world", 595.28, 841.89, font, 11.0, &doc) == OW_STATUS_OK
          && ow_pdf_document_save_path(doc, "hello.pdf") == OW_STATUS_OK;
    if (!ok) fprintf(stderr, "error: %s\n", ow_last_error_message());
    ow_pdf_document_free(doc);
    ow_pdf_font_free(font);
    return ok ? 0 : 1;
}
```

Every function that writes a file also has an in-memory variant
(`ow_pdf_document_save_bytes`), and any language with a C FFI can load the shared
library; `tests/ffi-python` does it with Python's `ctypes`.

## Roadmap

1. Workspace, licenses, quality gates and CI.
2. Minimal PDF writer, C ABI, `officewright text-to-pdf`, prebuilt native libraries.
3. TrueType/OpenType font embedding with subsetting (Cyrillic and other non-Latin text).
4. DOCX → PDF: reader, layout engine, fidelity tests against Microsoft Word.
5. XLSX and PPTX read/write, PDF reading and text extraction.
6. Language packages (.NET, Python, Node, Java, Go, WebAssembly).

0.1.0 has steps 1 to 3; step 4 is in progress, and the main branch converts DOCX to PDF
in an [early version](#docx-to-pdf-early-version).

## Clean room

Officewright is written only from public specifications (ECMA-376, ISO 32000,
Microsoft Open Specifications) and the observable behaviour of the original
applications. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Officewright is source-available under the [Business Source License 1.1](LICENSE):

- Anyone may read, change and redistribute the code, and use it to develop, test and
  evaluate software.
- Production use is free for individuals acting for non-commercial purposes,
  non-profits, educational institutions and organizations with less than USD 1M in
  annual revenue, as long as they do not offer Officewright to others as a document
  library, SDK, API, command-line tool or hosted service, and the product that uses it
  credits Officewright.
- Everyone else needs a [commercial license](COMMERCIAL.md).
- Each version becomes available under the Apache License 2.0 four years after its
  first public release.

The exact terms are the Additional Use Grant in [LICENSE](LICENSE).

The Helvetica metrics are derived from Adobe's Core 14 AFM files, which have their own
terms; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Contributions are accepted under the [Contributor License Agreement](CLA.md); see
[CONTRIBUTING.md](CONTRIBUTING.md).
