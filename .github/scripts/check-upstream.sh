#!/usr/bin/env bash
# Components the build scripts pin by hand (Dependabot does not see them):
# compares each pinned version with the latest one published upstream and
# prints a Markdown report of what is outdated. Exit code 0 = all current,
# 10 = something to update, other = could not check.
#
#   bash .github/scripts/check-upstream.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

pinned() { # file, pattern with one group -> the pinned value
    sed -n "s/$2/\1/p" "$1" | head -1
}
highest() { # the highest of the versions on stdin
    sort -uV | tail -1
}
fetch() {
    curl -fsSL --retry 3 "$1"
}

msi=$(pinned rust/build-release.ps1 '^\$msiVersao = "\(.*\)"$')
openvpn=$(pinned rust/build-release-macos.sh '^OPENVPN_VERSAO=\(.*\)$')
openssl=$(pinned rust/build-release-macos.sh '^OPENSSL_VERSAO=\(.*\)$')
lzo=$(pinned rust/build-release-macos.sh '^LZO_VERSAO=\(.*\)$')
lz4=$(pinned rust/build-release-macos.sh '^LZ4_VERSAO=\(.*\)$')
for v in "$msi" "$openvpn" "$openssl" "$lzo" "$lz4"; do
    [ -n "$v" ] || { echo "Could not read a pinned version." >&2; exit 2; }
done

# latest upstream (OpenSSL: the same release series, e.g. 3.5.x)
msi_latest=$(fetch https://build.openvpn.net/downloads/releases/ |
    grep -o 'OpenVPN-[0-9.]*-I[0-9]*-amd64\.msi' | sed 's/^OpenVPN-//; s/-amd64\.msi$//' | highest)
openvpn_latest=$(fetch https://build.openvpn.net/downloads/releases/ |
    grep -o 'openvpn-2\.[0-9]*\.[0-9]*\.tar\.gz' | sed 's/^openvpn-//; s/\.tar\.gz$//' | highest)
series=${openssl%.*}
openssl_latest=$(fetch "https://api.github.com/repos/openssl/openssl/releases?per_page=100" |
    grep -o "\"tag_name\": *\"openssl-${series//./\\.}\.[0-9]*\"" | grep -o "${series//./\\.}\.[0-9]*" | highest)
lzo_latest=$(fetch https://www.oberhumer.com/opensource/lzo/download/ |
    grep -o 'lzo-[0-9.]*[0-9]\.tar\.gz' | sed 's/^lzo-//; s/\.tar\.gz$//' | highest)
lz4_latest=$(fetch https://api.github.com/repos/lz4/lz4/releases/latest |
    grep -o '"tag_name": *"v[0-9.]*"' | grep -o '[0-9][0-9.]*')
for v in "$msi_latest" "$openvpn_latest" "$openssl_latest" "$lzo_latest" "$lz4_latest"; do
    [ -n "$v" ] || { echo "Could not read an upstream version." >&2; exit 3; }
done

rows=""
outdated() { # name, pinned, latest, where
    if [ "$2" != "$3" ] && [ "$(printf '%s\n%s\n' "$2" "$3" | highest)" = "$3" ]; then
        rows+="| $1 | $2 | **$3** | $4 |"$'\n'
    fi
}
outdated "OpenVPN installer (Windows, embedded in VPN.exe)" "$msi" "$msi_latest" \
    '`rust/build-release.ps1` ($msiVersao, $msiSha256), `rust/build.rs` (MSI_VERSION), `THIRD-PARTY-LICENSES.txt`'
outdated "OpenVPN (macOS build)" "$openvpn" "$openvpn_latest" \
    '`rust/build-release-macos.sh` (OPENVPN_VERSAO, OPENVPN_SHA256), `THIRD-PARTY-LICENSES.txt`'
outdated "OpenSSL $series (macOS build)" "$openssl" "$openssl_latest" \
    '`rust/build-release-macos.sh` (OPENSSL_VERSAO, OPENSSL_SHA256), `THIRD-PARTY-LICENSES.txt`'
outdated "LZO (macOS build)" "$lzo" "$lzo_latest" \
    '`rust/build-release-macos.sh` (LZO_VERSAO, LZO_SHA256), `THIRD-PARTY-LICENSES.txt`'
outdated "LZ4 (macOS build)" "$lz4" "$lz4_latest" \
    '`rust/build-release-macos.sh` (LZ4_VERSAO, LZ4_SHA256), `THIRD-PARTY-LICENSES.txt`'

if [ -z "$rows" ]; then
    echo "All pinned components are current: OpenVPN installer $msi, OpenVPN $openvpn, OpenSSL $openssl, LZO $lzo, LZ4 $lz4."
    exit 0
fi
cat <<EOF
New upstream versions of components that the build scripts pin by hand
(Dependabot does not cover them):

| Component | Pinned | Latest | Where to update |
|---|---|---|---|
${rows}
For each one: update the version and its SHA-256 (taken from the official
download and checked against the published checksum or signature), run the
release build, and publish a new version of the app.
EOF
exit 10
