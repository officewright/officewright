# Contributing to Officewright

## License and Contributor License Agreement

Officewright is source-available under the Business Source License 1.1 ([LICENSE](LICENSE)).
Before your first pull request can be merged, you must agree to the
[Contributor License Agreement](CLA.md): post the comment it asks for on the pull request.
You keep the copyright in your contribution; the agreement lets the project ship it under
its license and under commercial licenses.

## Security problems

Do not report them in public issues or pull requests: [SECURITY.md](SECURITY.md) says how
to report them privately.

## Clean-room policy

This policy is not negotiable. A contribution that breaks it will be reverted.

- Never decompile, disassemble or read the code of Spire, Aspose or any other
  proprietary library, and never paste output from such tools into this repository,
  an issue, a pull request or a prompt.
- Allowed sources: ECMA-376 (OOXML), ISO 32000 (PDF), the Microsoft Open
  Specifications ([MS-DOC], [MS-XLS], [MS-PPT], [MS-OI29500], [MS-OE376], [MS-DOCX],
  [MS-XLSX], [MS-PPTX], [MS-ODRAWXML], …), other public documentation, and the
  observable behaviour of the original applications.
- Running other software as a black box and comparing its output with ours is
  allowed. Reading its source code is not.
- Never use the output of Spire or Aspose as a test reference. Ground truth for
  rendering is what Microsoft Word, Excel and PowerPoint produce.
- Never use "Spire" or "Aspose" in crate, package, type or repository names.

## Development setup

`scripts/check.sh` is the quality gate CI runs on every pull request. Run it before
you push. If a tool is missing, it lists every missing tool with an install command.

- Rust stable with `rustfmt` and `clippy` (MSRV is 1.85).
- `cargo-deny`, `cargo-machete`, `cargo-c`, `typos-cli`, `taplo-cli`: prebuilt
  release binaries, or `cargo install --locked <name>`.
- `shellcheck`, `actionlint`, `python3`.
- On Linux and macOS, `qpdf` and poppler's `pdftotext`, which check the PDFs we write
  (`apt install qpdf poppler-utils`, `brew install qpdf poppler`).
- A C compiler: `cc`/`c++` with AddressSanitizer and UBSan on Linux and macOS,
  MSVC (`cl`, from a Developer Command Prompt) on Windows. On Windows, run the script
  from Git Bash.
- For `scripts/nightly.sh`: a nightly toolchain with Miri
  (`rustup toolchain install nightly --component miri`).

Enable the pre-commit hook (format, clippy, tests) once per clone:

```sh
git config core.hooksPath .githooks
```

## Definition of done

Every change must meet all of these:

1. `scripts/check.sh` passes locally.
2. New behaviour has tests. A bug fix has a regression test that failed before the fix.
3. Every new public item has a doc comment. Non-trivial public APIs have a runnable
   doc example.
4. A new parser comes with a fuzz target in `fuzz/`.
5. Every public Rust feature that users need is exposed through the C ABI in the
   same pull request, with the header regenerated and covered by `tests/c` and
   `tests/ffi-python`.
6. No check is weakened to make it pass: no `#[allow]` without a `// reason:`
   comment, no skipped tests, no lowered lint levels.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), with the crate as
scope:

```
feat(pdf): write xref streams
fix(ffi): free buffer on error path
docs(readme): describe the C ABI
ci: cache the external corpus
```

Breaking changes use `!` (`feat(pdf)!: ...`). One logical change per commit.

## Pull requests

Open a pull request as a draft. The template lists what its description needs: what
changed and why, the risks, new dependencies, and how to check the change by hand. The
title becomes the commit message on `main`, so it follows the commit convention above.

## Test documents

- `tests/corpus/` holds small real documents (each under 1 MB) chosen for edge
  cases. Only add a file whose license allows redistribution, and list it in
  `tests/corpus/SOURCES.md` with its origin and license. Never commit personal,
  customer or copyrighted documents.
- Large external corpora are not committed. `scripts/fetch-corpus.sh` downloads them
  into `corpus-ext/` (gitignored), pinned by the manifest `scripts/corpus-ext.tsv`.
