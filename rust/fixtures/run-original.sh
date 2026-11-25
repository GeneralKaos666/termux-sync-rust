#!/bin/sh
# Fallback launcher for the Python original.
#
# During `rust-anything app replay` the front door is graded against the fixture
# home/prefix/PATH. In normal use it must run the original with the caller's own
# environment. Replay sets RUST_ANYTHING_FALLBACK_LOG; production does not.
set -eu
repo="$(cd "$(dirname "$0")/../.." && pwd)"

if [ -n "${RUST_ANYTHING_FALLBACK_LOG:-}" ] && [ -d "$(pwd)/rust/fixtures/app/home" ]; then
  base="$(pwd)/rust/fixtures/app"
  export HOME="$base/home"
  export PREFIX="$base/prefix"
  export TMPDIR="$base/tmp"
  export PATH="$base/bin:$PATH"
  export COLUMNS=200
  export LINES=50
  export NO_COLOR=1
fi

exec python3 "$repo/termux-sync.py" "$@"
