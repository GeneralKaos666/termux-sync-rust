#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build --locked --release --bins >/dev/null

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

./scripts/build-deb.sh aarch64 "$VERSION" target/release "$TMP"

DEB="$TMP/termux-sync_${VERSION}_aarch64.deb"
test -f "$DEB"

dpkg-deb -f "$DEB" Package | grep -qx 'termux-sync'
dpkg-deb -f "$DEB" Version | grep -qx "$VERSION"
dpkg-deb -f "$DEB" Architecture | grep -qx 'aarch64'
dpkg-deb -f "$DEB" Depends | grep -qx 'dpkg'

LISTING="$(dpkg-deb -c "$DEB")"
grep -q './data/data/com.termux/files/usr/bin/termux-sync$' <<<"$LISTING"
grep -q './data/data/com.termux/files/usr/bin/termux-sync-tui$' <<<"$LISTING"
grep -q './data/data/com.termux/files/usr/share/doc/termux-sync/README.md$' <<<"$LISTING"

mkdir -p "$TMP/root"
dpkg-deb -x "$DEB" "$TMP/root"
PREFIX="$TMP/root/data/data/com.termux/files/usr/bin"
"$PREFIX/termux-sync" --version | grep -qx "termux-sync v$VERSION"
"$PREFIX/termux-sync-tui" --version | grep -qx "termux-sync-tui $VERSION"

echo "deb packaging ok"
