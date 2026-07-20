#!/bin/sh
set -eu

if [ -n "${FAKE_HERDR_ARGV_LOG:-}" ]; then
    : > "$FAKE_HERDR_ARGV_LOG"
    for argument in "$@"; do
        printf '%s\0' "$argument" >> "$FAKE_HERDR_ARGV_LOG"
    done
fi

case "${FAKE_HERDR_MODE:-fixture}" in
    fixture)
        cat "${FAKE_HERDR_STDOUT_FILE:?FAKE_HERDR_STDOUT_FILE is required}"
        ;;
    empty)
        ;;
    api-error)
        printf '%s\n' '{"id":"fake","error":{"code":"pane_not_found","message":"pane vanished"}}' >&2
        exit 1
        ;;
    json-stdout-plain-stderr-error)
        printf '%s\n' '{"id":"fake","error":{"code":"stdout_only_code","message":"not public contract output"}}'
        printf '%s\n' 'public stderr failure' >&2
        exit 1
        ;;
    malformed-json)
        printf '%s\n' '{not json'
        ;;
    ansi-error)
        printf '\033[31mfailed\033[0m\033]8;;https://example.invalid\007 link\033]8;;\007\001' >&2
        exit 1
        ;;
    large-stdout)
        dd if=/dev/zero bs=1024 count=257 2>/dev/null | tr '\000' x
        ;;
    over-json-stdout)
        dd if=/dev/zero bs=1024 count=4097 2>/dev/null | tr '\000' x
        ;;
    large-stderr)
        dd if=/dev/zero bs=1024 count=257 2>/dev/null | tr '\000' y >&2
        exit 1
        ;;
    large-both)
        (dd if=/dev/zero bs=1024 count=4097 2>/dev/null | tr '\000' x) &
        stdout_pid=$!
        (dd if=/dev/zero bs=1024 count=257 2>/dev/null | tr '\000' y >&2) &
        stderr_pid=$!
        wait "$stdout_pid"
        wait "$stderr_pid"
        ;;
    hang)
        if [ -n "${FAKE_HERDR_PID_FILE:-}" ]; then
            printf '%s\n' "$$" > "$FAKE_HERDR_PID_FILE"
        fi
        exec sleep 30
        ;;
    fork-retain-pipes)
        (exec sleep 30) &
        descendant=$!
        printf '%s\n%s\n' "$$" "$descendant" > "${FAKE_HERDR_PID_FILE:?FAKE_HERDR_PID_FILE is required}"
        wait "$descendant"
        ;;
    leader-exits-retain-pipes)
        (exec sleep 30) &
        descendant=$!
        printf '%s\n' "$descendant" > "${FAKE_HERDR_PID_FILE:?FAKE_HERDR_PID_FILE is required}"
        exit 0
        ;;
    infinite-output)
        (
            while :; do
                printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
            done
        ) &
        descendant=$!
        printf '%s\n%s\n' "$$" "$descendant" > "${FAKE_HERDR_PID_FILE:?FAKE_HERDR_PID_FILE is required}"
        wait "$descendant"
        ;;
    *)
        printf 'unknown fake mode: %s\n' "$FAKE_HERDR_MODE" >&2
        exit 2
        ;;
esac
