#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || $EUID == 0 || $(uname -s) != Linux ]]; then
    printf 'Usage (non-root Linux): bash %s APP_EXECUTABLE EVIDENCE_DIR\n' "$0" >&2
    exit 2
fi

application=$(realpath -e -- "$1")
test -f "$application" && test -x "$application" || exit 2
mkdir -p -- "$2"
evidence=$(realpath -e -- "$2")
smoke=$(realpath -e -- "$(dirname -- "${BASH_SOURCE[0]}")/smoke-desktop-webview.mjs")

profile=$(mktemp -d)
trap 'rm -rf -- "$profile"' EXIT
mkdir -m 700 -- "$profile/home" "$profile/run" "$profile/tmp"
case "$application" in
    *.AppImage)
        extract=1
        printf 'Testing AppImage extraction mode (not FUSE).\n'
        ;;
    *) extract= ;;
esac

cd "$profile"
env -i PATH="$PATH" HOME="$profile/home" LANG=C.UTF-8 TMPDIR="$profile/tmp" \
    XDG_DATA_HOME="$profile/data" XDG_CONFIG_HOME="$profile/config" \
    XDG_STATE_HOME="$profile/state" XDG_CACHE_HOME="$profile/cache" XDG_RUNTIME_DIR="$profile/run" \
    GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1 \
    AMM_LINUX_AGENT_SMOKE=1 APPIMAGE_EXTRACT_AND_RUN="$extract" \
    dbus-run-session -- xvfb-run -a -s '-screen 0 1600x1100x24' \
    bash -seu -- "$application" "$smoke" "$evidence" <<'BASH' 2>&1 | tee "$evidence/smoke.log"
application=$1
smoke=$2
evidence=$3

tauri-driver --port 4444 > "$evidence/tauri-driver.log" 2>&1 &
driver=$!
trap 'kill "$driver" 2>/dev/null || true; wait "$driver" 2>/dev/null || true' EXIT
for attempt in {1..40}; do
    kill -0 "$driver"
    if curl -fsS --max-time 1 http://127.0.0.1:4444/status > /dev/null 2>&1; then break; fi
    sleep 0.25
done
curl -fsS --max-time 1 http://127.0.0.1:4444/status > /dev/null
node "$smoke" "$application" "$evidence/native-smoke.png"
BASH

db="$profile/data/com.glossmod.amm/manager.db"
test -f "$db"
actual=$(sqlite3 -readonly -batch -noheader "$db" '
    PRAGMA quick_check;
    SELECT source FROM plugins;
    SELECT status, count(*) FROM file_transactions GROUP BY status ORDER BY status;
    SELECT status, count(*) FROM agent_turns GROUP BY status ORDER BY status;
    SELECT count(*) FROM conversations WHERE archived = 1;
')
printf 'SQLite verification:\n%s\n' "$actual" | tee -a "$evidence/smoke.log"
if [[ $actual != $'ok\nexternal\ncommitted|6\ncancelled|1\ndone|3\n1' ]]; then
    printf 'Expected a healthy database, one external adapter, six commits, one cancelled and three completed turns, and one archived conversation.\n' >&2
    exit 1
fi
