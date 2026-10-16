#!/usr/bin/env bash
# Linux release build (the counterpart of build-release.ps1): runs the tests,
# builds with the machine's paths removed from the binary and checks it.
# Requires: Rust, pkg-config and the GTK 3 development files
# (Debian/Ubuntu: sudo apt install build-essential pkg-config libgtk-3-dev).
set -euo pipefail
cd "$(dirname "$0")"

crate=$(pwd)
home=${HOME%/}
cargo_home=${CARGO_HOME:-$home/.cargo}
target=$crate/target

# the last matching prefix wins: the crate (inside the home folder) comes after it
flags=(
    "--remap-path-prefix=$home=home"
    "--remap-path-prefix=$cargo_home=cargo"
    "--remap-path-prefix=$crate=src"
    "--remap-path-prefix=$target=target"
)
CARGO_ENCODED_RUSTFLAGS=$(IFS=$'\x1f'; echo "${flags[*]}")
export CARGO_ENCODED_RUSTFLAGS

# the tests use a temporary folder and an in-memory key of their own
cargo test --locked

cargo clean --release
cargo build --release --locked

binario=$target/release/vpn
for valor in "$home" "$crate"; do
    if grep -aqF -- "$valor" "$binario"; then
        echo "The executable contains local build information: $valor" >&2
        exit 1
    fi
done
if ! head -c 4 "$binario" | grep -q "ELF"; then
    echo "Not an ELF executable." >&2
    exit 1
fi

echo "Release verified: $binario"
echo "SHA256: $(sha256sum "$binario" | cut -d' ' -f1)"
