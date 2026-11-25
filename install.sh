#!/data/data/com.termux/files/usr/bin/bash
# Build and install the pure Rust termux-sync binaries.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${PREFIX:-/data/data/com.termux/files/usr}"
BIN="$PREFIX/bin"

echo "Building termux-sync (release)..."
cargo build --locked --release --manifest-path "$ROOT/Cargo.toml" -p termux-sync-rs

echo "Building termux-sync-tui (release)..."
cargo build --locked --release --manifest-path "$ROOT/Cargo.toml" -p termux-sync-tui

install -Dm755 "$ROOT/target/release/termux-sync-rs" "$BIN/termux-sync"
install -Dm755 "$ROOT/target/release/termux-sync-tui" "$BIN/termux-sync-tui"

echo "Installed:"
"$BIN/termux-sync" --version
"$BIN/termux-sync-tui" --version 2>/dev/null || true
