#!/usr/bin/env bash
# Installs the pinned prebuilt cargo-c into <bin-dir>, for CI and release builds.
# Usage: scripts/install-cargo-c.sh <bin-dir>
set -euo pipefail

[ $# -eq 1 ] || {
	echo "usage: $0 <bin-dir>" >&2
	exit 2
}
bin_dir=$1
base=https://github.com/lu-zero/cargo-c/releases/download/v0.10.25
case "$(uname -s)-$(uname -m)" in
Linux-x86_64)
	file=cargo-c-x86_64-unknown-linux-musl.tar.gz
	sha256=2fbf0d1b56a9a0b52431c2a159d9544b4b0729f2eabaed3904a41cce0e8cd4b0
	;;
Linux-aarch64)
	file=cargo-c-aarch64-unknown-linux-musl.tar.gz
	sha256=8df4475f48d7dc4d6a7fcd60bf96d569e5dabc24dce98a4a189419822c594257
	;;
Darwin-arm64)
	file=cargo-c-macos.zip
	sha256=0e73d1841be8a582ab9c37e1049f6a2fa352b3a40b0f97276603f987b19f64e8
	;;
MINGW*-* | MSYS*-*)
	# x86_64; Windows on Arm runs it under emulation.
	file=cargo-c-windows-msvc.zip
	sha256=f5fc4ae9e973fdec1d905ca7f44a614b33941e40216322a51a681a74fa99223e
	;;
*)
	echo "install-cargo-c.sh: no prebuilt cargo-c for $(uname -s) $(uname -m)" >&2
	exit 1
	;;
esac

download=$(mktemp -d)
trap 'rm -rf "$download"' EXIT
curl -fsSL -o "$download/$file" "$base/$file"
if command -v sha256sum >/dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
(cd "$download" && echo "$sha256  $file" | "${sum[@]}" -c -)
mkdir -p "$bin_dir"
case $file in
*.zip) unzip -oq "$download/$file" -d "$bin_dir" ;;
*) tar -xzf "$download/$file" -C "$bin_dir" ;;
esac
