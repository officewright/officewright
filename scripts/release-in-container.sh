#!/bin/sh
# Builds a Linux release archive inside a container, so the library links against the
# container's C library: manylinux_2_28 (glibc 2.28) for the -gnu targets, Alpine for
# x86_64-unknown-linux-musl. Run by .github/workflows/release.yml from the repository root:
#   docker run -v "$PWD:/src" -w /src <image> \
#     sh scripts/release-in-container.sh <version> <out-dir> <target>
set -eu

version=$1
out=$2
target=$3

if command -v apk >/dev/null; then
	apk add --no-cache bash curl gcc musl-dev
	# Rust links musl statically by default, which rules out a shared library. Link it
	# dynamically, as every Alpine library does; the static library is unaffected.
	RUSTFLAGS="-C target-feature=-crt-static"
	export RUSTFLAGS
fi

# rustup-init 1.29.1, pinned by the SHA-256 that static.rust-lang.org publishes.
case "$(uname -m)-$(command -v apk >/dev/null && echo musl || echo gnu)" in
x86_64-gnu)
	triple=x86_64-unknown-linux-gnu
	sha256=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71
	;;
aarch64-gnu)
	triple=aarch64-unknown-linux-gnu
	sha256=15f6e4ce9f583b929c996c91562bad6d4454f3281de858b02cdfdef615fac433
	;;
x86_64-musl)
	triple=x86_64-unknown-linux-musl
	sha256=331228566cc931f32cd684f9bebc5956a5e5d7394c8d5de595f196a658ab98e6
	;;
*)
	echo "release-in-container.sh: unsupported $(uname -m)" >&2
	exit 1
	;;
esac
curl -fsSL -o /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/1.29.1/$triple/rustup-init"
echo "$sha256  /tmp/rustup-init" | sha256sum -c -
chmod +x /tmp/rustup-init
# RUST_TOOLCHAIN comes from release.yml, which pins the compiler for release builds.
/tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN:?}"
PATH=$HOME/.cargo/bin:$PATH
export PATH

bash scripts/install-cargo-c.sh "$HOME/.cargo/bin"
bash scripts/package.sh "$version" "$out" "$target"
