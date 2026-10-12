#!/usr/bin/env bash
# Checks an installed C library the way a user would consume it: from a prefix made by
# scripts/install-capi.sh (scripts/check.sh) or an unpacked release archive
# (.github/workflows/release.yml). Needs a C/C++ compiler, binutils and Python, not Rust.
# Usage: scripts/check-capi.sh <prefix>
set -euo pipefail
cd "$(dirname "$0")/.."

fail() {
	echo "check-capi.sh: $*" >&2
	exit 1
}
step() { printf '\n==> %s\n' "$1"; }

[ $# -eq 1 ] || {
	echo "usage: $0 <prefix>" >&2
	exit 2
}
prefix=$(cd "$1" && pwd)

case $(uname -s) in
Linux)
	os=linux python=python3
	# The compiler's target tells musl from glibc (a glibc system can have musl's loader
	# installed too); musl has no sanitizer runtimes.
	case $(cc -dumpmachine) in
	*-musl*) sanitizers=0 ;;
	*) sanitizers=1 ;;
	esac
	;;
Darwin) os=macos python=python3 sanitizers=1 ;;
MINGW* | MSYS* | CYGWIN*) os=windows python=python ;;
*) fail "unsupported system $(uname -s)" ;;
esac

step "installed files"
header=$prefix/include/officewright.h
pc=$prefix/lib/pkgconfig/officewright.pc
case $os in
linux) shared=$prefix/lib/libofficewright.so static=$prefix/lib/libofficewright.a ;;
macos) shared=$prefix/lib/libofficewright.dylib static=$prefix/lib/libofficewright.a ;;
windows) shared=$prefix/lib/officewright.dll static=$prefix/lib/officewright.lib ;;
esac
for file in "$header" "$pc" "$shared" "$static" "$prefix/LICENSE" "$prefix/README.md" \
	"$prefix/THIRD-PARTY-NOTICES.md"; do
	[ -f "$file" ] || fail "missing $file"
done
[ "$os" != windows ] || [ -f "$prefix/lib/officewright.dll.lib" ] || fail "missing import library"
# Nothing else may ship: the archive layout is the one CLAUDE.md describes.
common='include/officewright\.h|lib/pkgconfig/officewright\.pc|LICENSE|README\.md|THIRD-PARTY-NOTICES\.md'
case $os in
linux) libraries='lib/libofficewright\.(a|so|so\.[0-9]+|so\.[0-9]+\.[0-9]+\.[0-9]+)' ;;
macos) libraries='lib/libofficewright\.(a|dylib|[0-9]+\.dylib|[0-9]+\.[0-9]+\.[0-9]+\.dylib)' ;;
windows) libraries='lib/officewright\.(dll|dll\.lib|lib)' ;;
esac
# A separate assignment, so a failing find stops the script instead of passing silently.
files=$(cd "$prefix" && find . -type f -o -type l)
unexpected=$(printf '%s\n' "$files" | sed 's|^\./||' | grep -Ev "^($common|$libraries)$" || true)
[ -z "$unexpected" ] || fail "unexpected files in $prefix:
$unexpected"

step "officewright.pc is relocatable"
# pkg-config resolves ${pcfiledir} to the directory of the .pc file, so the paths stay
# correct wherever the prefix is unpacked.
for line in "prefix=\${pcfiledir}/../.." "libdir=\${prefix}/lib" "includedir=\${prefix}/include"; do
	grep -qxF "$line" "$pc" || fail "$pc lacks the line '$line':
$(cat "$pc")"
done

step "exported symbols"
case $os in
linux)
	listing=$(nm -D --defined-only "$shared")
	exports=$(printf '%s\n' "$listing" | awk '{ print $3 }')
	soname=$(readelf -d "$shared" | sed -n 's/.*Library soname: \[\(.*\)\]/\1/p')
	[ "$soname" = libofficewright.so.0 ] || fail "SONAME is '$soname', expected libofficewright.so.0"
	;;
macos)
	listing=$(nm -gU "$shared")
	exports=$(printf '%s\n' "$listing" | awk '{ sub(/^_/, "", $3); print $3 }')
	install_name=$(otool -D "$shared" | sed -n 2p)
	case $install_name in
	@rpath/*) ;;
	*) fail "install name is '$install_name', expected @rpath/..." ;;
	esac
	;;
windows)
	listing=$(dumpbin -nologo -exports "$(cygpath -m "$shared")" | tr -d '\r')
	# Export table rows: ordinal, hint, RVA, name, sometimes followed by "= internal name".
	exports=$(printf '%s\n' "$listing" |
		awk 'NF >= 4 && $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9A-F]+$/ && $3 ~ /^[0-9A-F]+$/ { print $4 }')
	;;
esac
printf '%s\n' "$exports" | grep -qx ow_abi_version ||
	fail "ow_abi_version is not exported from $shared; symbol listing:
$listing"
unexpected=$(printf '%s\n' "$exports" | grep -v '^ow_' || true)
[ -z "$unexpected" ] || fail "$shared exports symbols outside ow_*: $unexpected"

step "tests/c"
c_out=$(mktemp -d)
trap 'rm -rf "$c_out"' EXIT
# Link flags for the static library, as cargo-c recorded them from rustc.
read -r -a libs <<<"$(tr -d '\r' <"$pc" | sed -n 's/^Libs.private: //p')"
[ ${#libs[@]} -gt 0 ] || fail "no Libs.private in $pc"
case $os in
windows)
	# Every path below is already a Windows path (cygpath -m); stop Git Bash from
	# rewriting option arguments that hold one, as it mangled -Fo"C:/..." output paths.
	export MSYS2_ARG_CONV_EXCL='*'
	# -MD matches the CRT the Rust static library links against.
	win_libs=()
	for lib in "${libs[@]}"; do win_libs+=("${lib#-l}.lib"); done
	msvc=(-nologo -MD -W4 -WX "-I$(cygpath -m "$prefix/include")")
	static=$(cygpath -m "$static")
	c_dir=$(cygpath -m "$c_out")
	cl "${msvc[@]}" -std:c17 tests/c/abi_test.c "$static" "${win_libs[@]}" \
		-Fo"$c_dir/abi_c17.obj" -Fe"$c_dir/abi_c17.exe"
	cl "${msvc[@]}" -std:c++17 -EHsc -Tp tests/c/abi_test.c "$static" "${win_libs[@]}" \
		-Fo"$c_dir/abi_cxx17.obj" -Fe"$c_dir/abi_cxx17.exe"
	"$c_out/abi_c17.exe" "$c_dir" tests/fonts
	"$c_out/abi_cxx17.exe" "$c_dir" tests/fonts
	;;
*)
	strict=(-Wall -Wextra -Wpedantic -Werror "-I$prefix/include")
	cc -std=c99 "${strict[@]}" tests/c/abi_test.c "$static" "${libs[@]}" -o "$c_out/abi_c99"
	c++ -x c++ -std=c++17 "${strict[@]}" tests/c/abi_test.c -x none "$static" "${libs[@]}" -o "$c_out/abi_cxx17"
	"$c_out/abi_c99" "$c_out" tests/fonts
	"$c_out/abi_cxx17" "$c_out" tests/fonts
	if [ "$sanitizers" = 0 ]; then
		echo "skipping the ASan+UBSan build: $(cc -dumpmachine) has no sanitizer runtimes"
	else
		cc -std=c99 "${strict[@]}" -g -fsanitize=address,undefined -fno-sanitize-recover=all \
			tests/c/abi_test.c "$static" "${libs[@]}" -o "$c_out/abi_sanitized"
		"$c_out/abi_sanitized" "$c_out" tests/fonts
	fi
	;;
esac

step "tests/ffi-python"
ow_lib=$shared
[ "$os" != windows ] || ow_lib=$(cygpath -w "$shared")
OW_LIB=$ow_lib "$python" -B -m unittest discover -s tests/ffi-python -v
