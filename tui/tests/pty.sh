#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo build --locked >/dev/null

BIN="$PWD/target/debug/termux-sync-tui"
FAKE="$PWD/tests/pty_fake.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/home/.config/termux-sync" "$TMP/backups/termux_backup_test"
cat > "$TMP/home/.config/termux-sync/config.json" <<EOF
{"storage":"local","local_path":"$TMP/backups","compression":"gz","max_backups":2}
EOF
cat > "$TMP/backups/termux_backup_test/manifest.json" <<'EOF'
{"name":"termux_backup_test","date":"2026-01-01T12:00:00","label":"","packages":["git","new-pkg"],"files":{"home":{"archive":"home.tar.gz","size":1024,"checksum":"deadbeef"}},"version":"1.3.9","compression":"gz"}
EOF

plain() {
    sed -E $'s/\x1B\\[[0-9;?]*[A-Za-z]//g' "$1"
}

run_case() {
    local name="$1" mode="$2" feed="$3"
    local out="$TMP/$name.out"
    set +e
    ( eval "$feed" ) | HOME="$TMP/home" TEST_MODE="$mode" TERMUX_SYNC_BIN="$FAKE" \
        timeout 15 script -qec "stty rows 24 cols 90; $BIN" /dev/null >"$out" 2>&1
    status=$?
    set -e
    if [[ $status -ne 0 ]]; then
        printf '%s: exit %s\n' "$name" "$status"
        return 1
    fi
    printf '%s: ' "$name"
    if [[ ! -s "$out" ]]; then
        printf 'empty output\n'
        return 1
    fi
    printf 'captured %s bytes\n' "$(wc -c <"$out")"
}

feed_backup='sleep 1; printf "\r"; sleep 0.3; printf "\r"; sleep 1.2; printf "\033"; sleep 0.3; printf "q"'
feed_cancel='sleep 1; printf "\r"; sleep 0.3; printf "\r"; sleep 0.6; printf "\033"; sleep 0.4; printf "\r"; sleep 1.0; printf "\033"; sleep 0.3; printf "q"'
feed_restore='sleep 1; printf "\t"; sleep 0.2; printf "\r"; sleep 1.0; printf "RESTORE"; sleep 0.2; printf "\r"; sleep 1.2; printf "\033"; sleep 0.3; printf "q"'

run_case backup-ok backup-ok "$feed_backup"
grep -q "SUCCESS" "$TMP/backup-ok.out"
grep -q "VERIFIED" "$TMP/backup-ok.out"
printf 'backup-ok: assertions passed\n'

run_case backup-cancel backup-cancel "$feed_cancel"
grep -q "Operation cancelled" "$TMP/backup-cancel.out"
printf 'backup-cancel: assertions passed\n'

run_case restore-plan restore-plan "$feed_restore"
grep -q "Close and reopen Termux" "$TMP/restore-plan.out"
printf 'restore-plan: assertions passed\n'

feed_old='sleep 1; printf "L"; sleep 1.0; printf "q"'
run_case old-list old-cli "$feed_old"
plain "$TMP/old-list.out" | tr -d '\n' | grep -Eq "old.*list.*output"
printf 'old-list: assertions passed\n'

feed_status='sleep 1; printf "s"; sleep 0.5; printf "\033"; sleep 0.3; printf "q"'
run_case status status "$feed_status"
plain "$TMP/status.out" | tr -d '\n' | grep -q "STATUS"
printf 'status: assertions passed\n'

feed_protected='sleep 1; printf "\t"; sleep 0.2; printf "\r"; sleep 1.0; printf "RESTORE"; sleep 0.2; printf "\r"; sleep 1.2; printf "\033"; sleep 2.5; printf "\033"; sleep 0.3; printf "q"'
run_case restore-protected restore-protected "$feed_protected"
plain "$TMP/restore-protected.out" | tr -d '\n' | grep -q "CANNOT"
plain "$TMP/restore-protected.out" | tr -d '\n' | grep -q "reopen Termux"
printf 'restore-protected: assertions passed\n'
