#!/usr/bin/env bash
# Downloads the external test corpora listed in scripts/corpus-ext.tsv into corpus-ext/.
#
# Usage: scripts/fetch-corpus.sh [--check] [all | NAME...]
#   (no arguments)  every git source
#   all             every source, including the large archives
#   NAME...         only the named sources
#   --check         download nothing; only confirm that every pinned commit can still be
#                   fetched and every archive still has its pinned size
#
# Git sources are shallow, partial, sparse clones of a pinned commit, so git itself
# verifies their content. Archives are verified against their pinned SHA-256.
# A source that is already at its pin is skipped.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dest="$root/corpus-ext"
python=${PYTHON:-python3}

die() {
	echo "fetch-corpus: $*" >&2
	exit 1
}

failed=
report() {
	echo "fetch-corpus: $*" >&2
	failed=1
}

sha256() {
	if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

fetch_git() { # name url commit paths
	local dir="$dest/$1"
	if [ "$(git -C "$dir" rev-parse -q --verify HEAD 2>/dev/null)" = "$3" ]; then
		echo "$1: up to date"
		return
	fi
	echo "$1: fetching $2 at $3"
	rm -rf "$dir" "$dir.tmp"
	git init -q "$dir.tmp"
	git -C "$dir.tmp" config gc.auto 0
	git -C "$dir.tmp" remote add origin "$2"
	if [ "$4" != . ]; then
		# shellcheck disable=SC2086 # $4 is a space-separated list of paths
		git -C "$dir.tmp" sparse-checkout set $4
	fi
	git -C "$dir.tmp" fetch -q --depth 1 --filter=blob:none origin "$3"
	git -C "$dir.tmp" checkout -q --detach FETCH_HEAD
	[ "$(git -C "$dir.tmp" rev-parse HEAD)" = "$3" ] || die "$1: fetched commit is not $3"
	mv "$dir.tmp" "$dir"
}

fetch_archive() { # name url sha256
	local dir="$dest/$1" zip="$dest/$1.zip" got
	if [ "$(cat "$dir/.sha256" 2>/dev/null)" = "$3" ]; then
		echo "$1: up to date"
		return
	fi
	echo "$1: downloading $2"
	rm -rf "$dir" "$dir.tmp" "$zip"
	curl -fL --retry 3 -o "$zip" "$2"
	got=$(sha256 "$zip")
	[ "$got" = "$3" ] || die "$1: sha256 mismatch: manifest has $3, download has $got"
	# -I: never import modules from the current directory while handling downloaded data.
	"$python" -I -m zipfile -e "$zip" "$dir.tmp"
	rm "$zip"
	echo "$3" >"$dir.tmp/.sha256"
	mv "$dir.tmp" "$dir"
}

check_git() { # name url commit
	local tmp ok=
	tmp=$(mktemp -d)
	git init -q "$tmp"
	# --filter=tree:0 fetches the commit object alone, a few KB.
	git -C "$tmp" fetch -q --depth 1 --filter=tree:0 "$2" "$3" && ok=1
	rm -rf "$tmp"
	if [ -n "$ok" ]; then echo "$1: commit $3 is fetchable"; else report "$1: cannot fetch commit $3 from $2"; fi
}

check_archive() { # name url size
	local size
	size=$(curl -fsSIL --retry 3 "$2" | tr -d '\r' |
		awk 'tolower($1) == "content-length:" { n = $2 } END { print n }') || size=
	if [ "$size" = "$3" ]; then echo "$1: $3 bytes, as pinned"; else report "$1: $2 has Content-Length '$size', manifest has $3"; fi
}

mode=fetch
if [ "${1:-}" = --check ]; then
	mode=check
	shift
fi

manifest="$root/scripts/corpus-ext.tsv"
names=" $(grep -v '^#' "$manifest" | cut -f1 | tr '\n' ' ')"
for arg in "$@"; do
	case $arg in all) continue ;; esac
	case $names in *" $arg "*) ;; *) die "unknown source '$arg'" ;; esac
done

mkdir -p "$dest"
want=" $* "
while IFS=$'\t' read -r name kind url pin detail _license <&3; do
	case $name in '#'* | '') continue ;; esac
	case $name in *[!a-z0-9-]*) die "invalid source name '$name' in manifest" ;; esac
	if [ $# -eq 0 ]; then
		[ "$kind" = git ] || continue
	elif [ "$want" != " all " ]; then
		case $want in *" $name "*) ;; *) continue ;; esac
	fi
	case $mode-$kind in
	fetch-git) fetch_git "$name" "$url" "$pin" "$detail" ;;
	fetch-archive) fetch_archive "$name" "$url" "$pin" ;;
	check-git) check_git "$name" "$url" "$pin" ;;
	check-archive) check_archive "$name" "$url" "$detail" ;;
	*) die "$name: unknown kind '$kind'" ;;
	esac
done 3<"$manifest"
[ -z "$failed" ] || exit 1
