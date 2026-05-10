# chist wrapper for zsh
#
# This is a fuller version of the wrapper shown in the README. In addition to
# eval'ing `chist exec`, it adds:
#
#   chist save <name>      — snapshot all currently-running sessions to a JSON
#                            file under ~/.chist/saves/
#   chist restore <name>   — for KDE Konsole users only; reopens each saved
#                            session in a new tab and runs `claude --resume`
#                            in it.
#
# `restore` requires konsole's D-Bus interface (KONSOLE_DBUS_SERVICE and
# KONSOLE_DBUS_WINDOW are set automatically when zsh runs inside konsole).
# Linux-only.

_chist_save() {
    local name="${1:-default}"
    local dir="$HOME/.chist/saves"
    mkdir -p "$dir"
    local file="$dir/$name.json"
    command chist list -f json -a 2>/dev/null \
        | jq '[.[] | select(.status == "running") | {id: .session_id, project_path: .project_path, last_message: .last_message}]' \
        > "$file" || { echo "chist save: failed to write $file" >&2; return 1; }
    local count
    count=$(jq 'length' "$file")
    echo "saved $count running session(s) -> $file"
    [[ $count -gt 0 ]] && jq -r '.[] | "  \(.id[0:8])  \(.project_path)"' "$file"
}

_chist_current_session_id() {
    [[ -z "$CLAUDECODE" ]] && return 0
    local cwd_enc proj_dir latest
    cwd_enc=$(pwd | sed 's|/|-|g')
    proj_dir="$HOME/.claude/projects/$cwd_enc"
    [[ -d "$proj_dir" ]] || return 0
    latest=$(ls -t "$proj_dir"/*.jsonl 2>/dev/null | head -1)
    [[ -n "$latest" ]] && basename "$latest" .jsonl
}

_chist_restore() {
    local name="${1:-default}"
    local file="$HOME/.chist/saves/$name.json"
    local data
    if [[ -f "$file" ]]; then
        echo "restoring from $file" >&2
        data=$(cat "$file")
    else
        echo "no save '$name' - falling back to currently-running sessions" >&2
        data=$(command chist list -f json -a 2>/dev/null \
            | jq '[.[] | select(.status == "running") | {id: .session_id, project_path: .project_path}]')
    fi

    if [[ -z "$KONSOLE_DBUS_SERVICE" || -z "$KONSOLE_DBUS_WINDOW" ]]; then
        echo "chist restore: not in konsole (KONSOLE_DBUS_SERVICE/WINDOW unset)" >&2
        echo "would have run:" >&2
        print -r -- "$data" | jq -r '.[] | "  claude -r \(.id[0:8])   # \(.project_path)"' >&2
        return 1
    fi

    local current_id
    current_id=$(_chist_current_session_id)
    [[ -n "$current_id" ]] && echo "skipping current session ${current_id:0:8}" >&2

    local count=0 id project_path result sess_num
    while IFS=$'\t' read -r id project_path; do
        [[ -z "$id" ]] && continue
        [[ "$id" == "$current_id" ]] && continue
        result=$(gdbus call --session \
            --dest "$KONSOLE_DBUS_SERVICE" \
            --object-path "$KONSOLE_DBUS_WINDOW" \
            --method org.kde.konsole.Window.newSession "" "$project_path" 2>&1)
        sess_num=$(echo "$result" | grep -oE '\([0-9]+,' | head -1 | tr -d '(,')
        if [[ -z "$sess_num" ]]; then
            echo "  x failed to open tab for ${id:0:8}: $result" >&2
            continue
        fi
        sleep 0.4
        gdbus call --session \
            --dest "$KONSOLE_DBUS_SERVICE" \
            --object-path "/Sessions/$sess_num" \
            --method org.kde.konsole.Session.runCommand "claude -r $id" >/dev/null
        echo "  + tab $sess_num: claude -r ${id:0:8}  ($project_path)"
        count=$((count+1))
    done < <(print -r -- "$data" | jq -r '.[] | [.id, .project_path] | @tsv')

    echo "restored $count session(s)"
}

chist() {
    case "$1" in
        save)
            shift; _chist_save "$@" ;;
        restore|open)
            shift; _chist_restore "$@" ;;
        exec|e|-r|--resume|-e|--execute)
            local cmd
            cmd=$(command chist "$@")
            if [[ $? -eq 0 && -n "$cmd" ]]; then
                echo "$cmd"
                eval "$cmd"
            fi ;;
        *)
            command chist "$@" ;;
    esac
}
