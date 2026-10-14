#!/usr/bin/env bash
# Checks that every file in tests/fonts/ is byte for byte the file of the upstream source
# THIRD-PARTY-NOTICES.md names for it. tests/corpus-tools/test_test_fonts.py compares the
# files with the SHA-256 the notices list; this compares them with upstream. Liberation
# comes from its release archive, whose SHA-256 is checked first; Carlito and Noto Sans
# from shallow, partial, sparse clones of the pinned commits, which git verifies.
# Needs network, curl, tar and git. Run by scripts/nightly.sh.
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

curl -fsSL --retry 3 -o "$work/liberation.tar.gz" "$liberation_url"
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
	git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$commit"
	git -C "$dir" checkout -q --detach FETCH_HEAD
	[ "$(git -C "$dir" rev-parse HEAD)" = "$commit" ] || {
		echo "check-test-fonts.sh: $url did not give commit $commit" >&2
		exit 1
	}
}
clone "$work/carlito" "$carlito_url" "$carlito_commit" /fonts/ttf/ /OFL.txt
clone "$work/noto" "$noto_url" "$noto_commit" /fonts/NotoSans/unhinted/otf/NotoSans-Regular.otf /fonts/LICENSE

checked=0
while IFS= read -r -d '' file; do
	name=${file##*/}
	case $file in
	tests/fonts/liberation/*) upstream=$work/liberation-fonts-ttf-2.1.5/$name ;;
	tests/fonts/carlito/OFL.txt) upstream=$work/carlito/OFL.txt ;;
	tests/fonts/carlito/*) upstream=$work/carlito/fonts/ttf/$name ;;
	tests/fonts/noto/LICENSE) upstream=$work/noto/fonts/LICENSE ;;
	tests/fonts/noto/*) upstream=$work/noto/fonts/NotoSans/unhinted/otf/$name ;;
	*)
		fail "$file has no upstream source in this script"
		continue
		;;
	esac
	if [ ! -f "$upstream" ]; then
		fail "$file: upstream has no ${upstream#"$work"/}"
	elif ! cmp -s "$file" "$upstream"; then
		fail "$file differs from upstream ${upstream#"$work"/}"
	fi
	checked=$((checked + 1))
done < <(find tests/fonts -type f -print0 | sort -z)

[ "$checked" -gt 0 ] || fail "no files in tests/fonts"
[ -z "$failed" ] || exit 1
echo "check-test-fonts.sh: all $checked files in tests/fonts are upstream's"
