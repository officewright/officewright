#!/usr/bin/env bash
# Long-running checks, run on a schedule by .github/workflows/nightly.yml.
set -euo pipefail
cd "$(dirname "$0")/.."

# Checked up front, so a broken toolchain fails the run instead of skipping a check.
cargo +nightly fuzz --version >/dev/null 2>&1 || {
	echo "nightly.sh: cargo fuzz is missing: cargo install --locked cargo-fuzz" >&2
	exit 1
}

printf '==> %s\n' "miri (officewright-ffi)"
# The tests write PDF files, which Miri allows only without isolation.
MIRIFLAGS=-Zmiri-disable-isolation cargo +nightly miri test -p officewright-ffi

printf '==> %s\n' "external corpus pins"
# Downloads nothing. The full fetch with SHA-256 checks runs weekly (.github/workflows/weekly.yml).
scripts/fetch-corpus.sh --check all

printf '==> %s\n' "corpus runner (Word documents of corpus-ext)"
# The Word documents of Apache POI and LibreOffice: git sources, so fetching them verifies
# them. Each child gets the 1 GiB address-space limit of CLAUDE.md through prlimit; the
# report is uploaded by nightly.yml. The run fails on a panic, hang, memory blow-up or
# crash, and on a file worse than tools/corpus-runner/baseline-ext.json; a better file
# only shows in the output until the baseline is updated. A failed fetch or build fails
# the night but does not stop the fuzzing.
corpus_failed=
if scripts/fetch-corpus.sh poi libreoffice && cargo build --release -p corpus-runner; then
	target/release/corpus-runner run --enforce-memory-limit \
		--expected tools/corpus-runner/expected-errors.tsv \
		--baseline tools/corpus-runner/baseline-ext.json --allow-better \
		--report target/corpus-report.json corpus-ext/poi corpus-ext/libreoffice ||
		corpus_failed=1
else
	corpus_failed=1
fi

printf '==> %s\n' "fuzzing (30 min per target)"
# Each target is seeded with files of the format it reads; libFuzzer keeps the inputs it
# finds in fuzz/corpus/<target> and writes crashing inputs to fuzz/artifacts/<target>,
# which nightly.yml uploads. The limits are the corpus rules of CLAUDE.md: 30 s per
# input, 1 GiB of memory. A crash in one target does not stop the others. The targets
# and their seeds are looked up in statements of their own, so that `set -e` stops the
# script if either fails.
seeds() {
	case $1 in
	docx_open) echo tests/corpus/word-rules tests/corpus/resaved crates/officewright-doc/tests/data ;;
	font_* | pdf_embed) echo tests/fonts crates/officewright-font/tests/data ;;
	package) echo fuzz/seeds/package ;;
	xml) echo fuzz/seeds/xml ;;
	zip_archive) echo crates/officewright-opc/tests/data ;;
	*)
		echo "nightly.sh: no seed directories for fuzz target $1; add them to seeds()" >&2
		return 1
		;;
	esac
}
targets=$(cargo +nightly fuzz list)
if [ -z "$targets" ]; then
	echo "nightly.sh: cargo fuzz list found no targets" >&2
	exit 1
fi
crashed=
for target in $targets; do
	mkdir -p "fuzz/corpus/$target"
	target_seeds=$(seeds "$target")
	read -r -a dirs <<<"$target_seeds"
	cargo +nightly fuzz run "$target" "fuzz/corpus/$target" "${dirs[@]}" -- \
		-max_total_time=1800 -timeout=30 -rss_limit_mb=1024 ||
		crashed="$crashed $target"
done
failed=
if [ -n "$crashed" ]; then
	echo "nightly.sh: fuzzing failed in:$crashed (inputs in fuzz/artifacts/)" >&2
	failed=1
fi
if [ -n "$corpus_failed" ]; then
	echo "nightly.sh: the corpus runner failed (report in target/corpus-report.json)" >&2
	failed=1
fi
if [ -n "$failed" ]; then
	exit 1
fi
