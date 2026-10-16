#!/usr/bin/env bash
# macOS release build (the counterpart of build-release.ps1):
#   1. builds OpenVPN with OpenSSL, LZO and LZ4 from their official sources
#      (pinned SHA-256) for Apple Silicon and Intel, statically linked;
#   2. runs the tests, builds the app for both processors with OpenVPN
#      embedded (build.rs) and the machine's paths removed;
#   3. assembles the universal VPN.app (ad hoc signed) and VPN-macos.zip.
# Requires: Xcode Command Line Tools, and Rust with the aarch64-apple-darwin
# and x86_64-apple-darwin targets.
set -euo pipefail
cd "$(dirname "$0")"

MACOS_MIN=11.0
OPENSSL_VERSAO=3.5.9
OPENSSL_SHA256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a
LZO_VERSAO=2.10
LZO_SHA256=c0f892943208266f9b6543b3ae308fab6284c5c90e627931446fb49b4221a072
LZ4_VERSAO=1.10.0
LZ4_SHA256=537512904744b35e232912055ccf8ec66d768639ff3abe5788d90d792ec5f48b
OPENVPN_VERSAO=2.7.6
OPENVPN_SHA256=10e24a9385f23cc38cc5cf448f3ca0769f939bc4cbecc4f4647d7e006e52db74

# Paths compiled into openvpn and OpenSSL: root-owned places that do not
# exist, so neither reads a configuration file a user could have written
# (/usr/local can belong to the user on Macs with Homebrew).
PREFIXO_OPENVPN=/Library/PrivilegedHelperTools/io.github.antxj.vpn.openvpn-files
PASTA_OPENSSL=/Library/PrivilegedHelperTools/io.github.antxj.vpn.openssl

crate=$(pwd)
home=${HOME%/}
cargo_home=${CARGO_HOME:-$home/.cargo}
target=$crate/target
deps=$target/macos-deps
fontes=$deps/fontes
nucleos=$(sysctl -n hw.ncpu)

baixar() { # url sha256 file
    local arquivo=$fontes/$3
    mkdir -p "$fontes"
    if [ ! -f "$arquivo" ] || ! echo "$2  $arquivo" | shasum -a 256 -c - >/dev/null 2>&1; then
        curl -fsSL --retry 3 "$1" -o "$arquivo"
    fi
    if ! echo "$2  $arquivo" | shasum -a 256 -c - >/dev/null; then
        echo "SHA-256 of $3 does not match." >&2
        exit 1
    fi
}

baixar "https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSAO/openssl-$OPENSSL_VERSAO.tar.gz" \
    $OPENSSL_SHA256 openssl-$OPENSSL_VERSAO.tar.gz
baixar "https://www.oberhumer.com/opensource/lzo/download/lzo-$LZO_VERSAO.tar.gz" \
    $LZO_SHA256 lzo-$LZO_VERSAO.tar.gz
baixar "https://github.com/lz4/lz4/releases/download/v$LZ4_VERSAO/lz4-$LZ4_VERSAO.tar.gz" \
    $LZ4_SHA256 lz4-$LZ4_VERSAO.tar.gz
baixar "https://build.openvpn.net/downloads/releases/openvpn-$OPENVPN_VERSAO.tar.gz" \
    $OPENVPN_SHA256 openvpn-$OPENVPN_VERSAO.tar.gz

# --------------------------------------------------- OpenVPN, per processor --
compilar_openvpn() { # arm64 | x86_64
    local arch=$1 host
    case $arch in
        arm64) host=aarch64-apple-darwin; local openssl_alvo=darwin64-arm64-cc ;;
        x86_64) host=x86_64-apple-darwin; local openssl_alvo=darwin64-x86_64-cc ;;
    esac
    local pronto=$deps/$arch/openvpn
    # versions + build recipe (raise "receita" when the options below change)
    local marca="$OPENSSL_VERSAO $LZO_VERSAO $LZ4_VERSAO $OPENVPN_VERSAO $MACOS_MIN receita-3"
    if [ -f "$pronto" ] && [ "$(cat "$deps/$arch/versoes" 2>/dev/null)" = "$marca" ]; then
        echo "OpenVPN for $arch: already built ($marca)."
        return
    fi
    local p=$deps/$arch
    local obra=$deps/obra-$arch
    rm -rf "$p" "$obra"
    mkdir -p "$p" "$obra"
    # build folders become neutral names in the binaries (__FILE__ and the like)
    local mapas="-ffile-prefix-map=$obra=build -ffile-prefix-map=$p=deps"
    export CC="clang -arch $arch -mmacosx-version-min=$MACOS_MIN"
    export CFLAGS="-O2 $mapas"
    # the build folder is inside this git repository: openvpn must not take
    # its commit as its own version
    export GIT=/usr/bin/false

    tar xzf "$fontes/lzo-$LZO_VERSAO.tar.gz" -C "$obra"
    (cd "$obra/lzo-$LZO_VERSAO" &&
        ./configure --host=$host --prefix="$p" --enable-static --disable-shared >/dev/null &&
        make -j"$nucleos" >/dev/null && make install >/dev/null)

    tar xzf "$fontes/lz4-$LZ4_VERSAO.tar.gz" -C "$obra"
    (cd "$obra/lz4-$LZ4_VERSAO/lib" &&
        make -j"$nucleos" liblz4.a CC="$CC" CFLAGS="$CFLAGS" >/dev/null &&
        mkdir -p "$p/include" "$p/lib" &&
        cp lz4.h lz4hc.h lz4frame.h "$p/include/" && cp liblz4.a "$p/lib/")

    # no-dso/no-module: no code loaded from files at run time. Its folders
    # (configuration, modules) point to a root-only place; installed with
    # DESTDIR so the build folder is not compiled in. OpenSSL keeps its
    # compiler flags in the binary, so no folder goes in them (its file
    # names are relative already).
    tar xzf "$fontes/openssl-$OPENSSL_VERSAO.tar.gz" -C "$obra"
    (cd "$obra/openssl-$OPENSSL_VERSAO" &&
        CFLAGS="-O2" ./Configure $openssl_alvo no-shared no-dso no-module no-engine no-tests no-docs \
            --prefix="$PASTA_OPENSSL" --libdir=lib --openssldir="$PASTA_OPENSSL" \
            -mmacosx-version-min=$MACOS_MIN >/dev/null &&
        make -j"$nucleos" >/dev/null && make install_sw DESTDIR="$obra/destino" >/dev/null &&
        cp -R "$obra/destino$PASTA_OPENSSL/include/openssl" "$p/include/" &&
        cp "$obra/destino$PASTA_OPENSSL/lib/libssl.a" \
            "$obra/destino$PASTA_OPENSSL/lib/libcrypto.a" "$p/lib/")

    # $p/lib has only static libraries: -lssl, -llzo2 and -llz4 link them in
    tar xzf "$fontes/openvpn-$OPENVPN_VERSAO.tar.gz" -C "$obra"
    (cd "$obra/openvpn-$OPENVPN_VERSAO" &&
        ./configure --host=$host --prefix="$PREFIXO_OPENVPN" \
            --disable-plugins --disable-unit-tests --disable-dependency-tracking \
            OPENSSL_CFLAGS="-I$p/include" OPENSSL_LIBS="-L$p/lib -lssl -lcrypto" \
            LZO_CFLAGS="-I$p/include" LZO_LIBS="-L$p/lib -llzo2" \
            LZ4_CFLAGS="-I$p/include" LZ4_LIBS="-L$p/lib -llz4" >/dev/null &&
        make -j"$nucleos" >/dev/null)
    cp "$obra/openvpn-$OPENVPN_VERSAO/src/openvpn/openvpn" "$pronto"
    strip -x "$pronto"
    cp "$obra/openvpn-$OPENVPN_VERSAO/distro/dns-scripts/macos-dns-updown.sh" "$deps/dns-updown-macos.sh"

    if grep -aqF -- "$home" "$pronto"; then
        echo "openvpn ($arch) contains build folders:" >&2
        strings -a "$pronto" | grep -F -- "$home" | head -5 >&2
        exit 1
    fi
    # only system libraries (nothing from Homebrew or the build machine)
    if otool -L "$pronto" | tail -n +2 | grep -vE "^\s+/(usr/lib|System/Library)/"; then
        echo "openvpn ($arch) links to libraries outside the system." >&2
        exit 1
    fi
    if ! lipo -archs "$pronto" | grep -qx "$arch"; then
        echo "openvpn is not $arch." >&2
        exit 1
    fi
    "$pronto" --version | head -1 || true
    echo "$marca" > "$deps/$arch/versoes"
    rm -rf "$obra"
    unset CC CFLAGS GIT
}

compilar_openvpn arm64
compilar_openvpn x86_64
cp "$deps/arm64/openvpn" assets/openvpn-macos-aarch64
cp "$deps/x86_64/openvpn" assets/openvpn-macos-x86_64
cp "$deps/dns-updown-macos.sh" assets/dns-updown-macos.sh

# ------------------------------------------------------------------ the app --
export MACOSX_DEPLOYMENT_TARGET=$MACOS_MIN
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

for alvo in aarch64-apple-darwin x86_64-apple-darwin; do
    rm -rf "$target/$alvo/release"
    cargo build --release --locked --target $alvo
done

versao=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
dist=$target/macos-dist
app=$dist/VPN.app
rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create "$target/aarch64-apple-darwin/release/vpn" "$target/x86_64-apple-darwin/release/vpn" \
    -output "$app/Contents/MacOS/VPN"
cp assets/VPN.icns "$app/Contents/Resources/VPN.icns"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>VPN</string>
    <key>CFBundleExecutable</key>
    <string>VPN</string>
    <key>CFBundleIconFile</key>
    <string>VPN</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.antxj.vpn</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>VPN</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$versao</string>
    <key>CFBundleVersion</key>
    <string>$versao</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key>
    <string>$MACOS_MIN</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>GPL-3.0-or-later - https://github.com/Antxj/vpn</string>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null
# ad hoc signature: required on Apple Silicon (not an Apple Developer ID)
codesign --force --sign - "$app"
codesign --verify --strict "$app"

binario=$app/Contents/MacOS/VPN
for valor in "$home" "$crate"; do
    if grep -aqF -- "$valor" "$binario"; then
        echo "The executable contains local build information: $valor" >&2
        strings -a "$binario" | grep -F -- "$valor" | head -5 >&2
        exit 1
    fi
done
if [ "$(lipo -archs "$binario" | tr ' ' '\n' | sort | tr '\n' ' ')" != "arm64 x86_64 " ]; then
    echo "The executable is not universal (arm64 + x86_64)." >&2
    exit 1
fi
tamanho_openvpn=$(stat -f %z assets/openvpn-macos-aarch64)
if [ "$(stat -f %z "$target/aarch64-apple-darwin/release/vpn")" -lt "$tamanho_openvpn" ]; then
    echo "The executable is smaller than OpenVPN: it was not embedded." >&2
    exit 1
fi

(cd "$dist" && ditto -c -k --keepParent VPN.app VPN-macos.zip)
echo "Release verified: $dist/VPN-macos.zip"
echo "Embedded OpenVPN: $OPENVPN_VERSAO (OpenSSL $OPENSSL_VERSAO, LZO $LZO_VERSAO, LZ4 $LZ4_VERSAO)"
echo "SHA256: $(shasum -a 256 "$dist/VPN-macos.zip" | cut -d' ' -f1)"
