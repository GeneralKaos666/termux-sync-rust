#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <debian-arch> <version> <bin-dir> <out-dir>" >&2
    exit 2
fi

ARCH="$1"
VERSION="$2"
BIN_DIR="$3"
OUT_DIR="$4"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

case "$ARCH" in
    aarch64|arm) ;;
    *) echo "unsupported arch: $ARCH" >&2; exit 2 ;;
esac

test -x "$BIN_DIR/termux-sync"
test -x "$BIN_DIR/termux-sync-tui"
test -f "$ROOT/README.md"
test -f "$ROOT/LICENSE"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

PREFIX_DIR="$STAGE/data/data/com.termux/files/usr"
mkdir -p "$PREFIX_DIR/bin" "$PREFIX_DIR/share/doc/termux-sync" "$STAGE/DEBIAN"

install -m755 "$BIN_DIR/termux-sync" "$PREFIX_DIR/bin/termux-sync"
install -m755 "$BIN_DIR/termux-sync-tui" "$PREFIX_DIR/bin/termux-sync-tui"
install -m644 "$ROOT/README.md" "$PREFIX_DIR/share/doc/termux-sync/README.md"
install -m644 "$ROOT/LICENSE" "$PREFIX_DIR/share/doc/termux-sync/LICENSE"

INSTALLED_SIZE="$(du -sk "$STAGE" | cut -f1)"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: termux-sync
Version: $VERSION
Architecture: $ARCH
Maintainer: Ronald Sloan (for termux) <xkiller1177@gmail.com>
Installed-Size: $INSTALLED_SIZE
Depends: dpkg
Section: utils
Priority: optional
Homepage: https://github.com/GeneralKaos666/termux-sync-rust
Description: Back up and restore a Termux environment across devices
EOF

chmod 755 "$STAGE/DEBIAN"
mkdir -p "$OUT_DIR"
dpkg-deb --build --root-owner-group "$STAGE" "$OUT_DIR/termux-sync_${VERSION}_${ARCH}.deb"
