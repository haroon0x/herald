#!/bin/sh
# Herald installer for Linux and macOS.
#
# Downloads the release archive for this machine, verifies its SHA-256 against
# the release's SHA256SUMS, and installs the `herald` binary into
# ${HERALD_INSTALL_DIR:-$HOME/.local/bin}. It never pipes a remote script into a
# shell, never uses sudo, and fails closed if the checksum cannot be verified.
#
# Usage:
#   ./install.sh
#   HERALD_VERSION=v1.0.0 ./install.sh
#   HERALD_INSTALL_DIR="$HOME/bin" ./install.sh
#
# Note: SHA256SUMS is downloaded over HTTPS from the same GitHub release as the
# archive. It detects corrupt or truncated downloads; it does not protect against
# a compromised GitHub account. When a release is minisign-signed,
# HERALD_MINISIGN_PUB (path to the public key file) makes the installer verify
# SHA256SUMS.sig before trusting SHA256SUMS; without it, checksums only.

set -eu

REPO="haroon0x/herald"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"
DOWNLOAD_ROOT="https://github.com/${REPO}/releases/download"

usage() {
    cat <<'EOF'
Install Herald from a GitHub release.

Environment:
  HERALD_INSTALL_DIR  Directory for the binary (default: $HOME/.local/bin)
  HERALD_VERSION      Release tag to install (default: latest, e.g. v1.0.0)
  HERALD_MINISIGN_PUB Path to the project's minisign public key file. When set
                      (and `minisign` is on PATH), SHA256SUMS is signature-
                      verified before the checksum is used.

Examples:
  ./install.sh
  HERALD_VERSION=v1.0.0 ./install.sh
  HERALD_INSTALL_DIR="$HOME/bin" ./install.sh
  HERALD_MINISIGN_PUB=~/.herald.pub ./install.sh
EOF
}

err() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

info() {
    printf '%s\n' "$*"
}

case "${1:-}" in
    '') ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        err "unknown argument: $1 (try --help)"
        ;;
esac

# --- Platform detection ------------------------------------------------------

uname_s=$(uname -s)
uname_m=$(uname -m)

case "$uname_s" in
    Linux)
        os_suffix="unknown-linux-gnu"
        if [ -f /etc/alpine-release ]; then
            err "musl-based Linux (Alpine) is not supported by the prebuilt archives; build from source with cargo"
        fi
        ;;
    Darwin)
        os_suffix="apple-darwin"
        ;;
    *)
        err "unsupported operating system: ${uname_s} (supported: Linux, macOS)"
        ;;
esac

case "$uname_m" in
    x86_64 | amd64)
        arch="x86_64"
        ;;
    aarch64 | arm64)
        arch="aarch64"
        ;;
    *)
        err "unsupported architecture: ${uname_m} (supported: x86_64, aarch64)"
        ;;
esac

target="${arch}-${os_suffix}"

# --- Tools -------------------------------------------------------------------

if command -v curl >/dev/null 2>&1; then
    fetch() {
        curl -fsSL --retry 3 --proto '=https' --tlsv1.2 "$1" -o "$2"
    }
elif command -v wget >/dev/null 2>&1; then
    fetch() {
        wget -q --tries=3 -O "$2" "$1"
    }
else
    err "this installer needs curl or wget on PATH"
fi

if [ -z "${HERALD_INSTALL_DIR:-}" ]; then
    : "${HOME:?HOME is not set; set HERALD_INSTALL_DIR explicitly}"
fi
install_dir="${HERALD_INSTALL_DIR:-$HOME/.local/bin}"

version="${HERALD_VERSION:-}"
case "$version" in
    '') ;;
    v*) ;;
    *) version="v${version}" ;;
esac

tmp=""
cleanup() {
    if [ -n "$tmp" ]; then
        rm -rf "$tmp"
    fi
}
trap cleanup 0
trap 'exit 1' 1 2 3 15

tmp=$(mktemp -d "${TMPDIR:-/tmp}/herald-install.XXXXXX") ||
    err "could not create a temporary directory"

# --- Resolve the release -----------------------------------------------------

if [ -z "$version" ]; then
    info "Resolving the latest Herald release..."
    fetch "$API_URL" "$tmp/release.json" ||
        err "could not reach the GitHub API (rate limited or offline?); set HERALD_VERSION to skip the lookup"

    if command -v jq >/dev/null 2>&1; then
        version=$(jq -r '.tag_name // empty' "$tmp/release.json")
    elif command -v python3 >/dev/null 2>&1; then
        version=$(python3 - "$tmp/release.json" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        print(json.load(handle).get("tag_name", ""))
except (OSError, ValueError):
    print("")
PY
)
    else
        version=$(tr ',' '\n' <"$tmp/release.json" |
            sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
            head -n 1)
    fi

    [ "$version" != "null" ] || version=""
    [ -n "$version" ] ||
        err "could not determine the latest release tag; set HERALD_VERSION (for example v1.0.0)"
fi

archive="herald-${version}-${target}.tar.gz"
release_url="${DOWNLOAD_ROOT}/${version}"

info "Installing herald ${version} for ${target}"

# --- Download and verify -----------------------------------------------------

fetch "${release_url}/SHA256SUMS" "$tmp/SHA256SUMS" ||
    err "could not download SHA256SUMS for ${version}"
fetch "${release_url}/${archive}" "$tmp/${archive}" ||
    err "could not download ${archive} for ${version}"

# Optional signature verification of SHA256SUMS itself. Only when the user
# pins the public key with HERALD_MINISIGN_PUB; fails closed when asked for
# but unavailable.
if [ -n "${HERALD_MINISIGN_PUB:-}" ]; then
    command -v minisign >/dev/null 2>&1 ||
        err "HERALD_MINISIGN_PUB is set but minisign is not on PATH"
    fetch "${release_url}/SHA256SUMS.sig" "$tmp/SHA256SUMS.sig" ||
        err "could not download SHA256SUMS.sig for ${version}"
    minisign -Vm "$tmp/SHA256SUMS" -p "$HERALD_MINISIGN_PUB" -x "$tmp/SHA256SUMS.sig" ||
        err "minisign signature verification failed for SHA256SUMS"
    info "Verified minisign signature for SHA256SUMS"
fi

expected=$(awk -v name="$archive" '{ file = $2; sub(/^\*/, "", file); if (file == name) { print tolower($1); exit } }' "$tmp/SHA256SUMS")
[ -n "$expected" ] ||
    err "SHA256SUMS has no entry for ${archive}; refusing to install"

if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/${archive}" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/${archive}" | awk '{ print $1 }')
elif command -v openssl >/dev/null 2>&1; then
    actual=$(openssl dgst -sha256 "$tmp/${archive}" | sed 's/.*= //')
else
    err "no SHA-256 tool found (need sha256sum, shasum, or openssl)"
fi

actual=$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')
[ "$actual" = "$expected" ] ||
    err "checksum mismatch for ${archive}: expected ${expected}, got ${actual}"

info "Verified SHA-256 ${expected}"

# --- Extract and install -----------------------------------------------------

tar -xzf "$tmp/${archive}" -C "$tmp" ||
    err "could not extract ${archive}"

binary="$tmp/herald-${version}-${target}/herald"
if [ ! -f "$binary" ]; then
    binary=$(find "$tmp" -type f -name herald 2>/dev/null | head -n 1 || true)
fi
[ -n "$binary" ] && [ -f "$binary" ] ||
    err "archive did not contain a herald binary"

mkdir -p "$install_dir" ||
    err "could not create ${install_dir}; set HERALD_INSTALL_DIR to a writable directory"

staged="${install_dir}/.herald.$$"
cp "$binary" "$staged" || err "could not write to ${install_dir}"
chmod 755 "$staged"
mv -f "$staged" "${install_dir}/herald" ||
    err "could not install herald into ${install_dir}"

info ""
info "Installed herald ${version} to ${install_dir}/herald"

case ":${PATH}:" in
    *":${install_dir}:"*)
        ;;
    *)
        info ""
        info "Add ${install_dir} to your PATH:"
        info "  export PATH=\"${install_dir}:\$PATH\""
        ;;
esac

info ""
info "Next steps:"
info "  herald init      # create config, database, and data directory"
info "  herald doctor    # check storage, providers, and inference"
info "  herald --version"
