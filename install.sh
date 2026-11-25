#!/data/data/com.termux/files/usr/bin/bash
# Compatibility entry point: the Rust installer owns installation now.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"

if [[ ! -f "$ROOT/rust/install.sh" ]]; then
  echo "rust/install.sh not found. Clone the repository and run this script from it." >&2
  exit 1
fi

exec bash "$ROOT/rust/install.sh" "$@"
