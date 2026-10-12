#!/usr/bin/env bash
# Builds the release archive of the C library: officewright-<version>-<target>.tar.gz,
# or .zip for Windows, with the layout of scripts/install-capi.sh.
# Usage: scripts/package.sh <version> <out-dir> [<target>]
#   <target> defaults to the host; give it to cross-compile.
set -euo pipefail
cd "$(dirname "$0")/.."

[ $# -ge 2 ] && [ $# -le 3 ] || {
	echo "usage: $0 <version> <out-dir> [<target>]" >&2
	exit 2
}
version=$1
mkdir -p "$2"
out=$(cd "$2" && pwd)
target=${3:-$(rustc -vV | sed -n 's/^host: //p')}
target_args=()
[ $# -eq 2 ] || target_args=(--target "$target")

name=officewright-$version-$target
stage=target/package/$name
rm -rf "$stage"
scripts/install-capi.sh "$stage" --release ${target_args[@]+"${target_args[@]}"}

cd "$stage"
case $target in
*-windows-*)
	python -m zipfile -c "$(cygpath -w "$out/$name.zip")" ./*
	echo "$out/$name.zip"
	;;
*)
	# COPYFILE_DISABLE keeps macOS tar from adding AppleDouble ._ files.
	COPYFILE_DISABLE=1 tar -czf "$out/$name.tar.gz" ./*
	echo "$out/$name.tar.gz"
	;;
esac
