#!/usr/bin/env bash
set -euo pipefail

if [[ "${TEST_MODE:-}" == "old-cli" ]]; then
    case " $* " in
        *" --json-events "*) echo "unrecognized arguments: --json-events" >&2; exit 2 ;;
        *" list "*) echo "old list output"; exit 0 ;;
        *" --version "*) exit 0 ;;
        *) exit 0 ;;
    esac
fi

case " $* " in
    *" --version "* | *" logs "*) exit 0 ;;
esac

case "${TEST_MODE:-}" in
    backup-ok)
        printf '%s\n' '{"v":1,"event":"start","name":"termux_backup_pty","storage":"local"}'
        printf '%s\n' '{"v":1,"event":"stage","id":"packages","label":"Packages","state":"start","index":2,"total":7}'
        printf '%s\n' '{"v":1,"event":"log","level":"INFO","message":"Collected packages"}'
        printf '%s\n' '{"v":1,"event":"stage","id":"packages","label":"Packages","state":"complete","index":2,"total":7}'
        printf '%s\n' '{"v":1,"event":"result","ok":true,"name":"termux_backup_pty","size":1024,"verified":true,"verify_details":"local checksums match","duration_ms":42}'
        ;;
    backup-cancel)
        trap 'printf "%s\n" "{\"v\":1,\"event\":\"cancelled\",\"reason\":\"user\"}"; exit 130' INT TERM
        printf '%s\n' '{"v":1,"event":"start","name":"termux_backup_pty","storage":"local"}'
        printf '%s\n' '{"v":1,"event":"stage","id":"archive:home","label":"Home","state":"start","index":3,"total":7}'
        sleep 5
        ;;
    restore-plan)
        printf '%s\n' '{"v":1,"event":"start","name":"termux_backup_test","storage":"local"}'
        printf '%s\n' '{"v":1,"event":"stage","id":"metadata","label":"Metadata","state":"start","index":1,"total":7}'
        printf '%s\n' '{"v":1,"event":"stage","id":"metadata","label":"Metadata","state":"complete","index":1,"total":7}'
        printf '%s\n' '{"v":1,"event":"plan","name":"termux_backup_test","date":"2026-01-01T12:00:00","label":"","storage":"local","versions":{"backup":"1.3.9","current":"1.3.9"},"packages":2,"diff_available":true,"to_install":1,"already_installed":1,"device_only":0,"archives":["home.tar.gz"],"size":1024,"checksums":"pending"}'
        printf '%s\n' '{"v":1,"event":"package_diff","category":"to_install","part":1,"parts":1,"packages":["new-pkg"]}'
        printf '%s\n' '{"v":1,"event":"package_diff","category":"already_installed","part":1,"parts":1,"packages":["git"]}'
        printf '%s\n' '{"v":1,"event":"package_diff","category":"device_only","part":1,"parts":1,"packages":[]}'
        if ! read -r confirm; then
            printf '%s\n' '{"v":1,"event":"cancelled","reason":"declined"}'
            exit 130
        fi
        case "$confirm" in
            *RESTORE*) ;;
            *)
                printf '%s\n' '{"v":1,"event":"cancelled","reason":"declined"}'
                exit 130
                ;;
        esac
        printf '%s\n' '{"v":1,"event":"stage","id":"verify","label":"Verify","state":"complete","index":3,"total":7}'
        printf '%s\n' '{"v":1,"event":"stage","id":"extract:home","label":"Home","state":"start","index":4,"total":7}'
        printf '%s\n' '{"v":1,"event":"stage","id":"extract:home","label":"Home","state":"complete","index":4,"total":7}'
        printf '%s\n' '{"v":1,"event":"stage","id":"packages","label":"Packages","state":"complete","index":6,"total":7}'
        printf '%s\n' '{"v":1,"event":"stage","id":"cleanup","label":"Cleanup","state":"complete","index":7,"total":7}'
        printf '%s\n' '{"v":1,"event":"result","ok":true,"name":"termux_backup_test","size":1024,"packages":2,"packages_installed":2,"packages_failed":[],"archives":1,"archives_failed":[],"warnings":false,"restart_required":true,"duration_ms":100}'
        ;;
    restore-protected)
        trap 'printf "%s\n" "{\"v\":1,\"event\":\"cancelled\",\"reason\":\"unexpected\"}"; exit 130' INT TERM
        printf '%s\n' '{"v":1,"event":"start","name":"termux_backup_test","storage":"local"}'
        printf '%s\n' '{"v":1,"event":"plan","name":"termux_backup_test","date":"2026-01-01T12:00:00","label":"","storage":"local","versions":{"backup":"1.3.9","current":"1.3.9"},"packages":2,"diff_available":true,"to_install":1,"already_installed":1,"device_only":0,"archives":["home.tar.gz"],"size":1024,"checksums":"pending"}'
        read -r confirm
        case "$confirm" in
            *RESTORE*) ;;
            *) exit 130 ;;
        esac
        printf '%s\n' '{"v":1,"event":"stage","id":"extract:home","label":"Home","state":"start","index":4,"total":7}'
        sleep 3
        printf '%s\n' '{"v":1,"event":"stage","id":"extract:home","label":"Home","state":"complete","index":4,"total":7}'
        printf '%s\n' '{"v":1,"event":"stage","id":"packages","label":"Packages","state":"complete","index":6,"total":7}'
        printf '%s\n' '{"v":1,"event":"stage","id":"cleanup","label":"Cleanup","state":"complete","index":7,"total":7}'
        printf '%s\n' '{"v":1,"event":"result","ok":true,"name":"termux_backup_test","size":1024,"packages":2,"packages_installed":2,"packages_failed":[],"archives":1,"archives_failed":[],"warnings":false,"restart_required":true,"duration_ms":100}'
        ;;
    *)
        printf 'unknown TEST_MODE\n' >&2
        exit 1
        ;;
esac
