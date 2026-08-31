#!/usr/bin/env bash

set -euo pipefail
unset TMUX TMUX_PANE

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${TNX_BIN:-$ROOT/target/debug/tnx}"
[ -x "$BIN" ] || { echo "tmux integration: build tnx first" >&2; exit 1; }

SOCKET="tnx-integration-$$-$RANDOM"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/tnx-integration.XXXXXX")"
cleanup() {
    tmux -L "$SOCKET" kill-server >/dev/null 2>&1 || true
    rm -rf "$TMP"
}
trap cleanup EXIT

wait_for_text() {
    local pane="$1"
    local needle="$2"
    local remaining=100
    while [ "$remaining" -gt 0 ]; do
        if tmux -L "$SOCKET" capture-pane -p -J -S - -t "$pane" 2>/dev/null |
            grep -Fq "$needle"; then
            return 0
        fi
        sleep 0.02
        remaining=$((remaining - 1))
    done
    echo "tmux integration: pane $pane did not contain '$needle'" >&2
    return 1
}

assert_eq() {
    local actual="$1"
    local expected="$2"
    local description="$3"
    if [ "$actual" != "$expected" ]; then
        echo "tmux integration: $description: expected '$expected', got '$actual'" >&2
        return 1
    fi
}

assert_gt() {
    local actual="$1"
    local expected="$2"
    local description="$3"
    if [ "$actual" -le "$expected" ]; then
        echo "tmux integration: $description: expected $actual > $expected" >&2
        return 1
    fi
}

assert_le() {
    local actual="$1"
    local expected="$2"
    local description="$3"
    if [ "$actual" -gt "$expected" ]; then
        echo "tmux integration: $description: expected $actual <= $expected" >&2
        return 1
    fi
}

assert_num_eq() {
    local actual="$1"
    local expected="$2"
    local description="$3"
    if [ "$actual" -ne "$expected" ]; then
        echo "tmux integration: $description: expected $expected, got $actual" >&2
        return 1
    fi
}

pane="$(tmux -L "$SOCKET" -f /dev/null new-session -d -x 50 -y 8 -s review \
    -P -F '#{pane_id}' \
    "bash -lc 'for i in \$(seq 1 40); do case \"\$i\" in 5|36|40) echo DUPLICATE ;; 20) echo \"-rw-r--r-- query_test.go\" ;; *) echo hist-\$i ;; esac; done; echo visible-a; echo DUPLICATE; echo visible-z; sleep 60'")"
wait_for_text "$pane" visible-z
history_size="$(tmux -L "$SOCKET" display-message -p -t "$pane" '#{history_size}')"

visible_capture="$(TNX_TMUX_ARGS="-L $SOCKET" "$BIN" capture --scope pane --no-history -t "$pane")"
record="$(printf '%s\n' "$visible_capture" | awk -F '\t' '$6 == "DUPLICATE" { record = $0 } END { print record }')"
line_no="$(printf '%s\n' "$record" | awk -F '\t' '{print $5}')"
assert_gt "$line_no" "$history_size" "visible line is outside scrollback"
TNX_TMUX_ARGS="-L $SOCKET" "$BIN" action --action jump --record "$record"
scroll_position="$(tmux -L "$SOCKET" display-message -p -t "$pane" '#{scroll_position}')"
assert_num_eq "$scroll_position" 2 "jump scroll position"
tmux -L "$SOCKET" send-keys -t "$pane" -X cancel

full_capture="$(TNX_TMUX_ARGS="-L $SOCKET" "$BIN" capture --scope pane -t "$pane")"
hyphen_record="$(printf '%s\n' "$full_capture" |
    awk -F '\t' '$6 == "-rw-r--r-- query_test.go" { print; exit }')"
TNX_TMUX_ARGS="-L $SOCKET" "$BIN" action --action jump --record "$hyphen_record"
cursor_line="$(tmux -L "$SOCKET" display-message -p -t "$pane" '#{copy_cursor_line}')"
assert_eq "$cursor_line" "-rw-r--r-- query_test.go" "leading-hyphen jump target"
tmux -L "$SOCKET" send-keys -t "$pane" -X cancel

limited_capture="$(TNX_TMUX_ARGS="-L $SOCKET" "$BIN" capture --scope pane \
    --history-lines 5 -t "$pane")"
first_line="$(printf '%s\n' "$limited_capture" | awk -F '\t' 'NR == 1 {print $5}')"
expected_first=$((history_size - 5 + 1))
assert_num_eq "$first_line" "$expected_first" "limited capture first line"

wrapped_pane="$(tmux -L "$SOCKET" new-session -d -x 10 -y 8 -s wrapped -P -F '#{pane_id}' \
    "bash -lc 'printf \"wrapped-target-long\\n\"; sleep 60'")"
wait_for_text "$wrapped_pane" wrapped-target-long
wrapped_result="$(TNX_TMUX_ARGS="-L $SOCKET" "$BIN" search --scope pane -t "$wrapped_pane" \
    --print wrapped-target-long)"
assert_eq "$wrapped_result" wrapped-target-long "wrapped-line search result"

mkdir -p "$TMP/fake-bin"
cat > "$TMP/fake-bin/tmux" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
    show-options) exit 0 ;;
    list-panes)
        printf 'review\t0\t0\t%%9\tsleep\t0\tmain\n'
        ;;
    capture-pane)
        echo "can't find pane: %9" >&2
        exit 1
        ;;
    *) exit 0 ;;
esac
EOF
chmod +x "$TMP/fake-bin/tmux"
if failure="$(PATH="$TMP/fake-bin:$PATH" "$BIN" search --print needle 2>&1)"; then
    echo "tmux integration: disappearing pane unexpectedly succeeded" >&2
    exit 1
fi
if ! printf '%s\n' "$failure" | grep -Fq 'failed to capture pane %9'; then
    echo "tmux integration: disappearing pane error was: $failure" >&2
    exit 1
fi

echo "tmux integration tests passed"
