#!/usr/bin/env bash
# Checks that every file in tests/fonts/ and tests/fonts-word/ is byte for byte the file of the
# upstream source THIRD-PARTY-NOTICES.md names for it. tests/corpus-tools/test_test_fonts.py
# compares the files with the SHA-256 the notices list, and that the pins below are the
# notices'; this compares them with upstream. Liberation comes from its release archive,
# whose SHA-256 is checked first; Carlito and Noto Sans (its CFF and TrueType builds) from
# shallow, partial, sparse clones of the pinned commits, which git verifies. Every entry of
# both folders must be a regular file. Needs network, curl, tar and git. Run by
# scripts/nightly.sh.
set -euo pipefail
cd "$(dirname "$0")/.."

liberation_url=https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz
liberation_sha256=7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0
carlito_url=https://github.com/googlefonts/carlito
carlito_commit=3a810cab78ebd6e2e4eed42af9e8453c4f9b850a
noto_url=https://github.com/notofonts/notofonts.github.io
noto_commit=28b15b4b43b7bed62b5cf6e6b0b5ff5846270535

failed=
fail() {
	echo "check-test-fonts.sh: $*" >&2
	failed=1
}
# A download or clone that does not succeed: nothing can be compared.
cannot() {
	echo "check-test-fonts.sh: cannot $*; the test fonts were not checked" >&2
	exit 1
}

# The pins are the ones the notices give.
for pin in "$liberation_url" "$liberation_sha256" "$carlito_url" "$carlito_commit" \
	"$noto_url" "$noto_commit"; do
	grep -qF "$pin" THIRD-PARTY-NOTICES.md || fail "$pin is not in THIRD-PARTY-NOTICES.md"
done

sha256() {
	if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Every entry but directories, links among them, so that none is left out unseen; a
# failing find stops the script here.
find tests/fonts tests/fonts-word ! -type d -print0 | sort -z >"$work/files"

# Time limits, so that a server that does not answer cannot hold up the night: a transfer
# slower than 1 KB/s for a minute is given up, and the download after about 10 minutes in
# all. A transfer takes at most 5 minutes, and no retry starts after 5 minutes, though one
# that curl decided on before then starts after its wait (1, 2 or 4 seconds) and runs its
# full 5.
curl -fsSL --retry 3 --retry-max-time 300 --connect-timeout 30 --max-time 300 \
	--speed-limit 1000 --speed-time 60 -o "$work/liberation.tar.gz" \
	"$liberation_url" || cannot "download $liberation_url"
got=$(sha256 "$work/liberation.tar.gz")
if [ "$got" != "$liberation_sha256" ]; then
	echo "check-test-fonts.sh: the Liberation archive has SHA-256 $got, not $liberation_sha256" >&2
	exit 1
fi
tar -xzf "$work/liberation.tar.gz" -C "$work"

clone() { # directory url commit paths...
	local dir=$1 url=$2 commit=$3
	shift 3
	git init -q "$dir"
	git -C "$dir" remote add origin "$url"
	git -C "$dir" sparse-checkout set --no-cone "$@"
	# A transfer slower than 1 KB/s for a minute is given up.
	git -C "$dir" -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 \
		fetch -q --depth 1 --filter=blob:none origin "$commit" ||
		cannot "fetch $url at $commit"
	git -C "$dir" -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 \
		checkout -q --detach FETCH_HEAD || cannot "check out $url at $commit"
	[ "$(git -C "$dir" rev-parse HEAD)" = "$commit" ] || cannot "get commit $commit from $url"
}
clone "$work/carlito" "$carlito_url" "$carlito_commit" /fonts/ttf/ /OFL.txt
clone "$work/noto" "$noto_url" "$noto_commit" /fonts/NotoSans/unhinted/otf/NotoSans-Regular.otf \
	/fonts/NotoSans/unhinted/ttf/NotoSans-Regular.ttf /fonts/NotoSans/unhinted/ttf/NotoSans-Bold.ttf \
	/fonts/NotoSans/unhinted/ttf/NotoSans-BoldItalic.ttf /fonts/LICENSE

checked=0
while IFS= read -r -d '' file; do
	checked=$((checked + 1))
	if [ -L "$file" ] || [ ! -f "$file" ]; then
		fail "$file is not a regular file; the test fonts are committed as they are"
		continue
	fi
	name=${file##*/}
	case $file in
	tests/fonts/liberation/*) upstream=$work/liberation-fonts-ttf-2.1.5/$name ;;
	tests/fonts/carlito/OFL.txt) upstream=$work/carlito/OFL.txt ;;
	tests/fonts/carlito/*) upstream=$work/carlito/fonts/ttf/$name ;;
	tests/fonts/noto/LICENSE | tests/fonts-word/noto-truetype/LICENSE) upstream=$work/noto/fonts/LICENSE ;;
	tests/fonts/noto/*) upstream=$work/noto/fonts/NotoSans/unhinted/otf/$name ;;
	tests/fonts-word/noto-truetype/*) upstream=$work/noto/fonts/NotoSans/unhinted/ttf/$name ;;
	*)
		fail "$file has no upstream source in this script"
		continue
		;;
	esac
	if [ ! -f "$upstream" ]; then
		fail "$file: ${upstream#"$work"/} was not fetched: upstream has no such file, or this script does not fetch it"
	elif ! cmp -s "$file" "$upstream"; then
		fail "$file differs from upstream ${upstream#"$work"/}"
	fi
done <"$work/files"

[ "$checked" -gt 0 ] || fail "no files in tests/fonts and tests/fonts-word"
[ -z "$failed" ] || exit 1
echo "check-test-fonts.sh: all $checked files in tests/fonts and tests/fonts-word are upstream's"
