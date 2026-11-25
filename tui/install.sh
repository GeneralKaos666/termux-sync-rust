#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX_DIR="${PREFIX:-}"
SKIP_BUILD=0

usage() {
    printf 'usage: %s [--prefix PATH] [--skip-build]\n' "$(basename "$0")"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            PREFIX_DIR="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$PREFIX_DIR" ]]; then
    printf 'PREFIX is not set. Pass --prefix PATH.\n' >&2
    exit 2
fi

BIN="$SCRIPT_DIR/target/release/termux-sync-tui"
if [[ "$SKIP_BUILD" -eq 0 ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        printf 'cargo is required. Install Rust first: pkg install rust\n' >&2
        exit 1
    fi
    (cd "$SCRIPT_DIR" && cargo build --locked --release)
fi

if [[ ! -x "$BIN" ]]; then
    printf 'missing binary: %s\n' "$BIN" >&2
    printf 'Run without --skip-build, or build it with: cd tui && cargo build --release\n' >&2
    exit 1
fi

mkdir -p "$PREFIX_DIR/bin"
cp "$BIN" "$PREFIX_DIR/bin/termux-sync-tui"
chmod 755 "$PREFIX_DIR/bin/termux-sync-tui"

printf 'installed %s\n' "$PREFIX_DIR/bin/termux-sync-tui"
printf 'run: termux-sync-tui            (add --ascii if icons render badly)\n'
printf 'requires: termux-sync with --json-events\n'
