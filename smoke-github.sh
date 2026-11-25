#!/data/data/com.termux/files/usr/bin/bash
# Live GitHub storage smoke. Needs GITHUB_TOKEN and GITHUB_REPO (owner/repo).
set -euo pipefail

: "${GITHUB_TOKEN:?set GITHUB_TOKEN}"
: "${GITHUB_REPO:?set GITHUB_REPO=owner/repo}"
BIN="${TERMUX_SYNC_BIN:-$(cd "$(dirname "$0")" && pwd)/target/release/termux-sync-rs}"
[[ -x "$BIN" ]] || { echo "missing binary: $BIN" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
HOME_DIR="$TMP/home"
PREFIX_DIR="$TMP/prefix"
BIN_DIR="$TMP/bin"
mkdir -p "$HOME_DIR/.config/termux-sync" "$HOME_DIR/data" "$PREFIX_DIR/etc" "$TMP/tmp" "$BIN_DIR"
printf 'github smoke\n' > "$HOME_DIR/data/note.txt"
printf 'prefix\n' > "$PREFIX_DIR/etc/sample"
printf '#!/bin/sh\nprintf "bash\\tinstall\\n"\n' > "$BIN_DIR/dpkg"
printf '#!/bin/sh\nexit 0\n' > "$BIN_DIR/pkg"
chmod +x "$BIN_DIR/dpkg" "$BIN_DIR/pkg"
cat > "$HOME_DIR/.config/termux-sync/config.json" <<JSON
{
  "storage": "github",
  "local_path": "~/backups",
  "gdrive_folder_id": "",
  "github_token": "$GITHUB_TOKEN",
  "github_repo": "$GITHUB_REPO",
  "github_branch": "main",
  "compression": "gz",
  "exclude_patterns": [".cache"],
  "max_backups": 2,
  "encrypt": false,
  "encrypt_password": ""
}
JSON

run() {
  env HOME="$HOME_DIR" PREFIX="$PREFIX_DIR" TMPDIR="$TMP/tmp" \
    PATH="$BIN_DIR:$PATH" COLUMNS=200 NO_COLOR=1 "$BIN" "$@"
}

echo "backup"
OUT="$(run --json-events backup --label smoke)"
NAME="$(printf '%s\n' "$OUT" | sed -n 's/.*"event":"result".*"name":"\([^"]*\)".*/\1/p' | tail -1)"
[[ -n "$NAME" ]] || { echo "no backup name in events" >&2; exit 1; }
echo "created $NAME"

echo "list"
LIST="$(run --json-events list)"
printf '%s\n' "$LIST" | grep -q "$NAME" || { echo "backup missing from list" >&2; exit 1; }

echo "restore"
rm -f "$HOME_DIR/data/note.txt"
printf '{"confirm":"RESTORE"}\n' | run --json-events restore --name "$NAME" > "$TMP/restore.json"
grep -q '"event":"result"' "$TMP/restore.json" || { echo "restore result missing" >&2; exit 1; }
grep -qx 'github smoke' "$HOME_DIR/data/note.txt" || { echo "restored file mismatch" >&2; exit 1; }

echo "delete"
run delete --name "$NAME" --yes
if run --json-events list | grep -q "$NAME"; then
  echo "backup still listed after delete" >&2
  exit 1
fi

echo "GitHub smoke passed: $NAME"
