# Officewright

Source-available, clean-room library for reading, writing and converting office documents (DOCX/DOC, XLSX/XLS, PPTX/PPT, PDF) without Microsoft Office or LibreOffice. Positioned as an alternative to Spire.Office and Aspose. Core is written in Rust; other languages use it through a C ABI.

Priority order: (1) a correct Rust core, (2) a stable C ABI shipped as prebuilt libraries + header so any language with an FFI can use it without a Rust toolchain, (3) idiomatic packages for specific languages — only after (1) and (2) are solid.

- GitHub: https://github.com/officewright/officewright (org `officewright`, monorepo)
- License: Business Source License 1.1, SPDX `BUSL-1.1` (file `LICENSE`): free production use under the Additional Use Grant (individuals non-commercially, non-profits, education, organizations under USD 1M revenue; not as a document library/SDK/API/CLI/service; with credit), commercial license otherwise (`COMMERCIAL.md`), each version becomes Apache-2.0 four years after release.
- Contributions: under the Contributor License Agreement (`CLA.md`); no pull request from an outside contributor is merged without it.
- Rust: edition 2024, MSRV 1.85, `resolver = "3"`

## Language

- Code, comments, commit messages, docs, issues and PRs: English.

## Clean-room policy (non-negotiable)

- Never decompile, disassemble or read the code of Spire, Aspose or any other proprietary library, and never paste output from such tools into this repo or into prompts.
- Allowed sources: ECMA-376 (OOXML), ISO 32000 (PDF), Microsoft open specifications ([MS-DOC], [MS-XLS], [MS-PPT], [MS-OI29500] etc.), public documentation, and observable behaviour of the original applications.
- Ground truth for rendering tests: PDFs exported by Microsoft Word/Excel/PowerPoint (LibreOffice only as a secondary reference). The PDFs themselves are not committed, because they hold subsets of Microsoft's fonts: `tests/corpus/reference/` keeps every page as a grey PNG and the geometry of its lines and words, with the PDF's SHA-256 and provenance in `tests/corpus/SOURCES.md`.
- Names: never use "Spire" or "Aspose" in crate, package, type or repo names. They may appear only in README/descriptions as "alternative to", with a "not affiliated" disclaimer.

## Standard vs. what Office actually does

Office does not follow ECMA-376/ISO 29500 literally, and users judge us against Word, not against the spec.

- Read the spec together with Microsoft's implementer notes: [MS-OI29500] (how Office implements ISO/IEC 29500, including where it deviates or ignores parts) and [MS-OE376] (same for ECMA-376 1st edition). Office-only extensions are in [MS-DOCX], [MS-XLSX], [MS-PPTX], [MS-ODRAWXML] (w14/w15/w16 and similar namespaces).
- When the spec and Word disagree, match Word and leave a comment citing the implementer note or the corpus file that shows the behaviour (`// Word deviates from §17.3.1.33, see MS-OI29500 2.1.xxx`).
- Markup Compatibility (ECMA-376 Part 3): honour `mc:Ignorable`, pick `mc:Choice` only for namespaces we implement, otherwise `mc:Fallback`. Unknown extension elements are preserved on round-trip, never dropped.
- Layout depends on `w:compatSetting` `compatibilityMode` (11 = Word 2003, 12 = 2007, 14 = 2010, 15 = 2013+) and legacy `w:compat` flags. Treat the mode as an input to the layout engine; the corpus must contain documents saved in each mode.
- Much of Word's layout (line breaking, table autofit, spacing rounding, widow/orphan control) is not documented anywhere. It is learned empirically from the Word reference renders; every such rule gets a corpus file that demonstrates it.
- Fonts: Office fonts (Calibri, Cambria, Aptos…) cannot be bundled. Use the document's embedded fonts if present, then system fonts, then metric-compatible substitutes (Carlito for Calibri, Liberation for Arial/Times New Roman/Courier New), then fallbacks, preferring one of similar design (Caladea for Cambria, which lays out differently), and report every substitution so users know why output differs. A font counts as metric-compatible only when a measurement against Word's reference render confirms it (`tests/corpus/word-rules/E-line-fonts.docx`); Caladea does not, so documents in Cambria, like those in Aptos, get no strict position comparison on CI: the layout engine's unit tests check the line-height formula against Cambria's measured numbers, and Cambria's metrics are never copied into library code.

## Workspace layout

```
crates/
  officewright          facade crate; features doc, pdf, xls, slides; re-exports format crates, and
                        officewright-font as `font` with the pdf feature
  officewright-core     units (Pt, twips, EMU), PageSize, Color (sRGB), Error/Result
  officewright-font     font loading (TrueType/OpenType, collections), metrics, cmap, subsetting for
                        embedding, system font index and font substitution with a report
  officewright-opc      ZIP, XML, OPC packages and Markup Compatibility (ECMA-376 Parts 2 and 3) shared
                        by the OOXML readers; ZIP and XML are our own code, DEFLATE is miniz_oxide
  officewright-layout   page layout engine (shaping, text flow, line breaking, tables) — the core of DOCX→PDF;
                        its input is the layout IR (module `ir`), which the format crates build: public,
                        but exempt from semver until 1.0 and hidden in the docs unless feature `unstable-ir`
  officewright-doc      DOC/DOCX/RTF          (counterpart of Spire.Doc)
  officewright-pdf      PDF read/write        (counterpart of Spire.PDF)
  officewright-xls      XLS/XLSX              (counterpart of Spire.XLS)
  officewright-slides   PPT/PPTX              (counterpart of Spire.Presentation)
  officewright-ffi      C ABI; built by cargo-c (`cargo cinstall`) into the native lib `officewright`
  officewright-cli      binary `officewright`
bindings/               (Milestone 6, not before) idiomatic language packages
fuzz/                   cargo-fuzz targets, one per parser; a cargo workspace of its own (libFuzzer is never shipped)
tests/c/                C tests compiled against the generated header and the static library
tests/ffi-python/       Python ctypes tests loading the shared library — proves the ABI is usable
                        from a non-C language with no build step
tests/pdf-output/       checks text-to-pdf output, in Helvetica and with Liberation Sans (TrueType) and Noto Sans
                        (CFF) from tests/fonts embedded, with qpdf, pdftotext, pdffonts and pdftoppm (Linux,
                        macOS; Windows has no official poppler build, and the golden snapshot proves the bytes
                        are the same)
tests/corpus/           small curated real documents (committed) + SOURCES.md + baseline.json
tests/corpus/word-rules/  documents written by scripts/make-rule-docs.py, one undocumented Word rule each
tests/corpus/reference/ ground truth from PDFs of Microsoft Word/Excel/PowerPoint, one folder per document: every page
                        as a 110 dpi grey PNG and geometry.json (lines, words, page breaks); the PDFs are not
                        committed, their SHA-256 and the app version are in SOURCES.md
tests/corpus/resaved/   the copy Word saved of each document before exporting its reference PDF
tests/corpus-tools/     tests of make-rule-docs.py and corpus-import.py (Python) and of make-reference.ps1
                        without Word (Windows PowerShell, Windows only); poppler/ checks corpus-import.py's
                        font embedding with the real qpdf and poppler (Linux, macOS)
tests/fonts/            unmodified OFL test fonts with their license files, listed in THIRD-PARTY-NOTICES.md;
                        outside every crate, so never published
tools/corpus-runner/    workspace member, publish = false: runs the library over a corpus, writes a JSON report
tools/ooxml-validate/   small .NET tool: validates generated OOXML with Open XML SDK's OpenXmlValidator (MIT)
corpus-ext/             gitignored: large external corpora downloaded by scripts/fetch-corpus.sh
scripts/check.sh        PR quality gate (see below)
scripts/install-capi.sh installs the C library into a prefix with the release archive layout
scripts/check-capi.sh   checks such a prefix as a user would: files, exports, tests/c, tests/ffi-python
scripts/package.sh      builds one release archive; release-in-container.sh wraps it for the Linux builds
scripts/install-cargo-c.sh  the pinned prebuilt cargo-c, shared by ci.yml and release.yml
scripts/nightly.sh      long-running checks (see below)
scripts/make-rule-docs.py  writes tests/corpus/word-rules/ (stored ZIP, fixed dates: same bytes everywhere)
scripts/windows/make-reference.ps1  renders the reference PDFs with Word on the maintainer's Windows machine
scripts/corpus-import.py   checks make-reference.ps1 results and turns them into tests/corpus/ rasters and geometry
```

## Definition of done (every change)

1. `scripts/check.sh` passes locally. It is the same script CI runs.
2. New behaviour has tests; a bug fix has a regression test that failed before the fix.
3. Every new public item has a doc comment; non-trivial public APIs have a runnable doc example.
4. A new parser comes with a fuzz target in `fuzz/`.
5. Every public Rust feature that users need is exposed through the C ABI in the same PR, with the header regenerated and covered by `tests/c` and `tests/ffi-python`. A crate that no user-facing feature uses yet (a building block such as `officewright-font`) gets its C ABI in the PR that makes it user-facing. Work the maintainer planned as a chain of stacked PRs gets its C ABI in the PR of the chain the plan names: the DOCX reader in the last one, once the document model is complete.
6. Never weaken a check to make it pass (no `#[allow]` without a `// reason:` comment, no skipped tests, no lowered lint levels).

## Rust conventions

- `#![forbid(unsafe_code)]` in every crate except `officewright-ffi`.
- Library code never panics on input: no `unwrap`/`expect`/indexing that can go out of bounds on data that came from a file. Malformed documents produce `Err`, never a crash. Enforced by `[workspace.lints]` (errors under `-D warnings`): clippy `unwrap_used`, `expect_used`, `indexing_slicing`, `panic`, `todo`, `unimplemented`, `undocumented_unsafe_blocks`, and rustc `missing_docs`. `clippy.toml` allows the first four in tests. Use `.get()` instead of `[]`; an index that cannot fail gets `#[allow(clippy::indexing_slicing)] // reason: ...`.
- The same holds for dependencies. A dependency that can still panic on malformed input (known `unwrap`/`unreachable!`/overflow on data paths, such as `subsetter`) is called inside `std::panic::catch_unwind`, and the panic becomes an `Err` with a clear message. The call site links the upstream issue; panics that fuzzing finds are reported upstream, and the wrapper goes once upstream returns errors. Like the C ABI, this relies on unwinding.
- Errors: one `Error` enum per crate (`thiserror` allowed), `#[non_exhaustive]`, convertible into `officewright_core::Error`. Messages are lowercase without trailing punctuation and name the offending object/offset.
- Public API follows the Rust API Guidelines: `#[non_exhaustive]` on public enums and on structs that may grow, builders for types with many options, no public fields on types that may change, `impl AsRef<Path>`/`impl Read`/`impl Write` instead of concrete types.
- Domain names mirror the document model (`Document`, `Section`, `Paragraph`, `Run`, `Table`, `Worksheet`, `Cell`, `Slide`) so Spire users recognise them; implementations are original.
- All geometry is `officewright_core::Pt` internally; convert twips/EMU/half-points only at parse and serialize boundaries.
- Dependencies: as few as possible, justified in the PR; licenses restricted by `deny.toml` (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0). No GPL/LGPL/AGPL. Our own crates are `BUSL-1.1`, allowed in `deny.toml` by crate name only: a new crate in `crates/` sets `license.workspace = true` and `license-file.workspace = true` (published crates must carry `LICENSE`) and adds its `[[licenses.exceptions]]` entry to `deny.toml` in the same PR.
- Third-party data that is not a crate (e.g. Adobe's Core 14 AFM files in `crates/officewright-pdf/data/afm/`) keeps its own license: it is listed in `THIRD-PARTY-NOTICES.md` with its source and SHA-256, committed byte for byte (`-text` in `.gitattributes`), and left out of the published crate together with the tests that read it (`exclude` in the crate's `Cargo.toml`, verified by `check.sh`). Tables derived from it keep the original copyright notice in their source file.

## Linters

- Rust: `rustfmt`, `clippy -D warnings`, `cargo doc` with `RUSTDOCFLAGS="-D warnings"` (broken intra-doc links), `cargo deny` (licenses, advisories, duplicate deps), `cargo machete` (unused deps).
- Unsafe code: `cargo +nightly miri test -p officewright-ffi` catches undefined behaviour in the FFI layer; it runs with `MIRIFLAGS=-Zmiri-disable-isolation` because the tests write files.
- C side: `tests/c` links the static library from a prefix made by `scripts/install-capi.sh` or unpacked from a release archive (link flags from `Libs.private` in `officewright.pc`, never hardcoded). On Linux and macOS it compiles with `cc -std=c99 -Wall -Wextra -Wpedantic -Werror`, once more as C++17 (the header must work from C++), and once with `-fsanitize=address,undefined` (catches double free, leaks and misuse of `ow_*_free` across the boundary; leak detection runs on Linux only; not on musl, which has no sanitizer runtimes). On Windows it compiles with MSVC, the compiler Windows users have: `cl -std:c17 -W4 -WX` and, compiling the same file as C++ via `-Tp`, `cl -std:c++17 -W4 -WX`, both `-MD`; MSVC has no UBSan, so the sanitizer build is Linux/macOS only.
- Repo hygiene: `typos` (spelling), `taplo fmt --check` (TOML), `shellcheck` (scripts and the git hook), `actionlint` (workflows).

## Testing with real documents

Unit and snapshot tests (`insta`) cover small handcrafted inputs. Real-world correctness comes from corpora and external oracles. Using other software as a black-box oracle (run it, compare outputs) is allowed; reading its code is not.

### Corpora

- Committed, `tests/corpus/`: a few hundred small files (each < 1 MB, total < 50 MB) chosen for edge cases. Only files whose license allows redistribution; each listed in `SOURCES.md` with origin and license. Never commit personal, customer or copyrighted documents.
- External, `corpus-ext/`, downloaded by `scripts/fetch-corpus.sh` from the manifest `scripts/corpus-ext.tsv`, used in nightly runs only. Git sources are pinned by commit SHA and fetched as shallow, sparse, partial clones, so git verifies their content (GitHub does not guarantee stable checksums for generated archives, and a sparse clone fetches only the test data); archives are pinned by SHA-256.
  - Apache POI `test-data` (Apache-2.0): DOC, DOCX, XLS, XLSX, PPT, PPTX edge cases.
  - pdf.js `test/pdfs` (Apache-2.0), qpdf test files (Apache-2.0), PDFium testing corpus (BSD-3-Clause): broken and unusual PDFs.
  - LibreOffice `sw/qa`, `sc/qa`, `sd/qa` documents (MPL-2.0): used as test input only, never copied into the repo.
  - Digital Corpora GovDocs1 and the Common Crawl PDF corpus: large-scale robustness runs (samples of 10k–100k files).
- Ground truth for rendering, `tests/corpus/reference/`: PDFs exported by Microsoft Word/Excel/PowerPoint from the corpus files (`scripts/windows/make-reference.ps1`, COM automation, run by the maintainer on Windows). The PDFs stay with the maintainer, because they hold subsets of Microsoft's fonts. `scripts/corpus-import.py` commits instead every page rasterized by `pdftoppm -r 110 -gray` as a PNG and `geometry.json`: page sizes, the boxes of every line and word (`pdftotext -bbox-layout`, points from the top left), the page count, and Word's page breaks from the re-saved copy. It records each PDF's SHA-256 in SOURCES.md, and refuses a re-saved copy into which Word embedded fonts. `make-reference.ps1` also exports the formatting Word computes for every paragraph of every story (whether the style is built in, indents, spacing, line spacing, alignment, the list label `ListFormat.ListString`, the first character's font, and the runs of formatting when reading them takes no longer than the paragraphs); `corpus-import.py` commits it as `formatting.json` with the body paragraphs' style ids and names taken from the re-saved copy. Word's own style names, the names its font box gives theme fonts, and its error messages are in the language of its user interface, so they never leave the Windows side: theme fonts are recorded by their ST_Theme value (`minorHAnsi`), and the importer accepts only make-reference.ps1's fixed English notes. Fonts Word only references (Times New Roman, Arial) are embedded into a temporary copy of the PDF before rasterizing, from the maintainer's own installed font files (`--font-dir`, found by PostScript name, never copied into the repository), so every PNG shows Word's glyphs; SOURCES.md and `geometry.json` name those files by version and SHA-256, never by path. Word's printer is Microsoft Print to PDF (a printer that does not answer can stall Word, and with `w:usePrinterMetrics` Word lays out with the printer's metrics; the script warns otherwise). Word saves each document again first and exports from that copy, which goes to `tests/corpus/resaved/` (its `w:lastRenderedPageBreak` marks are Word's page breaks); `scripts/corpus-import.py` brings the results in and refuses any that carry a person's or company's name or a user profile path. LibreOffice exports are a secondary reference, marked as such. Never use output of Spire or Aspose as reference.
- Undocumented Word rules get a generated document each in `tests/corpus/word-rules/` (`scripts/make-rule-docs.py`, written from ECMA-376 markup), so a reference render isolates one rule.

### What runs against the corpora

1. Robustness: every file is opened; the result must be `Ok` or `Err`, never a panic, hang (timeout 30 s/file) or memory blow-up (limit 1 GiB). Corpus files also seed the fuzzers.
2. Round-trip: open → save → reopen; text, structure and metadata must survive.
3. Output validity:
   - PDF: `qpdf --check`, `pdfinfo`; PDF/A output additionally through veraPDF.
   - OOXML: every part validated against the ECMA-376 schemas via `tools/ooxml-validate`; the file must also open in LibreOffice headless (`soffice --headless --convert-to pdf`).
4. Text fidelity: extracted text compared with `pdftotext` (poppler) and `mutool draw -F txt` (MuPDF) on the same file, normalized for whitespace.
5. Visual fidelity (from Milestone 4): our PDF rasterized with `pdftoppm -r 110 -gray` and compared per page by SSIM with the committed reference rasters in `tools/corpus-runner`; score stored per file. Line and word positions are compared with `geometry.json`.

### Regression baseline

`tools/corpus-runner` opens every Word document of the paths it is given in a child process of its own (30 s time-out; on Linux a peak memory over 1 GiB is a failure, and with `--enforce-memory-limit` each child also runs under that address-space limit through `prlimit`) and writes a JSON report: per file its status (`opened`, `expected-error`, `unexpected-error`, `panic`, `timeout`, `memory`, `crash`), error, duration, peak memory, characters of text and diagnostics; later the round-trip result, text score and SSIM per page. An error counts as expected only when `tools/corpus-runner/expected-errors.tsv` lists the file with its reason (encrypted, another format under a Word extension, broken on purpose); a document Word opens is never listed. Every run fails on a panic, time-out, memory failure or crash. `tests/corpus/baseline.json` holds the accepted status of every committed file (durations and memory vary by machine and are left out); a run with `--baseline` fails if any file's status differs from it or the set of files does, or any score drops by more than 0.01. Improvements fail too until they are accepted by running with `--update-baseline` in the same PR, so progress is visible in the diff. The nightly run compares the Word documents of `corpus-ext/` with `tools/corpus-runner/baseline-ext.json` and fails only on worse files (`--allow-better`); a PR that changes the DOCX reader updates that baseline when it changes a file's status there, and so does a PR that moves the pins of POI or LibreOffice in `scripts/corpus-ext.tsv`.

## C ABI conventions (`officewright-ffi`)

The C ABI is the product's main integration surface. Design it so that the dumbest FFI (Python ctypes, PHP FFI, Go cgo, Excel VBA via a shim) can call it.

- Only C types cross the boundary: integers with fixed width (`int32_t`, `uint64_t`, `size_t`), `double`, `const char*`, `uint8_t*` + length, opaque pointers, `#[repr(C)]` enums. No Rust types, no structs passed by value, no bitfields, no `bool` (use `int32_t` 0/1).
- Every operation that takes a file has a path variant and an in-memory variant (`ow_pdf_document_open_path` / `ow_pdf_document_open_bytes`, `..._save_path` / `..._save_bytes`), because many hosts (web servers, serverless, sandboxes) have no filesystem.
- Callbacks only when unavoidable, always as `(fn_ptr, void* user_data)` pairs.
- Thread safety documented on every handle type; default rule: a handle is used by one thread at a time, different handles are independent.
- Functions prefixed `ow_`, types prefixed `Ow`. Edition 2024 syntax: `#[unsafe(no_mangle)] pub unsafe extern "C" fn`, each with a `/// # Safety` section. A function with no caller obligations (no pointers or handles, e.g. `ow_abi_version`) is a safe `pub extern "C" fn`.
- Objects are opaque handles (`OwPdfDocument*`), created by `ow_<type>_new` / `ow_<type>_open`, released by `ow_<type>_free` (null-safe).
- Every fallible function returns `OwStatus` (`#[repr(C)]` enum: OK, NULL_ARGUMENT, INVALID_UTF8, INVALID_ARGUMENT, IO, UNSUPPORTED, PANIC, MALFORMED, LIMIT_EXCEEDED); results go through out-parameters. Values are only ever appended, never renumbered. Damaged input (not a font, broken ZIP or XML) is MALFORMED and a configured limit LIMIT_EXCEEDED, mapped from `officewright_core::Error::Malformed` and `LimitExceeded`; INVALID_ARGUMENT is for the caller's own arguments, such as a face index the file does not have.
- Error text: `ow_last_error_message()` returns a thread-local UTF-8 string valid until the next `ow_*` call on the same thread.
- Strings in: `const char*` UTF-8, NUL-terminated. Buffers out: `ow_bytes_free(ptr, len)`. Strings out are `const char*` owned by the library: those of a handle (such as the message of `ow_pdf_font_note`) stay valid until the handle is freed, or for a handle that changes (`ow_font_book_skipped`) until it changes; each function says which.
- Every function body runs inside `catch_unwind`; a panic must never cross the boundary. Only a function that returns a constant may skip it.
- Build with cargo-c, not plain `cargo build`: the Rust lib is `officewright_ffi` (so it does not clash with the `officewright` facade crate) and `[package.metadata.capi]` names the native library `officewright`. `scripts/install-capi.sh <prefix>` runs `cargo cinstall --libdir lib --bindir lib` and writes the shared and static library (plus the import library on Windows), `officewright.pc` (with `Libs.private` for static linking), the header and the license files into the archive layout below. It makes the prefix relocatable: `officewright.pc` sets `prefix=${pcfiledir}/../..`, and on macOS the dylib's install name becomes `@rpath/...` (re-signed ad hoc, since arm64 macOS refuses an invalid signature). Never pass `-Cpanic=abort`: it would turn every `catch_unwind` into an abort.
- Header generated by cargo-c's cbindgen (`crates/officewright-ffi/cbindgen.toml`) and copied to `crates/officewright-ffi/include/officewright.h`, committed; CI fails if it is stale. Crate version constants are disabled so the header changes only when the ABI does.
- `ow_abi_version()` returns an integer bumped on every breaking ABI change; bindings check it at load time.
- Only `ow_*` symbols are exported from the shared library. On Linux the library has a versioned SONAME (`libofficewright.so.0`, set by cargo-c with `version_suffix_components = 1`).

## Distribution of the native library

Every tagged release publishes, per target, an archive `officewright-<version>-<target>.tar.gz` (`.zip` on Windows) containing:

```
include/officewright.h
lib/libofficewright.so | officewright.dll + officewright.dll.lib | libofficewright.dylib
lib/libofficewright.a  | officewright.lib (static)
lib/pkgconfig/officewright.pc
LICENSE, README.md, THIRD-PARTY-NOTICES.md
```

Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`, `x86_64-apple-darwin`, `aarch64-apple-darwin`. Built by `.github/workflows/release.yml` with `scripts/package.sh`; a separate verify job per target unpacks the archive on a fresh runner of that platform and runs `scripts/check-capi.sh` (files, exports, `tests/c`, `tests/ffi-python`) against it, not against the build tree.

- Build environments: the `-gnu` targets build in `manylinux_2_28` containers, so the library needs only glibc 2.28 (RHEL 8+, Debian 10+, Ubuntu 20.04+); the musl target builds in Alpine with `-C target-feature=-crt-static`, because Rust's default static musl rules out a shared library, so the musl shared library needs musl and libgcc at run time (Alpine: `apk add libgcc`), and so do programs linked with the static library, whose `Libs.private` includes `-lgcc_s`. Windows and macOS build natively, except `x86_64-apple-darwin`, which is cross-compiled on arm64 (cargo-c ships no x86_64 macOS build) and verified on an Intel runner.
- Release builds use the Rust version pinned in `RUST_TOOLCHAIN` in `release.yml`. Alpine packages in the build container are not pinned: Alpine keeps only the latest build of each package in a branch, so the image digest and the branch (3.22) are what is pinned.
- `scripts/check-capi.sh` rejects any file outside the layout above, so cargo-c's extra Windows output (`.def`, `.pdb`) is removed by `install-capi.sh`.
- A tag `v<version>` must match the workspace version; the workflow then drafts a GitHub release with the archives and `SHA256SUMS`. Pull requests that touch the packaging run everything but the release.

## Language packages (Milestone 6)

Not before the C ABI is stable. When they come: .NET (`netstandard2.0` + `net8.0`, `SafeHandle`, idiomatic API; for .NET Framework users migrating from Spire), Python (PyO3 + maturin), Node (napi-rs), Java (FFM over the C ABI), Go (cgo), WebAssembly (`wasm-bindgen`, separate crate, no C ABI). Each package exposes an idiomatic API for its language, not a mirror of the C names.

## Git workflow

- Conventional Commits with the crate as scope: `feat(pdf): write xref streams`, `fix(ffi): free buffer on error path`, `docs(readme): ...`, `ci: ...`. Breaking changes use `!` (`feat(pdf)!: ...`).
- One logical change per commit; no "wip"/"fix stuff" commits.
- Until Milestone 1 is merged, commit to `main`. After that, work on branches (`feat/<topic>`, `fix/<topic>`), open a PR, merge only with green CI. `main` is protected.
- Releases (later): release-plz generates versions and CHANGELOG from commit messages; cargo-semver-checks runs before publishing, without the feature `unstable-ir` of `officewright-layout` (the IR is exempt until 1.0).

## Review process

The maintainer does not review Rust code. Correctness rests on the automated gates plus an independent AI review. Every PR goes through these steps.

### 1. Author
- Keep PRs reviewable: aim for under ~1500 changed non-test lines; split larger work into stacked PRs.
- Open the PR as a draft. The description must contain:
  - **What changed** — plain language, no Rust jargon.
  - **Why** — the milestone or plan it implements.
  - **Risks** — what could break and for whom.
  - **Deviations from CLAUDE.md** — and confirm CLAUDE.md was updated.
  - **New dependencies** — each with justification and the `cargo deny` result.
  - **Corpus numbers** — before → after, when the PR touches a reader or writer.
  - **How to check by hand** — exact commands the maintainer can run (for example a CLI call on a sample file) and what to look at in the output (open the PDF, compare with Word).

### 2. Independent reviewer
- A fresh Claude Code session that has not seen the implementation conversation, on a different model than the author (Opus author → Fable reviewer, and vice versa).
- Reviews the diff against CLAUDE.md, the Definition of done and the clean-room policy. Focus: parsers and FFI safety, tests that cannot fail or test the implementation instead of behaviour, missing edge cases, public API and C ABI stability.
- For critical paths, temporarily breaks the code and confirms a test catches it.
- Verifies each finding adversarially, then posts one PR comment listing findings tagged **blocker**, **should-fix** or **nit**.

### 3. Fixes
- Every blocker is fixed. Every should-fix is fixed or answered in the PR. The reviewer re-checks the blockers; repeat until the reviewer's last comment says "no blockers".

### 4. Merge (maintainer)
Merge only when all hold:
- all required CI checks are green;
- the reviewer's last comment says "no blockers";
- the maintainer ran the "How to check by hand" steps and the output looks right;
- corpus numbers are not below the baseline.

PRs are squash-merged; the PR title must be a Conventional Commit, since it becomes the commit message on main.

### Always ask the maintainer first
The agent must not decide these alone:
- adding a runtime dependency;
- committing any third-party file, or anything whose license is not MIT/Apache-2.0;
- breaking changes to the public Rust API or the C ABI;
- weakening any check: lints, `deny.toml`, `check.sh`, CI, required check names;
- changing milestone scope;
- anything legal: trademarks, clean-room questions.

## Quality gates

`scripts/check.sh` (every PR, a few minutes), failing on the first error:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo test --workspace --all-features
cargo fmt --check and cargo clippy -D warnings on the fuzz/ workspace (Linux/macOS; cargo-fuzz does not run on Windows)
cargo deny check && cargo machete
cargo package --list -p officewright-pdf, -p officewright-font: no third-party data in the published crates
typos && taplo fmt --check && shellcheck scripts/*.sh .githooks/pre-commit && actionlint
tests/corpus-tools (Python unittest): committed rule documents equal make-rule-docs.py output; corpus-import.py refusals
tests/corpus-tools/poppler (Linux, macOS): fonts corpus-import.py embeds are what poppler draws
tests/corpus-tools/test_make_reference.ps1 (Windows): a Word step that hangs is stopped, checked and never waited on forever
scripts/install-capi.sh target/capi --debug, copy the header, git diff --exit-code crates/officewright-ffi/include/officewright.h
scripts/check-capi.sh target/capi (release.yml runs the same script on every unpacked archive):
  installed files; officewright.pc is relocatable; macOS install name is @rpath/...
  exported symbols of the shared library are exactly ow_*; Linux SONAME is libofficewright.so.0
  tests/c against the static library: C99, C++17, ASan+UBSan (glibc Linux, macOS); MSVC C17 and C++17 (Windows)
  tests/ffi-python against the shared library
PDF output of officewright text-to-pdf (tests/pdf-output): qpdf --check without warnings, pdftotext reads the input back;
  with Liberation Sans and Noto Sans of tests/fonts embedded: pdffonts lists each embedded, subset and with a ToUnicode map,
  pdftotext reads back every Latin, Greek and Cyrillic character both fonts have, pdftoppm renders a subset exactly as the
  whole font; characters that share a glyph read back through ActualText (Linux, macOS)
corpus-runner over tests/corpus (committed corpus), compared with tests/corpus/baseline.json
```

`scripts/nightly.sh` (scheduled CI, hours): Miri on `officewright-ffi`, `fetch-corpus.sh --check all` (every pinned commit is still fetchable and every archive still has its pinned size; downloads nothing), fuzzing (30 min per target, each seeded with files of the format it reads, listed in `seeds()` in nightly.sh; crashing inputs uploaded as a CI artifact; from Milestone 3), corpus-runner over the Word documents of `corpus-ext/` (Apache POI and LibreOffice, fetched by the run; each child under the 1 GiB limit; compared with `tools/corpus-runner/baseline-ext.json`), report uploaded as a CI artifact. The full `fetch-corpus.sh all` from scratch, which downloads every source and verifies every SHA-256, runs weekly.

`.githooks/pre-commit` runs fmt, clippy and test. Enable once per clone: `git config core.hooksPath .githooks`. CI (`.github/workflows/ci.yml`) runs `check.sh` on Linux, Windows (Git Bash with the MSVC environment from `ilammy/msvc-dev-cmd`) and macOS plus an MSRV job on Rust 1.85; `nightly.yml` runs `nightly.sh` on Linux; `weekly.yml` runs the full corpus fetch on Linux. Actions are pinned by commit SHA and tools by version, except qpdf and poppler for the PDF output check: they come from the runner's package manager (apt on `ubuntu-latest`, Homebrew on macOS), which keeps no installable old versions across runner image updates, so the CI step prints the versions it got.

Required local tools: `qpdf poppler-utils mupdf-tools libreoffice-writer-nogui shellcheck python3` (apt), `cargo-deny cargo-machete cargo-c typos-cli taplo-cli cargo-fuzz` (cargo install, or their prebuilt release binaries), `actionlint`, nightly toolchain with `miri`; on Windows, MSVC and Git Bash. `check.sh` must print a clear install hint for any missing tool instead of skipping the check.

## Milestones

1. Workspace skeleton, licenses, README (with "not affiliated" disclaimer), CONTRIBUTING (clean-room policy, Definition of done, commit convention), `rustfmt.toml`, `clippy.toml`, workspace lints, `deny.toml`, `typos.toml`, `taplo.toml`, `scripts/check.sh`, `scripts/nightly.sh` (stub), `.githooks/pre-commit`, CI, `tests/corpus/SOURCES.md`, `scripts/fetch-corpus.sh` with the manifest of external corpora.
2. Vertical slice: minimal PDF writer (pages, standard-14 Helvetica text, WinAnsi encoding, correct xref) → C ABI (path and in-memory save) → `tests/c` + `tests/ffi-python` → CLI `officewright text-to-pdf` → `release.yml` producing the native archives for all targets.
3. TrueType/OpenType embedding with subsetting and ToUnicode CMap (required for Cyrillic and other non-Latin text).
4. DOCX reader → layout → PDF (the main gap in open source; headline feature for launch). Includes `tools/corpus-runner` with SSIM comparison, Word reference renders for the committed corpus, `tools/ooxml-validate`, and the nightly corpus run.
5. XLSX read/write, PPTX read/write, PDF reading/text extraction.
6. Language packages, starting with whichever users ask for most in issues.
