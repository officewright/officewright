# Officewright

Read, write and convert office documents — DOCX/DOC, XLSX/XLS, PPTX/PPT and PDF —
without Microsoft Office or LibreOffice.

The core is written in Rust. Every other language uses it through a stable C ABI,
shipped as prebuilt libraries with a C header, so any language with an FFI can call
it without a Rust toolchain.

> **Status: pre-alpha.** A minimal PDF writer works from Rust, C and the command line;
> it embeds TrueType and OpenType fonts for Latin, Greek, Cyrillic and the other scripts
> that need no shaping. See the roadmap below.

Officewright is a source-available alternative to Spire.Office and Aspose. It is not
affiliated with, endorsed by or connected to the makers of either product; the
names are trademarks of their respective owners and are used here only to describe
what this project is an alternative to.

## Command line

```sh
officewright text-to-pdf notes.txt notes.pdf
officewright text-to-pdf --page-size letter --font-size 10 notes.txt notes.pdf
officewright text-to-pdf --font DejaVuSans.ttf notes.txt notes.pdf
officewright text-to-pdf --font "Times New Roman" --font-dir ./fonts notes.txt notes.pdf
```

The text file must be UTF-8. Without `--font` it is set in Helvetica, which shows Latin
text only (WinAnsiEncoding). With `--font` the font is embedded, reduced to the glyphs the
text uses, and shows the characters it has of the Latin, Greek, Cyrillic, Armenian,
Georgian, Han, Hiragana and Katakana scripts and Hangul syllables. Text is not shaped
yet, so scripts that need shaping or right-to-left layout (Arabic, Hebrew, the Indic
scripts…) are refused. `--font` also takes a family name, looked up among the system fonts and the
folders given with `--font-dir` (`--no-system-fonts` leaves the system out); when the family
is missing, its metric-compatible substitute (Carlito for Calibri, Liberation for Arial,
Times New Roman and Courier New) or a common font of the same kind is used. Characters a
font cannot show are reported with their line and column; substitutions and restrictions in
the font's license are printed as warnings.

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
