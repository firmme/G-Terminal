#!/bin/sh
# All paths are passed as data from Rust.
stage=$GTERMINAL_UPDATE_DIR
current=$GTERMINAL_UPDATE_CURRENT
staged=$GTERMINAL_UPDATE_STAGED
parent=$GTERMINAL_UPDATE_PARENT
backup="$stage/previous.app"
lock="$current.gterminal-update-lock"
report="$stage/update-failed.txt"

fail() {
    printf '%s\n' "$1" > "$report"
    /usr/bin/osascript - "$1" "$report" <<'APPLESCRIPT'
on run argv
    display alert "G-Terminal Update" message (item 1 of argv & return & return & "Details: " & item 2 of argv) as critical
end run
APPLESCRIPT
    exit 1
}

remaining=30
while kill -0 "$parent" 2>/dev/null; do
    if [ "$remaining" -eq 0 ]; then
        fail "G-Terminal did not exit within 30 seconds."
    fi
    /bin/sleep 1
    remaining=$((remaining - 1))
done

/bin/mkdir "$lock" || fail "Another G-Terminal update is in progress."
trap '/bin/rmdir "$lock"' EXIT
/bin/mv "$current" "$backup" 2>"$report" ||
    fail "Could not back up the application: $(cat "$report")"
if ! /bin/mv "$staged" "$current" 2>"$report"; then
    error=$(cat "$report")
    /bin/mv "$backup" "$current" ||
        fail "Could not restore the previous version. Backup: $backup"
    fail "Could not replace the application: $error"
fi
if ! /usr/bin/open "$current" 2>"$report"; then
    error=$(cat "$report")
    /bin/mv "$current" "$staged" && /bin/mv "$backup" "$current" ||
        fail "Could not restore the previous version. Backup: $backup"
    fail "Could not relaunch G-Terminal: $error"
fi
/bin/rm -rf "$stage"
