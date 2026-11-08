#!/bin/sh
# Build a Debian package for the `herald` CLI (Linux x86_64).
#
# Usage:
#   sh contrib/debian/build-deb.sh <path-to-herald-binary> <version> <outdir>
#
# The package installs the binary to /usr/bin/herald. It carries no
# dependency beyond libc, which every Debian/Ubuntu release provides.
#
# The script is plain POSIX sh; it stages the payload in a temp directory
# and calls `dpkg-deb --build`. It does not run as root (uses fakeroot
# semantics via --root-owner-group, available in dpkg-deb 1.18+).
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <herald-binary> <version> <outdir>" >&2
    exit 2
fi

binary="$1"
version="$2"
outdir="$3"

if [ ! -f "$binary" ]; then
    echo "binary not found: $binary" >&2
    exit 1
fi

# Strip a leading "v" so "v1.0.0" and "1.0.0" produce the same package.
version=$(printf '%s' "$version" | sed 's/^v//')
case "$version" in
    ''|*[!0-9.]*)
        echo "version must be numeric (e.g. 1.0.0): $version" >&2
        exit 1
        ;;
esac

mkdir -p "$outdir"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/herald-deb.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

pkgroot="$tmp/pkg"
mkdir -p "$pkgroot/DEBIAN" "$pkgroot/usr/bin" "$pkgroot/usr/share/doc/herald"

cp "$binary" "$pkgroot/usr/bin/herald"
chmod 755 "$pkgroot/usr/bin/herald"

# Minimal docs. The changelog is optional metadata; dpkg-deb does not
# require it, but Debian's lintian does, so ship a one-line changelog.
repo_root=$(dirname "$0")
repo_root=$(dirname "$repo_root")
if [ -f "$repo_root/README.md" ]; then
    cp "$repo_root/README.md" "$pkgroot/usr/share/doc/herald/README.md"
fi
printf 'herald (%s) unstable; urgency=medium\n\n  * Local-first communication agent.\n' "$version" \
    > "$pkgroot/usr/share/doc/herald/changelog.Debian"

cat > "$pkgroot/DEBIAN/control" <<EOF
Package: herald
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Maintainer: Herald contributors <haroon0x@users.noreply.github.com>
Homepage: https://github.com/haroon0x/herald
Rules-Requires-Root: no
Description: Local-first communication agent
 Herald reads connected communication providers, summarizes and drafts
 with a local model, and only sends after an explicit policy check and
 a human approval.
EOF

deb="$outdir/herald_${version}_amd64.deb"
if command -v dpkg-deb >/dev/null 2>&1; then
    dpkg-deb --root-owner-group --build "$pkgroot" "$deb"
else
    echo "dpkg-deb not found; install dpkg-dev" >&2
    exit 1
fi
echo "built $deb"
