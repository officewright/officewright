#!/usr/bin/env bash
# Installs the C library into a prefix laid out exactly like a release archive:
#   include/officewright.h, lib/ (shared, static and import libraries), lib/pkgconfig/,
#   and the license files. The prefix can be moved anywhere: officewright.pc finds its
#   files relative to itself and the macOS dylib is identified by @rpath.
# Usage: scripts/install-capi.sh <prefix> [cargo cinstall options, e.g. --debug]
#   <prefix> must be new or empty; the script never deletes anything.
set -euo pipefail
cd "$(dirname "$0")/.."

[ $# -ge 1 ] || {
	echo "usage: $0 <prefix> [cargo cinstall options]" >&2
	exit 2
}
prefix=$1
shift
if [ -e "$prefix" ] && [ -n "$(ls -A "$prefix")" ]; then
	echo "install-capi.sh: $prefix is not empty; give a new or empty directory" >&2
	exit 1
fi
mkdir -p "$prefix"
prefix=$(cd "$prefix" && pwd)
cargo_prefix=$prefix
case $(uname -s) in
MINGW* | MSYS* | CYGWIN*)
	cargo_prefix=$(cygpath -m "$prefix")
	# As in check.sh: in a Developer environment rustc runs the first link.exe on PATH,
	# and Git Bash puts its coreutils link.exe first. Use MSVC's, next to cl.exe.
	host=$(rustc -vV | sed -n 's/^host: //p')
	export "CARGO_TARGET_$(printf '%s' "$host" | tr 'a-z-' 'A-Z_')_LINKER=$(cygpath -w "$(dirname "$(command -v cl)")/link.exe")"
	;;
esac

# --bindir lib keeps the Windows DLL next to its import library, as the archives ship it.
cargo cinstall -p officewright-ffi --prefix "$cargo_prefix" --libdir lib --bindir lib "$@"

# On Windows cargo-c also installs the linker's .def file and the debug symbols (.pdb),
# which are not part of the archive layout.
rm -f "$prefix/lib/officewright.def" "$prefix/lib/officewright.pdb"

pc=$prefix/lib/pkgconfig/officewright.pc
tr -d '\r' <"$pc" | sed "s|^prefix=.*|prefix=\${pcfiledir}/../..|" >"$pc.tmp"
mv "$pc.tmp" "$pc"

if [ "$(uname -s)" = Darwin ]; then
	for dylib in "$prefix"/lib/libofficewright*.dylib; do
		[ -L "$dylib" ] && continue
		id=$(otool -D "$dylib" | sed -n 2p)
		install_name_tool -id "@rpath/${id##*/}" "$dylib"
		# Changing the install name invalidates the signature, and arm64 macOS refuses
		# to load an unsigned library; sign it again ad hoc.
		codesign --force --sign - "$dylib"
	done
fi

cp LICENSE README.md THIRD-PARTY-NOTICES.md "$prefix/"
