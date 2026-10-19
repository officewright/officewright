#!/usr/bin/env bash
# Long-running checks, run on a schedule by .github/workflows/nightly.yml.
#
#   scripts/nightly.sh [all]             all of it, the fuzz targets one after another
#   scripts/nightly.sh checks            Miri, the corpus pins, the corpus runner, the test fonts
#   scripts/nightly.sh fuzz [TARGET...]  fuzzing, 30 minutes per target (by default every target)
#   scripts/nightly.sh fuzz-targets      the fuzz targets as a JSON array
#
# nightly.yml runs the checks in one job and each fuzz target that fuzz-targets lists in a
# job of its own: one after another, the eleven targets alone took 5.5 of the 6 hours that
# GitHub allows a job.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
	echo "usage: scripts/nightly.sh [all | checks | fuzz [TARGET...] | fuzz-targets]" >&2
	exit 2
}

# Failures that do not stop the run; reported at the end.
corpus_failed=
fonts_failed=
crashed=

checks() {
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
	# the run but does not stop the parts after it.
	if scripts/fetch-corpus.sh poi libreoffice && cargo build --release -p corpus-runner; then
		# The committed POI documents are unmodified copies of the fetched ones.
		for file in tests/corpus/poi/*.docx; do
			if ! cmp -s "$file" "corpus-ext/poi/test-data/document/${file##*/}"; then
				echo "nightly.sh: $file is not POI's file of that name" >&2
				corpus_failed=1
			fi
		done
		target/release/corpus-runner run --enforce-memory-limit \
			--expected tools/corpus-runner/expected-errors.tsv \
			--baseline tools/corpus-runner/baseline-ext.json --allow-better \
			--report target/corpus-report.json corpus-ext/poi corpus-ext/libreoffice ||
			corpus_failed=1
	else
		corpus_failed=1
	fi

	printf '==> %s\n' "test fonts (upstream sources)"
	# Each file in tests/fonts and tests/fonts-word against the upstream source
	# THIRD-PARTY-NOTICES.md names for it. A failure, or a download that fails, fails the run but does not stop the parts
	# after it.
	scripts/check-test-fonts.sh || fonts_failed=1
}

# Checked before anything runs, so a broken toolchain fails the run instead of skipping
# the fuzzing.
require_cargo_fuzz() {
	cargo +nightly fuzz --version >/dev/null 2>&1 || {
		echo "nightly.sh: cargo fuzz is missing: cargo install --locked cargo-fuzz" >&2
		exit 1
	}
}

# One per line. Called in a statement of its own, so that `set -e` stops the script if it
# fails; inside the command substitution that calls it, bash turns `set -e` off.
fuzz_targets() {
	list=$(cargo +nightly fuzz list) || exit 1
	if [ -z "$list" ]; then
		echo "nightly.sh: cargo fuzz list found no targets" >&2
		exit 1
	fi
	echo "$list"
}

# Each target is seeded with files of the format it reads.
seeds() {
	case $1 in
	docx_open) echo tests/corpus/word-rules tests/corpus/resaved tests/corpus/poi crates/officewright-doc/tests/data ;;
	font_* | pdf_embed | shape_font) echo tests/fonts crates/officewright-font/tests/data ;;
	jpeg_header) echo fuzz/seeds/jpeg ;;
	layout_shape) echo tests/pdf-output fuzz/seeds/layout_shape ;;
	package) echo fuzz/seeds/package ;;
	png_decode) echo fuzz/seeds/png tests/corpus/reference ;;
	xml) echo fuzz/seeds/xml ;;
	# Word's re-saved copies, POI's files and the business documents hold DEFLATE streams;
	# hello.docx and the rule documents are stored (#10).
	zip_archive) echo crates/officewright-opc/tests/data tests/corpus/resaved tests/corpus/poi tests/corpus/business ;;
	*)
		echo "nightly.sh: no seed directories for fuzz target $1; add them to seeds()" >&2
		return 1
		;;
	esac
}

# libFuzzer keeps the inputs it finds in fuzz/corpus/<target> and writes crashing inputs to
# fuzz/artifacts/<target>, which nightly.yml uploads. The limits follow the corpus rules of
# CLAUDE.md: 30 s per input, and 1 GiB for what an input asks for at once
# (-malloc_limit_mb). The process may grow to 2.5 GiB, the limit OSS-Fuzz runs with
# (-rss_limit_mb), because it holds more than the input's own memory: libFuzzer's corpus,
# AddressSanitizer's quarantine of freed memory, and AddressSanitizer's overhead on every
# allocation. With the fonts as seeds the process passed 1 GiB within a quarter of an hour
# while no input took more than 50 MB, and layout_shape, whose seeds take up to 200 MB each
# without AddressSanitizer, passed it in under three minutes. A crash in one target does
# not stop the others. The seeds are looked up in a statement of their own, so that
# `set -e` stops the script if a target has none.
fuzz() {
	targets=$*
	if [ -z "$targets" ]; then
		targets=$(fuzz_targets)
	fi
	printf '==> %s\n' "fuzzing (30 min per target)"
	for target in $targets; do
		mkdir -p "fuzz/corpus/$target"
		target_seeds=$(seeds "$target")
		read -r -a dirs <<<"$target_seeds"
		cargo +nightly fuzz run "$target" "fuzz/corpus/$target" "${dirs[@]}" -- \
			-max_total_time=1800 -timeout=30 -malloc_limit_mb=1024 -rss_limit_mb=2560 ||
			crashed="$crashed $target"
	done
}

# For the matrix of nightly.yml. Target names are Cargo binary names; one that JSON would
# need to escape is refused rather than escaped. The letters are listed one by one, because
# a range such as a-z can also match other letters, depending on the locale and bash.
fuzz_targets_json() {
	targets=$(fuzz_targets)
	json=
	for target in $targets; do
		case $target in
		*[!abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-]*)
			echo "nightly.sh: fuzz target name $target is not letters, digits, - and _" >&2
			exit 1
			;;
		esac
		json="$json${json:+,}\"$target\""
	done
	printf '[%s]\n' "$json"
}

command=${1-all}
[ $# -eq 0 ] || shift
case $command in
all)
	[ $# -eq 0 ] || usage
	require_cargo_fuzz
	checks
	fuzz
	;;
checks)
	[ $# -eq 0 ] || usage
	checks
	;;
fuzz)
	require_cargo_fuzz
	fuzz "$@"
	;;
fuzz-targets)
	[ $# -eq 0 ] || usage
	require_cargo_fuzz
	fuzz_targets_json
	;;
*) usage ;;
esac

failed=
if [ -n "$crashed" ]; then
	echo "nightly.sh: fuzzing failed in:$crashed (inputs in fuzz/artifacts/)" >&2
	failed=1
fi
if [ -n "$corpus_failed" ]; then
	echo "nightly.sh: the corpus checks failed (messages above; report in target/corpus-report.json)" >&2
	failed=1
fi
if [ -n "$fonts_failed" ]; then
	echo "nightly.sh: the test fonts differ from their upstream files, or could not be checked (messages above)" >&2
	failed=1
fi
if [ -n "$failed" ]; then
	exit 1
fi
