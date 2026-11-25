#!/data/data/com.termux/files/usr/bin/bash
# Interactive cutover from the Python command to the pure Rust binaries.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${PREFIX:-/data/data/com.termux/files/usr}"
BIN="$PREFIX/bin"

step() { printf "\n== %s ==\n" "$*"; }
pause() { printf "\nPress Enter to continue, or Ctrl+C to stop. "; read -r _; }

step "1/5: Back up the current termux-sync command"
if [[ -f "$BIN/termux-sync" ]]; then
  cp -p "$BIN/termux-sync" "$BIN/termux-sync-python"
  echo "Saved: $BIN/termux-sync-python"
else
  echo "No existing termux-sync command; nothing to back up."
fi
pause

step "2/5: Build and install the Rust binaries"
bash "$ROOT/rust/install.sh"
pause

step "3/5: Check the new version"
"$BIN/termux-sync" --version
pause

step "4/5: Refresh the boot daemon script"
echo "Run: termux-sync schedule"
echo "Keep auto-backup disabled or set it again; the boot script must point at the Rust binary."
pause

step "5/5: Rollback"
echo "If anything fails, restore the previous command:"
echo "  cp -p $BIN/termux-sync-python $BIN/termux-sync"
echo "The saved binary stays in place until you remove it."
