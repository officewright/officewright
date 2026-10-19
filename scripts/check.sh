#!/usr/bin/env bash
# Quality gate for every pull request. CI runs exactly this script on Linux, macOS and
# Windows (Git Bash with the MSVC environment loaded). It stops at the first failure.
set -euo pipefail
cd "$(dirname "$0")/.."

fail() {
	echo "check.sh: $*" >&2
	exit 1
}
step() { printf '\n==> %s\n' "$1"; }

host=$(rustc -vV | sed -n 's/^host: //p')
case $host in
*-linux-*) os=linux python=python3 ;;
*-apple-darwin) os=macos python=python3 ;;
*-windows-msvc) os=windows python=python ;;
*) fail "unsupported host $host" ;;
esac

# Every tool is checked up front, so a fresh machine sees the whole list at once.
missing=
need() { # command, install hint
	command -v "$1" >/dev/null 2>&1 || missing="$missing  $1: $2\n"
}
need_cargo() { # cargo subcommand, install hint
	cargo "$1" --version >/dev/null 2>&1 || missing="$missing  cargo $1: $2\n"
}
need_cargo fmt "rustup component add rustfmt"
need_cargo clippy "rustup component add clippy"
need_cargo deny "cargo install --locked cargo-deny"
need_cargo machete "cargo install --locked cargo-machete"
need_cargo cinstall "cargo install --locked cargo-c"
need typos "cargo install --locked typos-cli"
need taplo "cargo install --locked taplo-cli"
need shellcheck "apt install shellcheck | brew install shellcheck | https://github.com/koalaman/shellcheck/releases"
need actionlint "https://github.com/rhysd/actionlint/releases"
need "$python" "https://www.python.org/downloads/"
case $os in
linux)
	need qpdf "apt install qpdf"
	need pdftotext "apt install poppler-utils"
	need pdffonts "apt install poppler-utils"
	need pdftoppm "apt install poppler-utils"
	need cc "apt install gcc"
	need c++ "apt install g++"
	need nm "apt install binutils"
	need readelf "apt install binutils"
	;;
macos)
	need qpdf "brew install qpdf"
	need pdftotext "brew install poppler"
	need pdffonts "brew install poppler"
	need pdftoppm "brew install poppler"
	need cc "xcode-select --install"
	need c++ "xcode-select --install"
	need nm "xcode-select --install"
	need otool "xcode-select --install"
	need install_name_tool "xcode-select --install"
	need codesign "xcode-select --install"
	;;
windows)
	need powershell.exe "Windows PowerShell 5.1 ships with Windows; check that C:\\Windows\\System32\\WindowsPowerShell\\v1.0 is on PATH"
	need cl "run from a Visual Studio Developer Command Prompt (vcvars64.bat)"
	need dumpbin "run from a Visual Studio Developer Command Prompt (vcvars64.bat)"
	need cygpath "run from Git Bash"
	;;
esac
if [ -n "$missing" ]; then
	printf 'check.sh: missing tools:\n%b' "$missing" >&2
	exit 1
fi

if [ "$os" = windows ]; then
	# In a Developer environment rustc runs the first link.exe on PATH, and Git Bash puts
	# its coreutils link.exe first. Point cargo at MSVC's, which sits next to cl.exe.
	linker_var=CARGO_TARGET_$(printf '%s' "$host" | tr 'a-z-' 'A-Z_')_LINKER
	export "$linker_var=$(cygpath -w "$(dirname "$(command -v cl)")/link.exe")"
fi

step "rustfmt"
cargo fmt --all --check

step "clippy"
cargo clippy --workspace --all-targets --all-features -- -D warnings

step "officewright-doc without its features (clippy)"
# The crate's tests also build without the feature pdf, as `cargo test -p officewright-doc`
# builds them (#354).
cargo clippy -p officewright-doc --all-targets -- -D warnings

step "rustdoc"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

step "tests"
cargo test --workspace --all-features

# fuzz/ is a cargo workspace of its own, so the steps above do not reach it. cargo-fuzz
# runs on Unix-like systems only; nightly.sh runs the targets.
if [ "$os" != windows ]; then
	step "fuzz targets (rustfmt, clippy)"
	cargo fmt --manifest-path fuzz/Cargo.toml --check
	cargo clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
fi

step "cargo deny"
cargo deny check

step "cargo machete"
cargo machete

step "published crates leave out third-party data"
# Adobe's AFM files, the test fonts and the tests that read them are not under
# Officewright's license; see THIRD-PARTY-NOTICES.md. The test fonts live outside every crate.
packaged=$(cargo package --list --allow-dirty --quiet -p officewright-pdf)
if printf '%s\n' "$packaged" | grep -E '^(data/afm/|tests/(helvetica_afm|embedded_fonts|glyph_runs)\.rs$)'; then
	fail "officewright-pdf would publish the files above"
fi
packaged=$(cargo package --list --allow-dirty --quiet -p officewright-font)
if printf '%s\n' "$packaged" | grep -E '^tests/(metric_compatible|third_party_fonts)\.rs$'; then
	fail "officewright-font would publish the file above"
fi
packaged=$(cargo package --list --allow-dirty --quiet -p officewright-layout)
if printf '%s\n' "$packaged" | grep -E '^(data/|tests/(shaping|fonts|symbol_table)\.rs$)'; then
	fail "officewright-layout would publish the file above"
fi
packaged=$(cargo package --list --allow-dirty --quiet -p officewright-cli)
if printf '%s\n' "$packaged" | grep -E '^tests/fonts\.rs$'; then
	fail "officewright-cli would publish the file above"
fi

step "typos"
typos

step "taplo"
RUST_LOG=warn taplo fmt --check

step "shellcheck"
shellcheck scripts/*.sh .githooks/pre-commit

step "actionlint"
actionlint

step "committed files: no Python bytecode, no home directory paths"
scripts/check-added-files.sh
"$python" -B -m unittest discover -s tests/scripts -v

step "corpus tools (tests/corpus-tools)"
# The committed rule documents must be what scripts/make-rule-docs.py writes.
"$python" -B -m unittest discover -s tests/corpus-tools -v

# make-reference.ps1 is written for Windows PowerShell 5.1, which exists only on Windows.
if [ "$os" = windows ]; then
	step "make-reference.ps1 without Word (Windows PowerShell)"
	powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File tests/corpus-tools/test_make_reference.ps1
fi

step "corpus runner over tests/corpus (against tests/corpus/baseline.json)"
# Every committed Word document opens and is rendered into PDF in a child process, with the
# fonts of tests/fonts-word/noto-truetype and tests/fonts and none of the system's; a panic, hang, memory blow-up, a document
# that opens but cannot be rendered, or any change of a file's status from the baseline
# fails. On Linux each document with a Word reference is also scored, and a score that
# moves by more than 0.01 fails; macOS and Windows compare the statuses only, since
# rasterizing differs between poppler builds and Windows has no official one. An
# improvement is accepted by running this on Linux with --update-baseline and committing
# the baseline.
# The TrueType Noto Sans comes first: of two faces of one family and style, the font book
# takes the one added first, and the references that use Noto Sans were rendered by Word
# with that build, but for E-line-fonts (THIRD-PARTY-NOTICES.md).
rm -rf target/corpus-pdf
corpus_options=(--render target/corpus-pdf --font-dir tests/fonts-word/noto-truetype --font-dir tests/fonts)
if [ "$os" = linux ]; then
	corpus_options+=(--references tests/corpus/reference)
else
	corpus_options+=(--statuses-only)
fi
cargo run --quiet -p corpus-runner -- run --baseline tests/corpus/baseline.json \
	--expected tools/corpus-runner/expected-errors.tsv "${corpus_options[@]}" tests/corpus

step "C library (cargo cinstall)"
# The same layout the release archives ship; scripts/check-capi.sh tests it as a user would.
rm -rf target/capi
scripts/install-capi.sh target/capi --debug

step "header is up to date"
header=crates/officewright-ffi/include/officewright.h
cp target/capi/include/officewright.h "$header"
git ls-files --error-unmatch "$header" >/dev/null 2>&1 || fail "$header is not committed"
git diff --exit-code -- "$header" || fail "$header was stale and has been regenerated; commit it"

scripts/check-capi.sh target/capi

# The PDF bytes do not depend on the platform (the golden snapshot test checks them on
# every OS), and Windows has no official poppler build, so this runs on Linux and macOS.
if [ "$os" != windows ]; then
	step "PDF output (qpdf, pdftotext, pdffonts, pdfimages, pdftoppm)"
	cargo build -p officewright-cli
	cargo build -p officewright-pdf --example primitives --example images --example embed_report
	OW_CLI=${CARGO_TARGET_DIR:-target}/debug/officewright \
		OW_PRIMITIVES=${CARGO_TARGET_DIR:-target}/debug/examples/primitives \
		OW_IMAGES=${CARGO_TARGET_DIR:-target}/debug/examples/images \
		OW_EMBED_REPORT=${CARGO_TARGET_DIR:-target}/debug/examples/embed_report \
		"$python" -B -m unittest discover -s tests/pdf-output -v

	step "reference font embedding (qpdf, pdffonts, pdftoppm)"
	# corpus-import.py embeds the fonts Word did not embed before rasterizing its PDFs.
	"$python" -B -m unittest discover -s tests/corpus-tools/poppler -v
fi

printf '\nAll checks passed.\n'
