#!/usr/bin/env bash

set -euo pipefail
unset TMUX TMUX_PANE

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/tnx-shell-test.XXXXXX")"
SOCKET="tnx-shell-test-$$"
REAL_TMUX="$(command -v tmux)"
cleanup() {
    "$REAL_TMUX" -L "$SOCKET" kill-server >/dev/null 2>&1 || true
    rm -rf "$TMP"
}
trap cleanup EXIT

plugin="$TMP/plugin space'quote"
mkdir -p "$plugin/scripts" "$TMP/bin"
cp "$ROOT/tmux_nexus.tmux" "$plugin/tmux_nexus.tmux"
cp "$ROOT/tnx" "$plugin/tnx"
cp "$ROOT/scripts/"*.sh "$plugin/scripts/"

cat > "$TMP/bin/tmux" <<'EOF'
#!/usr/bin/env bash
exec "$TNX_TEST_REAL_TMUX" -L "$TNX_TEST_SOCKET" "$@"
EOF
chmod +x "$TMP/bin/tmux"
export TNX_TEST_REAL_TMUX="$REAL_TMUX"
export TNX_TEST_SOCKET="$SOCKET"
export PATH="$TMP/bin:$PATH"

"$REAL_TMUX" -L "$SOCKET" -f /dev/null new-session -d -s quote 'sleep 60'
"$REAL_TMUX" -L "$SOCKET" set-option -g @tmux_nexus_pane_key /
TNX_LAUNCH_KEY=G TNX_MOTION_KEY=M TNX_MOTION2_KEY=N bash "$plugin/tmux_nexus.tmux"

bindings="$("$REAL_TMUX" -L "$SOCKET" list-keys -T prefix)"
search_binding="$(printf '%s\n' "$bindings" | grep -E 'prefix[[:space:]]+G[[:space:]].*tnx search')"
pane_binding="$(printf '%s\n' "$bindings" | grep -E 'prefix[[:space:]]+/[[:space:]].*tnx search --scope pane')"
motion_binding="$(printf '%s\n' "$bindings" | grep -E 'prefix[[:space:]]+M[[:space:]].*motion-s.sh')"
motion2_binding="$(printf '%s\n' "$bindings" | grep -E 'prefix[[:space:]]+N[[:space:]].*motion-s2.sh')"
manager_binding="$(printf '%s\n' "$bindings" | grep -E 'prefix[[:space:]]+F[[:space:]].*tnx manage')"
printf '%s\n' "$search_binding" | grep -Fq 'plugin\\ space'
printf '%s\n' "$search_binding" | grep -Fq 'quote/tnx search'
printf '%s\n' "$pane_binding" | grep -Fq 'plugin\\ space'
printf '%s\n' "$pane_binding" | grep -Fq 'quote/tnx search --scope pane'
printf '%s\n' "$motion_binding" | grep -Fq 'plugin\\ space'
printf '%s\n' "$motion_binding" | grep -Fq 'quote/scripts/motion-s.sh'
printf '%s\n' "$motion2_binding" | grep -Fq 'plugin\\ space'
printf '%s\n' "$motion2_binding" | grep -Fq 'quote/scripts/motion-s2.sh'
printf '%s\n' "$manager_binding" | grep -Fq 'plugin\\ space'
printf '%s\n' "$manager_binding" | grep -Fq 'quote/tnx manage'

"$REAL_TMUX" -L "$SOCKET" unbind-key F
"$REAL_TMUX" -L "$SOCKET" set-option -g @tmux_nexus_manager_key ''
bash "$plugin/tmux_nexus.tmux"
if "$REAL_TMUX" -L "$SOCKET" list-keys -T prefix | \
    grep -Eq 'prefix[[:space:]]+F[[:space:]].*tnx manage'; then
    echo "shell quoting: empty manager key still created a binding" >&2
    exit 1
fi

command_path="$TMP/command space'quote"
cat > "$command_path" <<'EOF'
#!/bin/sh
printf 'quoted command ran\n'
EOF
chmod +x "$command_path"
# shellcheck source=../scripts/utils.sh
source "$ROOT/scripts/utils.sh"
quoted="$(tnx_shell_quote "$command_path")"
[ "$(sh -c "$quoted")" = "quoted command ran" ]

echo "shell quoting tests passed"
