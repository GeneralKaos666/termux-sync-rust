#!/data/data/com.termux/files/usr/bin/bash
# Build and install the pure Rust termux-sync binaries.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${PREFIX:-/data/data/com.termux/files/usr}"
BIN="$PREFIX/bin"

echo "Building termux-sync (release)..."
cargo build --locked --release --manifest-path "$ROOT/rust/app/Cargo.toml"

echo "Building termux-sync-tui (release)..."
cargo build --locked --release --manifest-path "$ROOT/tui/Cargo.toml"

install -Dm755 "$ROOT/rust/app/target/release/termux-sync-rs" "$BIN/termux-sync"
install -Dm755 "$ROOT/tui/target/release/termux-sync-tui" "$BIN/termux-sync-tui"

echo "Installed:"
"$BIN/termux-sync" --version
"$BIN/termux-sync-tui" --version 2>/dev/null || true
